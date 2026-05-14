use crate::common;
pub use crate::config::APPROX_TXT_FILE_SIZE;
use crate::db_autofix;
use crate::pinyin;
use regex::Regex;
use rusqlite::{Connection, Error as SqliteError, Transaction};
use std::collections::HashMap;

use crate::db_to_txt;
use crate::txt_to_db;

// Represents either a single Unicode code point or a range of code points.
enum HanChar {
    Single(u32),
    Range(u32, u32),
}

// A static slice holding the Unicode ranges for Han characters.
static LHAN: &[HanChar] = &[
    HanChar::Range(0x2E80, 0x2E99), // CJK RADICAL REPEAT, CJK RADICAL RAP
    HanChar::Range(0x2E9B, 0x2EF3), // CJK RADICAL CHOKE, CJK RADICAL C-SIMPLIFIED TURTLE
    HanChar::Range(0x2F00, 0x2FD5), // KANGXI RADICAL ONE, KANGXI RADICAL FLUTE
    HanChar::Single(0x3005),        // IDEOGRAPHIC ITERATION MARK
    HanChar::Single(0x3007),        // IDEOGRAPHIC NUMBER ZERO
    HanChar::Range(0x3021, 0x3029), // HANGZHOU NUMERAL ONE, HANGZHOU NUMERAL NINE
    HanChar::Range(0x3038, 0x303A), // HANGZHOU NUMERAL TEN, HANGZHOU NUMERAL THIRTY
    HanChar::Single(0x303B),        // VERTICAL IDEOGRAPHIC ITERATION MARK
    HanChar::Range(0x3400, 0x4DB5), // CJK UNIFIED IDEOGRAPH-3400, CJK UNIFIED IDEOGRAPH-4DB5
    HanChar::Range(0x4E00, 0x9FC3), // CJK UNIFIED IDEOGRAPH-4E00, CJK UNIFIED IDEOGRAPH-9FC3
    HanChar::Range(0xF900, 0xFA2D), // CJK COMPATIBILITY IDEOGRAPH-F900, CJK COMPATIBILITY IDEOGRAPH-FA2D
    HanChar::Range(0xFA30, 0xFA6A), // CJK COMPATIBILITY IDEOGRAPH-FA30, CJK COMPATIBILITY IDEOGRAPH-FA6A
    HanChar::Range(0xFA70, 0xFAD9), // CJK COMPATIBILITY IDEOGRAPH-FA70, CJK COMPATIBILITY IDEOGRAPH-FAD9
    HanChar::Range(0x20000, 0x2A6D6), // CJK UNIFIED IDEOGRAPH-20000, CJK UNIFIED IDEOGRAPH-2A6D6
    HanChar::Range(0x2F800, 0x2FA1D), // CJK COMPATIBILITY IDEOGRAPH-2F800, CJK COMPATIBILITY IDEOGRAPH-2FA1D
];

/// Compiles and returns a regex that matches only Hanzi characters.
fn get_hanzi_only_regex_pattern() -> Regex {
    let mut pattern_list = String::new();

    for han_char in LHAN {
        match *han_char {
            HanChar::Range(from, to) => {
                pattern_list.push_str(&format!(
                    "{}-{}",
                    char::from_u32(from).unwrap(),
                    char::from_u32(to).unwrap()
                ));
            }
            HanChar::Single(val) => {
                pattern_list.push(char::from_u32(val).unwrap());
            }
        }
    }
    let pattern = format!("[{pattern_list}]");

    Regex::new(&pattern).unwrap()
}


