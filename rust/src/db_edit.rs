use crate::common::SqliteId;
use crate::config;
use crate::db_read;
use crate::pinyin;
use rusqlite::{Error as SqliteError, Transaction, params};
use std::cmp::max;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead};

/// An enum to identify the target entity for an operation.
/// It holds the primary key of the entity in its respective table.
#[derive(Debug)]
pub enum EntryId {
    Word(SqliteId),
    Definition(SqliteId),
    /// id of the dict_shared_pron entry
    Pinyin(SqliteId),
    Reference(SqliteId),
}

/// An enum to represent a tag.
#[derive(Debug)]
pub enum Tag<'a> {
    /// An ASCII tag which is a shorthand for a full tag,
    Ascii(char),
    /// A full tag with a name and a category.
    Full { name: &'a str, category: &'a str },
}

pub fn finalize_note_ids(conn: &Transaction, max_ext_note_id: u32) -> Result<u32, SqliteError> {
    let mut stmt_max_ext_note_id = conn.prepare(
        r"
        SELECT MAX(dict_note.ext_note_id)
        FROM dict_note;
        ",
    )?;
    let max_ext_note_id_db: u32 = stmt_max_ext_note_id
        .query_one((), |row| row.get(0))
        .unwrap_or_default();
    let mut base_ext_note_id = max(max_ext_note_id, max_ext_note_id_db);
    let mut stmt_note_ids_to_update = conn.prepare(
        r"
        SELECT dict_note.id
        FROM dict_note
        WHERE  dict_note.ext_note_id < 100;
        ",
    )?;
    let mut stmt_update_note_id = conn.prepare_cached(
        r"
        UPDATE dict_note
        SET ext_note_id=?2
        WHERE id=?1;
        ",
    )?;
    let mut stmt_shared_max_note_id = conn.prepare_cached(
        r"
        UPDATE dict_shared
        SET note_id=?1
        WHERE id=1;
        ",
    )?;
    let mut rows = stmt_note_ids_to_update.query([])?;

    while let Some(row) = rows.next()? {
        base_ext_note_id += 1;
        let note_id: SqliteId = row.get(0)?;
        stmt_update_note_id.execute((note_id, base_ext_note_id))?;
        stmt_shared_max_note_id.execute((note_id,))?;
    }
    Ok(base_ext_note_id)
}

