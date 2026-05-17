use crate::common::SqliteId;
use rusqlite::{params, Connection, Error as SqliteError, OptionalExtension, Row, ToSql};
use std::cmp::max;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub enum SimpTrad {
    Simp,
    Trad,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Tag {
    /// An ASCII tag which is a shorthand for a full tag,
    Ascii(char),
    /// A full tag with a name and a category.
    Full { name: String, category: String },
}

/// An enum to identify the target entity for an operation.
/// It holds the primary key of the entity in its respective table.
#[derive(Debug)]
pub enum EntryId {
    Word(SqliteId),
    Definition(SqliteId),
    /// id of the `dict_shared_pron` entry
    Pinyin(SqliteId),
    Reference(SqliteId),
    Sentence(SqliteId),
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct SharedIds {
    pub tag_ids: Vec<SqliteId>,
    pub note_id: Option<SqliteId>,
    pub comment_id: Option<SqliteId>,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct WordEntry {
    pub id: SqliteId,
    pub shared_id: SqliteId,
    pub simp: String,
    pub trad: String,
    pub variant_of: Option<SqliteId>,
    pub shared_ids: SharedIds,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct DefinitionEntry {
    pub id: SqliteId,
    pub shared_id: SqliteId,
    pub parent_id: Option<SqliteId>,
    pub ext_def_id: u32,
    pub definition: String,
    pub nested_level: usize,
    pub word_id: SqliteId,
    pub pron_shared_ids: Vec<SqliteId>,
    pub class_id: SqliteId,
    pub class_name: String,
    pub shared_ids: SharedIds,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct ReferenceEntry {
    pub id: SqliteId,
    pub shared_id: SqliteId,
    pub ascii_symbol: char,
    pub src: (SqliteId, Option<SqliteId>), // word_id, def_id
    pub dst: (SqliteId, Option<SqliteId>), // word_id, def_id
    pub shared_ids: SharedIds,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct SentenceWordEntry {
    pub word: Option<(SqliteId, Option<SqliteId>)>, // word_id, def_id
    pub part_of_word: Option<(SqliteId, Option<SqliteId>)>, // word_id, def_id
    pub ascii_txt: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct SentenceEntry {
    pub id: SqliteId,
    pub shared_id: SqliteId,
    pub ext_sent_id: u32,
    pub for_word_id: SqliteId,
    pub for_definition_id: SqliteId,
    pub translation: String,
    pub words: Vec<SentenceWordEntry>,
    pub shared_ids: SharedIds,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct EntryGrouping {
    pub new_word: bool,
    pub new_pron: bool,
    pub new_class: bool,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct PronEntry {
    pub id: SqliteId,
    pub shared_id: SqliteId,
    pub pinyin_num: String,
    pub shared_ids: SharedIds,
}

pub fn read_shared_ids(conn: &Connection, shared_id: SqliteId) -> Result<SharedIds, SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
            SELECT
                dict_shared.note_id,
                dict_shared.comment_id,
                GROUP_CONCAT(t.id) AS tag_ids -- NULLS FIRST default
            FROM dict_shared
            LEFT JOIN dict_shared_tag st ON st.for_shared_id = dict_shared.id
            LEFT JOIN dict_tag t ON st.tag_id = t.id
            WHERE dict_shared.id = ?1
            ",
    )?;

    let (note_id, comment_id, tag_ids_csv): (Option<SqliteId>, Option<SqliteId>, Option<String>) =
        stmt.query_row([shared_id], |row| {
            Ok((
                row.get("note_id")?,
                row.get("comment_id")?,
                row.get("tag_ids")?,
            ))
        })?;
    let tag_ids: Vec<SqliteId> = if let Some(tag_ids_csv) = tag_ids_csv {
        tag_ids_csv
            .split(',')
            .map(|s| s.parse::<SqliteId>().unwrap())
            .collect()
    } else {
        vec![]
    };

    Ok(SharedIds { tag_ids, note_id, comment_id })
}



fn row_to_definition_entry(conn: &Connection, row: &Row) -> Result<DefinitionEntry, SqliteError> {
    let pron_shared_ids_str: Option<String> = row.get("pron_shared_ids")?;
    let pron_shared_ids = pron_shared_ids_str
        .unwrap()
        .split(',')
        .map(|s| s.parse::<SqliteId>().unwrap())
        .collect();
    let shared_id = row.get("def_shared_id")?;
    Ok(DefinitionEntry {
        id: row.get("def_id")?,
        shared_id,
        word_id: row.get("word_id")?,
        pron_shared_ids,
        class_id: row.get("class_id")?,
        class_name: row.get("class_name")?,
        ext_def_id: row.get("ext_def_id")?,
        definition: row.get("definition")?,
        parent_id: row.get("parent_id")?,
        nested_level: 0,
        shared_ids: read_shared_ids(conn, shared_id)?,
    })
}

/// Add indentation level and grouping (word, pinyin, word class) information
fn group_definitions(defs: Vec<DefinitionEntry>) -> Vec<(DefinitionEntry, EntryGrouping)> {
    let mut def_groups = vec![];

    let mut last_word_id = -1;
    let mut last_pron_shared_ids = vec![];
    let mut last_class_id = -1;
    let mut definition_stack: Vec<SqliteId> = vec![];

    for mut def_entry in defs {
        let mut entry_grouping = EntryGrouping::default();

        // set flags if changes in word, pronunciation or class occur
        if def_entry.word_id != last_word_id {
            last_word_id = def_entry.word_id;
            last_pron_shared_ids.clear();
            last_class_id = -1;
            entry_grouping.new_word = true;
            definition_stack.clear();
        }
        if def_entry.pron_shared_ids != last_pron_shared_ids {
            last_pron_shared_ids = def_entry.pron_shared_ids.clone();
            last_class_id = -1;
            entry_grouping.new_pron = true;
            definition_stack.clear();
        }
        if def_entry.class_id != last_class_id {
            last_class_id = def_entry.class_id;
            entry_grouping.new_class = true;
            definition_stack.clear();
        }

        // determine the number of parent definitions
        if let Some(parent_id) = def_entry.parent_id {
            // note: it has to be ensured that, if parent definitions are filtered out, the child definitions are also removed
            let parent_level = 1 + definition_stack
                .iter()
                .position(|i| *i == parent_id)
                .expect("missing parent definition");
            def_entry.nested_level += parent_level;
            definition_stack.truncate(parent_level);
        } else {
            definition_stack.clear();
        }
        definition_stack.push(def_entry.id);

        def_groups.push((def_entry, entry_grouping));
    }
    def_groups
}

/// Read all definitions in the dictionary
pub fn read_definitions(
    conn: &Connection,
) -> Result<Vec<(DefinitionEntry, EntryGrouping)>, SqliteError> {
    let mut definitions = vec![];
    let mut stmt = conn.prepare(
            r"
            SELECT
                w.id AS word_id,
                c.id AS class_id,
                c.name AS class_name,
                def.id AS def_id,
                def.shared_id AS def_shared_id,
                def.ext_def_id,
                def.definition,
                def.parent_id,
                GROUP_CONCAT(p_s.id ORDER BY p_s.rank, p_s.rank_relative) AS pron_shared_ids -- NULLS FIRST default
            FROM dict_definition def
            JOIN dict_shared s ON def.shared_id = s.id
            JOIN dict_word w ON def.word_id = w.id
            JOIN dict_class c ON def.class_id = c.id
            LEFT JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
            LEFT JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
            LEFT JOIN dict_pron p ON sp.pron_id = p.id
            LEFT JOIN dict_shared p_s ON sp.shared_id = p_s.id
            WHERE w.variant_of IS NULL
            GROUP BY def.id
            ORDER BY s.rank, s.rank_relative; -- NULLS FIRST default
            ",
        )?;

    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        definitions.push(row_to_definition_entry(conn, row)?);
    }
    let def_groups = group_definitions(definitions);
    Ok(def_groups)
}

/// Replace word ids with the target of `variant_of` (if not NULL), does NOT remove resulting duplicated ids
pub fn resolve_word_variants(
    conn: &Connection,
    word_ids: &Vec<SqliteId>,
) -> Result<Vec<SqliteId>, SqliteError> {
    if word_ids.is_empty() {
        return Ok(word_ids.clone());
    }
    // Since variants are rare, we only query for the specific IDs in the input list
    // that have a 'variant_of' value set.
    let placeholders: Vec<&str> = word_ids.iter().map(|_| "?").collect();
    let sql = format!(
        "SELECT id, variant_of FROM dict_word WHERE id IN ({}) AND variant_of IS NOT NULL",
        placeholders.join(",")
    );

    let params: Vec<&dyn ToSql> = word_ids.iter().map(|id| id as &dyn ToSql).collect();

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(&*params)?;

    let mut replacement_map = std::collections::HashMap::new();

    while let Some(row) = rows.next()? {
        let id: SqliteId = row.get(0)?;
        let variant_of: SqliteId = row.get(1)?;
        replacement_map.insert(id, variant_of);
    }

    // Optimization: If no variants were found, we can simply return the original list
    // without reallocating or iterating.
    if replacement_map.is_empty() {
        return Ok(word_ids.clone());
    }

    // Replace IDs found in the map, keep others as they are
    let resolved_ids = word_ids
        .iter()
        .map(|id| *replacement_map.get(id).unwrap_or(id))
        .collect();

    Ok(resolved_ids)
}

/// Read definitions for the provided word ids
/// note: this implementation passes all word ids in one sql query
pub fn read_definitions_for_words(
    conn: &Connection,
    word_ids: &Vec<SqliteId>,
    must_have_tag: &Vec<SqliteId>,
    without_tag: &Vec<SqliteId>,
) -> Result<Vec<(DefinitionEntry, EntryGrouping)>, SqliteError> {
    if word_ids.is_empty() {
        return Ok(vec![]);
    }
    let mut sql = r"
            SELECT
                w.id AS word_id,
                c.id AS class_id,
                c.name AS class_name,
                def.id AS def_id,
                def.shared_id AS def_shared_id,
                def.ext_def_id,
                def.definition,
                def.parent_id,
                GROUP_CONCAT(p_s.id ORDER BY p_s.rank, p_s.rank_relative) AS pron_shared_ids -- NULLS FIRST default
            FROM dict_definition def
            JOIN dict_shared s ON def.shared_id = s.id
            JOIN dict_word w ON def.word_id = w.id
            JOIN dict_class c ON def.class_id = c.id
            LEFT JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
            LEFT JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
            LEFT JOIN dict_pron p ON sp.pron_id = p.id
            LEFT JOIN dict_shared p_s ON sp.shared_id = p_s.id
            WHERE ".to_owned();

    let mut params: Vec<&dyn ToSql> =
        Vec::with_capacity(word_ids.len() + must_have_tag.len() + without_tag.len());

    let word_ids = resolve_word_variants(conn, word_ids)?;

    // Filter by word IDs
    let word_placeholders: Vec<&str> = word_ids.iter().map(|_| "?").collect();
    sql.push_str(&format!("w.id IN ({})", word_placeholders.join(",")));
    for id in &word_ids {
        params.push(id);
    }

    // Filter by tags that must be present
    for tag_id in must_have_tag {
        sql.push_str(" AND EXISTS (SELECT 1 FROM dict_shared_tag st WHERE st.for_shared_id = def.shared_id AND st.tag_id = ?)");
        params.push(tag_id);
    }

    // Filter by tags that must NOT be present
    if !without_tag.is_empty() {
        let tag_placeholders: Vec<&str> = without_tag.iter().map(|_| "?").collect();
        sql.push_str(&format!(" AND NOT EXISTS (SELECT 1 FROM dict_shared_tag st WHERE st.for_shared_id = def.shared_id AND st.tag_id IN ({}))", tag_placeholders.join(",")));
        for tag_id in without_tag {
            params.push(tag_id);
        }
    }

    sql.push_str(" GROUP BY def.id ORDER BY s.rank, s.rank_relative;");

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(&*params)?;
    let mut definitions = vec![];
    let mut available_def_ids = HashSet::new();
    while let Some(row) = rows.next()? {
        let def = row_to_definition_entry(conn, row)?;
        // only add definition if their parent definition is also included
        if def.parent_id.is_none() || available_def_ids.contains(&def.parent_id.unwrap()) {
            available_def_ids.insert(def.id);
            definitions.push(def);
        }
    }
    let def_groups = group_definitions(definitions);
    Ok(def_groups)
}

pub fn read_pinyin_entries_for_definition(
    conn: &Connection,
    def_id: SqliteId,
    pron_shared_ids: &[SqliteId],
) -> Result<Vec<PronEntry>, SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
            SELECT
                p.id,
                p_s.id,
                p.pinyin_num
            FROM dict_definition def
            LEFT JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
            LEFT JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
            LEFT JOIN dict_pron p ON sp.pron_id = p.id
            LEFT JOIN dict_shared p_s ON sp.shared_id = p_s.id
            WHERE def.id = ?1 AND p_s.id = ?2
            ",
    )?;

    // 1. Fetch all data into a Vec of PinyinData structs
    let pinyin_data: Result<Vec<PronEntry>, SqliteError> = pron_shared_ids
        .iter()
        .map(|pron_shared_id| {
            let (id, shared_id, pinyin_num) = stmt.query_row([def_id, *pron_shared_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            Ok(PronEntry {
                id,
                shared_id,
                pinyin_num,
                shared_ids: read_shared_ids(conn, shared_id)?,
            })
        })
        .collect();
    pinyin_data
}

pub fn get_words_starting_with_char(
    conn: &Connection,
    c: char,
    simp_trad: &SimpTrad,
) -> Result<Vec<(SqliteId, String)>, SqliteError> {
    let mut stmt = if *simp_trad == SimpTrad::Trad {
        conn.prepare_cached(
            r"
                SELECT
                    id,
                    trad
                FROM dict_word
                WHERE trad GLOB ?1
                ",
        )?
    } else {
        conn.prepare_cached(
            r"
                SELECT
                    id,
                    simp
                FROM dict_word
                WHERE simp GLOB ?1
                ",
        )?
    };
    let mut rows = stmt.query(params![format!("{c}*")])?;

    let mut words = vec![];

    while let Some(row) = rows.next()? {
        let word_id: SqliteId = row.get(0)?;
        let word_str: String = row.get(1)?;
        words.push((word_id, word_str));
    }
    Ok(words)
}

pub fn read_sentences_for_definition(
    conn: &Connection,
    def_id: SqliteId,
) -> Result<Vec<SentenceEntry>, SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
        SELECT
            s.id,
            s.shared_id,
            s.ext_sent_id,
            s.for_word_id,
            s.for_definition_id,
            s.translation
        FROM dict_sentence s
        JOIN dict_shared sh ON s.shared_id = sh.id
        WHERE s.for_definition_id = ?1
        ORDER BY sh.rank, sh.rank_relative -- NULLS FIRST default
        ",
    )?;

    let mut rows = stmt.query(params![def_id])?;
    let mut sentences = vec![];

    while let Some(row) = rows.next()? {
        let id: SqliteId = row.get(0)?;
        let shared_id: SqliteId = row.get(1)?;
        let ext_sent_id: u32 = row.get(2)?;
        let for_word_id: SqliteId = row.get(3)?;
        let for_definition_id: SqliteId = row.get(4)?;
        let translation: String = row.get(5)?;

        let mut word_stmt = conn.prepare_cached(
            r"
            SELECT
                word_id,
                definition_id,
                part_of_word_id,
                part_of_definition_id,
                ascii_txt
            FROM dict_sentence_word
            WHERE sentence_id = ?1
            ORDER BY word_rank
            ",
        )?;
        let mut word_rows = word_stmt.query(params![id])?;
        let mut words = vec![];

        while let Some(w_row) = word_rows.next()? {
            let word_id: Option<SqliteId> = w_row.get(0)?;
            let def_id: Option<SqliteId> = w_row.get(1)?;
            let p_word_id: Option<SqliteId> = w_row.get(2)?;
            let p_def_id: Option<SqliteId> = w_row.get(3)?;
            let ascii_txt: Option<String> = w_row.get(4)?;

            words.push(SentenceWordEntry {
                word: word_id.map(|w| (w, def_id)),
                part_of_word: p_word_id.map(|w| (w, p_def_id)),
                ascii_txt,
            });
        }

        sentences.push(SentenceEntry {
            id,
            shared_id,
            ext_sent_id,
            for_word_id,
            for_definition_id,
            translation,
            words,
            shared_ids: read_shared_ids(conn, shared_id)?,
        });
    }

    Ok(sentences)
}

/// Return all dictionary words in the provided string and a list of characters not covered by any dictionary entry
pub fn get_words_in_str<'a>(
    conn: &Connection,
    s: &'a str,
    simp_trad: SimpTrad,
) -> Result<(Vec<(SqliteId, &'a str)>, Vec<char>), SqliteError> {
    let mut words: Vec<(SqliteId, &str)> = vec![];
    let mut unknown_chars: Vec<char> = vec![];

    let mut covered_end_idx = 0;
    for (char_idx, c) in s.char_indices() {
        let possible_words = get_words_starting_with_char(conn, c, &simp_trad)?;
        for (word_id, word_str) in possible_words {
            if s[char_idx..].starts_with(&word_str) {
                let end_idx = char_idx + word_str.len();
                words.push((word_id, &s[char_idx..end_idx]));
                covered_end_idx = max(covered_end_idx, end_idx);
            }
        }
        if char_idx + c.len_utf8() > covered_end_idx {
            unknown_chars.push(c);
        }
    }

    Ok((words, unknown_chars))
}

pub fn get_tag_ids(
    conn: &Connection,
    tags: Vec<Tag>,
) -> Result<Vec<Option<SqliteId>>, SqliteError> {
    let mut stmt_ascii = conn.prepare_cached("SELECT id FROM dict_tag WHERE ascii_symbol = ?1")?;
    let mut stmt_full =
        conn.prepare_cached("SELECT id FROM dict_tag WHERE tag = ?1 AND type = ?2")?;

    let mut ids = Vec::with_capacity(tags.len());

    for tag in tags {
        let id = match tag {
            Tag::Ascii(c) => stmt_ascii
                .query_row(params![c.to_string()], |row| row.get(0))
                .optional()?,
            Tag::Full { name, category } => stmt_full
                .query_row(params![name, category], |row| row.get(0))
                .optional()?,
        };
        ids.push(id);
    }
    Ok(ids)
}

pub fn read_tags_for_shared_id(
    conn: &Connection,
    shared_id: SqliteId,
) -> Result<Vec<Tag>, SqliteError> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.ascii_symbol, t.tag, t.type FROM dict_shared_tag st JOIN dict_tag t ON st.tag_id = t.id WHERE st.for_shared_id = ?1",
    )?;
    let mut rows = stmt.query([shared_id])?;
    let mut tags = vec![];

    while let Some(row) = rows.next()? {
        let ascii_symbol: Option<String> = row.get(0)?;
        let tag: String = row.get(1)?;
        let category: String = row.get(2)?;

        if let Some(symbol) = ascii_symbol {
            if !symbol.is_empty() {
                tags.push(Tag::Ascii(symbol.chars().next().unwrap()));
            }
        } else {
            tags.push(Tag::Full {
                name: tag,
                category,
            });
        }
    }

    Ok(tags)
}

pub fn read_comment(conn: &Connection, id: SqliteId) -> Result<String, SqliteError> {
    let mut stmt = conn.prepare_cached("SELECT comment FROM dict_comment WHERE id = ?1")?;
    stmt.query_row([id], |row| row.get(0))
}

pub fn read_note(conn: &Connection, id: SqliteId) -> Result<(String, SqliteId), SqliteError> {
    let mut stmt = conn.prepare_cached("SELECT note, ext_note_id FROM dict_note WHERE id = ?1")?;
    stmt.query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
}

pub fn read_references_for_item(
    conn: &Connection,
    src_word_id: SqliteId,
    src_def_id: Option<SqliteId>,
) -> Result<Vec<ReferenceEntry>, SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
        SELECT
            r.id,
            r.ascii_symbol,
            r.shared_id,
            r.word_id_dst,
            r.definition_id_dst
        FROM dict_reference r
        JOIN dict_shared s ON r.shared_id = s.id
        WHERE
            r.word_id_src = ?1 AND
            r.definition_id_src IS ?2
        ORDER BY s.rank, s.rank_relative -- NULLS FIRST default
        ",
    )?;

    let mut rows = stmt.query(params![src_word_id, src_def_id])?;
    let mut references = vec![];

    while let Some(row) = rows.next()? {
        let id: SqliteId = row.get(0)?;
        let ascii_symbol_str: String = row.get(1)?;
        let ascii_symbol = ascii_symbol_str.chars().next().unwrap_or(' ');
        let shared_id: SqliteId = row.get(2)?;
        let word_id_dst: SqliteId = row.get(3)?;
        let definition_id_dst: Option<SqliteId> = row.get(4)?;

        references.push(ReferenceEntry {
            id,
            shared_id,
            ascii_symbol,
            src: (src_word_id, src_def_id),
            dst: (word_id_dst, definition_id_dst),
            shared_ids: read_shared_ids(conn, shared_id)?,
        });
    }

    Ok(references)
}

