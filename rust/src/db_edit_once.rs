// Functions which may be used occasionally to edit data, e.g. import, cleanup, etc.
#![allow(clippy::all)]

use crate::common::SqliteId;
use crate::config;
use crate::db_edit;
use crate::db_read;
use crate::pinyin;
use rusqlite::{Error as SqliteError, Transaction, params};
use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader};

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
            db_edit::add_tag(
                conn,
                db_edit::EntryId::Definition(definition.id),
                db_read::Tag::Full {
                    name: full_tag.to_owned(),
                    category: "".to_owned(),
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
                db_edit::add_tag(
                    conn,
                    db_edit::EntryId::Definition(definition.id),
                    db_read::Tag::Ascii(ascii_char),
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
            db_edit::update_definition_text(conn, definition.id, &new_definition_text.trim())
                .unwrap();
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

pub fn apply_definition_tags(
    conn: &Transaction,
    json_path: &str,
    tags: Vec<db_read::Tag>,
) -> Result<(), Box<dyn Error>> {
    let file = File::open(json_path)?;
    let reader = BufReader::new(file);
    let entries: Vec<(String, String, usize)> = serde_json::from_reader(reader)?;

    for (trad, simp, ext_def_id) in entries {
        // Resolve Word ID
        if let Some(word_id) = db_edit::get_word_id(conn, &trad, &simp)? {
            // Resolve Definition ID
            if let Some(def_id) = db_edit::get_definition_id_for_ext_id(conn, word_id, ext_def_id)?
            {
                // Add Tags
                for tag in &tags {
                    // Clone tag manually to avoid assuming Clone trait is derived/public
                    let tag_clone = match tag {
                        db_read::Tag::Ascii(c) => db_read::Tag::Ascii(*c),
                        db_read::Tag::Full { name, category } => db_read::Tag::Full {
                            name: name.clone(),
                            category: category.clone(),
                        },
                    };
                    db_edit::add_tag(conn, db_edit::EntryId::Definition(def_id), tag_clone)?;
                }
            } else {
                eprintln!(
                    "Warning: Definition not found for {} {} #{}",
                    trad, simp, ext_def_id
                );
            }
        } else {
            eprintln!("Warning: Word not found for {} {}", trad, simp);
        }
    }

    Ok(())
}

pub fn apply_pinyin_tags_from_json(
    conn: &Transaction,
    json_path: &str,
) -> Result<(), Box<dyn Error>> {
    // 1. Read and parse the JSON file into a HashMap.
    let file = File::open(json_path)?;
    let reader = BufReader::new(file);
    let pinyin_tags_map: HashMap<String, HashMap<String, String>> =
        serde_json::from_reader(reader)?;

    // 2. Prepare a statement to retrieve all unique word-pronunciation pairs.
    let mut stmt = conn.prepare(
        r"
        SELECT DISTINCT
            w.trad,
            p.pinyin_num,
            sp.id AS shared_pron_id
        FROM dict_word w
        JOIN dict_definition d ON w.id = d.word_id
        JOIN dict_pron_definition pdp ON d.id = pdp.definition_id
        JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
        JOIN dict_pron p ON sp.pron_id = p.id
        ",
    )?;

    // 3. Iterate over all pronunciations found in the database.
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let trad: String = row.get("trad")?;
        let pinyin_num: String = row.get("pinyin_num")?;
        let shared_pron_id: SqliteId = row.get("shared_pron_id")?;
        let pinyin_num_norm = pinyin::pinyin_num_normalized(&pinyin_num);

        // 4. Look for a matching word in the parsed JSON data.
        if let Some(pinyin_map) = pinyin_tags_map.get(&trad) {
            // 5. If the word matches, look for a matching pinyin.
            if let Some(tags) = pinyin_map.get(&pinyin_num_norm) {
                // 6. If both match, apply each character in the tag string.
                for tag_char in tags.chars() {
                    if config::tag_to_txt_ascii_common(tag_char).is_some() {
                        db_edit::add_tag(
                            conn,
                            db_edit::EntryId::Pinyin(shared_pron_id),
                            db_read::Tag::Ascii(tag_char),
                        )?;
                    } else {
                        eprintln!(
                            "Warning: Unknown tag character '{}' for word '{}', pinyin '{}'",
                            tag_char, trad, pinyin_num_norm
                        );
                    }
                }
            } else {
                eprintln!("No match for {trad}: {pinyin_num_norm}");
            }
        }
    }

    Ok(())
}

/// Sets the class of all definitions starting with "Classifier for" to the entry in dict_class
/// with the name "classifier".
///
/// Any definition entry that was changed is then set as the last definition for that word
/// among those with the same pronunciation(s), using rank_relative.
/// Assumes initial rank_relative are NULL.
pub fn add_classifier_class(conn: &Transaction) -> Result<(), SqliteError> {
    // 1. Get the target class ID for "classifier"
    let classifier_class_id: SqliteId = conn.query_row(
        "SELECT id FROM dict_class WHERE name = 'classifier'",
        [],
        |row| row.get(0),
    )?;

    // 2. Find all definitions that match the criteria but have the wrong class
    let mut stmt_candidates = conn.prepare(
        r#"
        SELECT id, word_id, shared_id
        FROM dict_definition
        WHERE definition LIKE 'Classifier for%'
          AND class_id <> ?1
        "#,
    )?;

    // Collect updates to avoid borrowing issues
    let candidates = stmt_candidates
        .query_map(params![classifier_class_id], |row| {
            Ok((
                row.get::<_, SqliteId>(0)?, // definition id
                row.get::<_, SqliteId>(1)?, // word id
                row.get::<_, SqliteId>(2)?, // shared id
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut update_class_stmt =
        conn.prepare("UPDATE dict_definition SET class_id = ?1 WHERE id = ?2")?;

    let mut update_rank_stmt =
        conn.prepare("UPDATE dict_shared SET rank = ?1, rank_relative = ?2 WHERE id = ?3")?;

    // Helper statement to get all definitions and their pronunciation signatures for a word.
    // We join to get the rank of the pronunciation to ensure the signature vector is ordered correctly.
    let mut stmt_word_defs = conn.prepare(
        r#"
        SELECT 
            d.id, 
            s.rank, 
            s.rank_relative, 
            pd.shared_pron_id
        FROM dict_definition d
        JOIN dict_shared s ON d.shared_id = s.id
        LEFT JOIN dict_pron_definition pd ON d.id = pd.definition_id
        LEFT JOIN dict_shared_pron sp ON pd.shared_pron_id = sp.id
        LEFT JOIN dict_shared sp_s ON sp.shared_id = sp_s.id
        WHERE d.word_id = ?1
        ORDER BY d.id, sp_s.rank, sp_s.rank_relative
        "#,
    )?;

    for (def_id, word_id, shared_id) in candidates {
        // 1. Update the class
        update_class_stmt.execute(params![classifier_class_id, def_id])?;

        // 2. Find the correct rank to move to
        // Fetch all definitions for this word to group them by pronunciation
        let rows = stmt_word_defs.query_map(params![word_id], |row| {
            Ok((
                row.get::<_, SqliteId>(0)?,    // def_id
                row.get::<_, i64>(1)?,         // rank
                row.get::<_, Option<i64>>(2)?, // rank_relative
                row.get::<_, Option<i64>>(3)?, // shared_pron_id
            ))
        })?;

        struct DefGroup {
            rank: i64,
            rank_relative: Option<i64>,
            prons: Vec<i64>,
        }

        let mut groups: HashMap<SqliteId, DefGroup> = HashMap::new();

        for row in rows {
            let (did, rank, rank_rel, pid) = row?;
            groups
                .entry(did)
                .and_modify(|g| {
                    if let Some(p) = pid {
                        g.prons.push(p);
                    }
                })
                .or_insert_with(|| {
                    let mut prons = Vec::new();
                    if let Some(p) = pid {
                        prons.push(p);
                    }
                    DefGroup {
                        rank,
                        rank_relative: rank_rel,
                        prons,
                    }
                });
        }

        // Identify the pronunciation signature of the target definition
        let target_prons = if let Some(g) = groups.get(&def_id) {
            g.prons.clone()
        } else {
            continue;
        };

        // Find the highest rank/rank_relative among definitions with the same pronunciation signature
        let mut max_rank = i64::MIN;
        let mut max_rel: Option<i64> = None;
        let mut found_group = false;

        for g in groups.values() {
            if g.prons == target_prons {
                // Update max if this group is 'larger'
                let is_larger = if !found_group {
                    true
                } else if g.rank > max_rank {
                    true
                } else if g.rank == max_rank {
                    match (g.rank_relative, max_rel) {
                        (Some(r), Some(m)) => r > m,
                        (Some(_), None) => true,
                        (None, _) => false,
                    }
                } else {
                    false
                };

                if is_larger {
                    max_rank = g.rank;
                    max_rel = g.rank_relative;
                    found_group = true;
                }
            }
        }

        if found_group {
            // Calculate new rank_relative (append to the end)
            let new_rel = max_rel.unwrap_or(0) + 1;
            update_rank_stmt.execute(params![max_rank, new_rel, shared_id])?;
        }
    }

    Ok(())
}

pub fn add_references_from_json(
    conn: &Transaction,
    json_path: &str,
    ref_type: char,
    add_dst_to_src: bool,
    with_ascii_tags: Vec<char>,
) -> Result<(), Box<dyn Error>> {
    // Determine id of reference type
    let ref_type_id = db_edit::get_ref_type_id(conn, ref_type)?
        .ok_or_else(|| format!("Reference type '{}' not found", ref_type))?;

    // Get tag ids for all ascii tags
    let mut tag_ids = Vec::new();
    for c in with_ascii_tags {
        let tag_id = db_edit::get_or_insert_tag_id(conn, &db_read::Tag::Ascii(c))?;
        tag_ids.push(tag_id);
    }

    // Read JSON file
    let file = File::open(json_path)?;
    let reader = BufReader::new(file);
    // Parse list of tuples: (src_trad, src_simp, src_ext_def_id, dst_trad, dst_simp, dst_ext_def_id)
    let entries: Vec<(String, String, Option<usize>, String, String, Option<usize>)> =
        serde_json::from_reader(reader)?;

    // Prepared statement for adding tags to the shared entries
    let mut stmt_add_tag = conn.prepare_cached(
        "INSERT OR IGNORE INTO dict_shared_tag (for_shared_id, tag_id) VALUES (?1, ?2)",
    )?;

    let mut rank_relative = 0;
    for (src_trad, src_simp, src_def_ext, dst_trad, dst_simp, dst_def_ext) in entries {
        // Resolve word IDs
        let src_word_id = db_edit::get_word_id(conn, &src_trad, &src_simp)?;
        let dst_word_id = db_edit::get_word_id(conn, &dst_trad, &dst_simp)?;

        rank_relative += 1;

        // Only proceed if both words exist in the dictionary
        if let (Some(s_id), Some(d_id)) = (src_word_id, dst_word_id) {
            let src_def_id = match src_def_ext {
                Some(s_def_ext) => db_edit::get_definition_id_for_ext_id(conn, s_id, s_def_ext)?,
                None => None,
            };
            let dst_def_id = match dst_def_ext {
                Some(d_def_ext) => db_edit::get_definition_id_for_ext_id(conn, d_id, d_def_ext)?,
                None => None,
            };
            // Forward direction: Source -> Destination
            let Ok((_, shared_id_fwd, newly_added)) = db_edit::insert_reference(
                conn,
                ref_type_id,
                s_id,
                src_def_id,
                d_id,
                dst_def_id,
                Some(rank_relative),
            ) else {
                continue; // skip entries which e.g. are duplicated
            };

            if newly_added {
                // Add tags to the new reference
                for &tag_id in &tag_ids {
                    stmt_add_tag.execute(params![shared_id_fwd, tag_id])?;
                }
            }

            // Inverse direction: Destination -> Source (if requested)
            if add_dst_to_src && newly_added {
                let Ok((_, shared_id_inv, newly_added)) = db_edit::insert_reference(
                    conn,
                    ref_type_id,
                    d_id,
                    dst_def_id,
                    s_id,
                    src_def_id,
                    Some(rank_relative),
                ) else {
                    continue; // skip entries which e.g. are duplicated
                };
                if newly_added {
                    for &tag_id in &tag_ids {
                        stmt_add_tag.execute(params![shared_id_inv, tag_id])?;
                    }
                }
            }
        }
    }

    Ok(())
}

pub fn import_entries_from_json(
    conn: &Transaction,
    path: &str,
    from_idx: usize,
    to_idx: usize, // [from_idx..to_idx]
) -> Result<(), Box<dyn Error>> {
    /*
    json format, one json object with each entry looking like this:
    "光是": {
            "after_simp": "", (if not empty, add the new entry after this one)
            "after_trad": "說了算",  (if not empty, add the new entry after this one, has priority over afer_simp)
            "source": "MDBG",
            "word_tags": "m", (add letters as ascii tags to the word)
            "simp": "光是", (simp in dict_word)
            "trad": "光是", (trad in dict_word)
            "defs": { (this object contains the pronunciation in pinyin as key and the list of definitions to be added as values)
            "guang1shi4": [
                "solely",
                "just"
        ]
        },
        "pinyins": [
            "guang1shi4" (use this as fallback if the pinyin of a definition is an empty string, which is true for some words)
        ],
        "comment": [ (add all lines as a single comment to the word, merge entries with new-lines)
        ]
    },
    */
    #[derive(Deserialize)]
    struct ImportEntry {
        after_simp: String,
        after_trad: String,
        #[serde(default)]
        word_tags: String,
        simp: String,
        trad: String,
        defs: HashMap<String, Vec<String>>,
        pinyins: Vec<String>,
        comment: Vec<String>,
    }

    // 1. Parse JSON into a HashMap
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let entries: HashMap<String, ImportEntry> = serde_json::from_reader(reader)?;

    // 2. Sort keys to ensure deterministic order and slice the range
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();

    // Handle out of bounds indices gracefully
    let start = std::cmp::min(from_idx, keys.len());
    let end = std::cmp::min(to_idx, keys.len());

    if start >= end {
        return Ok(());
    }

    // 3. Prepare DB statements
    let class_word: SqliteId =
        conn.query_row("SELECT id FROM dict_class WHERE name = 'word'", [], |row| {
            row.get(0)
        })?;
    let class_char: SqliteId = conn.query_row(
        "SELECT id FROM dict_class WHERE name = 'character'",
        [],
        |row| row.get(0),
    )?;

    // Check if word exists
    let mut stmt_check_exists =
        conn.prepare("SELECT 1 FROM dict_word WHERE trad = ?1 AND simp = ?2")?;

    // Find anchor rank
    let mut stmt_find_anchor = conn.prepare(
        r#"
        SELECT s.rank
        FROM dict_word w
        JOIN dict_shared s ON w.shared_id = s.id
        WHERE w.trad = ?1 OR (w.simp = ?2 AND ?2 <> '')
        ORDER BY CASE WHEN w.trad = ?1 THEN 0 ELSE 1 END
        LIMIT 1
        "#,
    )?;

    let mut stmt_get_max_rel =
        conn.prepare("SELECT MAX(rank_relative) FROM dict_shared WHERE rank = ?1")?;

    let mut stmt_get_max_rank = conn.prepare("SELECT MAX(rank) FROM dict_shared")?;

    let mut stmt_insert_comment = conn.prepare("INSERT INTO dict_comment (comment) VALUES (?1)")?;

    let mut stmt_insert_shared = conn
        .prepare("INSERT INTO dict_shared (rank, rank_relative, comment_id) VALUES (?1, ?2, ?3)")?;

    let mut stmt_insert_word =
        conn.prepare("INSERT INTO dict_word (shared_id, trad, simp) VALUES (?1, ?2, ?3)")?;

    let mut stmt_insert_def = conn.prepare(
        r#"
        INSERT INTO dict_definition (shared_id, word_id, definition, ext_def_id, class_id)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
    )?;

    let mut stmt_find_pron = conn.prepare("SELECT id FROM dict_pron WHERE pinyin_num = ?1")?;
    let mut stmt_insert_pron =
        conn.prepare("INSERT INTO dict_pron (pinyin_num, pinyin_mark) VALUES (?1, ?2)")?;

    let mut stmt_insert_shared_pron =
        conn.prepare("INSERT INTO dict_shared_pron (shared_id, pron_id) VALUES (?1, ?2)")?;

    let mut stmt_insert_pron_def = conn.prepare(
        "INSERT INTO dict_pron_definition (shared_pron_id, definition_id) VALUES (?1, ?2)",
    )?;

    let mut stmt_remove_prefixed_comments = conn.prepare(
        "UPDATE dict_shared
            SET comment_id = NULL
            FROM dict_comment
            WHERE dict_shared.comment_id = dict_comment.id
            AND dict_comment.comment LIKE ?1;",
    )?;

    // 4. Iterate through the selected range
    for key in &keys[start..end] {
        let entry = &entries[*key];

        // Check if the word is already in the database
        if stmt_check_exists.exists(params![entry.trad, entry.simp])? {
            continue;
        }

        // --- Determine Rank ---
        let (rank, mut rank_relative) = {
            let anchor_trad = &entry.after_trad;
            let anchor_simp = &entry.after_simp;

            let target_rank: Option<i64> = if !anchor_trad.is_empty() {
                stmt_find_anchor
                    .query_row(params![anchor_trad, ""], |row| row.get(0))
                    .ok()
            } else if !anchor_simp.is_empty() {
                stmt_find_anchor
                    .query_row(params!["", anchor_simp], |row| row.get(0))
                    .ok()
            } else {
                None
            };

            if let Some(r) = target_rank {
                // Found anchor. Insert after it (same rank, increment relative).
                // Target relative is NULL (0-ish logic), so we look for max relative currently at this rank.
                let max_rel: Option<i64> = stmt_get_max_rel
                    .query_row(params![r], |row| row.get(0))
                    .ok()
                    .flatten();
                let next_rel = max_rel.unwrap_or(0) + 1;
                (r, next_rel)
            } else {
                // Append to end
                let max_rank: i64 = stmt_get_max_rank
                    .query_row([], |row| row.get(0))
                    .unwrap_or(0);
                (max_rank + 1, 0)
            }
        };

        // --- Insert Comment ---
        let comment_id = if !entry.comment.is_empty() {
            let joined = entry.comment.join("\n");
            stmt_insert_comment.execute(params![joined])?;
            Some(conn.last_insert_rowid())
        } else {
            None
        };

        // --- Insert Shared ---
        stmt_insert_shared.execute(params![rank, rank_relative, comment_id])?;
        let shared_id = conn.last_insert_rowid();

        // --- Insert Word ---
        println!("Writing word {} / {}", entry.trad, entry.simp);
        stmt_insert_word.execute(params![shared_id, entry.trad, entry.simp])?;
        let word_id = conn.last_insert_rowid();

        // --- Add Tags ---
        for tag_char in entry.word_tags.chars() {
            if config::tag_to_txt_ascii_common(tag_char).is_some() {
                db_edit::add_tag(
                    conn,
                    db_edit::EntryId::Word(word_id),
                    db_read::Tag::Ascii(tag_char),
                )?;
            }
        }

        // --- Definitions ---
        let class_id = if entry.trad.chars().count() > 1 {
            class_word
        } else {
            class_char
        };

        let mut ext_def_id = 1;
        for (pinyin_key, def_list) in &entry.defs {
            // Resolve Pinyins for this definition group
            // If the key is empty, use the fallback list from entry.pinyins
            let effective_pinyins = if pinyin_key.is_empty() {
                &entry.pinyins
            } else {
                // Otherwise use the key as the single pinyin (wrapped in a slice/vec for iteration)
                // We create a temporary vec here to allow iteration
                // Note: constructing a temporary vec of references is tricky with mixed lifetimes,
                // so we just clone the string key into a vec for the loop.
                // It's not the most efficient but safe and simple.
                // However, entry.pinyins is Vec<String>.
                // We need to iterate over &String.
                // Let's just normalize logic inside the loop.
                &vec![pinyin_key.clone()] // Temporary vector
            };

            // Use fallback if the specific pinyin list is somehow empty (though logic above handles keys)
            // The JSON logic: "use [pinyins] as fallback if the pinyin of a definition is an empty string"
            let pinyins_to_iterate = if pinyin_key.is_empty() {
                &entry.pinyins
            } else {
                effective_pinyins
            };

            let mut shared_pron_ids = Vec::new();
            for p in pinyins_to_iterate {
                let p_norm = pinyin::pinyin_num_normalized(p);

                // Get or Insert Pron
                let pron_id: SqliteId =
                    if let Ok(id) = stmt_find_pron.query_row(params![p_norm], |row| row.get(0)) {
                        id
                    } else {
                        let mark = pinyin::pinyin_mark_from_num(&p_norm);
                        stmt_insert_pron.execute(params![p_norm, mark])?;
                        conn.last_insert_rowid()
                    };

                // Get or Insert Shared Pron (Word <-> Pron link)
                let sp_id: SqliteId = {
                    rank_relative += 1;
                    let no_comment: Option<SqliteId> = None;
                    stmt_insert_shared.execute(params![rank, rank_relative, no_comment])?;
                    let shared_id = conn.last_insert_rowid();
                    stmt_insert_shared_pron.execute(params![shared_id, pron_id])?;
                    conn.last_insert_rowid()
                };
                shared_pron_ids.push(sp_id);
            }

            // Insert Definitions
            for def_text in def_list {
                println!("  Def: {ext_def_id} {def_text}");
                rank_relative += 1;
                let no_comment: Option<SqliteId> = None;
                stmt_insert_shared.execute(params![rank, rank_relative, no_comment])?;
                let shared_id = conn.last_insert_rowid();
                stmt_insert_def
                    .execute(params![shared_id, word_id, def_text, ext_def_id, class_id])?;
                let def_id = conn.last_insert_rowid();

                // Link Definition to Pinyins
                for sp_id in &shared_pron_ids {
                    stmt_insert_pron_def.execute(params![sp_id, def_id])?;
                }

                ext_def_id += 1;
            }
        }
    }

    stmt_remove_prefixed_comments.execute(params!["TODOx"])?;

    Ok(())
}

pub fn fix_contains_references_from_part_of(conn: &Transaction) -> Result<usize, SqliteError> {
    // 1. Get all "is-part-of" references ('<') that specify a source definition.
    //    comp_word_id (def_id) IS PART OF whole_word_id
    let mut stmt_part_of = conn.prepare(
        r"
        SELECT
            r.word_id_src AS comp_word_id,
            r.definition_id_src AS comp_def_id,
            r.word_id_dst AS whole_word_id
        FROM dict_reference r
        JOIN dict_ref_type rt ON r.ref_type_id = rt.id
        WHERE rt.ascii_symbol = '<'
          AND r.definition_id_src IS NOT NULL
        ",
    )?;

    // Collect results first to release the borrow on conn needed for the update statement
    let references: Vec<(u32, u32, u32)> = stmt_part_of
        .query_map([], |row| {
            Ok((
                row.get("comp_word_id")?,
                row.get("comp_def_id")?,
                row.get("whole_word_id")?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // 2. Update matching "contains" references ('>').
    //    We look for: whole_word_id (generic) CONTAINS comp_word_id (generic)
    //    And update to: whole_word_id (generic) CONTAINS comp_word_id (def_id)
    let mut stmt_update = conn.prepare(
        r"
        UPDATE dict_reference
        SET definition_id_dst = ?1
        WHERE word_id_src = ?2
          AND word_id_dst = ?3
          AND ref_type_id = (SELECT id FROM dict_ref_type WHERE ascii_symbol = '>')
          AND definition_id_src IS NULL
        ",
    )?;

    let mut updated_count = 0;
    for (comp_word_id, comp_def_id, whole_word_id) in references {
        // params: [new_def_id, src_word (whole), dst_word (component)]
        updated_count += stmt_update.execute([comp_def_id, whole_word_id, comp_word_id])?;
    }

    Ok(updated_count)
}

// TODO map synonyms to cross-strait references if applicable:
/*
SELECT
  src_word.simp AS source_simp,
  src_word.trad AS source_trad,
  src_def.definition AS source_definition,
  dst_word.simp AS dest_simp,
  dst_word.trad AS dest_trad,
  dst_def.definition AS dest_definition
FROM dict_reference
JOIN dict_ref_type
  ON dict_reference.ref_type_id = dict_ref_type.id
-- Join Source Definition and Word
JOIN dict_definition AS src_def
  ON dict_reference.definition_id_src = src_def.id
JOIN dict_word AS src_word
  ON src_def.word_id = src_word.id
-- Join Destination Definition and Word
JOIN dict_definition AS dst_def
  ON dict_reference.definition_id_dst = dst_def.id
JOIN dict_word AS dst_word
  ON dst_def.word_id = dst_word.id
WHERE dict_ref_type.ascii_symbol = '='
  -- Check Source Tags for 'C' or 'c'
  AND EXISTS (
    SELECT 1
    FROM dict_shared_tag
    JOIN dict_tag
      ON dict_shared_tag.tag_id = dict_tag.id
    WHERE dict_shared_tag.for_shared_id = src_def.shared_id
      AND dict_tag.ascii_symbol IN ('C', 'c')
  )
  -- Check Destination Tags for 'T' or 't'
  AND EXISTS (
    SELECT 1
    FROM dict_shared_tag
    JOIN dict_tag
      ON dict_shared_tag.tag_id = dict_tag.id
    WHERE dict_shared_tag.for_shared_id = dst_def.shared_id
      AND dict_tag.ascii_symbol IN ('T', 't')
  );

*/
