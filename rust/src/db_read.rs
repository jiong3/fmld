use crate::common::SqliteId;
use rusqlite::{Connection, Error as SqliteError, Row};

#[derive(Debug, PartialEq, Eq, Default)]
pub struct DefinitionEntry {
    pub word_id: SqliteId,
    pub word_shared_id: SqliteId,
    pub simp: String,
    pub trad: String,
    pub pinyin_shared_ids: Vec<SqliteId>,
    pub class_id: SqliteId,
    pub class_name: String,
    pub def_id: SqliteId,
    pub parent_id: Option<SqliteId>,
    pub def_shared_id: SqliteId,
    pub ext_def_id: u32,
    pub definition: String,
    pub nested_level: usize,
    pub new_word: bool,
    pub new_pinyin: bool,
    pub new_class: bool,
}

fn row_to_definition_entry(row: &Row) -> Result<DefinitionEntry, SqliteError> {
    let pinyin_shared_ids_str: Option<String> = row.get("pron_shared_ids")?;
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
        parent_id: row.get("parent_id")?,
        nested_level: 0,
        new_class: false,
        new_pinyin: false,
        new_word: false,
    })
}

pub fn read_definitions(conn: &Connection) -> Result<Vec<DefinitionEntry>, SqliteError> {
    let mut definitions = vec![];
    let mut stmt = conn.prepare(
            r"
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
    let mut last_pinyin_shared_ids = vec![];
    let mut last_class_id = -1;
    let mut definition_stack: Vec<SqliteId> = vec![];

    while let Some(row) = rows.next()? {
        let mut def_entry = row_to_definition_entry(row)?;

        // set flags if changes in word, pronunciation or class occur
        if def_entry.word_id != last_word_id {
            last_word_id = def_entry.word_id;
            last_pinyin_shared_ids.clear();
            last_class_id = -1;
            def_entry.new_word = true;
            definition_stack.clear();
        }
        if def_entry.pinyin_shared_ids != last_pinyin_shared_ids {
            last_pinyin_shared_ids = def_entry.pinyin_shared_ids.clone();
            last_class_id = -1;
            def_entry.new_pinyin = true;
            definition_stack.clear();
        }
        if def_entry.class_id != last_class_id {
            last_class_id = def_entry.class_id;
            def_entry.new_class = true;
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
        definition_stack.push(def_entry.def_id);

        definitions.push(def_entry);
    }
    Ok(definitions)
}
