// rust/src/db_to_txt.rs
use itertools::Itertools;
use rusqlite::{Connection, Error as SqliteError};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Write;

use crate::common;
use crate::common::SqliteId;
use crate::config;
use crate::db_read;
use crate::db_read::{DefinitionEntry, WordEntry};

// --- Error Handling ---
#[derive(Debug)]
pub enum DbToTxtError {
    SqliteError(SqliteError),
    IoError(std::io::Error),
    InvalidDbData(String),
}

impl fmt::Display for DbToTxtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SqliteError(e) => write!(f, "Database error: {e}"),
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::InvalidDbData(s) => write!(f, "Invalid data in DB: {s}"),
        }
    }
}

impl From<SqliteError> for DbToTxtError {
    fn from(err: SqliteError) -> Self {
        Self::SqliteError(err)
    }
}

impl From<std::io::Error> for DbToTxtError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err)
    }
}

impl std::error::Error for DbToTxtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::IoError(ref source) => Some(source),
            Self::SqliteError(ref source) => Some(source),
            Self::InvalidDbData(_) => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, DbToTxtError>;

fn format_multiline(s: &str, indent_level: usize, indent_char: &str) -> String {
    let indented_newline = format!("\n{}", indent_char.repeat(indent_level + 2));
    s.lines().join(&indented_newline)
}

pub fn db_to_txt(
    writer: &mut dyn Write,
    conn: &Connection,
    indent_with_tabs: bool,
    limit_to_word: Option<&str>,
) -> Result<()> {
    let mut db2txt = DbToTxt::new(conn, writer, indent_with_tabs);
    db2txt.generate_txt_file(limit_to_word)?;
    Ok(())
}

pub struct DbToTxt<'a> {
    conn: &'a Connection,
    writer: &'a mut dyn Write,
    indent_str: String,
    written_notes: HashSet<SqliteId>,
    words: HashMap<SqliteId, WordEntry>,
    def_ext_ids: HashMap<SqliteId, u32>,
    word_variants: HashMap<SqliteId, Vec<SqliteId>>,
}

impl<'a> DbToTxt<'a> {
    pub fn new(conn: &'a Connection, writer: &'a mut dyn Write, indent_with_tabs: bool) -> Self {
        DbToTxt {
            conn,
            writer,
            indent_str: if indent_with_tabs {
                "\t".to_owned()
            } else {
                " ".to_owned()
            },
            written_notes: HashSet::new(),
            words: HashMap::new(),
            def_ext_ids: HashMap::new(),
            word_variants: HashMap::new(),
        }
    }

    pub fn generate_txt_file(&mut self, _limit_to_word: Option<&str>) -> Result<()> {
        // Pre-load all words into memory maps for rapid lookups without joining 
        let all_words = db_read::read_all_words(self.conn)?;
        for w in all_words {
            if let Some(parent_id) = w.variant_of {
                self.word_variants.entry(parent_id).or_default().push(w.id);
            }
            self.words.insert(w.id, w);
        }

        let definitions = db_read::read_definitions(self.conn)?;
        for (def, _) in &definitions {
            self.def_ext_ids.insert(def.id, def.ext_def_id);
        }

        // Output header comment
        let header_shared_ids = db_read::read_shared_ids(self.conn, 1)?;
        self.write_shared_items_from_ids(
            header_shared_ids.comment_id,
            header_shared_ids.note_id,
            0,
        )?;

        for (definition, grouping) in definitions {
            if grouping.new_word {
                self.write_word_entry(definition.word_id)?;
            }
            if grouping.new_pron {
                self.write_pinyin_entries(definition.id, &definition.pron_shared_ids)?;
            }
            if grouping.new_class {
                self.write_class_entry(&definition.class_name)?;
            }
            self.write_definition_entry(&definition)?;
        }

        Ok(())
    }

    fn write_word_entry(&mut self, word_id: SqliteId) -> Result<()> {
        let word = self.words.get(&word_id).ok_or_else(|| {
            DbToTxtError::InvalidDbData(format!("Word missing from cache: {word_id}"))
        })?;
        let tags = self.get_formatted_tags(word.shared_id)?;

        let mut word_str = common::format_word_def(&word.trad, &word.simp, None);

        // collect optional variants of the word
        let mut variants = vec![];
        if let Some(variant_ids) = self.word_variants.get(&word_id) {
            for v_id in variant_ids {
                if let Some(v_word) = self.words.get(v_id) {
                    variants.push(common::format_word_def(&v_word.trad, &v_word.simp, None));
                }
            }
        }

        if !variants.is_empty() {
            variants.sort(); // keep order deterministic
            word_str.push_str(config::ITEMS_SEP);
            word_str.push_str(&variants.join(config::ITEMS_SEP));
        }
        let (word_comment_id, word_note_id, word_id) = (word.shared_ids.comment_id, word.shared_ids.note_id, word.id);

        writeln!(self.writer, "W{tags}{word_str}")?;
        self.write_shared_items_from_ids(
            word_comment_id,
            word_note_id,
            1,
        )?;
        self.write_cross_references(word_id, None, 1)?;
        Ok(())
    }