#[allow(clippy::similar_names, reason = "a vs b")]
pub fn check_conflicting_notes_on_symmetric_references(
    conn: &Transaction,
) -> Result<Vec<String>, SqliteError> {
    let mut errors = vec![];
    let mut stmt = conn.prepare(
        r"
        SELECT
            -- Information about the first side of the relationship (Word A)
            word_A.trad AS word_A_trad,
            word_A.simp AS word_A_simp,
            def_A.ext_def_id AS word_A_ext_def_id, -- This will be NULL if the reference is not from a specific definition

            -- Information about the second side of the relationship (Word B)
            word_B.trad AS word_B_trad,
            word_B.simp AS word_B_simp,
            def_B.ext_def_id AS word_B_ext_def_id, -- This will be NULL if the reference is not to a specific definition

            -- Conflicting information from the two symmetric references
            ref1.id AS reference_A_to_B_id,
            shared1.note_id AS reference_A_to_B_note_id,
            ref2.id AS reference_B_to_A_id,
            shared2.note_id AS reference_B_to_A_note_id
        FROM
            dict_reference AS ref1
        JOIN
            dict_ref_type AS ref_type ON ref1.ascii_symbol = ref_type.ascii_symbol
        -- This self-join finds the symmetric pair
        JOIN
            dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst
                                AND ref1.word_id_dst = ref2.word_id_src
                                AND ref1.ascii_symbol = ref2.ascii_symbol
                                AND (ref1.definition_id_src = ref2.definition_id_dst OR (ref1.definition_id_src IS NULL AND ref2.definition_id_dst IS NULL))
                                AND (ref1.definition_id_dst = ref2.definition_id_src OR (ref1.definition_id_dst IS NULL AND ref2.definition_id_src IS NULL))
        -- Joins to get note information
        JOIN
            dict_shared AS shared1 ON ref1.shared_id = shared1.id
        JOIN
            dict_shared AS shared2 ON ref2.shared_id = shared2.id
        -- New joins to get user-friendly identifiers
        JOIN
            dict_word AS word_A ON ref1.word_id_src = word_A.id
        JOIN
            dict_word AS word_B ON ref1.word_id_dst = word_B.id
        LEFT JOIN
            dict_definition AS def_A ON ref1.definition_id_src = def_A.id
        LEFT JOIN
            dict_definition AS def_B ON ref1.definition_id_dst = def_B.id
        WHERE
            ref_type.is_symmetric = 1
            -- This condition ensures we process each pair only once
            AND ref1.id < ref2.id
            -- The actual conflict condition: both have different, non-null notes
            AND shared1.note_id IS NOT NULL
            AND shared2.note_id IS NOT NULL
            AND shared1.note_id <> shared2.note_id;
        "
    )?;
    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        let word_a_trad: String = row.get("word_A_trad")?;
        let word_a_simp: String = row.get("word_A_simp")?;
        let word_a_ext_def_id: Option<u32> = row.get("word_A_ext_def_id")?;
        let word_a = common::format_word_def(&word_a_trad, &word_a_simp, word_a_ext_def_id);

        let word_b_trad: String = row.get("word_B_trad")?;
        let word_b_simp: String = row.get("word_B_simp")?;
        let word_b_ext_def_id: Option<u32> = row.get("word_B_ext_def_id")?;
        let word_b = common::format_word_def(&word_b_trad, &word_b_simp, word_b_ext_def_id);

        errors.push(format!("Validation Error: Different notes on symmetric references between {word_a} and {word_b}"));
    }
    todo!(); // TODO test this
    Ok(errors)
}