pub fn add_missing_symmetric_references(conn: &Transaction) -> Result<(), SqliteError> {
    // find all references with missing symmetric counterpart
    let mut stmt_missing_references = conn.prepare(
        r"
        SELECT
            original_ref.id,
            original_ref.ref_type_id,
            original_ref.word_id_src,
            original_ref.definition_id_src,
            original_ref.word_id_dst,
            original_ref.definition_id_dst
        FROM
            dict_reference AS original_ref
        JOIN
            dict_ref_type AS ref_type ON original_ref.ref_type_id = ref_type.id
        LEFT JOIN
            dict_reference AS symmetric_ref ON original_ref.word_id_src = symmetric_ref.word_id_dst
                                            AND original_ref.word_id_dst = symmetric_ref.word_id_src
                                            AND original_ref.ref_type_id = symmetric_ref.ref_type_id
                                            AND (original_ref.definition_id_src = symmetric_ref.definition_id_dst OR (original_ref.definition_id_src IS NULL AND symmetric_ref.definition_id_dst IS NULL))
                                            AND (original_ref.definition_id_dst = symmetric_ref.definition_id_src OR (original_ref.definition_id_dst IS NULL AND symmetric_ref.definition_id_src IS NULL))
        WHERE
            ref_type.is_symmetric = 1
            AND symmetric_ref.id IS NULL;
        "
    )?;
    let mut stmt_insert_at_shared_id = conn.prepare_cached(
        r"
        SELECT
            CASE
                /*
                * First, check if the original reference points to a specific definition (definition_id_dst is not NULL).
                */
                WHEN original_ref.definition_id_dst IS NOT NULL THEN
                    COALESCE(
                        /*
                        * Priority 1: Find the rank of the last outgoing reference from the destination definition.
                        * The subquery looks for all references originating from that specific definition and picks the highest rank.
                        * If no such references exist, this subquery will return NULL.
                        */
                        (
                            SELECT MAX(shared.rank)
                            FROM dict_reference AS outgoing_ref
                            JOIN dict_shared AS shared ON outgoing_ref.shared_id = shared.id
                            WHERE outgoing_ref.word_id_src = original_ref.word_id_dst
                            AND outgoing_ref.definition_id_src = original_ref.definition_id_dst
                        ),
                        /*
                        * Priority 2: If the first subquery was NULL (no outgoing references), COALESCE falls back to this one.
                        * This finds the rank of the destination definition itself.
                        */
                        (
                            SELECT shared.rank
                            FROM dict_definition AS def
                            JOIN dict_shared AS shared ON def.shared_id = shared.id
                            WHERE def.id = original_ref.definition_id_dst
                        )
                    )

                /*
                * If definition_id_dst is NULL, the original reference points to a word in general.
                * This corresponds to your third and fourth priority rules.
                */
                ELSE
                    COALESCE(
                        /*
                        * Priority 3: Find the rank of the last outgoing reference from the destination word.
                        * This subquery looks for references originating from the word itself (not tied to a specific definition).
                        * It will return NULL if no such references exist.
                        */
                        (
                            SELECT MAX(shared.rank)
                            FROM dict_reference AS outgoing_ref
                            JOIN dict_shared AS shared ON outgoing_ref.shared_id = shared.id
                            WHERE outgoing_ref.word_id_src = original_ref.word_id_dst
                            AND outgoing_ref.definition_id_src IS NULL
                        ),
                        /*
                        * Priority 4: If the third subquery was NULL, COALESCE falls back to this one.
                        * This finds the rank of the destination word itself.
                        */
                        (
                            SELECT shared.rank
                            FROM dict_word AS word
                            JOIN dict_shared AS shared ON word.shared_id = shared.id
                            WHERE word.id = original_ref.word_id_dst
                        )
                    )
            END AS correct_rank
        FROM
            dict_reference AS original_ref
        WHERE
            original_ref.id = ?1;
        "
    )?;

    let mut rows = stmt_missing_references.query([])?;

    // TODO log which lines have been added
    while let Some(row) = rows.next()? {
        let ref_id: SqliteId = row.get("id")?;
        let ref_type_id: SqliteId = row.get("ref_type_id")?;
        let word_id_src: SqliteId = row.get("word_id_src")?;
        let definition_id_src: Option<SqliteId> = row.get("definition_id_src")?;
        let word_id_dst: SqliteId = row.get("word_id_dst")?;
        let definition_id_dst: Option<SqliteId> = row.get("definition_id_dst")?;
        let rank_to_insert_at: SqliteId =
            stmt_insert_at_shared_id.query_one((ref_id,), |row| row.get(0))?;
        // TODO use insert_reference function, potentially modify the insert_at_shared_id query so that references of the same kind are grouped together
        let mut stmt =
            conn.prepare_cached("INSERT INTO dict_shared (rank, rank_relative) VALUES (?1,?2)")?;
        stmt.execute((rank_to_insert_at, 1))?;
        let shared_id = conn.last_insert_rowid();
        let mut stmt = conn
            .prepare_cached("INSERT INTO dict_reference (shared_id, ref_type_id, word_id_src, definition_id_src, word_id_dst, definition_id_dst) VALUES (?1,?2,?3,?4,?5,?6)")?;
        stmt.execute((
            shared_id,
            ref_type_id,
            // switch source and destination ids
            word_id_dst,
            definition_id_dst,
            word_id_src,
            definition_id_src,
        ))?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines, reason = "SQL")]
