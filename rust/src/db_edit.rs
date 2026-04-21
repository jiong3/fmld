use crate::common::SqliteId;
use crate::config;
use crate::db_read::Tag;
pub use crate::db_read::EntryId;
use crate::db_read;
use rusqlite::{Error as SqliteError, Transaction, params};

/// Insert a new reference for the specified word or definition
/// Return (reference id, shared id, is newly added)
#[allow(clippy::too_many_arguments)]
pub fn insert_reference(
    conn: &Transaction,
    ascii_symbol: char,
    src_word_id: SqliteId,
    src_def_id: Option<SqliteId>,
    dst_word_id: SqliteId,
    dst_def_id: Option<SqliteId>,
    rank_relative: Option<usize>,
) -> Result<(SqliteId, SqliteId, bool), SqliteError> {
    // Check if the reference already exists to avoid duplicates (also ensured by partial unique index)
    if ascii_symbol != '>'
    {
        let mut stmt_check = conn.prepare_cached(
            r"
            SELECT id, shared_id 
            FROM dict_reference 
            WHERE ascii_symbol = ?1 
              AND word_id_src = ?2 
              AND definition_id_src IS ?3 
              AND word_id_dst = ?4 
              AND definition_id_dst IS ?5
            ",
        )?;

        // SQLite's IS operator matches NULLs if the parameter is NULL (None)
        let result = stmt_check.query_row(
            params![ascii_symbol.to_string(), src_word_id, src_def_id, dst_word_id, dst_def_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        match result {
            Ok((id, shared_id)) => return Ok((id, shared_id, false)),
            Err(SqliteError::QueryReturnedNoRows) => { /* continue to insert */ }
            Err(e) => return Err(e),
        }
    }

    // insert new reference after existing ones with the same type (depending on the type they will be resorted by the autofix function)
    let rank_to_insert_at: i64 = if let Some(def_id) = src_def_id {
        let mut stmt = conn.prepare_cached(
            r"
            SELECT COALESCE(
                (SELECT MAX(s.rank) FROM dict_reference r JOIN dict_shared s ON r.shared_id = s.id WHERE r.word_id_src = ?1 AND r.definition_id_src = ?2 AND r.ascii_symbol = ?3),
                (SELECT MAX(s.rank) FROM dict_reference r JOIN dict_shared s ON r.shared_id = s.id WHERE r.word_id_src = ?1 AND r.definition_id_src = ?2),
                (SELECT s.rank FROM dict_definition d JOIN dict_shared s ON d.shared_id = s.id WHERE d.id = ?2)
            )
            ",
        )?;
        stmt.query_row(params![src_word_id, def_id, ascii_symbol.to_string()], |row| row.get(0))?
    } else {
        let mut stmt = conn.prepare_cached(
            r"
            SELECT COALESCE(
                (SELECT MAX(s.rank) FROM dict_reference r JOIN dict_shared s ON r.shared_id = s.id WHERE r.word_id_src = ?1 AND r.definition_id_src IS NULL AND r.ascii_symbol = ?2),
                (SELECT MAX(s.rank) FROM dict_reference r JOIN dict_shared s ON r.shared_id = s.id WHERE r.word_id_src = ?1 AND r.definition_id_src IS NULL),
                (SELECT s.rank FROM dict_word w JOIN dict_shared s ON w.shared_id = s.id WHERE w.id = ?1)
            )
            ",
        )?;
        stmt.query_row(params![src_word_id, ascii_symbol.to_string()], |row| row.get(0))?
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
        INSERT INTO dict_reference (shared_id, ascii_symbol, word_id_src, definition_id_src, word_id_dst, definition_id_dst)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6);
        ",
    )?;
    stmt_insert_ref.execute(params![
        shared_id,
        ascii_symbol.to_string(),
        src_word_id,
        src_def_id,
        dst_word_id,
        dst_def_id,
    ])?;

    let new_ref_id = conn.last_insert_rowid();
    Ok((new_ref_id, shared_id, true))
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
    let shared_id = db_read::get_shared_id(conn, &target)?;
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
