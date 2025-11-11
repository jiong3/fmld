use crate::common::SqliteId;
use rusqlite::{Connection, Error as SqliteError, Row};

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

    let mut last_word_id = -1;
    let mut last_pron_shared_ids = vec![];
    let mut last_class_id = -1;
    let mut definition_stack: Vec<SqliteId> = vec![];

    while let Some(row) = rows.next()? {
        let mut def_entry = row_to_definition_entry(conn, row)?;
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

        definitions.push((def_entry, entry_grouping));
    }
    Ok(definitions)
}

pub fn read_pinyin_entries_for_definition(
    conn: &Connection,
    def_id: SqliteId,
    pron_shared_ids: &[SqliteId],
) -> Result<Vec<PronEntry>, SqliteError> {
    let mut stmt = conn
        .prepare_cached(
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
        )
        .unwrap();

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