// TODO take list of stuff to check, e.g. if the source is a parsed text file some things might be ensured by the parser, SQL ensures other stuff
pub fn check_entries(conn: &Connection) -> Result<Vec<String>, SqliteError> {
    let mut errors = vec![];
    let mut stmt = conn.prepare(
        r"
        SELECT
            w.trad,
            w.simp,
            c.name AS class_name,
		GROUP_CONCAT(t_w.ascii_symbol, ';') AS word_tags,
        GROUP_CONCAT(p.pinyin_num, ';') AS pinyin_nums,
		GROUP_CONCAT(t_p.ascii_symbol, ';') AS pinyin_tags
        FROM dict_definition def
        JOIN dict_shared s ON def.shared_id = s.id
        JOIN dict_word w ON def.word_id = w.id
		JOIN dict_shared sw ON w.shared_id = sw.id
		LEFT JOIN dict_shared_tag st_w ON w.shared_id = st_w.for_shared_id
		LEFT JOIN dict_tag t_w ON st_w.tag_id = t_w.id
        JOIN dict_class c ON def.class_id = c.id
        LEFT JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
        LEFT JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
        LEFT JOIN dict_pron p ON sp.pron_id = p.id
        LEFT JOIN dict_shared p_s ON sp.shared_id = p_s.id
		LEFT JOIN dict_shared_tag st_p ON p_s.id = st_p.for_shared_id
		LEFT JOIN dict_tag t_p ON st_p.tag_id = t_p.id
        GROUP BY def.id
        ORDER BY s.rank, s.rank_relative;
        ",
    )?;

    let hanzi_pattern = get_hanzi_only_regex_pattern();
    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        let trad: String = row.get("trad")?;
        let simp: String = row.get("simp")?;
        let word_tags: Option<String> = row.get("word_tags")?;
        let pinyin_tags: Option<String> = row.get("pinyin_tags")?;
        let _class_name: String = row.get("class_name")?;
        let pinyin_nums: Vec<String> = row
            .get::<_, String>("pinyin_nums")?
            .split(';')
            .map(std::borrow::ToOwned::to_owned)
            .collect();

        // check if number of characters is the same in trad and simp
        if trad.chars().count() != simp.chars().count() {
            if let Some(w_t) = word_tags {
                if !w_t.contains(['i', 'X']) {
                    errors.push(format!("Validation Error: Different numbers of characters, traditional: {trad} simplified: {simp}"));
                    continue;
                }
            }
        }

        // check if the number of pinyin syllables matches the number of Chinese characters
        let trad_hanzi_only: String = hanzi_pattern
            .find_iter(&trad)
            .map(|mat| mat.as_str())
            .collect();
        if trad_hanzi_only.len() == trad.len()
            || trad_hanzi_only.len() == trad.replace('，', "").len()
        {
            let num_trad_chars = trad_hanzi_only.chars().count();
            for pinyin_num in pinyin_nums {
                if let Some(p_t) = &pinyin_tags {
                    if p_t.contains(['i', 'X']) {
                        continue;
                    }
                }

                let num_pinyin_syllables = pinyin::count_syllables(&pinyin_num);
                if num_pinyin_syllables != num_trad_chars {
                    errors.push(format!("Validation Error: pinyin syllables don't match number of characters, traditional: {trad} pinyin: {pinyin_num}"));
                }
            }
        }
    }

    let mut decomp_pron_errors = check_decomposition_pronunciations(conn)?;
    errors.append(&mut decomp_pron_errors);
    let mut decomp_incomplete_errors = check_incomplete_decompositions(conn)?;
    errors.append(&mut decomp_incomplete_errors);

    println!("Checks complete!");

    Ok(errors)
}

pub fn round_trip_check(conn: &Connection) -> Result<Vec<u8>, SqliteError> {
    eprintln!("Round trip check: db -> txt a");
    let mut txt_a: Vec<u8> = Vec::with_capacity(APPROX_TXT_FILE_SIZE);
    db_to_txt::db_to_txt(&mut txt_a, conn, false, None).unwrap();

    eprintln!("Round trip check: txt a -> db");
    let mut conn_b = Connection::open_in_memory().unwrap();
    let errors = txt_to_db::txt_to_db(&mut txt_a.as_slice(), &conn_b, None);
    if !errors.is_empty() {
        for err in errors {
            eprintln!("{err}");
        }
    }

    let tx = conn_b.transaction()?;
    db_autofix::autofix(&tx)?;
    tx.commit()?;

    eprintln!("Round trip check: db -> txt b");
    let mut txt_b: Vec<u8> = Vec::with_capacity(APPROX_TXT_FILE_SIZE);
    db_to_txt::db_to_txt(&mut txt_b, &conn_b, false, None).unwrap();

    eprintln!("Round trip check: compare txt a and txt b");

    if txt_a == txt_b {
        Ok(vec![])
    } else {
        Ok(txt_b)
    }
}

