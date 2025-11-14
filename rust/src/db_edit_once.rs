use crate::common::SqliteId;
use crate::db_read;
use crate::db_edit;
use crate::config;
use crate::pinyin;
use rusqlite::{Error as SqliteError, Transaction, params};
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
                db_edit::Tag::Full {
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
                db_edit::add_tag(
                    conn,
                    db_edit::EntryId::Definition(definition.id),
                    db_edit::Tag::Ascii(ascii_char),
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
            db_edit::update_definition_text(conn, definition.id, &new_definition_text.trim()).unwrap();
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


pub fn apply_pinyin_tags_from_json(
    conn: &Transaction,
    json_path: &str,
) -> Result<(), Box<dyn Error>> {
    // 1. Read and parse the JSON file into a HashMap.
    let file = File::open(json_path)?;
    let reader = BufReader::new(file);
    let pinyin_tags_map: HashMap<String, HashMap<String, String>> = serde_json::from_reader(reader)?;

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

        // 4. Look for a matching word in the parsed JSON data.
        if let Some(pinyin_map) = pinyin_tags_map.get(&trad) {
            // 5. If the word matches, look for a matching pinyin.
            if let Some(tags) = pinyin_map.get(&pinyin_num) {
                // 6. If both match, apply each character in the tag string.
                for tag_char in tags.chars() {
                    if config::tag_to_txt_ascii_common(tag_char).is_some() {
                        db_edit::add_tag(
                            conn,
                            db_edit::EntryId::Pinyin(shared_pron_id),
                            db_edit::Tag::Ascii(tag_char),
                        )?;
                    } else {
                        eprintln!("Warning: Unknown tag character '{}' for word '{}', pinyin '{}'", tag_char, trad, pinyin_num);
                    }
                }
            }
        }
    }

    Ok(())
}