    fn write_pinyin_entries(
        &mut self,
        def_id: SqliteId,
        pron_shared_ids: &[SqliteId],
    ) -> Result<()> {
        let pron_entries =
            db_read::read_pinyin_entries_for_definition(self.conn, def_id, pron_shared_ids)?;

        // group the data and format it into lines
        let mut indent_level = 1;
        for ((note_id, comment_id), tag_group) in &pron_entries
            .into_iter()
            .chunk_by(|item| (item.shared_ids.note_id, item.shared_ids.comment_id))
        {
            let tags_pinyins = tag_group
                .into_iter()
                .chunk_by(|item| item.shared_ids.tag_ids.clone())
                .into_iter()
                .map(|(_, tag_group)| {
                    let pinyins_shared_ids: Vec<(SqliteId, String)> = tag_group
                        .map(|item| (item.shared_id, item.pinyin_num))
                        .collect();
                    let one_shared_id = pinyins_shared_ids.first().unwrap().0;
                    let pinyins = pinyins_shared_ids
                        .into_iter()
                        .map(|item| item.1)
                        .join(config::ITEMS_SEP);
                    let tags = self.get_formatted_tags(one_shared_id).unwrap();
                    format!("{tags}{pinyins}")
                })
                .join(" ");

            writeln!(
                self.writer,
                "{}P{}",
                self.indent_str.repeat(indent_level),
                tags_pinyins
            )?;

            self.write_shared_items_from_ids(comment_id, note_id, indent_level + 1)?;
            indent_level = 2;
        }

        Ok(())
    }

    fn write_class_entry(&mut self, class_name: &str) -> Result<()> {
        writeln!(self.writer, "{}C {}", self.indent_str.repeat(2), class_name)?;
        Ok(())
    }

    fn write_definition_entry(&mut self, entry: &DefinitionEntry) -> Result<()> {
        let tags = self.get_formatted_tags(entry.shared_id)?;
        let def_id_indent = 3 + entry.nested_level;
        writeln!(
            self.writer,
            "{}D{}{}{}",
            self.indent_str.repeat(def_id_indent),
            entry.ext_def_id,
            tags,
            format_multiline(&entry.definition, def_id_indent, &self.indent_str),
        )?;
        self.write_shared_items_from_ids(
            entry.shared_ids.comment_id,
            entry.shared_ids.note_id,
            def_id_indent + 1,
        )?;
        self.write_sentences(entry.id, def_id_indent + 1)?;
        self.write_cross_references(entry.word_id, Some(entry.id), def_id_indent + 1)?;
        Ok(())
    }

    fn get_formatted_tags(&self, shared_id: SqliteId) -> Result<String> {
        let tags = db_read::read_tags_for_shared_id(self.conn, shared_id)?;
        let mut ascii_tags = vec![];
        let mut full_tags = vec![];

        for tag in tags {
            match tag {
                db_read::Tag::Ascii(c) => ascii_tags.push(c.to_string()),
                db_read::Tag::Full { name, .. } => full_tags.push(format!("#{name}")),
            }
        }

        ascii_tags.sort_by_key(|x| {
            config::tag_to_txt_ascii_common(x.chars().next().unwrap())
                .unwrap_or(("", "", 0))
                .2
        });
        full_tags.sort();

        let space = if full_tags.is_empty() || ascii_tags.is_empty() {
            ""
        } else {
            " "
        };

        if ascii_tags.is_empty() && full_tags.is_empty() {
            Ok("||".to_owned())
        } else {
            Ok(format!(
                "|{}{}{}|",
                ascii_tags.iter().join(""),
                space,
                full_tags.iter().join(" ")
            ))
        }
    }