/// Check for decompositions where concatenated destination components don't match the source word.
pub fn check_incomplete_decompositions(conn: &Connection) -> Result<Vec<String>, SqliteError> {
    let mut errors = vec![];
    let mut stmt = conn.prepare(
        r"
        WITH OrderedDecompositions AS (
            SELECT 
                r.word_id_src,
                w_src.trad AS src_trad,
                w_src.simp AS src_simp,
                w_dst.trad AS dst_trad,
                w_dst.simp AS dst_simp
            FROM dict_reference r
            JOIN dict_word w_src ON r.word_id_src = w_src.id
            JOIN dict_word w_dst ON r.word_id_dst = w_dst.id
            JOIN dict_shared s ON r.shared_id = s.id
            WHERE r.ascii_symbol = '>' AND r.definition_id_src IS NULL
            ORDER BY r.word_id_src, s.rank ASC
        )
        SELECT 
            word_id_src, 
            src_trad AS trad, 
            src_simp AS simp,
            GROUP_CONCAT(dst_trad, '') AS concat_trad,
            GROUP_CONCAT(dst_simp, '') AS concat_simp
        FROM OrderedDecompositions
        GROUP BY word_id_src, src_trad, src_simp
        HAVING GROUP_CONCAT(dst_trad, '') != src_trad 
            OR GROUP_CONCAT(dst_simp, '') != src_simp;
        ",
    )?;

    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let trad: String = row.get("trad")?;
        let simp: String = row.get("simp")?;
        let concat_trad: String = row.get("concat_trad")?;
        let concat_simp: String = row.get("concat_simp")?;
        if concat_trad.contains("…") {
            continue;
        }
        let word = common::format_word_def(&trad, &simp, None);
        errors.push(format!(
            "Decomposition Error: Incomplete decomposition for {word}. Components combined to traditional: {concat_trad}, simplified: {concat_simp}"
        ));
    }

    Ok(errors)
}

