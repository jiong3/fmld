use crate::common::SqliteId;
use rusqlite::{Error as SqliteError, Transaction, params};
use std::cmp::max;
use std::collections::HashSet;

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

/// Sorts pronunciations within the same definition group based on tag-derived ranks.
///
/// For each group of pronunciations belonging to a single definition, this function
/// calculates a 'relative rank' for each pronunciation based on its tags.
/// It then updates the database to normalize the `rank` for all pronunciations
/// in the group to the minimum rank found within that group, and sets the
/// `rank_relative` to the calculated score. This allows for sorted retrieval
/// of pronunciations based on their tags.
///
/// The tag-to-score mapping is as follows:
/// - 'C' (china-only): 1
/// - 'T' (taiwan-only): 2
/// - '-' (extended): 5
/// - 'x' (excluded): 10
/// - 'X' (deleted): 20
/// - Any other tag or no tag: 0
/// The final score is the sum of the scores of all tags.
pub fn sort_pronunciations_by_tag_rank(conn: &Transaction) -> Result<(), SqliteError> {
    // Select all definition_ids that are associated with more than one pronunciation,
    // as these are the only groups that require sorting.
    let mut stmt_groups = conn.prepare(
        r"
        SELECT definition_id
        FROM dict_pron_definition
        GROUP BY definition_id
        HAVING COUNT(shared_pron_id) > 1;
    ",
    )?;
    let definition_ids: Vec<SqliteId> = stmt_groups
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    // Prepare statements for efficient reuse within the loop.
    let mut stmt_pron_info = conn.prepare_cached(
        r"
        SELECT
            dsp.shared_id,
            ds.rank
        FROM dict_pron_definition AS dpd
        JOIN dict_shared_pron AS dsp ON dpd.shared_pron_id = dsp.id
        JOIN dict_shared AS ds ON dsp.shared_id = ds.id
        WHERE dpd.definition_id = ?1;
    ",
    )?;

    let mut stmt_tags = conn.prepare_cached(
        r"
        SELECT
            dt.ascii_symbol
        FROM dict_shared_tag AS dst
        JOIN dict_tag AS dt ON dst.tag_id = dt.id
        WHERE dst.for_shared_id = ?1 AND dt.ascii_symbol IS NOT NULL;
    ",
    )?;

    let mut stmt_update = conn.prepare_cached(
        r"
        UPDATE dict_shared
        SET rank = ?1, rank_relative = ?2
        WHERE id = ?3;
    ",
    )?;

    let mut processed_pron_groups = HashSet::new();

    // Iterate over each definition group that needs processing.
    for def_id in definition_ids {
        // Fetch the shared_id and rank for all pronunciations in the current group.
        let pron_infos: Vec<(SqliteId, i64)> =
            stmt_pron_info // (shared_id, rank)
                .query_map(params![def_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;

        if pron_infos.is_empty() {
            continue;
        }

        // Determine the minimum rank within the group. This will become the new
        // standard rank for all pronunciations in this group to ensure they are
        // grouped together during ordered retrieval.
        let min_rank = pron_infos.iter().map(|&(_, rank)| rank).min().unwrap();
        if processed_pron_groups.contains(&min_rank) {
            // skip groups which have been sorted already
            continue;
        }
        processed_pron_groups.insert(min_rank);

        // Calculate the score for each pronunciation based on its tags.
        let mut pron_scores = Vec::new();
        for (shared_id, _original_rank) in pron_infos {
            let tags: Vec<String> = stmt_tags
                .query_map(params![shared_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;

            let score: i32 = tags
                .iter()
                .map(|tag| match tag.chars().next().unwrap_or(' ') {
                    'C' => 1,
                    'T' => 5,
                    '-' => 2,
                    'x' => 10,
                    'X' => 20,
                    _ => 0,
                })
                .sum();

            pron_scores.push((shared_id, score));
        }

        // Update each pronunciation's shared entry with the new common rank
        // and its calculated relative rank (score).
        for (shared_id, score) in pron_scores {
            stmt_update.execute(params![min_rank, score, shared_id])?;
        }
    }

    Ok(())
}

/// Sorts the words in the second part, which is not sorted by frequency, by the codepoint of the first traditional character
/// The "pivot" indicating the start of the second part is "%" or any other character with a codepoint smaller than "%".
/// All entries up to "%" keep their order. The internal order of a word block (word, definitions, pronunciations) is preserved.
/// Word entries which have variant_of != NULL are not considered, since they share the same shared_id as the main variant.
///
/// The goal of the sorting is to be able to split the file into smaller files based on unicode ranges, if necessary.
/// 
/// This function assumes the database is in a consistent state (no orphans) and that 'rank_relative'
/// is NULL for all entries in the sorted region before this operation.
pub fn sort_words_after_pivot(conn: &Transaction) -> Result<(), SqliteError> {
    // 1. Identify the rank of the pivot word (the last word <= "%").
    // We treat everything up to and including this rank (and its dependent definitions) as the "fixed" header.
    let pivot_rank: i64 = conn
        .query_row(
            r"
        SELECT COALESCE(MAX(s.rank), 0)
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        WHERE w.trad <= '%'
        ",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // 2. Find the starting rank of the first word that appears strictly after the pivot.
    // This defines the start of the region we are allowed to reorder. Using the rank of the first
    // word ensures we do not accidentally move definitions belonging to the pivot word itself.
    let start_rank: Option<i64> = conn.query_row(
        r"
        SELECT MIN(s.rank)
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        WHERE s.rank > ?1
        ",
        params![pivot_rank],
        |row| row.get(0),
    )?;

    // If there are no words after the pivot, there is nothing to sort.
    let start_rank = match start_rank {
        Some(r) => r,
        None => return Ok(()),
    };

    // 3. Fetch all shared entries in the sortable region.
    // These are the IDs that will be assigned new ranks.
    let mut stmt_ids = conn.prepare(
        r"
        SELECT id
        FROM dict_shared
        WHERE rank >= ?1
        ORDER BY rank ASC
        ",
    )?;

    let all_shared_id_ids: Vec<SqliteId> = stmt_ids
        .query_map(params![start_rank], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    // 4. Fetch the headers for the word blocks in the sortable region.
    // These contain the data we need to perform the sort.
    struct WordBlockHeader {
        shared_id: SqliteId,
        trad: String,
    }

    let mut stmt_words = conn.prepare(
        r"
        SELECT w.shared_id, w.trad, w.simp
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        WHERE s.rank >= ?1 AND w.variant_of IS NULL
        ORDER BY s.rank ASC
        ",
    )?;

    let word_headers: Vec<WordBlockHeader> = stmt_words
        .query_map(params![start_rank], |row| {
            Ok(WordBlockHeader {
                shared_id: row.get(0)?,
                trad: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    if word_headers.is_empty() {
        return Ok(());
    }

    // 5. Group the fetched IDs into "blocks".
    // Each block consists of a Word's shared_id followed by the shared_ids of its dependent entries
    // (definitions, pronunciations, etc.), up to the start of the next Word.
    struct Block {
        sort_key: char,
        ids: Vec<SqliteId>,
    }

    let mut blocks = Vec::with_capacity(word_headers.len());
    let mut shared_id_ids_iter = all_shared_id_ids.into_iter().peekable();

    for (i, header) in word_headers.iter().enumerate() {
        let next_header_shared_id = word_headers.get(i + 1).map(|h| h.shared_id);

        let mut block_ids = Vec::new();

        // Accumulate IDs into the current block until we hit the shared_id of the next word.
        // Note: matching assumes the DB is consistent (no orphans) and `start_rank` aligns exactly
        // with the first word in the list, which it should by definition.
        loop {
            match shared_id_ids_iter.peek() {
                Some(id) => {
                    if Some(*id) == next_header_shared_id {
                        break; // Stop: this ID belongs to the start of the next block
                    }
                    block_ids.push(*id);
                    shared_id_ids_iter.next();
                }
                None => break,
            }
        }

        blocks.push(Block {
            sort_key: header.trad.chars().next().unwrap_or_default(),
            ids: block_ids,
        });
    }

    // 6. Sort the blocks based on Unicode codepoint of the first traditional character
    blocks.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

    // 7. Write the new order back to the database.
    // We increment the rank sequentially starting from start_rank.
    let mut current_rank = start_rank;
    let mut stmt_update = conn
        .prepare_cached("UPDATE dict_shared SET rank = ?1, rank_relative = NULL WHERE id = ?2")?;

    for block in blocks {
        for id in block.ids {
            stmt_update.execute(params![current_rank, id])?;
            current_rank += 1;
        }
    }

    Ok(())
}
