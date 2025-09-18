// LLM generated:
// - regex check if character is Chinese character (translated from python, which was based on stackoverflow answer)
// - SQL to check for conflicts and add missing things

use crate::common;
pub use crate::config::APPROX_TXT_FILE_SIZE;
use crate::pinyin;
use regex::Regex;
use rusqlite::{Connection, Error as SqliteError, Transaction};

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

#[allow(clippy::similar_names, reason="a vs b")]
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
            dict_ref_type AS ref_type ON ref1.ref_type_id = ref_type.id
        -- This self-join finds the symmetric pair
        JOIN
            dict_reference AS ref2 ON ref1.word_id_src = ref2.word_id_dst
                                AND ref1.word_id_dst = ref2.word_id_src
                                AND ref1.ref_type_id = ref2.ref_type_id
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
        GROUP_CONCAT(p.pinyin_num, ';') AS pinyin_nums
        FROM dict_definition def
        JOIN dict_shared s ON def.shared_id = s.id
        JOIN dict_word w ON def.word_id = w.id
        JOIN dict_class c ON def.class_id = c.id
        LEFT JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
        LEFT JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
        LEFT JOIN dict_pron p ON sp.pron_id = p.id
        LEFT JOIN dict_shared p_s ON sp.shared_id = p_s.id
        GROUP BY def.id
        ORDER BY s.rank, s.rank_relative;
        ",
    )?;

    let hanzi_pattern = get_hanzi_only_regex_pattern();
    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        let trad: String = row.get("trad")?;
        let simp: String = row.get("simp")?;
        let _class_name: String = row.get("class_name")?;
        let pinyin_nums: Vec<String> = row
            .get::<_, String>("pinyin_nums")?
            .split(';')
            .map(std::borrow::ToOwned::to_owned)
            .collect();

        // check if number of characters is the same in trad and simp
        if trad.chars().count() != simp.chars().count() {
            errors.push(format!("Validation Error: Different numbers of characters, traditional: {trad} simplified: {simp}"));
            continue;
        }

        // check if the number of pinyin syllables matches the number of Chinese characters
        let trad_hanzi_only: String = hanzi_pattern
            .find_iter(&trad)
            .map(|mat| mat.as_str())
            .collect();
        if trad_hanzi_only.len() == trad.len() {
            let possible_erhuas = trad.chars().filter(|c| *c == '兒').count();
            let num_trad_chars = trad.chars().count();
            let expected_syllables = (num_trad_chars - possible_erhuas)..=num_trad_chars;
            for pinyin_num in pinyin_nums {
                let num_pinyin_syllables = pinyin::count_syllables(&pinyin_num);
                if !expected_syllables.contains(&num_pinyin_syllables) {
                    errors.push(format!("Validation Error: pinyin syllables don't match number of characters, traditional: {trad} pinyin: {pinyin_num}"));
                }
            }
        }
    }
    Ok(errors)
}

pub fn round_trip_check(conn: &Connection) -> Result<Vec<u8>, SqliteError> {
    eprintln!("Round trip check: db -> txt a");
    let mut txt_a: Vec<u8> = Vec::with_capacity(APPROX_TXT_FILE_SIZE);
    db_to_txt::db_to_txt(&mut txt_a, conn, false, None).unwrap();

    eprintln!("Round trip check: txt a -> db");
    let conn_b = Connection::open_in_memory().unwrap();
    let errors = txt_to_db::txt_to_db(&mut txt_a.as_slice(), &conn_b, None);
    if !errors.is_empty() {
        for err in errors {
            eprintln!("{err}");
        }
    }

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
