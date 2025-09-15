use rusqlite::{Connection, Error as SqliteError};

use crate::pinyin;
use crate::config;
use crate::txt_parser::*;

use std::{fmt, mem};

use crate::common::SqliteId;

#[derive(Debug, PartialEq)]
struct CrossReferenceEntry {
    shared_id: SqliteId,
    ref_type: char,
    src_word_id: SqliteId,
    src_definition_id: Option<SqliteId>,
    dst_word: Word,
    dst_ext_def_id: Option<u32>,
    err_line_idx: usize,
}

#[derive(Debug)]
struct NoteReferenceEntry {
    target_shared_id: SqliteId,
    ext_note_id: u32,
    err_line_idx: usize,
}

#[derive(Debug, PartialEq, Copy, Clone)]
enum DictNode {
    Word((SqliteId, SqliteId)),                 // shared_id, word_id
    Pinyin((SqliteId, SqliteId)),               // shared_id, shared_pron_id
    Class(SqliteId),                            // class_id
    Definition((SqliteId, SqliteId, SqliteId)), // shared_id, word_id, definition_id
    CrossReference(SqliteId),                   // shared_id
}

#[derive(Debug)]
pub struct TxtToDbErrorLine {
    pub err_line_idx: usize,
    pub error: TxtToDbError,
}

#[derive(Debug)]
pub enum TxtToDbError {
    ParseError,
    SqliteError { source: SqliteError },
    InvalidAsciiTag(char),
    NoUsableParentNode,
    UnknownReferenceType(char),
    ReferenceTargetNotFound(String),
    NoteIdNotFound(u32),
}

pub type Result<T> = std::result::Result<T, TxtToDbError>;

impl fmt::Display for TxtToDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParseError => write!(f, "Parser Error"),
            Self::SqliteError { source } => write!(f, "{}", source),
            Self::InvalidAsciiTag(ascii_tag) => write!(f, "Invalid ASCII tag: {}", ascii_tag),
            Self::NoUsableParentNode => write!(
                f,
                "No usable parent node, check indentation and whether the entry is compatible to previous line."
            ),
            Self::UnknownReferenceType(ref_type) => {
                write!(f, "Unknown reference type X?: {}", ref_type)
            }
            Self::ReferenceTargetNotFound(word) => {
                write!(f, "Reference target not found: {}", word)
            }
            Self::NoteIdNotFound(id) => {
                write!(f, "No note with found for id: {}", id)
            }
        }
    }
}

impl From<SqliteError> for TxtToDbError {
    fn from(err: SqliteError) -> Self {
        Self::SqliteError { source: err }
    }
}

impl std::error::Error for TxtToDbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            TxtToDbError::ParseError => None,
            TxtToDbError::SqliteError { ref source } => Some(source),
            TxtToDbError::InvalidAsciiTag(_) => None,
            TxtToDbError::NoUsableParentNode => None,
            TxtToDbError::UnknownReferenceType(_) => None,
            TxtToDbError::ReferenceTargetNotFound(_) => None,
            TxtToDbError::NoteIdNotFound(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct TxtToDb<'a> {
    conn: &'a Connection,
    rank_counter: u64,
    line_stack: Vec<Vec<DictNode>>,
    cross_references: Vec<CrossReferenceEntry>, // references are added after all entries are in the DB
    note_references: Vec<NoteReferenceEntry>,
    err_lines: Vec<(String, LineInfo)>, // (word, line_info) keep line info for errors
    pub errors: Vec<TxtToDbErrorLine>,
}

