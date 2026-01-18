use crate::common::SqliteId;
use crate::config;
use crate::db_read::Tag;
use rusqlite::{Error as SqliteError, Transaction, params};

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

/// Insert a new reference for the specified word or definition and resort all existing references by type and rank of destination
pub fn insert_reference(
    conn: &Transaction,
    ref_type_id: SqliteId,
    src_word_id: SqliteId,
    src_def_ext_id: Option<usize>,
    dst_word_id: SqliteId,
    dst_def_ext_id: Option<usize>,
) -> Result<(SqliteId, SqliteId), SqliteError> {
    // Helper to resolve external definition ID to internal ID
    let resolve_def_id =
        |word_id: SqliteId, ext_id: Option<usize>| -> Result<Option<SqliteId>, SqliteError> {
            if let Some(ext) = ext_id {
                let mut stmt = conn.prepare_cached(
                    "SELECT id FROM dict_definition WHERE word_id = ?1 AND ext_def_id = ?2",
                )?;
                match stmt.query_row(params![word_id, ext as i64], |row| row.get(0)) {
                    Ok(id) => Ok(Some(id)),
                    Err(SqliteError::QueryReturnedNoRows) => Err(SqliteError::QueryReturnedNoRows),
                    Err(e) => Err(e),
                }
            } else {
                Ok(None)
            }
        };

    let src_def_id = resolve_def_id(src_word_id, src_def_ext_id)?;
    let dst_def_id = resolve_def_id(dst_word_id, dst_def_ext_id)?;

    // Determine the maximum rank in the database
    let rank_max: i64 = conn.query_row(
        "SELECT COALESCE(MAX(rank), 0) FROM dict_shared",
        [],
        |row| row.get(0),
    )?;

    // Determine the rank of the source (word or definition)
    let source_rank: i64 = if let Some(def_id) = src_def_id {
        let mut stmt = conn.prepare_cached(
            "SELECT s.rank FROM dict_definition d JOIN dict_shared s ON d.shared_id = s.id WHERE d.id = ?1",
        )?;
        stmt.query_row(params![def_id], |row| row.get(0))?
    } else {
        let mut stmt = conn.prepare_cached(
            "SELECT s.rank FROM dict_word w JOIN dict_shared s ON w.shared_id = s.id WHERE w.id = ?1",
        )?;
        stmt.query_row(params![src_word_id], |row| row.get(0))?
    };

    // Insert new entry into dict_shared
    // We insert with temporary rank_relative=0, it will be updated in the reordering step
    let mut stmt_insert_shared =
        conn.prepare_cached("INSERT INTO dict_shared (rank, rank_relative) VALUES (?1, ?2)")?;
    stmt_insert_shared.execute(params![source_rank, 0])?;
    let new_shared_id = conn.last_insert_rowid();

    // Insert new entry into dict_reference
    let mut stmt_insert_ref = conn.prepare_cached(
        r"
        INSERT INTO dict_reference (shared_id, ref_type_id, word_id_src, definition_id_src, word_id_dst, definition_id_dst)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6);
        ",
    )?;
    stmt_insert_ref.execute(params![
        new_shared_id,
        ref_type_id,
        src_word_id,
        src_def_id,
        dst_word_id,
        dst_def_id,
    ])?;
    let new_ref_id = conn.last_insert_rowid();

    // Find all existing references with the same source word OR definition (including the new one)
    // and determine their new relative ranks.
    let (sql, params) = if let Some(def_id) = src_def_id {
        (
            r"
            SELECT
                r.shared_id,
                rt.ascii_symbol,
                COALESCE(s_def.rank, s_word.rank) as dest_rank
            FROM dict_reference r
            JOIN dict_ref_type rt ON r.ref_type_id = rt.id
            JOIN dict_word w_dst ON r.word_id_dst = w_dst.id
            JOIN dict_shared s_word ON w_dst.shared_id = s_word.id
            LEFT JOIN dict_definition d_dst ON r.definition_id_dst = d_dst.id
            LEFT JOIN dict_shared s_def ON d_dst.shared_id = s_def.id
            WHERE r.word_id_src = ?1 AND r.definition_id_src = ?2
            ",
            vec![src_word_id, def_id],
        )
    } else {
        (
            r"
            SELECT
                r.shared_id,
                rt.ascii_symbol,
                COALESCE(s_def.rank, s_word.rank) as dest_rank
            FROM dict_reference r
            JOIN dict_ref_type rt ON r.ref_type_id = rt.id
            JOIN dict_word w_dst ON r.word_id_dst = w_dst.id
            JOIN dict_shared s_word ON w_dst.shared_id = s_word.id
            LEFT JOIN dict_definition d_dst ON r.definition_id_dst = d_dst.id
            LEFT JOIN dict_shared s_def ON d_dst.shared_id = s_def.id
            WHERE r.word_id_src = ?1 AND r.definition_id_src IS NULL
            ",
            vec![src_word_id],
        )
    };

    let mut stmt_fetch = conn.prepare_cached(sql)?;
    let params_refs: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();

    let mut updates = Vec::new();
    let rows = stmt_fetch.query_map(&*params_refs, |row| {
        let shared_id: SqliteId = row.get(0)?;
        let ascii_symbol: String = row.get(1)?;
        let dest_rank: i64 = row.get(2)?;
        Ok((shared_id, ascii_symbol, dest_rank))
    })?;

    for row in rows {
        let (shared_id, ascii_symbol, dest_rank) = row?;

        // Determine REF_RANK
        let symbol_char = ascii_symbol.chars().next().unwrap_or('?');
        let ref_rank = if let Some((_, _, rank)) = config::get_ref_type(symbol_char) {
            rank as i64
        } else {
            0
        };

        // Calculate rank_relative: (RANK_MAX * REF_RANK) + RANK_DEST
        let rank_relative = (rank_max * ref_rank) + dest_rank;

        updates.push((shared_id, rank_relative));
    }

    // Apply updates to dict_shared
    let mut stmt_update =
        conn.prepare_cached("UPDATE dict_shared SET rank = ?1, rank_relative = ?2 WHERE id = ?3")?;

    for (shared_id, rank_relative) in updates {
        stmt_update.execute(params![source_rank, rank_relative, shared_id])?;
    }

    Ok((new_ref_id, new_shared_id))
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
pub fn get_or_insert_tag_id(conn: &Transaction, tag: &Tag) -> Result<SqliteId, SqliteError> {
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
pub fn update_definition_text(
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