/// Check if component references from a word have a pronunciation which fits to any pronunciation of the word
/// Only decompositions on a word level (definition source id of the reference is NULL) are checked
pub fn check_decomposition_pronunciations(conn: &Connection) -> Result<Vec<String>, SqliteError> {
    let mut errors = vec![];

    // 1. Load all pronunciations into memory for fast lookup.
    // Map: word_id -> List of (definition_id, pinyin_syllables)
    let mut word_to_def_prons: HashMap<u32, Vec<(u32, Vec<String>)>> = HashMap::new();

    let mut stmt_prons = conn.prepare(
        r"
        SELECT 
            def.word_id, 
            def.id AS def_id, 
            p.pinyin_num
        FROM dict_definition def
        JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
        JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
        JOIN dict_pron p ON sp.pron_id = p.id
        ",
    )?;

    let mut rows_prons = stmt_prons.query([])?;
    while let Some(row) = rows_prons.next()? {
        let word_id: u32 = row.get("word_id")?;
        let def_id: u32 = row.get("def_id")?;
        let pinyin_num: String = row.get("pinyin_num")?;

        let syllables = pinyin::pinyin_num_normalized_syllables(&pinyin_num);

        word_to_def_prons
            .entry(word_id)
            .or_default()
            .push((def_id, syllables));
    }

    // 2. Query all decomposition references (type '>'), ordered by source word and rank
    let mut stmt_refs = conn.prepare(
        r"
        SELECT
            r.word_id_src,
            ws.trad AS src_trad,
            ws.simp AS src_simp,
            r.word_id_dst,
            r.definition_id_dst,
            wd.trad AS dst_trad,
            wd.simp AS dst_simp
        FROM dict_reference r
        JOIN dict_shared s ON r.shared_id = s.id
        JOIN dict_word ws ON r.word_id_src = ws.id
        JOIN dict_word wd ON r.word_id_dst = wd.id
        WHERE r.ascii_symbol = '>' AND r.definition_id_src IS NULL
        ORDER BY r.word_id_src, s.rank, s.rank_relative
        ",
    )?;

    let mut rows_refs = stmt_refs.query([])?;

    struct Component {
        word_id: u32,
        def_id: Option<u32>,
        trad: String,
        simp: String,
    }

    let mut current_src_id = None;
    let mut current_src_trad = String::new();
    let mut current_src_simp = String::new();
    let mut current_components: Vec<Component> = Vec::new();

    // Helper closure to validate the current group
    let mut validate_current_group =
        |src_word_id: u32, src_trad: &str, src_simp: &str, components: &[Component]| {
            let Some(src_prons) = word_to_def_prons.get(&src_word_id) else {
                return; // No source pronunciations to check against
            };

            // We try to find *one* pronunciation of the source word that satisfies the sequence of components.
            let mut any_src_pron_valid = false;

            for (_, src_syllables) in src_prons {
                let mut current_idx = 0;
                let mut all_components_found = true;

                for comp in components {
                    if comp.trad.starts_with("…") || comp.trad == "，" {
                        continue;
                    }
                    let Some(comp_prons) = word_to_def_prons.get(&comp.word_id) else {
                        // If a component has no pronunciation, we can't verify it.
                        // Assuming valid data, this might be skippable or flagged elsewhere.
                        continue;
                    };

                    let mut comp_found = false;

                    // Try to find any valid pronunciation of this component
                    // in the remaining part of the source word (src_syllables[current_idx..])
                    'comp_pron_loop: for (def_id, comp_syllables) in comp_prons {
                        if let Some(req_def) = comp.def_id {
                            if req_def != *def_id {
                                continue;
                            }
                        }

                        if comp_syllables.is_empty() {
                            continue;
                        }

                        // Optimization: Not enough space left in source
                        if current_idx + comp_syllables.len() > src_syllables.len() {
                            continue;
                        }

                        let src_slice =
                            &src_syllables[current_idx..current_idx + comp_syllables.len()];

                        // Check if this slice matches the component pronunciation (with fuzzy tone 5)
                        let mut match_seq = true;
                        for k in 0..comp_syllables.len() {
                            if !pinyin::pinyin_match_excl_neutral_tone(
                                &src_slice[k],
                                &comp_syllables[k],
                            ) {
                                match_seq = false;
                                break;
                            }
                        }

                        if match_seq {
                            // Found the component! Move the index past this match.
                            current_idx += comp_syllables.len();
                            comp_found = true;
                            break 'comp_pron_loop;
                        }
                    }

                    if !comp_found {
                        all_components_found = false;
                        break;
                    }
                }

                if all_components_found {
                    any_src_pron_valid = true;
                    break;
                }
            }

            if !any_src_pron_valid {
                let comp_str: Vec<String> = components
                    .iter()
                    .map(|c| common::format_word_def(&c.trad, &c.simp, None))
                    .collect();
                errors.push(format!(
                    "Decomposition Error: Components of {} do not match any of its pronunciations (allowing neutral tones). Components: {}",
                    common::format_word_def(&src_trad, &src_simp, None), comp_str.join(";")
                ));
            }
        };

    while let Some(row) = rows_refs.next()? {
        let word_id_src: u32 = row.get("word_id_src")?;
        let src_trad: String = row.get("src_trad")?;
        let src_simp: String = row.get("src_simp")?;

        let comp = Component {
            word_id: row.get("word_id_dst")?,
            def_id: row.get("definition_id_dst")?,
            trad: row.get("dst_trad")?,
            simp: row.get("dst_simp")?,
        };

        if Some(word_id_src) != current_src_id {
            if let Some(id) = current_src_id {
                validate_current_group(
                    id,
                    &current_src_trad,
                    &current_src_simp,
                    &current_components,
                );
            }

            current_src_id = Some(word_id_src);
            current_src_trad = src_trad;
            current_src_simp = src_simp;
            current_components.clear();
        }

        current_components.push(comp);
    }

    if let Some(id) = current_src_id {
        validate_current_group(
            id,
            &current_src_trad,
            &current_src_simp,
            &current_components,
        );
    }

    Ok(errors)
}