/// Get ids for words matching the provided simplified or traditional word (both are considered if provided)
pub fn get_words(
    conn: &Connection,
    simp: Option<&str>,
    trad: Option<&str>,
) -> Result<Vec<SqliteId>, SqliteError> {
    assert!(
        simp.is_some() || trad.is_some(),
        "At least one of simp or trad must be provided"
    );

    let mut ids = vec![];

    if let (Some(s), Some(t)) = (simp, trad) {
        let mut stmt =
            conn.prepare_cached("SELECT id FROM dict_word WHERE simp = ?1 AND trad = ?2")?;
        let mut rows = stmt.query(params![s, t])?;
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
        }
    } else if let Some(s) = simp {
        let mut stmt = conn.prepare_cached("SELECT id FROM dict_word WHERE simp = ?1")?;
        let mut rows = stmt.query(params![s])?;
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
        }
    } else if let Some(t) = trad {
        let mut stmt = conn.prepare_cached("SELECT id FROM dict_word WHERE trad = ?1")?;
        let mut rows = stmt.query(params![t])?;
        while let Some(row) = rows.next()? {
            ids.push(row.get(0)?);
        }
    }

    Ok(ids)
}

/// Get ids for words matching `trad_or_simp` (+ simp) and the optional definition id
pub fn get_word_def_ids(
    conn: &Connection,
    trad_or_simp: &str,
    simp: Option<&str>,
    ext_def_id: Option<u32>,
) -> Vec<(SqliteId, Option<SqliteId>)> {
    // first try trad and simp together, simp either provided or assumed to be same as trad
    let simp = simp.unwrap_or(trad_or_simp);
    let mut word_ids = get_words(conn, Some(simp), Some(trad_or_simp)).unwrap_or_default();
    // if no words are found, try only trad
    if word_ids.is_empty() {
        let word_ids_trad = get_words(conn, None, Some(trad_or_simp)).unwrap_or_default();
        word_ids.extend(word_ids_trad);
    }
    // is still no words are found, try only simp
    if word_ids.is_empty() {
        let word_ids_simp = get_words(conn, Some(trad_or_simp), None).unwrap_or_default();
        word_ids.extend(word_ids_simp);
    }
    if ext_def_id.is_none() || word_ids.is_empty() {
        return word_ids.into_iter().map(|i| (i, None)).collect();
    }
    
    // add definition id
    let mut word_def_ids = vec![];
    let mut stmt = conn
        .prepare_cached("SELECT id FROM dict_definition WHERE word_id=?1 AND ext_def_id=?2")
        .unwrap();
    for word_id in word_ids {
        let mut rows = stmt.query(params![word_id, ext_def_id.unwrap()]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            word_def_ids.push((word_id, row.get(0).unwrap()));
        }
    }
    word_def_ids
}