pub fn add_missing_notes_and_tags_for_symmetric_references(
    conn: &Transaction,
) -> Result<(), SqliteError> {
    conn.execute_batch(
        r"
        -- ref1 to ref2

        -- Use INSERT OR IGNORE to prevent errors if the tag relationship already exists
        INSERT OR IGNORE INTO dict_shared_tag (for_shared_id, tag_id)
        SELECT
            ref2.shared_id,
            tags1.tag_id
        FROM
            dict_reference AS ref1
        JOIN
            dict_ref_type AS ref_type ON ref1.ref_type_id = ref_type.id
        JOIN
            dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst AND ref1.word_id_dst = ref2.word_id_src AND ref1.ref_type_id = ref2.ref_type_id AND (ref1.definition_id_src = ref2.definition_id_dst OR (ref1.definition_id_src IS NULL AND ref2.definition_id_dst IS NULL)) AND (ref1.definition_id_dst = ref2.definition_id_src OR (ref1.definition_id_dst IS NULL AND ref2.definition_id_src IS NULL))
        -- Get tags from ref1
        JOIN
            dict_shared_tag AS tags1 ON ref1.shared_id = tags1.for_shared_id
        WHERE
            ref_type.is_symmetric = 1
            AND ref1.id < ref2.id
            -- And the tag does not exist for ref2
            AND NOT EXISTS (
                SELECT 1
                FROM dict_shared_tag AS tags2
                WHERE tags2.for_shared_id = ref2.shared_id AND tags2.tag_id = tags1.tag_id
            );

        -- ref2 to ref1
        INSERT OR IGNORE INTO dict_shared_tag (for_shared_id, tag_id)
        SELECT
            ref1.shared_id,
            tags2.tag_id
        FROM
            dict_reference AS ref1
        JOIN
            dict_ref_type AS ref_type ON ref1.ref_type_id = ref_type.id
        JOIN
            dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst AND ref1.word_id_dst = ref2.word_id_src AND ref1.ref_type_id = ref2.ref_type_id AND (ref1.definition_id_src = ref2.definition_id_dst OR (ref1.definition_id_src IS NULL AND ref2.definition_id_dst IS NULL)) AND (ref1.definition_id_dst = ref2.definition_id_src OR (ref1.definition_id_dst IS NULL AND ref2.definition_id_src IS NULL))
        -- Get tags from ref2
        JOIN
            dict_shared_tag AS tags2 ON ref2.shared_id = tags2.for_shared_id
        WHERE
            ref_type.is_symmetric = 1
            AND ref1.id < ref2.id
            -- And the tag does not exist for ref1
            AND NOT EXISTS (
                SELECT 1
                FROM dict_shared_tag AS tags1
                WHERE tags1.for_shared_id = ref1.shared_id AND tags1.tag_id = tags2.tag_id
            );
        "
    )?;
    conn.execute_batch(
        r"
        -- copy note from ref2 to ref1
        UPDATE
            dict_shared
        SET
            note_id = (
                SELECT shared2.note_id
                FROM dict_reference AS ref1
                JOIN dict_ref_type AS ref_type ON ref1.ref_type_id = ref_type.id
                JOIN dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst
                    AND ref1.word_id_dst = ref2.word_id_src
                    AND ref1.ref_type_id = ref2.ref_type_id
                    AND (ref1.definition_id_src = ref2.definition_id_dst
                        OR (ref1.definition_id_src IS NULL AND ref2.definition_id_dst IS NULL))
                    AND (ref1.definition_id_dst = ref2.definition_id_src
                        OR (ref1.definition_id_dst IS NULL AND ref2.definition_id_src IS NULL))
                JOIN dict_shared AS shared2 ON ref2.shared_id = shared2.id
                WHERE ref1.shared_id = dict_shared.id
                    AND ref_type.is_symmetric = 1
                    AND ref1.id < ref2.id
                    AND shared2.note_id IS NOT NULL
            )
        WHERE
            dict_shared.note_id IS NULL
            AND dict_shared.id IN (
                SELECT ref1.shared_id
                FROM dict_reference AS ref1
                JOIN dict_ref_type AS ref_type ON ref1.ref_type_id = ref_type.id
                JOIN dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst
                    AND ref1.word_id_dst = ref2.word_id_src
                    AND ref1.ref_type_id = ref2.ref_type_id
                    AND (ref1.definition_id_src = ref2.definition_id_dst
                        OR (ref1.definition_id_src IS NULL AND ref2.definition_id_dst IS NULL))
                    AND (ref1.definition_id_dst = ref2.definition_id_src
                        OR (ref1.definition_id_dst IS NULL AND ref2.definition_id_src IS NULL))
                JOIN dict_shared AS shared2 ON ref2.shared_id = shared2.id
                WHERE ref_type.is_symmetric = 1
                    AND ref1.id < ref2.id
                    AND shared2.note_id IS NOT NULL
            );

        -- copy note from ref1 to ref2
        UPDATE
            dict_shared
        SET
            note_id = (
                SELECT shared1.note_id
                FROM dict_reference AS ref2
                JOIN dict_ref_type AS ref_type ON ref2.ref_type_id = ref_type.id
                JOIN dict_reference AS ref1 ON ref2.word_id_src = ref1.word_id_dst
                    AND ref2.word_id_dst = ref1.word_id_src
                    AND ref2.ref_type_id = ref1.ref_type_id
                    AND (ref2.definition_id_src = ref1.definition_id_dst
                        OR (ref2.definition_id_src IS NULL AND ref1.definition_id_dst IS NULL))
                    AND (ref2.definition_id_dst = ref1.definition_id_src
                        OR (ref2.definition_id_dst IS NULL AND ref1.definition_id_src IS NULL))
                JOIN dict_shared AS shared1 ON ref1.shared_id = shared1.id
                WHERE ref2.shared_id = dict_shared.id
                    AND ref_type.is_symmetric = 1
                    AND ref1.id < ref2.id
                    AND shared1.note_id IS NOT NULL
            )
        WHERE
            dict_shared.note_id IS NULL
            AND dict_shared.id IN (
                SELECT ref2.shared_id
                FROM dict_reference AS ref2
                JOIN dict_ref_type AS ref_type ON ref2.ref_type_id = ref_type.id
                JOIN dict_reference AS ref1 ON ref2.word_id_src = ref1.word_id_dst
                    AND ref2.word_id_dst = ref1.word_id_src
                    AND ref2.ref_type_id = ref1.ref_type_id
                    AND (ref2.definition_id_src = ref1.definition_id_dst
                        OR (ref2.definition_id_src IS NULL AND ref1.definition_id_dst IS NULL))
                        AND (ref2.definition_id_dst = ref1.definition_id_src
                            OR (ref2.definition_id_dst IS NULL AND ref1.definition_id_src IS NULL))
                JOIN dict_shared AS shared1 ON ref1.shared_id = shared1.id
                WHERE ref_type.is_symmetric = 1
                    AND ref1.id < ref2.id
                    AND shared1.note_id IS NOT NULL
            );
        ",
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_reference(
    conn: &Transaction,
    ref_type_id: SqliteId,
    src_word_id: SqliteId,
    src_def_id: Option<SqliteId>,
    dst_word_id: SqliteId,
    dst_def_id: Option<SqliteId>,
    rank_relative: usize,
) -> Result<(), SqliteError> {
    let rank_to_insert_at: i64 = if let Some(def_id) = src_def_id {
        let mut stmt = conn.prepare_cached(
            r"
            SELECT COALESCE(
                (SELECT MAX(s.rank) FROM dict_reference r JOIN dict_shared s ON r.shared_id = s.id WHERE r.word_id_src = ?1 AND r.definition_id_src = ?2 AND r.ref_type_id = ?3),
                (SELECT MAX(s.rank) FROM dict_reference r JOIN dict_shared s ON r.shared_id = s.id WHERE r.word_id_src = ?1 AND r.definition_id_src = ?2),
                (SELECT s.rank FROM dict_definition d JOIN dict_shared s ON d.shared_id = s.id WHERE d.id = ?2)
            )
            ",
        )?;
        stmt.query_row(params![src_word_id, def_id, ref_type_id], |row| row.get(0))?
    } else {
        let mut stmt = conn.prepare_cached(
            r"
            SELECT COALESCE(
                (SELECT MAX(s.rank) FROM dict_reference r JOIN dict_shared s ON r.shared_id = s.id WHERE r.word_id_src = ?1 AND r.definition_id_src IS NULL AND r.ref_type_id = ?2),
                (SELECT MAX(s.rank) FROM dict_reference r JOIN dict_shared s ON r.shared_id = s.id WHERE r.word_id_src = ?1 AND r.definition_id_src IS NULL),
                (SELECT s.rank FROM dict_word w JOIN dict_shared s ON w.shared_id = s.id WHERE w.id = ?1)
            )
            ",
        )?;
        stmt.query_row(params![src_word_id, ref_type_id], |row| row.get(0))?
    };

    // Insert into dict_shared
    let mut stmt_insert_shared = conn.prepare_cached(
        r"
        INSERT INTO dict_shared (rank, rank_relative)
        VALUES (?1, ?2);
        ",
    )?;
    stmt_insert_shared.execute(params![rank_to_insert_at, rank_relative])?;
    let shared_id = conn.last_insert_rowid();

    // Insert into dict_reference
    let mut stmt_insert_ref = conn.prepare_cached(
        r"
        INSERT INTO dict_reference (shared_id, ref_type_id, word_id_src, definition_id_src, word_id_dst, definition_id_dst)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6);
        ",
    )?;
    stmt_insert_ref.execute(params![
        shared_id,
        ref_type_id,
        src_word_id,
        src_def_id,
        dst_word_id,
        dst_def_id,
    ])?;

    Ok(())
}

/// Get reference type id for ascii_char, only if this type already exists in DB
pub fn get_ref_type_id(
    conn: &Transaction,
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
    conn: &Transaction,
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

pub fn get_definition_id(
    conn: &Transaction,
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

/// Retrieves the shared_id for a given target entity.
pub fn get_shared_id(conn: &Transaction, id: EntryId) -> Result<SqliteId, SqliteError> {
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
    }
}

/// Retrieves the ID of a tag from the dict_tag table.
/// If the tag does not exist, it is inserted into the table, and the new ID is returned.
fn get_or_insert_tag_id(conn: &Transaction, tag: &Tag) -> Result<SqliteId, SqliteError> {
    match tag {
        Tag::Ascii(ascii_char) => {
            if let Some((name, category, _rank)) = config::tag_to_txt_ascii_common(*ascii_char) {
                let mut stmt =
                    conn.prepare_cached("SELECT id FROM dict_tag WHERE tag = ?1 AND type = ?2")?;
                match stmt.query_row(params![name, category], |row| row.get(0)) {
                    Ok(id) => Ok(id),
                    Err(SqliteError::QueryReturnedNoRows) => {
                        let mut insert_stmt = conn.prepare_cached(
                            "INSERT INTO dict_tag (tag, type, ascii_symbol) VALUES (?1, ?2, ?3)",
                        )?;
                        insert_stmt.execute(params![name, category, &ascii_char.to_string()])?;
                        Ok(conn.last_insert_rowid())
                    }
                    Err(e) => Err(e),
                }
            } else {
                panic!("Invalid ASCII tag: {ascii_char}");
            }
        }
        Tag::Full { name, category } => {
            let mut stmt =
                conn.prepare_cached("SELECT id FROM dict_tag WHERE tag = ?1 AND type = ?2")?;
            match stmt.query_row(params![name, category], |row| row.get(0)) {
                Ok(id) => Ok(id),
                Err(SqliteError::QueryReturnedNoRows) => {
                    let mut insert_stmt =
                        conn.prepare_cached("INSERT INTO dict_tag (tag, type) VALUES (?1, ?2)")?;
                    insert_stmt.execute(params![name, category])?;
                    Ok(conn.last_insert_rowid())
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// Adds a tag to a word, definition, pinyin, or reference.
///
/// If the target ID does not exist, the function does nothing and succeeds.
/// If the provided tag does not exist in the dict_tag table, it will be created automatically.
pub fn add_tag(conn: &Transaction, target: EntryId, tag: Tag) -> Result<(), SqliteError> {
    let shared_id = get_shared_id(conn, target)?;
    let tag_id = get_or_insert_tag_id(conn, &tag)?;
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO dict_shared_tag (for_shared_id, tag_id) VALUES (?1, ?2)",
    )?;
    stmt.execute(params![shared_id, tag_id])?;

    Ok(())
}

/// Updates the definition text for a given definition ID.
fn update_definition_text(
    conn: &Transaction,
    definition_id: SqliteId,
    text: &str,
) -> Result<(), SqliteError> {
    let mut stmt = conn.prepare_cached(
        r"
        UPDATE dict_definition
        SET definition = ?2
        WHERE id = ?1;
        ",
    )?;
    stmt.execute(params![definition_id, text])?;
    Ok(())
}

/// not for regular use, uses unwrap
pub fn definition_text_to_tags(conn: &Transaction, csv_path: &str) {
    // Read the CSV file into a HashMap
    let mut tag_to_data = HashMap::new();
    let file = File::open(csv_path).unwrap();
    let reader = io::BufReader::new(file);

    for line in reader.lines() {
        let line = line.unwrap();
        let columns: Vec<&str> = line.split(';').collect();
        if columns.len() == 4 {
            let key = columns[0].to_string();
            let value = (
                columns[1].to_string(),
                columns[2].to_string(),
                columns[3].to_string(),
            );
            tag_to_data.insert(key, value);
        }
    }

    // Iterate over all definitions from the database
    let definitions = db_read::read_definitions(conn).unwrap();

    'defloop: for (definition, grouping) in definitions {
        if !definition.definition.starts_with('(') {
            continue;
        }
        let mut new_definition_text = definition.definition.clone();
        let mut tag_start_idx = 1; // skip first (
        if definition.definition.starts_with("(～") {
            if let Some((index, _)) = definition.definition[tag_start_idx..]
                .char_indices()
                .find(|(i, c)| *c == '(')
            {
                if let Some((prev_closing_index, _)) = definition.definition[tag_start_idx..]
                    .char_indices()
                    .find(|(i, c)| *c == ')')
                {
                    if index - prev_closing_index > 2 {
                        continue; // not at the beginning of the definition -> not a tag group
                    }
                }
                tag_start_idx += index + 1;
            }
        }
        let mut tag_end_idx = tag_start_idx
            + definition.definition[tag_start_idx..]
                .char_indices()
                .find(|(i, c)| *c == ')')
                .map(|t| t.0)
                .unwrap_or(0);
        let tags: Vec<String> = definition.definition[tag_start_idx..tag_end_idx]
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let mut as_tags = vec![];
        let mut in_parens = vec![];
        let mut ascii_tags = "".to_owned();
        let mut keep_following_in_parens = false;
        let mut override_exclusion = false;
        for tag in &tags {
            if keep_following_in_parens {
                in_parens.push(tag.to_owned());
                continue;
            }
            if tag.starts_with("of ")
                || tag.starts_with("chiefly of ")
                || (tag.starts_with("in ") && tag_to_data.get(tag).is_none())
                || tag.starts_with("chiefly in ")
            {
                keep_following_in_parens = true;
                in_parens.push(tag.to_owned());
                continue;
            }
            if tag.ends_with("etc.") {
                // skip this definition completely
                debug_assert!(!keep_following_in_parens);
                continue 'defloop;
            }
            if let Some((full_tags, tag_ascii_tags, overwrite_x)) = tag_to_data.get(tag) {
                if !full_tags.is_empty() {
                    // in the csv file some tags are separated into multiple tags by a space
                    for full_tag in full_tags.split(' ') {
                        as_tags.push(full_tag);
                    }
                }
                ascii_tags.push_str(&tag_ascii_tags);
                override_exclusion = override_exclusion || (overwrite_x == "1");
            } else {
                in_parens.push(tag.to_owned());
            }
        }
        for full_tag in as_tags {
            add_tag(
                conn,
                EntryId::Definition(definition.id),
                Tag::Full {
                    name: full_tag,
                    category: "",
                },
            )
            .unwrap();
        }
        if override_exclusion {
            ascii_tags = ascii_tags.replace('x', "");
        }
        if ascii_tags.to_lowercase().contains('t') && ascii_tags.to_lowercase().contains('c') {
            // if it's applicable to Taiwan and China, no indication is needed
            ascii_tags = ascii_tags.replace('T', "");
            ascii_tags = ascii_tags.replace('t', "");
            ascii_tags = ascii_tags.replace('C', "");
            ascii_tags = ascii_tags.replace('c', "");
        }
        if ascii_tags.to_lowercase().contains('x') && ascii_tags.to_lowercase().contains('-') {
            // exclude definition, even if one tag is only mapped to -
            ascii_tags = ascii_tags.replace('-', "");
        }
        for ascii_char in ascii_tags.chars() {
            if let Some(_) = config::tag_to_txt_ascii_common(ascii_char) {
                add_tag(
                    conn,
                    EntryId::Definition(definition.id),
                    Tag::Ascii(ascii_char),
                )
                .unwrap();
            } else {
                println!("not a tag:{ascii_char}");
            }
        }
        let new_parens = if !in_parens.is_empty() {
            format!("({})", in_parens.join(", "))
        } else {
            // check if there is whitespace to remove before or after tag boundaries
            if new_definition_text.is_char_boundary(tag_start_idx - 1)
                && new_definition_text[..tag_start_idx - 1].ends_with(' ')
            {
                tag_start_idx -= 1;
            }
            if new_definition_text.is_char_boundary(tag_end_idx + 1)
                && new_definition_text[tag_end_idx + 1..].starts_with(' ')
            {
                tag_end_idx += 1;
            }
            " ".to_owned()
        };
        if !(new_definition_text.is_char_boundary(tag_start_idx - 1)
            && new_definition_text.is_char_boundary(tag_end_idx + 1))
        {
            println!("buggy: {new_definition_text}");
            continue;
        }
        new_definition_text.replace_range(tag_start_idx - 1..tag_end_idx + 1, &new_parens);

        if new_definition_text != definition.definition {
            update_definition_text(conn, definition.id, &new_definition_text.trim()).unwrap();
        }
    }
}

/// Converts pinyin for Erhua entries.
///
/// This function identifies entries where the traditional form of a word ends in '兒' (indicating Erhua),
/// and the pinyin ends with 'r' followed by a tone number (e.g., "yi1dianr3"). It converts such pinyins
/// to a format where the tone is applied to the preceding syllable and the 'r' is marked with a neutral
/// tone '5' (e.g., "yi1dian3r5"). This ensures the number of pinyin syllables matches the number of characters.
///
/// Entries where the pinyin already ends in 'er2' are not modified, as these are not considered Erhua.
/// 
/// not for regular use, uses unwrap
pub fn convert_erhua_pinyin(conn: &Transaction) -> Result<(), SqliteError> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            p.id,
            p.pinyin_num
        FROM dict_pron p
        JOIN dict_shared_pron sp ON p.id = sp.pron_id
        JOIN dict_pron_definition pd ON sp.id = pd.shared_pron_id
        JOIN dict_definition d ON pd.definition_id = d.id
        JOIN dict_word w ON d.word_id = w.id
        WHERE w.trad LIKE '%兒' AND p.pinyin_num LIKE '%r_'
        "#,
    )?;

    let mut rows = stmt.query([])?;
    let mut updates = Vec::new();

    while let Some(row) = rows.next()? {
        let pron_id: SqliteId = row.get(0)?;
        let pinyin_num: String = row.get(1)?;

        if let Some(last_char) = pinyin_num.chars().last() {
            if last_char.is_ascii_digit()
                && pinyin_num != "er2"
                && !(pinyin_num.ends_with("er2")
                    && pinyin_num
                        .chars()
                        .nth(pinyin_num.len() - 4)
                        .unwrap_or('0')
                        .is_ascii_digit())
            {
                let tone = last_char.to_digit(10).unwrap();
                let r_index = pinyin_num.len() - 2;
                if pinyin_num.chars().nth(r_index) == Some('r') {
                    let mut new_pinyin_num = pinyin_num[..r_index].to_string();
                    new_pinyin_num.push_str(&tone.to_string());
                    new_pinyin_num.push_str("r5");

                    updates.push((pron_id, new_pinyin_num));
                }
            }
        }
    }

    let mut update_stmt = conn.prepare(
        r#"
        UPDATE dict_pron
        SET pinyin_num = ?2, pinyin_mark = ?3
        WHERE id = ?1
        "#,
    )?;

    for (pron_id, new_pinyin_num) in updates {
        let new_pinyin_mark = pinyin::pinyin_mark_from_num(&new_pinyin_num);
        update_stmt.execute(params![pron_id, new_pinyin_num, new_pinyin_mark])?;
    }

    Ok(())
}
