use crate::common::SqliteId;
use crate::db_read::Tag;
use crate::config;
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


