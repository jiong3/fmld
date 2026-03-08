// LLM generated except for the schema, context: common.rs, config.rs, db_to_txt.rs, db_edit.rs

use std::collections::HashMap;

use crate::common::SqliteId;
use rusqlite::{Connection, Error as SqliteError, Row, Transaction, params};


const TARGET_SCHEMA: &str = r#"
/* Schema of a dictionary for Mandarin Chinese. 

Each entry consists of a word (dict_word), which can have several definitions (dict_definition). Each definition must have one or more pronunciations (dict_pron).
Words and definitions can be linked (dict_reference), e.g. to indicate synonyms, antonyms etc.. */
CREATE TABLE IF NOT EXISTS "dict_definition" (
	"id" INTEGER NOT NULL UNIQUE,
	"parent_id" INTEGER,
	"word_id" INTEGER NOT NULL,
	"definition" TEXT NOT NULL,
	"class" INTEGER NOT NULL,
	"note_id" INTEGER,
	"tags" INTEGER NOT NULL,
	"tags_full" TEXT,
	PRIMARY KEY("id"),
	FOREIGN KEY ("word_id") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("parent_id") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("note_id") REFERENCES "dict_note"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE TABLE IF NOT EXISTS "dict_word" (
	"id" INTEGER NOT NULL UNIQUE,
	-- word in traditional characters
	"trad" TEXT NOT NULL,
	-- word in simplified characters
	"simp" TEXT NOT NULL,
	-- link to the main variant if not NULL, the entry will have the same shared_id as the main variant and no definitions should link to this entry, only to the main variant
	"variant_of" INTEGER,
	"note_id" INTEGER,
	"tags" INTEGER NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY ("variant_of") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("note_id") REFERENCES "dict_note"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_word_index_0"
ON "dict_word" ("trad", "simp");

CREATE INDEX IF NOT EXISTS "dict_word_index_1"
ON "dict_word" ("variant_of");
CREATE TABLE IF NOT EXISTS "dict_pron" (
	"id" INTEGER NOT NULL UNIQUE,
	"pinyin_num" TEXT NOT NULL,
	"pinyin_mark" TEXT NOT NULL,
	PRIMARY KEY("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_pron_index_0"
ON "dict_pron" ("pinyin_num");
/* Relationship from a to b, e.g. measureword, antonym, synonym or variant. */
CREATE TABLE IF NOT EXISTS "dict_reference" (
	"id" INTEGER NOT NULL UNIQUE,
	"ref_type" INTEGER NOT NULL,
	"word_id_src" INTEGER NOT NULL,
	"definition_id_src" INTEGER,
	"word_id_dst" INTEGER NOT NULL,
	"definition_id_dst" INTEGER,
	"note_id" INTEGER,
	"tags" INTEGER NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY ("word_id_dst") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("word_id_src") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("definition_id_src") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("definition_id_dst") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("note_id") REFERENCES "dict_note"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE INDEX IF NOT EXISTS "dict_reference_index_0"
ON "dict_reference" ("word_id_src", "definition_id_src");
/* ext_note_id is a globally unique id for each note (but same id for different translations), exported into txt format */
CREATE TABLE IF NOT EXISTS "dict_note" (
	"id" INTEGER NOT NULL UNIQUE,
	"note" TEXT NOT NULL,
	PRIMARY KEY("id")
);

CREATE TABLE IF NOT EXISTS "dict_def_pron" (
	"id" INTEGER NOT NULL UNIQUE,
	"definition_id" INTEGER NOT NULL,
	"pron_id" INTEGER NOT NULL,
	"note_id" INTEGER,
	"tags" INTEGER NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY ("definition_id") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("pron_id") REFERENCES "dict_pron"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("note_id") REFERENCES "dict_note"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE INDEX IF NOT EXISTS "dict_def_pron_index_0"
ON "dict_def_pron" ("definition_id");

CREATE INDEX IF NOT EXISTS "dict_def_pron_index_1"
ON "dict_def_pron" ("pron_id");
"#;

/// Maps an ASCII tag character to its corresponding bit (2^n) for the bitfield.
fn get_ascii_tag_bit(tag_char: char) -> Option<i64> {
    let bit_pos = match tag_char {
        'T' => 0,
        't' => 1,
        'C' => 2,
        'c' => 3,
        '&' => 4,
        'i' => 5,
        'A' => 6,
        'a' => 7,
        '{' => 8,
        '}' => 9,
        'w' => 10,
        'm' => 11,
        '+' => 12,
        '-' => 13,
        'x' => 14,
        'X' => 15,
        _ => return None,
    };
    Some(1 << bit_pos)
}

/// Converts the normalized database schema to a denormalized version
/// ids are perserved
#[allow(clippy::too_many_lines, reason = "Database conversion function is complex by nature")]
pub fn convert_db_to_denormalized(
    source_conn: &Connection,
    target_conn: &mut Connection,
) -> Result<(), SqliteError> {
    let tx = target_conn.transaction()?;

    // Step 1: Create the new schema in the target database
    tx.execute_batch(TARGET_SCHEMA)?;

    // Step 2: Pre-fetch and compute a map of all tags from the source database.
    // The key is the `shared_id`, and the value is a tuple containing the
    // bitfield for ASCII tags and a concatenated string for full tags.
    let mut tags_map: HashMap<SqliteId, (i64, String)> = HashMap::new();
    {
        let mut stmt = source_conn.prepare(
            r#"
            SELECT st.for_shared_id, t.ascii_symbol, t.tag
            FROM dict_shared_tag st
            JOIN dict_tag t ON st.tag_id = t.id
            ORDER BY st.for_shared_id, t.id
            "#,
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let shared_id: SqliteId = row.get(0)?;
            let entry = tags_map.entry(shared_id).or_default();
            let ascii_symbol: Option<String> = row.get(1)?;

            if let Some(symbol_str) = ascii_symbol {
                if let Some(symbol_char) = symbol_str.chars().next() {
                    if let Some(bit) = get_ascii_tag_bit(symbol_char) {
                        entry.0 |= bit;
                    }
                }
            } else {
                let tag: String = row.get(2)?;
                if !entry.1.is_empty() {
                    entry.1.push(';');
                }
                entry.1.push_str(&tag);
            }
        }
    }

    // Step 3: Migrate dict_note
    {
        let mut insert_stmt = tx.prepare("INSERT INTO dict_note (id, note) VALUES (?1, ?2)")?;
        let mut select_stmt = source_conn.prepare("SELECT id, note FROM dict_note")?;
        let mut rows = select_stmt.query([])?;
         while let Some(row) = rows.next()? {
            insert_stmt.execute(params![row.get::<_, SqliteId>(0)?, row.get::<_, String>(1)?])?;
        }
    }

    // Step 4: Migrate dict_word
    {
        let mut select_stmt = source_conn.prepare(
            r#"
            SELECT w.id, w.shared_id, w.trad, w.simp, w.variant_of, s.note_id
            FROM dict_word w
            JOIN dict_shared s ON w.shared_id = s.id
            "#,
        )?;
        let mut insert_stmt = tx.prepare_cached(
            r#"
            INSERT INTO dict_word (id, trad, simp, variant_of, note_id, tags)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )?;
        let mut rows = select_stmt.query([])?;
        while let Some(row) = rows.next()? {
            let shared_id: SqliteId = row.get(1)?;
            let (tags, _full_tags) = tags_map.get(&shared_id).cloned().unwrap_or_default();
            insert_stmt.execute(params![
                row.get::<_, SqliteId>(0)?, // id
                row.get::<_, String>(2)?, // trad
                row.get::<_, String>(3)?, // simp
                row.get::<_, Option<SqliteId>>(4)?, // variant_of
                row.get::<_, Option<SqliteId>>(5)?, // note_id
                tags,
            ])?;
        }
    }

    // Step 5: Migrate dict_definition
    {
        let mut select_stmt = source_conn.prepare(
            r#"
            SELECT
                def.id, def.parent_id, def.word_id, def.definition,
                def.class_id as class, s.note_id, def.shared_id
            FROM dict_definition def
            JOIN dict_shared s ON def.shared_id = s.id
            "#,
        )?;
        let mut insert_stmt = tx.prepare_cached(
            r#"
            INSERT INTO dict_definition (id, parent_id, word_id, definition, class, note_id, tags, tags_full)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )?;
        let mut rows = select_stmt.query([])?;
        while let Some(row) = rows.next()? {
            let shared_id: SqliteId = row.get(6)?;
            let (tags, tags_full) = tags_map.get(&shared_id).cloned().unwrap_or_default();
            insert_stmt.execute(params![
                row.get::<_, SqliteId>(0)?, // id
                row.get::<_, Option<SqliteId>>(1)?, // parent_id
                row.get::<_, SqliteId>(2)?, // word_id
                row.get::<_, String>(3)?, // definition
                row.get::<_, SqliteId>(4)?, // class
                row.get::<_, Option<SqliteId>>(5)?, // note_id
                tags,
                tags_full,
            ])?;
        }
    }

    // Step 6: Migrate dict_pron and dict_def_pron
    {
        // First, migrate all unique pronunciations to dict_pron, preserving their IDs
        let mut select_pron_stmt = source_conn.prepare("SELECT id, pinyin_num, pinyin_mark FROM dict_pron")?;
        let mut insert_pron_stmt = tx.prepare_cached("INSERT OR IGNORE INTO dict_pron (id, pinyin_num, pinyin_mark) VALUES (?1, ?2, ?3)")?;
        let mut pron_rows = select_pron_stmt.query([])?;
        while let Some(row) = pron_rows.next()? {
            insert_pron_stmt.execute(params![
                row.get::<_, SqliteId>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ])?;
        }

        // Then, create the relationships in dict_def_pron
        let mut select_rel_stmt = source_conn.prepare(
            r#"
            SELECT
                pdp.definition_id, sp.pron_id, s.note_id, sp.shared_id
            FROM dict_pron_definition pdp
            JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
            JOIN dict_shared s ON sp.shared_id = s.id
            "#,
        )?;
        let mut insert_rel_stmt = tx.prepare_cached(
            r#"
            INSERT INTO dict_def_pron (definition_id, pron_id, note_id, tags)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )?;
        let mut rel_rows = select_rel_stmt.query([])?;
        while let Some(row) = rel_rows.next()? {
            let shared_id: SqliteId = row.get(3)?;
            let (tags, _full_tags) = tags_map.get(&shared_id).cloned().unwrap_or_default();
            insert_rel_stmt.execute(params![
                row.get::<_, SqliteId>(0)?, // definition_id
                row.get::<_, SqliteId>(1)?, // pron_id
                row.get::<_, Option<SqliteId>>(2)?, // note_id
                tags,
            ])?;
        }
    }

    // Step 7: Migrate dict_reference
    {
        let mut select_stmt = source_conn.prepare(
            r#"
            SELECT
                r.id, r.ascii_symbol, r.word_id_src, r.definition_id_src,
                r.word_id_dst, r.definition_id_dst, s.note_id, r.shared_id
            FROM dict_reference r
            JOIN dict_shared s ON r.shared_id = s.id
            "#,
        )?;
        let mut insert_stmt = tx.prepare_cached(
            r#"
            INSERT INTO dict_reference (id, ascii_symbol, word_id_src, definition_id_src, word_id_dst, definition_id_dst, note_id, tags)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )?;
        let mut rows = select_stmt.query([])?;
        while let Some(row) = rows.next()? {
            let shared_id: SqliteId = row.get(7)?;
            let (tags, _full_tags) = tags_map.get(&shared_id).cloned().unwrap_or_default();
            insert_stmt.execute(params![
                row.get::<_, SqliteId>(0)?, // id
                row.get::<_, SqliteId>(1)?, // ascii_symbol
                row.get::<_, SqliteId>(2)?, // word_id_src
                row.get::<_, Option<SqliteId>>(3)?, // definition_id_src
                row.get::<_, SqliteId>(4)?, // word_id_dst
                row.get::<_, Option<SqliteId>>(5)?, // definition_id_dst
                row.get::<_, Option<SqliteId>>(6)?, // note_id
                tags,
            ])?;
        }
    }

    tx.commit()?;
    Ok(())
}