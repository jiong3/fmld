// LLM generated with larger modifications, context: txt_parser.rs and txt_to_db.rs

use itertools::Itertools;
use rusqlite::{Connection, Error as SqliteError, Row};
use std::collections::HashSet;
use std::fmt;
use std::io::Write;

use crate::common;
use crate::common::SqliteId;
use crate::config;
use crate::db_read;
use crate::db_read::DefinitionEntry;

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

// --- Data Structures to hold query results ---

struct PinyinData {
    pinyin_num: String,
    note_id: Option<SqliteId>,
    comment_id: Option<SqliteId>,
    tags: String,
}

struct CrossReferenceData {
    ref_type_symbol: String,
    tags: String,
    note_id: Option<SqliteId>,
    comment_id: Option<SqliteId>,
    reference_str: String,
}

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
        }
    }

    pub fn generate_txt_file(&mut self, limit_to_word: Option<&str>) -> Result<()> {
        let definitions = db_read::read_definitions(self.conn)?;

        self.write_shared_items(1, 0)?; // header comment

        for definition_entry in definitions {
            if let Some(stop_word) = limit_to_word {
                if definition_entry.trad == stop_word {
                    break;
                }
            }

            if definition_entry.new_word {
                self.write_word_entry(&definition_entry)?;
            }
            if definition_entry.new_pinyin {
                self.write_pinyin_entries(
                    definition_entry.def_id,
                    &definition_entry.pinyin_shared_ids,
                )?;
            }
            if definition_entry.new_class {
                self.write_class_entry(&definition_entry.class_name)?;
            }
            self.write_definition_entry(&definition_entry)?;
        }

        Ok(())
    }

    fn write_word_entry(&mut self, entry: &DefinitionEntry) -> Result<()> {
        let tags = self.get_formatted_tags(entry.word_shared_id)?;

        let mut word_str = common::format_word_def(&entry.trad, &entry.simp, None);

        // collect optional variants of the word
        let mut variants = vec![];

        let mut stmt = self
            .conn
            .prepare_cached("SELECT trad, simp FROM dict_word WHERE variant_of = ?1")?;
        let mut rows = stmt.query([entry.word_id])?;

        while let Some(row) = rows.next()? {
            let trad: String = row.get(0)?;
            let simp: String = row.get(1)?;
            variants.push(common::format_word_def(&trad, &simp, None));
        }

        if !variants.is_empty() {
            variants.sort(); // keep order deterministic
            word_str.push_str(config::ITEMS_SEP);
            word_str.push_str(&variants.join(config::ITEMS_SEP));
        }

        writeln!(self.writer, "W{tags}{word_str}")?;
        self.write_shared_items(entry.word_shared_id, 1)?;
        self.write_cross_references(entry.word_id, None, 1)?;
        Ok(())
    }

    fn write_pinyin_entries(
        &mut self,
        def_id: SqliteId,
        pinyin_shared_ids: &[SqliteId],
    ) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare_cached(
                r"
            SELECT
                p.pinyin_num,
                p_s.note_id,
                p_s.comment_id
            FROM dict_definition def
            LEFT JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
            LEFT JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
            LEFT JOIN dict_pron p ON sp.pron_id = p.id
            LEFT JOIN dict_shared p_s ON sp.shared_id = p_s.id
            WHERE def.id = ?1 AND p_s.id = ?2
            ",
            )
            .unwrap();

        // 1. Fetch all data into a Vec of PinyinData structs
        let pinyin_data: Result<Vec<PinyinData>> = pinyin_shared_ids
            .iter()
            .map(|pron_shared_id| {
                let (pinyin_num, note_id, comment_id) = stmt
                    .query_row([def_id, *pron_shared_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })?;
                let tags = self.get_formatted_tags(*pron_shared_id)?;
                Ok(PinyinData {
                    pinyin_num,
                    note_id,
                    comment_id,
                    tags,
                })
            })
            .collect();

        let pinyin_data = pinyin_data?;

        // group the data and format it into lines
        let mut indent_level = 1;
        for ((note_id, comment_id), tag_group) in &pinyin_data
            .into_iter()
            .chunk_by(|item| (item.note_id, item.comment_id))
        {
            let tags_pinyins = tag_group
                .into_iter()
                .chunk_by(|item| item.tags.clone())
                .into_iter()
                .map(|(tags, tag_group)| {
                    let pinyins = tag_group
                        .map(|item| item.pinyin_num)
                        .join(config::ITEMS_SEP);
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
        let tags = self.get_formatted_tags(entry.def_shared_id)?;
        let mut def_id_indent = 3 + entry.nested_level;
        writeln!(
            self.writer,
            "{}D{}{}{}",
            self.indent_str.repeat(def_id_indent),
            entry.ext_def_id,
            tags,
            format_multiline(&entry.definition, def_id_indent, &self.indent_str),
        )?;
        self.write_shared_items(entry.def_shared_id, def_id_indent + 1)?;
        self.write_cross_references(entry.word_id, Some(entry.def_id), def_id_indent + 1)?;
        Ok(())
    }

    fn get_formatted_tags(&self, shared_id: SqliteId) -> rusqlite::Result<String> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.ascii_symbol, t.tag, t.type FROM dict_shared_tag st JOIN dict_tag t ON st.tag_id = t.id WHERE st.for_shared_id = ?1",
        )?;
        let mut rows = stmt.query([shared_id])?;
        let mut ascii_tags = vec![];
        let mut full_tags = vec![];

        while let Some(row) = rows.next()? {
            let ascii_symbol: Option<String> = row.get(0)?;
            let tag: String = row.get(1)?;

            if let Some(symbol) = ascii_symbol {
                if !symbol.is_empty() {
                    ascii_tags.push(symbol);
                }
            } else {
                full_tags.push(format!("#{tag}"));
            }
        }
        // sort ascii tags by defined order, unwrap() is safe due to previous is_empty() check
        ascii_tags.sort_by_key(|x| {
            config::tag_to_txt_ascii_common(x.chars().nth(0).unwrap())
                .unwrap_or(("", "", 0))
                .2
        });
        // sort full tags with default order
        full_tags.sort();

        let space = if full_tags.is_empty() { "" } else { " " };
        if ascii_tags.is_empty() && full_tags.is_empty() {
            // leaving out the || would require checks in case there is a tag group without tags coming after a group with tags on the same line
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

    fn write_shared_items(&mut self, shared_id: SqliteId, indent: usize) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT comment_id, note_id FROM dict_shared WHERE id = ?1")?;
        let (comment_id, note_id): (Option<SqliteId>, Option<SqliteId>) =
            stmt.query_row([shared_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        self.write_shared_items_from_ids(comment_id, note_id, indent)
    }

    fn write_shared_items_from_ids(
        &mut self,
        comment_id: Option<SqliteId>,
        note_id: Option<SqliteId>,
        indent: usize,
    ) -> Result<()> {
        let indentation = self.indent_str.repeat(indent);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT comment FROM dict_comment WHERE id = ?1")?;
        // Write Comment
        if let Some(id) = comment_id {
            let comment: String = stmt.query_row([id], |row| row.get(0))?;
            let comment = format_multiline(&comment, indent, &self.indent_str);
            writeln!(self.writer, "{indentation}# {comment}")?;
        }
        // Write Note
        if let Some(id) = note_id {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT note, ext_note_id FROM dict_note WHERE id = ?1")?;
            let (note_txt, ext_id): (String, SqliteId) =
                stmt.query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))?;
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

    /// Writes cross-references for a given word or definition.
    ///
    /// This function implements the specified grouping logic:
    /// 1. All references are fetched from the database, ordered by their rank.
    /// 2. They are grouped by the combination of (`ref_type_symbol`, `note_id`, `comment_id`).
    /// 3. Each of these primary groups results in a new, single `X...` output line.
    /// 4. Within each line, references are further sub-grouped by their tags to
    ///    construct the final formatted string.
    fn write_cross_references(
        &mut self,
        src_word_id: SqliteId,
        src_def_id: Option<SqliteId>,
        indent: usize,
    ) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            r"
            SELECT
                rt.ascii_symbol,
                r.shared_id,
                s.note_id,
                s.comment_id,
                w_dst.trad,
                w_dst.simp,
                def_dst.ext_def_id
            FROM dict_reference r
            JOIN dict_shared s ON r.shared_id = s.id
            JOIN dict_ref_type rt ON r.ref_type_id = rt.id
            JOIN dict_word w_dst ON r.word_id_dst = w_dst.id
            LEFT JOIN dict_definition def_dst ON r.definition_id_dst = def_dst.id
            LEFT JOIN dict_definition def_src ON r.definition_id_src = def_src.id
            WHERE
                r.word_id_src = ?1 AND
                ((?2 IS NULL AND r.definition_id_src IS NULL) OR def_src.id = ?2)
            ORDER BY s.rank, s.rank_relative -- NULLS FIRST default
        ",
        )?;

        // 1. Fetch all data into a Vec of CrossReferenceData structs.
        let cross_ref_data_result: rusqlite::Result<Vec<CrossReferenceData>> = stmt
            .query_map((src_word_id, src_def_id), |row| {
                let shared_id: SqliteId = row.get(1)?;
                let trad: String = row.get(4)?;
                let simp: String = row.get(5)?;
                let dst_ext_def_id: Option<u32> = row.get(6)?;
                let reference_str = common::format_word_def(&trad, &simp, dst_ext_def_id);

                Ok(CrossReferenceData {
                    ref_type_symbol: row.get(0)?,
                    tags: self.get_formatted_tags(shared_id)?,
                    note_id: row.get(2)?,
                    comment_id: row.get(3)?,
                    reference_str,
                })
            })?
            .collect();

        let cross_ref_data = cross_ref_data_result?;
        if cross_ref_data.is_empty() {
            return Ok(());
        }

        let indentation = self.indent_str.repeat(indent);

        // 2. Primary Grouping: Group by ref_type, note_id, and comment_id.
        // Each chunk from this operation represents exactly one line of output.
        for ((ref_type, note_id, comment_id), group) in &cross_ref_data
            .into_iter()
            .chunk_by(|item| (item.ref_type_symbol.clone(), item.note_id, item.comment_id))
        {
            let items: Vec<_> = group.collect();

            // 3. Secondary Grouping (within the line): Group by tags.
            // These will be joined with spaces on the same line.
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

            // Write the fully constructed line.
            writeln!(
                self.writer,
                "{}X{}{}",
                indentation,
                ref_type,
                tag_groups.join(" ")
            )?;

            // Write any associated comments or notes for this line, indented one level deeper.
            self.write_shared_items_from_ids(comment_id, note_id, indent + 1)?;
        }

        Ok(())
    }
}
