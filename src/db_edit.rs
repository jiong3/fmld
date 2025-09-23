use std::cmp::max;

use rusqlite::{Error as SqliteError, Transaction};

use crate::common::SqliteId;

pub fn finalize_note_ids(conn: &Transaction, max_ext_note_id: u32) -> Result<u32, SqliteError> {
    let mut stmt_max_ext_note_id = conn.prepare(
        r"
        SELECT MAX(dict_note.ext_note_id)
        FROM dict_note;
        "
    )?;
    let max_ext_note_id_db: u32 = stmt_max_ext_note_id.query_one((), |row| row.get(0))?;
    let mut base_ext_note_id = max(max_ext_note_id, max_ext_note_id_db);
    let mut stmt_note_ids_to_update = conn.prepare(
        r"
        SELECT dict_note.id
        FROM dict_note
        WHERE  dict_note.ext_note_id < 100;
        "
    )?;
    let mut stmt_update_note_id = conn.prepare_cached(
        r"
        UPDATE dict_note
        SET ext_note_id=?2
        WHERE id=?1;
        "
    )?;
    let mut stmt_shared_max_note_id = conn.prepare_cached(
        r"
        UPDATE dict_shared
        SET note_id=?1
        WHERE id=1;
        "
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
