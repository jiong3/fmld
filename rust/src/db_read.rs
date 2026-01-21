use crate::common::SqliteId;
use rusqlite::{Connection, Error as SqliteError, OptionalExtension, Row, ToSql, params};
use std::cmp::max;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub enum SimpTrad {
    Simp,
    Trad,
}

pub enum Tag {
    /// An ASCII tag which is a shorthand for a full tag,
    Ascii(char),
    /// A full tag with a name and a category.
    Full { name: String, category: String },
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

    Ok(SharedIds {
        note_id,
        comment_id,
        tag_ids,
    })
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

/// Replace word ids with the target of variant_of (if not NULL), does NOT remove resulting duplicated ids
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
        .into_iter()
        .map(|id| *replacement_map.get(&id).unwrap_or(&id))
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
) -> rusqlite::Result<Vec<Tag>> {
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
                tags.push(Tag::Ascii(symbol.chars().nth(0).unwrap()));
            }
        } else {
            tags.push(Tag::Full {
                name: tag,
                category: category,
            });
        }
    }

    Ok(tags)
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
