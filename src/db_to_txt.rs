// LLM generated with larger modifications
// LLM input: parser file, txt_to_db.rs

use itertools::Itertools;
use rusqlite::{Connection, Error as SqliteError, Row};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Write;

use crate::config;

type SqliteId = i64;

const INDENT_STR: &str = " "; // only one byte characters allowed
const WORD_SEP: &str = "／"; // TODO shared module?
const ITEMS_SEP: &str = "; ";

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
            DbToTxtError::SqliteError(e) => write!(f, "Database error: {}", e),
            DbToTxtError::IoError(e) => write!(f, "I/O error: {}", e),
            DbToTxtError::InvalidDbData(s) => write!(f, "Invalid data in DB: {}", s),
        }
    }
}

impl From<SqliteError> for DbToTxtError {
    fn from(err: SqliteError) -> Self {
        DbToTxtError::SqliteError(err)
    }
}

impl From<std::io::Error> for DbToTxtError {
    fn from(err: std::io::Error) -> Self {
        DbToTxtError::IoError(err)
    }
}

pub type Result<T> = std::result::Result<T, DbToTxtError>;

// --- Data Structures to hold query results ---

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PinyinInfo {
    pinyin_num: String,
    shared_id: SqliteId,
    note_id: Option<SqliteId>,
    comment_id: Option<SqliteId>,
}

#[derive(Debug)]
struct DefinitionEntry {
    word_id: SqliteId,
    word_shared_id: SqliteId,
    trad: String,
    simp: String,
    pinyin_shared_ids: Vec<SqliteId>,
    class_id: SqliteId,
    class_name: String,
    def_id: SqliteId,
    def_shared_id: SqliteId,
    ext_def_id: u32,
    definition: String,
}

// A helper struct to hold the fetched data
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

fn format_word(trad: &str, simp: &str) -> String {
    if trad == simp {
        trad.to_owned()
    } else {
        format!("{}{}{}", trad, WORD_SEP, simp)
    }
}

// --- Main Struct and Implementation ---

pub struct DbToTxt<'a> {
    conn: &'a Connection,
    writer: &'a mut dyn Write,
    written_notes: HashSet<SqliteId>,
}

impl<'a> DbToTxt<'a> {
    pub fn new(conn: &'a Connection, writer: &'a mut dyn Write) -> Self {
        DbToTxt {
            conn,
            writer,
            written_notes: HashSet::new(),
        }
    }