impl<'a> TxtToDb<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        conn.execute_batch(config::DB_SCHEMA).unwrap();
        TxtToDb {
            conn,
            rank_counter: 0,
            line_stack: vec![],
            cross_references: vec![],
            note_references: vec![],
            err_lines: vec![],
            errors: vec![],
        }
    }

    pub fn txt_to_db(&mut self, lines: impl IntoIterator<Item = String>) {
        self.conn
            .execute_batch(
                "PRAGMA synchronous = OFF; PRAGMA journal_mode = MEMORY; BEGIN TRANSACTION",
            )
            .unwrap();
        let parser = ParserIterator::new(lines.into_iter());
        let mut cur_word = "header".to_owned();
        let mut cur_word_error = false;
        for line in parser {
            match line.parsed_line {
                Ok(parsed) => {
                    if let DictLine::Word(word_line) = &parsed {
                        cur_word = word_line
                            .first()
                            .and_then(|w| w.words.first().map(|v| v.trad.clone()))
                            .unwrap_or("unknown".to_owned());
                        cur_word_error = false;
                    }
                    if cur_word_error {
                        continue;
                    }
                    let (is_ok, keep_line) = self.add_line_to_db(&line.line, parsed);
                    cur_word_error = cur_word_error || !is_ok;
                    if keep_line {
                        self.err_lines.push((cur_word.clone(), line.line));
                    }
                }
                Err(_e) => {
                    self.errors.push(TxtToDbErrorLine {
                        err_line_idx: self.err_lines.len(),
                        error: TxtToDbError::ParseError,
                    });
                    self.err_lines.push((cur_word.clone(), line.line));
                    cur_word_error = true;
                }
            }
        }
        self.complete_cross_reference_entries();
        self.complete_id_reference_entries();
        self.conn.execute("COMMIT", ()).unwrap();
    }

    pub fn print_errors(&self) {
        for err in &self.errors {
            let (err_word, line_info) = &self.err_lines[err.err_line_idx];
            if line_info.source_line_num > 1 {
                println!(
                    "Error for {} in line {} to line {}:",
                    err_word,
                    line_info.source_line_start,
                    line_info.source_line_start + line_info.source_line_num
                );
            } else {
                println!(
                    "Error for {} in line {}:",
                    err_word, line_info.source_line_start
                );
            }
            println!("  {}", line_info.line);
            println!("  {}", err.error);
        }
    }

    fn add_tag_for_entry(
        &mut self,
        shared_id: SqliteId,
        tag_ascii: Option<char>,
        tag_txt: &str,
        tag_type: &str,
    ) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT OR IGNORE INTO dict_tag (tag, type, ascii_symbol) VALUES (?1,?2,?3)",
        )?;
        stmt.execute((tag_txt, tag_type, tag_ascii.map(|c| c.to_string())))?;

        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM dict_tag WHERE tag=?1 AND type=?2")?;
        let tag_id: SqliteId = stmt.query_row((tag_txt, tag_type), |row| row.get(0))?;

        let mut stmt = self
            .conn
            .prepare_cached("INSERT INTO dict_shared_tag (for_shared_id, tag_id) VALUES (?1,?2)")?;
        stmt.execute((shared_id, tag_id))?;
        Ok(())
    }

    fn add_tags_for_entry(
        &mut self,
        shared_id: SqliteId,
        entry_type: &DictNode,
        tags: &Tags,
    ) -> Result<()> {
        for tag in tags {
            let (ascii_tag, tag_txt, tag_type) = tag_to_txt(entry_type, tag)?;
            self.add_tag_for_entry(shared_id, ascii_tag, &tag_txt, &tag_type)?;
        }
        Ok(())
    }

    fn create_shared_entry(&mut self) -> Result<SqliteId> {
        self.rank_counter += 1;
        let mut stmt = self
            .conn
            .prepare_cached("INSERT INTO dict_shared (rank) VALUES (?1)")?;
        stmt.execute((self.rank_counter,))?;
        Ok(self.conn.last_insert_rowid())
    }

    fn create_word_entry(&mut self, word: &Word, tags: &Tags) -> Result<DictNode> {
        let trad = &word.trad;
        let simp = word.simp.as_ref().unwrap_or(&word.trad);
        let shared_id = self.create_shared_entry()?;
        let mut stmt = self
            .conn
            .prepare_cached("INSERT INTO dict_word (shared_id, trad, simp) VALUES (?1,?2,?3)")?;
        stmt.execute((shared_id, trad, simp))?;
        let word_entry = DictNode::Word((shared_id, self.conn.last_insert_rowid()));
        self.add_tags_for_entry(shared_id, &word_entry, tags)?;
        Ok(word_entry)
    }

    fn create_pinyin_entry(&mut self, pinyin_num: &str, tags: &Tags) -> Result<DictNode> {
        let shared_id = self.create_shared_entry()?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT OR IGNORE INTO dict_pron (pinyin_num, pinyin_mark) VALUES (?1,?2)",
        )?;
        stmt.execute((pinyin_num, pinyin::pinyin_mark_from_num(pinyin_num)))?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM dict_pron WHERE pinyin_num=?1")?;
        let pron_id: SqliteId = stmt.query_row((pinyin_num,), |row| row.get(0))?;
        let mut stmt = self
            .conn
            .prepare_cached("INSERT INTO dict_shared_pron (shared_id, pron_id) VALUES (?1,?2)")?;
        stmt.execute((shared_id, pron_id))?;
        let shared_pron_id = self.conn.last_insert_rowid();
        let pinyin_entry = DictNode::Pinyin((shared_id, shared_pron_id));
        self.add_tags_for_entry(shared_id, &pinyin_entry, &tags)?;

        Ok(pinyin_entry)
    }

    fn create_class_entry(&self, class_name: &str) -> Result<Vec<DictNode>> {
        let mut stmt = self
            .conn
            .prepare_cached("INSERT OR IGNORE INTO dict_class (name) VALUES (?1)")?;
        stmt.execute((class_name,))?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM dict_class WHERE name=?1")?;
        let class_id: SqliteId = stmt.query_row((class_name,), |row| row.get(0))?;
        Ok(vec![DictNode::Class(class_id)])
    }

    fn create_definition_entry(
        &mut self,
        word_id: SqliteId,
        definition_tag: &DefinitionTag,
        class: SqliteId,
    ) -> Result<DictNode> {
        let shared_id = self.create_shared_entry()?;
        let mut stmt = self
            .conn
            .prepare_cached("INSERT INTO dict_definition (shared_id, word_id, definition, ext_def_id, class_id) VALUES (?1,?2,?3,?4,?5)")?;
        stmt.execute((
            shared_id,
            word_id,
            &definition_tag.definition,
            definition_tag.id,
            class,
        ))?;
        let definition_id = self.conn.last_insert_rowid();
        let definition_entry = DictNode::Definition((shared_id, word_id, definition_id));
        self.add_tags_for_entry(shared_id, &definition_entry, &definition_tag.tags)?;
        Ok(definition_entry)
    }

    fn create_pron_definition_entry(
        &mut self,
        shared_pron_id: SqliteId,
        definition_id: SqliteId,
    ) -> Result<SqliteId> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO dict_pron_definition (shared_pron_id, definition_id) VALUES (?1,?2)",
        )?;
        stmt.execute((shared_pron_id, definition_id))?;
        Ok(self.conn.last_insert_rowid())
    }

    fn create_cross_reference_entry(
        &mut self,
        ref_type: char,
        word_id_src: SqliteId,
        definition_id_src: Option<SqliteId>,
        word_dst: Word,
        ext_def_id_dst: Option<u32>,
        tags: &Tags,
    ) -> Result<DictNode> {
        let shared_id = self.create_shared_entry()?;
        let ref_entry = DictNode::CrossReference(shared_id);
        self.add_tags_for_entry(shared_id, &ref_entry, tags)?;
        self.cross_references.push(CrossReferenceEntry {
            shared_id: shared_id,
            ref_type: ref_type,
            src_word_id: word_id_src,
            src_definition_id: definition_id_src,
            dst_word: word_dst,
            dst_ext_def_id: ext_def_id_dst,
            err_line_idx: self.err_lines.len(),
        });

        Ok(ref_entry)
    }

    fn complete_cross_reference_entries(&mut self) {
        for reference in mem::take(&mut self.cross_references) {
            // identify target word and definition
            let trad = &reference.dst_word.trad;
            let simp = &reference
                .dst_word
                .simp
                .as_ref()
                .unwrap_or(&reference.dst_word.trad);
            let potential_dst_word_id: std::result::Result<SqliteId, rusqlite::Error> =
                self.conn.query_row(
                    "SELECT id FROM dict_word WHERE trad=?1 AND simp=?2",
                    (trad, simp),
                    |row| row.get(0),
                );
            let Ok(dst_word_id) = potential_dst_word_id else {
                self.errors.push(TxtToDbErrorLine {
                    err_line_idx: reference.err_line_idx,
                    error: TxtToDbError::ReferenceTargetNotFound(format!(
                        "{}",
                        &reference.dst_word
                    )),
                });
                continue;
            };
            let dst_definition_id: Option<SqliteId> = {
                if let Some(dst_ext_ref_id) = reference.dst_ext_def_id {
                    let potential_dst_definition_id = self.conn.query_row(
                        "SELECT id FROM dict_definition WHERE word_id=?1 AND ext_def_id=?2",
                        (dst_word_id, dst_ext_ref_id),
                        |row| row.get(0),
                    );
                    let Ok(dst_definition_id) = potential_dst_definition_id else {
                        self.errors.push(TxtToDbErrorLine {
                            err_line_idx: reference.err_line_idx,
                            error: TxtToDbError::ReferenceTargetNotFound(format!(
                                "{}D#{}",
                                &reference.dst_word, dst_ext_ref_id
                            )),
                        });
                        continue;
                    };
                    Some(dst_definition_id)
                } else {
                    None
                }
            };

            // create/get reference type
            let Some((ref_type_full, is_symmetric)) = config::get_ref_type(&reference.ref_type) else {
                self.errors.push(TxtToDbErrorLine {
                        err_line_idx: reference.err_line_idx,
                        error: TxtToDbError::UnknownReferenceType(reference.ref_type),
                    });
                    continue;
            };

            self.conn
                .execute(
                    "INSERT OR IGNORE INTO dict_ref_type (type, ascii_symbol, is_symmetric) VALUES (?1,?2,?3)",
                    (ref_type_full, reference.ref_type.to_string(), is_symmetric),
                )
                .unwrap();
            let ref_type_id: SqliteId = self
                .conn
                .query_row(
                    "SELECT id FROM dict_ref_type WHERE type=?1 ",
                    (ref_type_full,),
                    |row| row.get(0),
                )
                .unwrap();
            // create reference and link to shared_id
            let mut stmt = self
            .conn
            .prepare_cached("INSERT INTO dict_reference (shared_id, ref_type_id, word_id_src, definition_id_src, word_id_dst, definition_id_dst) VALUES (?1,?2,?3,?4,?5,?6)").unwrap();
            stmt.execute((
                reference.shared_id,
                ref_type_id,
                reference.src_word_id,
                reference.src_definition_id,
                dst_word_id,
                dst_definition_id,
            ))
            .unwrap();
        }
    }

    fn complete_id_reference_entries(&mut self) {
        for reference in mem::take(&mut self.note_references) {
            let note_id = self.conn.query_row(
                "SELECT id FROM dict_note WHERE ext_note_id=?1",
                (reference.ext_note_id,),
                |row| row.get(0),
            );
            let Ok(note_id) = note_id else {
                self.errors.push(TxtToDbErrorLine {
                    err_line_idx: reference.err_line_idx,
                    error: TxtToDbError::NoteIdNotFound(reference.ext_note_id),
                });
                continue;
            };
            // unwrap since note_id and shared_id must be ok here
            self.add_note_to_entry(note_id, reference.target_shared_id).unwrap();
        }
    }

    fn create_note(&self, ext_note_id: u32, note_txt: &str) -> Result<SqliteId> {
        self.conn.execute(
            "INSERT INTO dict_note (note, ext_note_id) VALUES (?1,?2)",
            (note_txt, ext_note_id),
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    fn add_note_to_entry(&self, note_id: SqliteId, target_shared_id: SqliteId) -> Result<usize> {
        let rows_updated = self.conn.execute(
            "UPDATE dict_shared SET note_id=?1 WHERE id=?2 ",
            (note_id, target_shared_id),
        )?;
        if rows_updated == 0 {
            return Err(TxtToDbError::NoUsableParentNode);
        }
        Ok(1)
    }

    fn create_comment(&mut self, comment_txt: &str) -> Result<SqliteId> {
        self.conn.execute(
            "INSERT INTO dict_comment (comment) VALUES (?1)",
            (comment_txt,),
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    fn add_comment_to_entry(
        &mut self,
        comment_id: SqliteId,
        target_shared_id: SqliteId,
    ) -> Result<usize> {
        let rows_updated = self.conn.execute(
            "UPDATE dict_shared SET comment_id=?1 WHERE id=?2 ",
            (comment_id, target_shared_id),
        )?;
        if rows_updated == 0 {
            return Err(TxtToDbError::NoUsableParentNode);
        }
        Ok(1)
    }

    fn add_line_to_db(&mut self, line_info: &LineInfo, line: DictLine) -> (bool, bool) {
        self.line_stack.truncate(line_info.indentation);

        let (line_items, keep_line) = match line {
            DictLine::Word(word_tag_groups) => (self.add_word_line_to_db(word_tag_groups), false),
            DictLine::Pinyin(pinyin_tag_groups) => {
                (self.add_pinyin_line_to_db(pinyin_tag_groups), false)
            }
            DictLine::Class(class_name) => (self.create_class_entry(&class_name), false),
            DictLine::Definition(definition_tag) => {
                (self.add_definition_line_to_db(definition_tag), false)
            }
            DictLine::CrossReference(reference_tag_groups) => (
                self.add_cross_reference_line_to_db(reference_tag_groups),
                true,
            ),
            DictLine::Note(note) => {
                let is_link = note.is_link;
                (self.add_note_line_to_db(note), is_link)
            }
            DictLine::Comment(comment) => (self.add_comment_line_to_db(comment), false),
        };
        match line_items {
            Ok(line_items) => {
                self.line_stack.push(line_items);
                (true, keep_line)
            }
            Err(r) => {
                self.errors.push(TxtToDbErrorLine {
                    err_line_idx: self.err_lines.len(),
                    error: r,
                });
                (false, true)
            }
        }
    }

    fn add_word_line_to_db(&mut self, word_tag_groups: Vec<WordTagGroup>) -> Result<Vec<DictNode>> {
        let mut line_items = vec![];
        for word_tag_group in word_tag_groups {
            for word in &word_tag_group.words {
                let word_entry = self.create_word_entry(&word, &word_tag_group.tags)?;
                if line_items.is_empty() {
                    line_items.push(word_entry);
                } else {
                    // Ignore more words, even though the parser can still parse a list of word groups. The original
                    // intention was to have the option for several variants on one line.
                }
            }
        }
        Ok(line_items)
    }

    fn add_comment_line_to_db(&mut self, comment: String) -> Result<Vec<DictNode>> {
        let comment_id = self.create_comment(&comment)?;
        if self.line_stack.is_empty() {
            // create new shared entry to attach comment for the initial header comment
            if self.rank_counter == 0 {
                let shared_id = self.create_shared_entry()?;
                self.add_comment_to_entry(comment_id, shared_id)?;
            } else {
                return Err(TxtToDbError::NoUsableParentNode);
            }
        } else {
            let mut num_targets = 0;
            if let Some(prev_dict_nodes) = self.line_stack.last() {
                for dict_node in prev_dict_nodes.clone() {
                    let shared_id = get_shared_id_for_dict_node(&dict_node)?;
                    num_targets += self.add_comment_to_entry(comment_id, shared_id)?;
                }
            }
            if num_targets == 0 {
                return Err(TxtToDbError::NoUsableParentNode);
            }
        }
        Ok(vec![])
    }

    fn add_note_line_to_db(&mut self, note: Note) -> Result<Vec<DictNode>> {
        let note_id: Option<SqliteId> = {
            if !note.is_link {
                Some(self.create_note(note.id, &note.note)?)
            } else {
                None
            }
        };
        let mut num_targets = 0;
        if let Some(prev_dict_nodes) = self.line_stack.last() {
            for dict_node in prev_dict_nodes.clone() {
                let shared_id = get_shared_id_for_dict_node(&dict_node)?;
                if note.is_link {
                    self.note_references.push(NoteReferenceEntry {
                        target_shared_id: shared_id,
                        ext_note_id: note.id,
                        err_line_idx: self.err_lines.len(),
                    });
                    num_targets += 1;
                } else {
                    num_targets += self.add_note_to_entry(note_id.unwrap(), shared_id)?;
                }
            }
        }
        if num_targets == 0 {
            return Err(TxtToDbError::NoUsableParentNode);
        }
        Ok(vec![])
    }

    fn add_cross_reference_line_to_db(
        &mut self,
        reference_tag_groups: Vec<ReferenceTagGroup>,
    ) -> Result<Vec<DictNode>> {
        let mut line_items = vec![];
        if let Some(DictNode::Word((_, src_word_id))) =
            self.line_stack.first().and_then(|v| v.first().copied())
        {
            let src_definition_id: Option<SqliteId> = {
                if let Some(DictNode::Definition((_, _, src_definition_id))) =
                    self.line_stack.last().and_then(|v| v.first().copied())
                {
                    Some(src_definition_id)
                } else {
                    None
                }
            };

            for reference_tag_group in reference_tag_groups {
                for reference in reference_tag_group.references {
                    let dst_definition_id: Option<u32> = reference.target_id.map(|i| i.1);
                    let ref_entry = self.create_cross_reference_entry(
                        reference_tag_group.ref_type,
                        src_word_id,
                        src_definition_id,
                        reference.target_word,
                        dst_definition_id,
                        &reference_tag_group.tags,
                    )?;
                    line_items.push(ref_entry);
                }
            }
        }
        Ok(line_items)
    }

    fn add_definition_line_to_db(
        &mut self,
        definition_tag: DefinitionTag,
    ) -> Result<Vec<DictNode>> {
        let mut line_items = vec![];
        if let Some(DictNode::Word((_, word_id))) = self.line_stack.first().and_then(|v| v.first())
        {
            if let Some(DictNode::Class(class_id)) = self.line_stack.get(2).and_then(|v| v.first())
            {
                let definition_entry =
                    self.create_definition_entry(*word_id, &definition_tag, *class_id)?;
                if let DictNode::Definition((_, _, definition_id)) = definition_entry {
                    // add links between definition and pronunciation
                    let pinyin_entries = self.line_stack.get(1).unwrap().clone();
                    for pinyin_entry in pinyin_entries {
                        if let DictNode::Pinyin((_, shared_pron_id)) = pinyin_entry {
                            self.create_pron_definition_entry(shared_pron_id, definition_id)?;
                        } else {
                            return Err(TxtToDbError::NoUsableParentNode);
                        }
                    }
                } else {
                    debug_assert!(false)
                }

                line_items.push(definition_entry);
            } else {
                return Err(TxtToDbError::NoUsableParentNode);
            }
        } else {
            return Err(TxtToDbError::NoUsableParentNode);
        }
        Ok(line_items)
    }

    fn add_pinyin_line_to_db(
        &mut self,
        pinyin_tag_groups: Vec<PinyinTagGroup>,
    ) -> Result<Vec<DictNode>> {
        let mut line_items = vec![];
        for PinyinTagGroup { pinyins, ref tags } in pinyin_tag_groups {
            for pinyin in pinyins {
                let pinyin_entry = self.create_pinyin_entry(&pinyin, tags)?;
                line_items.push(pinyin_entry);
                // if pinyin is nested one level below another pinyin, also add it to that list to make the link to definitions easier
                if self.line_stack.len() == 2 {
                    self.line_stack[1].push(pinyin_entry);
                }
            }
        }
        Ok(line_items)
    }
}

fn get_shared_id_for_dict_node(dict_node: &DictNode) -> Result<SqliteId> {
    let shared_id = match dict_node {
        DictNode::Word((shared_id, _)) => shared_id,
        DictNode::Pinyin((shared_id, _)) => shared_id,
        DictNode::Class(_) => {
            return Err(TxtToDbError::NoUsableParentNode);
        }
        DictNode::Definition((shared_id, _, _)) => shared_id,
        DictNode::CrossReference(shared_id) => shared_id,
    };
    Ok(*shared_id)
}


fn tag_to_txt(entry_type: &DictNode, tag: &Tag) -> Result<(Option<char>, String, String)> {
    match tag {
        Tag::Full(full_tag) => {
            return Ok((None, full_tag.to_owned(), "definition".to_owned()));
        }
        Tag::Ascii(ascii_tag) => {
            let tag_str = match entry_type {
                DictNode::Word(_) => match ascii_tag {
                    _ => config::tag_to_txt_ascii_common(ascii_tag),
                },
                DictNode::Definition(_) => match ascii_tag {
                    _ => config::tag_to_txt_ascii_common(ascii_tag),
                },
                _ => config::tag_to_txt_ascii_common(ascii_tag),
            };
            if let Some(t) = tag_str {
                Ok((Some(*ascii_tag), t.0.to_owned(), t.1.to_owned()))
            } else {
                Err(TxtToDbError::InvalidAsciiTag(*ascii_tag))
            }
        }
    }
}
