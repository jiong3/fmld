use crate::common::SqliteId;
use crate::db_edit;
use crate::config;
use rusqlite::{Error as SqliteError, Transaction, params};
use std::cmp::max;
use std::collections::HashSet;

pub fn autofix(tx: &Transaction) -> Result<(), SqliteError> {
    delete_references_marked_for_deletion(&tx)?;
    add_missing_symmetric_references(&tx)?;
    add_missing_notes_and_tags_for_symmetric_references(&tx)?;
    sort_references(&tx)?;
    sort_sentences(&tx)?;
    apply_graded_tags_to_sentences(&tx)?;
    sort_pronunciations_by_tag_rank(&tx)?;
    sort_words_after_pivot(&tx)?;
    Ok(())
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

/// Add corresponding symmetric inverse reference to all symmetric references if they are missing
pub fn add_missing_symmetric_references(conn: &Transaction) -> Result<(), SqliteError> {
    // find all references with missing symmetric counterpart
    let mut stmt_missing_references = conn.prepare(
        r"
        SELECT
            original_ref.id,
            original_ref.ascii_symbol,
            original_ref.word_id_src,
            original_ref.definition_id_src,
            original_ref.word_id_dst,
            original_ref.definition_id_dst
        FROM
            dict_reference AS original_ref
        JOIN
            dict_ref_type AS ref_type ON original_ref.ascii_symbol = ref_type.ascii_symbol
        LEFT JOIN
            dict_reference AS symmetric_ref ON original_ref.word_id_src = symmetric_ref.word_id_dst
                                            AND original_ref.word_id_dst = symmetric_ref.word_id_src
                                            AND original_ref.ascii_symbol = symmetric_ref.ascii_symbol
                                            AND (original_ref.definition_id_src = symmetric_ref.definition_id_dst OR (original_ref.definition_id_src IS NULL AND symmetric_ref.definition_id_dst IS NULL))
                                            AND (original_ref.definition_id_dst = symmetric_ref.definition_id_src OR (original_ref.definition_id_dst IS NULL AND symmetric_ref.definition_id_src IS NULL))
        WHERE
            ref_type.is_symmetric = 1
            AND symmetric_ref.id IS NULL;
        "
    )?;

    let mut rows = stmt_missing_references.query([])?;

    while let Some(row) = rows.next()? {
        let ascii_symbol: String = row.get("ascii_symbol")?;
        let word_id_src: SqliteId = row.get("word_id_src")?;
        let definition_id_src: Option<SqliteId> = row.get("definition_id_src")?;
        let word_id_dst: SqliteId = row.get("word_id_dst")?;
        let definition_id_dst: Option<SqliteId> = row.get("definition_id_dst")?;

        db_edit::insert_reference(conn, ascii_symbol.chars().next().unwrap(), word_id_dst, definition_id_dst, word_id_src, definition_id_src, None)?;
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
            dict_ref_type AS ref_type ON ref1.ascii_symbol = ref_type.ascii_symbol
        JOIN
            dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst AND ref1.word_id_dst = ref2.word_id_src AND ref1.ascii_symbol = ref2.ascii_symbol AND (ref1.definition_id_src = ref2.definition_id_dst OR (ref1.definition_id_src IS NULL AND ref2.definition_id_dst IS NULL)) AND (ref1.definition_id_dst = ref2.definition_id_src OR (ref1.definition_id_dst IS NULL AND ref2.definition_id_src IS NULL))
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
            dict_ref_type AS ref_type ON ref1.ascii_symbol = ref_type.ascii_symbol
        JOIN
            dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst AND ref1.word_id_dst = ref2.word_id_src AND ref1.ascii_symbol = ref2.ascii_symbol AND (ref1.definition_id_src = ref2.definition_id_dst OR (ref1.definition_id_src IS NULL AND ref2.definition_id_dst IS NULL)) AND (ref1.definition_id_dst = ref2.definition_id_src OR (ref1.definition_id_dst IS NULL AND ref2.definition_id_src IS NULL))
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
                JOIN dict_ref_type AS ref_type ON ref1.ascii_symbol = ref_type.ascii_symbol
                JOIN dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst
                    AND ref1.word_id_dst = ref2.word_id_src
                    AND ref1.ascii_symbol = ref2.ascii_symbol
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
                JOIN dict_ref_type AS ref_type ON ref1.ascii_symbol = ref_type.ascii_symbol
                JOIN dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst
                    AND ref1.word_id_dst = ref2.word_id_src
                    AND ref1.ascii_symbol = ref2.ascii_symbol
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
                JOIN dict_ref_type AS ref_type ON ref2.ascii_symbol = ref_type.ascii_symbol
                JOIN dict_reference AS ref1 ON ref2.word_id_src = ref1.word_id_dst
                    AND ref2.word_id_dst = ref1.word_id_src
                    AND ref2.ascii_symbol = ref1.ascii_symbol
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
                JOIN dict_ref_type AS ref_type ON ref2.ascii_symbol = ref_type.ascii_symbol
                JOIN dict_reference AS ref1 ON ref2.word_id_src = ref1.word_id_dst
                    AND ref2.word_id_dst = ref1.word_id_src
                    AND ref2.ascii_symbol = ref1.ascii_symbol
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
///  - 'C' (china-only): 1
///  - 'T' (taiwan-only): 2
///  - '-' (extended): 5
///  - 'x' (excluded): 10
///  - 'X' (deleted): 20
///  - Any other tag or no tag: 0
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


/// Removes all references which have the ascii_tag 'X' (excluded/deleted).
/// If the reference type is symmetric, also removes the corresponding inverse reference.
pub fn delete_references_marked_for_deletion(conn: &Transaction) -> Result<(), SqliteError> {
    // 1. Get the ID for the 'X' tag
    let tag_id_res: Result<SqliteId, SqliteError> = conn.query_row(
        "SELECT id FROM dict_tag WHERE ascii_symbol = 'X'",
        [],
        |row| row.get(0),
    );

    let tag_id = match tag_id_res {
        Ok(id) => id,
        Err(SqliteError::QueryReturnedNoRows) => return Ok(()), // Tag 'X' doesn't exist, nothing to delete
        Err(e) => return Err(e),
    };

    // 2. Identify references marked with 'X'
    // We select details needed to find the symmetric inverse if necessary
    let mut stmt_candidates = conn.prepare(
        r"
        SELECT
            r.shared_id,
            r.ascii_symbol,
            r.word_id_src,
            r.definition_id_src,
            r.word_id_dst,
            r.definition_id_dst,
            rt.is_symmetric
        FROM dict_reference r
        JOIN dict_shared_tag st ON r.shared_id = st.for_shared_id
        JOIN dict_ref_type rt ON r.ascii_symbol = rt.ascii_symbol
        WHERE st.tag_id = ?1
        ",
    )?;

    let mut rows = stmt_candidates.query(params![tag_id])?;
    let mut shared_ids_to_delete = HashSet::new();
    
    // Helper list to process symmetric checks outside the borrow of stmt_candidates if strictly needed,
    // but here we can just collect everything first.
    let mut candidates = Vec::new();

    while let Some(row) = rows.next()? {
        let shared_id: SqliteId = row.get(0)?;
        let ascii_symbol: String = row.get(1)?;
        let w_src: SqliteId = row.get(2)?;
        let d_src: Option<SqliteId> = row.get(3)?;
        let w_dst: SqliteId = row.get(4)?;
        let d_dst: Option<SqliteId> = row.get(5)?;
        let is_symmetric: bool = row.get(6)?;

        candidates.push((shared_id, ascii_symbol, w_src, d_src, w_dst, d_dst, is_symmetric));
    }
    // Drop the statement to free the borrow on conn
    drop(rows);
    drop(stmt_candidates);

    // 3. Process candidates and find inverses
    let mut stmt_find_inverse = conn.prepare_cached(
        r"
        SELECT shared_id 
        FROM dict_reference 
        WHERE ascii_symbol = ?1
          AND word_id_src = ?2
          AND definition_id_src IS ?3
          AND word_id_dst = ?4
          AND definition_id_dst IS ?5
        ",
    )?;

    for (shared_id, ascii_symbol, w_src, d_src, w_dst, d_dst, is_symmetric) in candidates {
        shared_ids_to_delete.insert(shared_id);

        if is_symmetric {
            // Find reference where Src and Dst are swapped
            // Note: We map arguments: (ref_id, w_dst, d_dst, w_src, d_src)
            let inverse_rows = stmt_find_inverse.query_map(
                params![ascii_symbol, w_dst, d_dst, w_src, d_src],
                |row| row.get(0),
            )?;

            for id in inverse_rows {
                shared_ids_to_delete.insert(id?);
            }
        }
    }

    if shared_ids_to_delete.is_empty() {
        return Ok(());
    }

    // 4. Perform deletions
    // Note: Due to foreign key constraints (ON DELETE NO ACTION), we must delete children first.
    // Order: dict_shared_tag -> dict_reference -> dict_shared
    
    let mut stmt_del_tag = conn.prepare_cached("DELETE FROM dict_shared_tag WHERE for_shared_id = ?1")?;
    let mut stmt_del_ref = conn.prepare_cached("DELETE FROM dict_reference WHERE shared_id = ?1")?;
    let mut stmt_del_shared = conn.prepare_cached("DELETE FROM dict_shared WHERE id = ?1")?;

    for shared_id in shared_ids_to_delete {
        stmt_del_tag.execute(params![shared_id])?;
        stmt_del_ref.execute(params![shared_id])?;
        stmt_del_shared.execute(params![shared_id])?;
    }

    Ok(())
}

/// Normalizes the ranks in the DB.
/// Reads entries in dict_shared ordered by (rank, rank_relative),
/// then updates them to have a continuous rank sequence and rank_relative = NULL.
///
/// This effectively "bakes in" any relative ordering (insertions) into the main rank.
pub fn normalize_ranks(conn: &Transaction) -> Result<(), SqliteError> {
    // Collect all IDs first because we are modifying the columns used for sorting
    // in the SELECT statement (rank, rank_relative), which could otherwise affect cursor iteration.
    let mut stmt = conn.prepare(
        "SELECT id FROM dict_shared ORDER BY rank ASC, rank_relative ASC",
    )?;

    let ids: Vec<SqliteId> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut stmt_update = conn.prepare_cached(
        "UPDATE dict_shared SET rank = ?1, rank_relative = NULL WHERE id = ?2",
    )?;

    for (i, id) in ids.iter().enumerate() {
        // Start ranks at 1 to maintain a clean counter
        let new_rank = (i as i64) + 1;
        stmt_update.execute(params![new_rank, id])?;
    }

    Ok(())
}

/// Re-sorts all references in the database based on the reference type configuration and destination rank.
pub fn sort_references(conn: &Transaction) -> Result<(), SqliteError> {
    normalize_ranks(conn)?;
    
    // 1. Determine the maximum rank in the database (scaling factor)
    let rank_max: i64 = conn.query_row(
        "SELECT COALESCE(MAX(rank), 0) FROM dict_shared",
        [],
        |row| row.get(0),
    )?;

    // 2. Fetch all references with necessary data to calculate sorting
    // We join word/definition tables to get the ranks of the source and destination.
    // COALESCE is used to select the definition rank if it exists, otherwise the word rank.
    let mut stmt = conn.prepare_cached(
        r"
        SELECT
            r.shared_id,
            rt.ascii_symbol,
            COALESCE(s_src_def.rank, s_src_word.rank) as source_rank,
            COALESCE(s_dst_def.rank, s_dst_word.rank) as dest_rank,
            s_ref.rank as current_ref_rank
        FROM dict_reference r
        JOIN dict_shared s_ref ON r.shared_id = s_ref.id
        JOIN dict_ref_type rt ON r.ascii_symbol = rt.ascii_symbol
        -- Source Joins
        JOIN dict_word w_src ON r.word_id_src = w_src.id
        JOIN dict_shared s_src_word ON w_src.shared_id = s_src_word.id
        LEFT JOIN dict_definition d_src ON r.definition_id_src = d_src.id
        LEFT JOIN dict_shared s_src_def ON d_src.shared_id = s_src_def.id
        -- Destination Joins
        JOIN dict_word w_dst ON r.word_id_dst = w_dst.id
        JOIN dict_shared s_dst_word ON w_dst.shared_id = s_dst_word.id
        LEFT JOIN dict_definition d_dst ON r.definition_id_dst = d_dst.id
        LEFT JOIN dict_shared s_dst_def ON d_dst.shared_id = s_dst_def.id
        ",
    )?;

    // We store updates in a vector to execute them after iteration
    // Tuple: (shared_id, new_rank, new_rank_relative)
    let mut updates: Vec<(SqliteId, i64, i64)> = Vec::new();

    let rows = stmt.query_map([], |row| {
        let shared_id: SqliteId = row.get(0)?;
        let ascii_symbol: String = row.get(1)?;
        let source_rank: i64 = row.get(2)?;
        let dest_rank: i64 = row.get(3)?;
        let current_ref_rank: i64 = row.get(4)?;
        Ok((
            shared_id,
            ascii_symbol,
            source_rank,
            dest_rank,
            current_ref_rank,
        ))
    })?;

    for row in rows {
        let (shared_id, ascii_symbol, source_rank, dest_rank, current_ref_rank) = row?;

        // 3. Determine sorting parameters from config
        let symbol_char = ascii_symbol.chars().next().unwrap_or('?');
        let (sort_by_dest_rank, ref_relative_rank) =
            if let Some((_, _, sort_by_dest_rank, rank)) = config::get_ref_type(symbol_char) {
                (sort_by_dest_rank, i64::from(rank))
            } else {
                (false, 0)
            };

        // 4. Calculate new relative rank
        let sort_rank = if sort_by_dest_rank {
            dest_rank
        } else {
            current_ref_rank
        };

        let rank_relative = (rank_max * ref_relative_rank) + sort_rank;

        updates.push((shared_id, source_rank, rank_relative));
    }

    // 5. Apply updates
    let mut stmt_update =
        conn.prepare_cached("UPDATE dict_shared SET rank = ?1, rank_relative = ?2 WHERE id = ?3")?;

    for (shared_id, new_rank, new_rank_relative) in updates {
        stmt_update.execute(params![new_rank, new_rank_relative, shared_id])?;
    }

    Ok(())
}

/// Adds an ASCII tag 'g' to sentences considered "graded", and removes it from those that are not.
/// A sentence is "graded" if all its words (excluding punctuation and ASCII words) appear
/// before the frequency-sorted pivot ('%'), and are either within the first 500 words 
/// or appear before the word to which the sentence is attached.
pub fn apply_graded_tags_to_sentences(conn: &Transaction) -> Result<(), SqliteError> {
    // 1. Get or insert the tag ID for 'g' (graded-sentence)
    let tag_id = db_edit::get_or_insert_tag_id(conn, &crate::db_read::Tag::Ascii('g'))?;

    // 2. Query all words which have a definition with word class 'punctuation'
    let mut stmt_punct = conn.prepare_cached(
        r"
        SELECT DISTINCT w.id
        FROM dict_word w
        JOIN dict_definition d ON w.id = d.word_id
        JOIN dict_class c ON d.class_id = c.id
        WHERE c.name = 'punctuation'
        "
    )?;
    let punct_word_ids: HashSet<SqliteId> = stmt_punct
        .query_map([], |row| row.get(0))?
        .collect::<Result<HashSet<_>, _>>()?;

    // 3. Find the rank of the word entry for '%'
    let pivot_rank: i64 = conn.query_row(
        r"
        SELECT s.rank
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        WHERE w.trad = '%'
        LIMIT 1
        ",
        [],
        |row| row.get(0),
    ).unwrap_or(i64::MAX);

    // 4. Find the rank for the 500th word (representing the base vocabulary boundary)
    let rank_500: i64 = conn.query_row(
        r"
        SELECT s.rank
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        ORDER BY s.rank ASC
        LIMIT 1 OFFSET 499
        ",
        [],
        |row| row.get(0),
    ).unwrap_or(i64::MAX);

    // 5. Check all sentences 
    // Gather sentence metadata, its words, and their respective ranks
    let mut stmt_sentences = conn.prepare(
        r"
        SELECT
            s.id,
            s.shared_id,
            (SELECT 1 FROM dict_shared_tag st WHERE st.for_shared_id = s.shared_id AND st.tag_id = ?1) AS has_tag,
            w_for_s.rank AS for_rank,
            sw.word_id,
            w_sw_s.rank AS sw_rank
        FROM dict_sentence s
        JOIN dict_word w_for ON s.for_word_id = w_for.id
        JOIN dict_shared w_for_s ON w_for.shared_id = w_for_s.id
        JOIN dict_sentence_word sw ON s.id = sw.sentence_id
        LEFT JOIN dict_word w_sw ON sw.word_id = w_sw.id
        LEFT JOIN dict_shared w_sw_s ON w_sw.shared_id = w_sw_s.id
        ORDER BY s.id
        "
    )?;

    let mut current_sentence_id: Option<SqliteId> = None;
    let mut current_shared_id: SqliteId = 0;
    let mut has_tag = false;
    let mut is_graded = true;

    // Buffers for database update
    let mut tags_to_add: Vec<SqliteId> = Vec::new();
    let mut tags_to_remove: Vec<SqliteId> = Vec::new();

    let mut rows = stmt_sentences.query(params![tag_id])?;

    while let Some(row) = rows.next()? {
        let s_id: SqliteId = row.get(0)?;
        let shared_id: SqliteId = row.get(1)?;
        let tag_present: Option<i32> = row.get(2)?;
        let for_rank: i64 = row.get(3)?;
        let word_id: Option<SqliteId> = row.get(4)?;
        let sw_rank: Option<i64> = row.get(5)?;

        // If we switch to a new sentence, evaluate and flush the status of the previous one
        if current_sentence_id != Some(s_id) {
            if current_sentence_id.is_some() {
                if is_graded && !has_tag {
                    tags_to_add.push(current_shared_id);
                } else if !is_graded && has_tag {
                    tags_to_remove.push(current_shared_id);
                }
            }

            current_sentence_id = Some(s_id);
            current_shared_id = shared_id;
            has_tag = tag_present.is_some();
            is_graded = true;
        }

        // If sentence was already disqualified, skip checking the remaining words in it
        if is_graded {
            if let Some(w_id) = word_id {
                // Exclude punctuation words
                if punct_word_ids.contains(&w_id) {
                    continue;
                }

                if let Some(r) = sw_rank {
                    // Check conditions
                    if r > pivot_rank {
                        is_graded = false; // Disqualified: Extends past the pivot word ('%')
                    } else if r > rank_500 && r > for_rank {
                        is_graded = false; // Disqualified: Outside base vocabulary AND outside attached word's boundary
                    }
                } else {
                    is_graded = false; // Graceful fallback if rank somehow wasn't retrieved
                }
            }
        }
    }

    // Capture the state for the very last sentence
    if current_sentence_id.is_some() {
        if is_graded && !has_tag {
            tags_to_add.push(current_shared_id);
        } else if !is_graded && has_tag {
            tags_to_remove.push(current_shared_id);
        }
    }

    // Drop the SELECT statement early to free up `conn` borrows
    //drop(rows);
    //drop(stmt_sentences);

    // 6. Apply all tag adjustments batched out
    let mut stmt_add = conn.prepare_cached("INSERT OR IGNORE INTO dict_shared_tag (for_shared_id, tag_id) VALUES (?1, ?2)")?;
    for shared_id in tags_to_add {
        stmt_add.execute(params![shared_id, tag_id])?;
    }

    let mut stmt_remove = conn.prepare_cached("DELETE FROM dict_shared_tag WHERE for_shared_id = ?1 AND tag_id = ?2")?;
    for shared_id in tags_to_remove {
        stmt_remove.execute(params![shared_id, tag_id])?;
    }

    Ok(())
}


/// Sorts example sentences for a definition by vocabulary frequency. 
pub fn sort_sentences(conn: &Transaction) -> Result<(), SqliteError> {
    // 1. Get word IDs that belong to the 'punctuation' class to exclude them
    let mut stmt_punct = conn.prepare_cached(
        r"
        SELECT DISTINCT w.id
        FROM dict_word w
        JOIN dict_definition d ON w.id = d.word_id
        JOIN dict_class c ON d.class_id = c.id
        WHERE c.name = 'punctuation'
        "
    )?;
    let punct_word_ids: HashSet<SqliteId> = stmt_punct
        .query_map([], |row| row.get(0))?
        .collect::<Result<HashSet<_>, _>>()?;

    // 2. Find the rank of the pivot word ('%')
    let pivot_rank: i64 = conn.query_row(
        r"
        SELECT s.rank
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        WHERE w.trad = '%'
        LIMIT 1
        ",
        [],
        |row| row.get(0),
    ).unwrap_or(i64::MAX);

    // 3. Query all sentences, their definitions' ranks, their original ranks, and their words' ranks
    let mut stmt_sentences = conn.prepare(
        r"
        SELECT
            s.id,
            s.shared_id,
            def_s.rank AS def_rank,
            s_s.rank AS original_sen_rank,
            sw.word_id,
            w_s.rank AS sw_rank
        FROM dict_sentence s
        JOIN dict_shared s_s ON s.shared_id = s_s.id
        JOIN dict_definition def ON s.for_definition_id = def.id
        JOIN dict_shared def_s ON def.shared_id = def_s.id
        LEFT JOIN dict_sentence_word sw ON s.id = sw.sentence_id
        LEFT JOIN dict_word w ON sw.word_id = w.id
        LEFT JOIN dict_shared w_s ON w.shared_id = w_s.id
        ORDER BY s.id
        "
    )?;

    let mut current_sentence_id: Option<SqliteId> = None;
    let mut current_shared_id: SqliteId = 0;
    let mut current_def_rank: i64 = 0;
    let mut current_original_sen_rank: i64 = 0;
    let mut count_after: i64 = 0;
    let mut highest_before: i64 = 0;

    let mut updates: Vec<(SqliteId, i64, i64)> = Vec::new();

    let mut rows = stmt_sentences.query([])?;

    while let Some(row) = rows.next()? {
        let s_id: SqliteId = row.get(0)?;
        let shared_id: SqliteId = row.get(1)?;
        let def_rank: i64 = row.get(2)?;
        let original_sen_rank: i64 = row.get(3)?;
        let word_id: Option<SqliteId> = row.get(4)?;
        let sw_rank: Option<i64> = row.get(5)?;

        // Transition to a new sentence
        if current_sentence_id != Some(s_id) {
            // Compute and store rank data for the completed sentence
            if current_sentence_id.is_some() {
                let rank_rel = (pivot_rank * count_after) 
                    + highest_before 
                    + (current_original_sen_rank - current_def_rank);
                
                updates.push((current_shared_id, current_def_rank, rank_rel));
            }

            current_sentence_id = Some(s_id);
            current_shared_id = shared_id;
            current_def_rank = def_rank;
            current_original_sen_rank = original_sen_rank;
            count_after = 0;
            highest_before = 0;
        }

        // Process the sentence word (if it exists)
        if let Some(w_id) = word_id {
            if !punct_word_ids.contains(&w_id) {
                if let Some(r) = sw_rank {
                    if r > pivot_rank {
                        count_after += 1;
                    } else if r > highest_before {
                        highest_before = r;
                    }
                }
            }
        }
    }

    // Capture the calculation for the very last sentence
    if current_sentence_id.is_some() {
        let rank_rel = (pivot_rank * count_after) 
            + highest_before 
            + (current_original_sen_rank - current_def_rank);
        
        updates.push((current_shared_id, current_def_rank, rank_rel));
    }

    // 4. Update the DB with the new ranks and relative ranks
    let mut stmt_update = conn.prepare_cached(
        "UPDATE dict_shared SET rank = ?1, rank_relative = ?2 WHERE id = ?3"
    )?;

    for (shared_id, new_rank, new_rank_relative) in updates {
        stmt_update.execute(params![new_rank, new_rank_relative, shared_id])?;
    }

    Ok(())
}