/// Get reference type id for `ascii_char`, only if this type already exists in DB
pub fn get_ref_type_id(
    conn: &Connection,
    ref_type_symbol: char,
) -> Result<Option<SqliteId>, SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
        SELECT id
        FROM dict_ref_type
        WHERE ascii_symbol = ?1;
        ",
    )?;
    let symbol_str = ref_type_symbol.to_string();
    match stmt.query_row(params![symbol_str], |row| row.get(0)) {
        Ok(id) => Ok(Some(id)),
        Err(SqliteError::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn get_word_id(
    conn: &Connection,
    trad: &str,
    simp: &str,
) -> Result<Option<SqliteId>, SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
        SELECT id
        FROM dict_word
        WHERE trad = ?1 AND simp = ?2;
        ",
    )?;
    match stmt.query_row(params![trad, simp], |row| row.get(0)) {
        Ok(id) => Ok(Some(id)),
        Err(SqliteError::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn read_word(conn: &Connection, word_id: SqliteId) -> Result<WordEntry, SqliteError> {
    let mut stmt = conn.prepare(
        r"
            SELECT
                w.shared_id AS shared_id,
                w.simp AS simp,
                w.trad AS trad,
                w.variant_of AS variant_of
            FROM dict_word w
            WHERE w.id = ?1
            ",
    )?;

    let (shared_id, simp, trad, variant_of): (SqliteId, String, String, Option<SqliteId>) = stmt
        .query_row([word_id], |row| {
            Ok((
                row.get("shared_id")?,
                row.get("simp")?,
                row.get("trad")?,
                row.get("variant_of")?,
            ))
        })?;
    Ok(WordEntry {
        id: word_id,
        shared_id,
        simp,
        trad,
        variant_of,
        shared_ids: read_shared_ids(conn, shared_id)?,
    })
}

pub fn read_all_words(conn: &Connection) -> Result<Vec<WordEntry>, SqliteError> {
    let mut stmt = conn.prepare("SELECT id, shared_id, simp, trad, variant_of FROM dict_word")?;
    let mut rows = stmt.query([])?;
    let mut words = vec![];

    while let Some(row) = rows.next()? {
        let id: SqliteId = row.get(0)?;
        let shared_id: SqliteId = row.get(1)?;
        let simp: String = row.get(2)?;
        let trad: String = row.get(3)?;
        let variant_of: Option<SqliteId> = row.get(4)?;

        words.push(WordEntry {
            id,
            shared_id,
            simp,
            trad,
            variant_of,
            shared_ids: read_shared_ids(conn, shared_id)?,
        });
    }

    Ok(words)
}

pub fn read_words(conn: &Connection) -> Result<Vec<WordEntry>, SqliteError> {
    let sql = r"
        SELECT DISTINCT
            w.id,
            w.shared_id,
            w.simp,
            w.trad,
            w.variant_of
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        JOIN dict_definition def ON def.word_id = w.id
        JOIN dict_class c ON def.class_id = c.id

        ORDER BY s.rank, s.rank_relative
    ";

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;

    let mut words = vec![];

    while let Some(row) = rows.next()? {
        let word_id: SqliteId = row.get(0)?;
        let shared_id: SqliteId = row.get(1)?;
        let simp: String = row.get(2)?;
        let trad: String = row.get(3)?;
        let variant_of: Option<SqliteId> = row.get(4)?;

        words.push(WordEntry {
            id: word_id,
            shared_id,
            simp,
            trad,
            variant_of,
            shared_ids: read_shared_ids(conn, shared_id)?,
        });
    }

    Ok(words)
}

/// Read all words which have at least one definition with one of the provided word classes
pub fn read_words_with_classes(
    conn: &Connection,
    word_classes: Vec<&str>,
) -> Result<Vec<WordEntry>, SqliteError> {
    if word_classes.is_empty() {
        return Ok(vec![]);
    }
    let placeholders: Vec<&str> = word_classes.iter().map(|_| "?").collect();
    let sql = format!(
        r"
        SELECT DISTINCT
            w.id,
            w.shared_id,
            w.simp,
            w.trad,
            w.variant_of
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        JOIN dict_definition def ON def.word_id = w.id
        JOIN dict_class c ON def.class_id = c.id
        WHERE c.name IN ({})
        ORDER BY s.rank, s.rank_relative
    ",
        placeholders.join(",")
    );

    let params: Vec<&dyn ToSql> = word_classes.iter().map(|c| c as &dyn ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(&*params)?;

    let mut words = vec![];

    while let Some(row) = rows.next()? {
        let word_id: SqliteId = row.get(0)?;
        let shared_id: SqliteId = row.get(1)?;
        let simp: String = row.get(2)?;
        let trad: String = row.get(3)?;
        let variant_of: Option<SqliteId> = row.get(4)?;

        words.push(WordEntry {
            id: word_id,
            shared_id,
            simp,
            trad,
            variant_of,
            shared_ids: read_shared_ids(conn, shared_id)?,
        });
    }

    Ok(words)
}

pub fn get_definition_id_for_ext_id(
    conn: &Connection,
    word_id: SqliteId,
    ext_def_id: usize,
) -> Result<Option<SqliteId>, SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
        SELECT id
        FROM dict_definition
        WHERE word_id = ?1 AND ext_def_id = ?2;
        ",
    )?;
    match stmt.query_row(params![word_id, ext_def_id], |row| row.get(0)) {
        Ok(id) => Ok(Some(id)),
        Err(SqliteError::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn get_definition_id_for_str(
    conn: &Connection,
    word_id: SqliteId,
    definition: &str,
) -> Result<Option<SqliteId>, SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
        SELECT id
        FROM dict_definition
        WHERE word_id = ?1 AND definition = ?2;
        ",
    )?;
    match stmt.query_row(params![word_id, definition], |row| row.get(0)) {
        Ok(id) => Ok(Some(id)),
        Err(SqliteError::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Retrieves the `shared_id` for a given target entity.
pub fn get_shared_id(conn: &Connection, id: &EntryId) -> Result<SqliteId, SqliteError> {
    match id {
        EntryId::Word(word_id) => {
            let mut stmt = conn.prepare_cached("SELECT shared_id FROM dict_word WHERE id = ?1")?;
            let id: SqliteId = stmt.query_row(params![word_id], |row| row.get(0))?;
            Ok(id)
        }
        EntryId::Definition(def_id) => {
            let mut stmt =
                conn.prepare_cached("SELECT shared_id FROM dict_definition WHERE id = ?1")?;
            let id: SqliteId = stmt.query_row(params![def_id], |row| row.get(0))?;
            Ok(id)
        }
        EntryId::Pinyin(pinyin_shared_pron_id) => {
            let mut stmt =
                conn.prepare_cached("SELECT shared_id FROM dict_shared_pron WHERE id = ?1")?;
            let id: SqliteId = stmt.query_row(params![pinyin_shared_pron_id], |row| row.get(0))?;
            Ok(id)
        }
        EntryId::Reference(ref_id) => {
            let mut stmt =
                conn.prepare_cached("SELECT shared_id FROM dict_reference WHERE id = ?1")?;
            let id: SqliteId = stmt.query_row(params![ref_id], |row| row.get(0))?;
            Ok(id)
        }
        EntryId::Sentence(sent_id) => {
            let mut stmt =
                conn.prepare_cached("SELECT shared_id FROM dict_sentence WHERE id = ?1")?;
            let id: SqliteId = stmt.query_row(params![sent_id], |row| row.get(0))?;
            Ok(id)
        }
    }
}