    fn write_sentences(&mut self, def_id: SqliteId, indent: usize) -> Result<()> {
        let sentences = db_read::read_sentences_for_definition(self.conn, def_id)?;
        if sentences.is_empty() {
            return Ok(());
        }

        let indentation = self.indent_str.repeat(indent);
        let translation_indentation = self.indent_str.repeat(indent + 2);

        for sentence in sentences {
            if sentence.translation.contains('\n') {
                return Err(DbToTxtError::InvalidDbData(format!(
                    "Sentence translation contains newline: {}",
                    sentence.id
                )));
            }

            let tags = self.get_formatted_tags(sentence.shared_id)?;
            let mut formatted_words = vec![];

            for sw in sentence.words {
                if let Some(ascii_txt) = sw.ascii_txt {
                    formatted_words.push(ascii_txt);
                } else if let Some((w_id, d_id)) = sw.word {
                    // Main word format
                    let word = self.words.get(&w_id).ok_or_else(|| {
                        DbToTxtError::InvalidDbData(format!(
                            "Sentence word missing from cache: {w_id}"
                        ))
                    })?;
                    let ext_d_id = d_id.and_then(|id| self.def_ext_ids.get(&id).copied());
                    let mut w_str = common::format_word_def(&word.trad, &word.simp, ext_d_id);

                    // Separable part logic (<partOfWord)
                    if let Some((pw_id, pd_id)) = sw.part_of_word {
                        let pword = self.words.get(&pw_id).ok_or_else(|| {
                            DbToTxtError::InvalidDbData(format!(
                                "Sentence part_of_word missing from cache: {pw_id}"
                            ))
                        })?;
                        let pext_d_id = pd_id.and_then(|id| self.def_ext_ids.get(&id).copied());
                        let pw_str =
                            common::format_word_def(&pword.trad, &pword.simp, pext_d_id);
                        w_str.push('<');
                        w_str.push_str(&pw_str);
                    }
                    formatted_words.push(w_str);
                } else {
                    return Err(DbToTxtError::InvalidDbData(format!(
                        "Sentence word has neither ascii_txt nor word_id: {}",
                        sentence.id
                    )));
                }
            }

            // Write 'S' line
            writeln!(
                self.writer,
                "{}S{}{}{}",
                indentation,
                sentence.ext_sent_id,
                tags,
                formatted_words.join(" ")
            )?;

            // Write translation line (+2 indent, no prefix)
            writeln!(
                self.writer,
                "{}{}",
                translation_indentation, sentence.translation
            )?;

            // Write shared items for sentence (indent + 1)
            self.write_shared_items_from_ids(
                sentence.shared_ids.comment_id,
                sentence.shared_ids.note_id,
                indent + 1,
            )?;
        }

        Ok(())
    }

    fn write_shared_items_from_ids(
        &mut self,
        comment_id: Option<SqliteId>,
        note_id: Option<SqliteId>,
        indent: usize,
    ) -> Result<()> {
        let indentation = self.indent_str.repeat(indent);

        // Write Comment
        if let Some(id) = comment_id {
            let comment = db_read::read_comment(self.conn, id)?;
            let comment = format_multiline(&comment, indent, &self.indent_str);
            writeln!(self.writer, "{indentation}# {comment}")?;
        }

        // Write Note
        if let Some(id) = note_id {
            let (note_txt, ext_id) = db_read::read_note(self.conn, id)?;
            if self.written_notes.contains(&ext_id) || indent == 0 {
                // indent == 0 hack for initial header pointer to highest note id
                writeln!(self.writer, "{indentation}N->{ext_id}")?;
            } else {
                let note_txt = format_multiline(&note_txt, indent, &self.indent_str);
                writeln!(self.writer, "{indentation}N{ext_id} {note_txt}")?;
                self.written_notes.insert(ext_id);
            }
        }
        Ok(())
    }

    fn write_cross_references(
        &mut self,
        src_word_id: SqliteId,
        src_def_id: Option<SqliteId>,
        indent: usize,
    ) -> Result<()> {
        let references = db_read::read_references_for_item(self.conn, src_word_id, src_def_id)?;
        if references.is_empty() {
            return Ok(());
        }

        // Struct solely used to hold local state during grouping operations in db_to_txt
        struct CrossReferenceFmt {
            ref_type_symbol: char,
            tags: String,
            note_id: Option<SqliteId>,
            comment_id: Option<SqliteId>,
            reference_str: String,
        }

        let mut cross_ref_data = vec![];
        for r in references {
            let dst_word = self.words.get(&r.dst.0).ok_or_else(|| {
                DbToTxtError::InvalidDbData(format!("Dst word missing from cache: {}", r.dst.0))
            })?;
            let dst_ext_def_id = r.dst.1.and_then(|id| self.def_ext_ids.get(&id).copied());
            let reference_str = common::format_word_def(&dst_word.trad, &dst_word.simp, dst_ext_def_id);
            
            let tags = self.get_formatted_tags(r.shared_id)?;
            cross_ref_data.push(CrossReferenceFmt {
                ref_type_symbol: r.ascii_symbol,
                tags,
                note_id: r.shared_ids.note_id,
                comment_id: r.shared_ids.comment_id,
                reference_str,
            });
        }

        let indentation = self.indent_str.repeat(indent);

        for ((ref_type, note_id, comment_id), group) in &cross_ref_data
            .into_iter()
            .chunk_by(|item| (item.ref_type_symbol, item.note_id, item.comment_id))
        {
            let items: Vec<_> = group.collect();

            let tag_groups: Vec<String> = items
                .iter()
                .chunk_by(|item| item.tags.clone())
                .into_iter()
                .map(|(tags, sub_group)| {
                    let references = sub_group
                        .map(|item| item.reference_str.clone())
                        .join(config::ITEMS_SEP);
                    format!("{tags}{references}")
                })
                .collect();

            writeln!(
                self.writer,
                "{}X{}{}",
                indentation,
                ref_type,
                tag_groups.join(" ")
            )?;

            self.write_shared_items_from_ids(comment_id, note_id, indent + 1)?;
        }

        Ok(())
    }
}