    pub fn generate_txt_file(&mut self) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                w.id AS word_id,
                w.shared_id AS word_shared_id,
                w.trad,
                w.simp,
                c.id AS class_id,
                c.name AS class_name,
                def.id AS def_id,
                def.shared_id AS def_shared_id,
                def.ext_def_id,
                def.definition,
                GROUP_CONCAT(p_s.id ORDER BY p_s.rank, p_s.rank_relative)
            FROM dict_definition def
            JOIN dict_shared s ON def.shared_id = s.id
            JOIN dict_word w ON def.word_id = w.id
            JOIN dict_class c ON def.class_id = c.id
            LEFT JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
            LEFT JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
            LEFT JOIN dict_pron p ON sp.pron_id = p.id
            LEFT JOIN dict_shared p_s ON sp.shared_id = p_s.id
            GROUP BY def.id
            ORDER BY s.rank, s.rank_relative;
            "#,
            )
            .unwrap();

        let mut rows = stmt.query([])?;
        let mut last_word_id = -1;
        let mut last_pinyin_shared_ids = vec![];
        let mut last_class_id = -1;

        self.write_shared_items(1, 0)?; // header comment

        while let Some(row) = rows.next()? {
            // TODO for loop?
            let definition_entry = self.row_to_definition_entry(row)?;

            // 1. Word Entry
            if definition_entry.word_id != last_word_id {
                self.write_word_entry(&definition_entry)?;
                last_word_id = definition_entry.word_id;
                // Reset child states when word changes
                last_pinyin_shared_ids.clear();
                last_class_id = -1;
            }

            // 2. Pinyin Entry
            if definition_entry.pinyin_shared_ids != last_pinyin_shared_ids {
                self.write_pinyin_entries(
                    definition_entry.def_id,
                    &definition_entry.pinyin_shared_ids,
                )?;
                last_pinyin_shared_ids = definition_entry.pinyin_shared_ids.clone();
            }

            // 3. Class Entry
            if definition_entry.class_id != last_class_id {
                self.write_class_entry(&definition_entry.class_name)?;
                last_class_id = definition_entry.class_id;
            }

            // 4. Definition Entry
            self.write_definition_entry(&definition_entry)?;
        }

        Ok(())
    }

    fn row_to_definition_entry(&self, row: &Row) -> Result<DefinitionEntry> {
        let pinyin_shared_ids_str: Option<String> = row.get(10)?;
        let pinyin_shared_ids = pinyin_shared_ids_str
            .unwrap()
            .split(',')
            .map(|s| s.parse::<SqliteId>().unwrap())
            .collect();

        Ok(DefinitionEntry {
            word_id: row.get("word_id")?,
            word_shared_id: row.get("word_shared_id")?,
            trad: row.get("trad")?,
            simp: row.get("simp")?,
            pinyin_shared_ids,
            class_id: row.get("class_id")?,
            class_name: row.get("class_name")?,
            def_id: row.get("def_id")?,
            def_shared_id: row.get("def_shared_id")?,
            ext_def_id: row.get("ext_def_id")?,
            definition: row.get("definition")?,
        })
    }

    fn write_word_entry(&mut self, entry: &DefinitionEntry) -> Result<()> {
        let tags = self.get_formatted_tags(entry.word_shared_id)?;
        let word_str = format_word(&entry.trad, &entry.simp);
        // TODO character variants (Xv reference, same word with different characters) should be listed in the same line, separated by ;
        writeln!(self.writer, "W{}{}", tags, word_str)?;
        self.write_shared_items(entry.word_shared_id, 1)?;
        self.write_cross_references(entry.word_id, None, 1)?;
        Ok(())
    }

    fn write_pinyin_entries(
        &mut self,
        def_id: SqliteId,
        pinyin_shared_ids: &Vec<SqliteId>,
    ) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare_cached(
                r#"
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
            "#,
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
                    let pinyins = tag_group.map(|item| item.pinyin_num).join(ITEMS_SEP);
                    format!("{}{}", tags, pinyins)
                })
                .join(" ");

            writeln!(
                self.writer,
                "{}P{}",
                INDENT_STR.repeat(indent_level),
                tags_pinyins
            )?;
            self.write_shared_items_from_ids(comment_id, note_id, indent_level + 1)?;
            indent_level = 2;
        }

        Ok(())
    }

    fn write_class_entry(&mut self, class_name: &str) -> Result<()> {
        writeln!(self.writer, "{}C {}", INDENT_STR.repeat(2), class_name)?;
        Ok(())
    }

    fn write_definition_entry(&mut self, entry: &DefinitionEntry) -> Result<()> {
        let tags = self.get_formatted_tags(entry.def_shared_id)?;
        writeln!(
            self.writer,
            "{}D{}{}{}",
            INDENT_STR.repeat(3),
            entry.ext_def_id,
            tags,
            format_multiline(&entry.definition, 3, INDENT_STR),
        )?;
        self.write_shared_items(entry.def_shared_id, 4)?;
        self.write_cross_references(entry.word_id, Some(entry.def_id), 4)?;
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
                full_tags.push(format!("#{}", tag));
            }
        }
        // sort ascii tags by defined order, unwrap() is safe due to previous is_empty() check
        ascii_tags.sort_by_key(|x| {
            config::tag_to_txt_ascii_common(&x.chars().nth(0).unwrap())
                .unwrap_or(("", "", 0))
                .2
        });
        // sort full tags with default order
        full_tags.sort();

        let space = if full_tags.is_empty() { "" } else { " " };
        if ascii_tags.is_empty() && full_tags.is_empty() {
            // leaving out the || would require checks in case there is a tag group without tags coming after a group with tags on the same line
            Ok("|| ".to_owned())
        } else {
            Ok(format!(
                "|{}{}{}| ",
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
        let indentation = INDENT_STR.repeat(indent);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT comment FROM dict_comment WHERE id = ?1")?;
        // Write Comment
        if let Some(id) = comment_id {
            let comment: String = stmt.query_row([id], |row| row.get(0))?;
            let comment = format_multiline(&comment, indent, INDENT_STR);
            writeln!(self.writer, "{}# {}", indentation, comment)?;
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
                writeln!(self.writer, "{}N->{}", indentation, ext_id)?;
            } else {
                let note_txt = format_multiline(&note_txt, indent, INDENT_STR);
                writeln!(self.writer, "{}N{} {}", indentation, ext_id, note_txt)?;
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
            r#"
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
            ORDER BY s.rank, s.rank_relative
        "#,
        )?;

        // 1. Fetch all data into a Vec of CrossReferenceData structs.
        let cross_ref_data_result: rusqlite::Result<Vec<CrossReferenceData>> = stmt
            .query_map((src_word_id, src_def_id), |row| {
                let shared_id: SqliteId = row.get(1)?;
                let trad: String = row.get(4)?;
                let simp: String = row.get(5)?;
                let dst_ext_def_id: Option<u32> = row.get(6)?;
                let word_str = format_word(&trad, &simp);
                let reference_str = if let Some(id) = dst_ext_def_id {
                    format!("{}#D{}", word_str, id)
                } else {
                    word_str
                };

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

        let indentation = INDENT_STR.repeat(indent);

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
                        .join(ITEMS_SEP);
                    format!("{}{}", tags, references)
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
