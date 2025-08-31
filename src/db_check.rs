// LLM generated: regex check if character is Chinese character (translated from python, which was based on stackoverflow answer)

use rusqlite::{Connection, Error as SqliteError, Row};
use regex::Regex;
use crate::pinyin::*;

type SqliteId = i64; // TODO common

// Represents either a single Unicode code point or a range of code points.
enum HanChar {
    Single(u32),
    Range(u32, u32),
}

// A static slice holding the Unicode ranges for Han characters.
static LHAN: &[HanChar] = &[
    HanChar::Range(0x2E80, 0x2E99),    // CJK RADICAL REPEAT, CJK RADICAL RAP
    HanChar::Range(0x2E9B, 0x2EF3),    // CJK RADICAL CHOKE, CJK RADICAL C-SIMPLIFIED TURTLE
    HanChar::Range(0x2F00, 0x2FD5),    // KANGXI RADICAL ONE, KANGXI RADICAL FLUTE
    HanChar::Single(0x3005),           // IDEOGRAPHIC ITERATION MARK
    HanChar::Single(0x3007),           // IDEOGRAPHIC NUMBER ZERO
    HanChar::Range(0x3021, 0x3029),    // HANGZHOU NUMERAL ONE, HANGZHOU NUMERAL NINE
    HanChar::Range(0x3038, 0x303A),    // HANGZHOU NUMERAL TEN, HANGZHOU NUMERAL THIRTY
    HanChar::Single(0x303B),           // VERTICAL IDEOGRAPHIC ITERATION MARK
    HanChar::Range(0x3400, 0x4DB5),    // CJK UNIFIED IDEOGRAPH-3400, CJK UNIFIED IDEOGRAPH-4DB5
    HanChar::Range(0x4E00, 0x9FC3),    // CJK UNIFIED IDEOGRAPH-4E00, CJK UNIFIED IDEOGRAPH-9FC3
    HanChar::Range(0xF900, 0xFA2D),    // CJK COMPATIBILITY IDEOGRAPH-F900, CJK COMPATIBILITY IDEOGRAPH-FA2D
    HanChar::Range(0xFA30, 0xFA6A),    // CJK COMPATIBILITY IDEOGRAPH-FA30, CJK COMPATIBILITY IDEOGRAPH-FA6A
    HanChar::Range(0xFA70, 0xFAD9),    // CJK COMPATIBILITY IDEOGRAPH-FA70, CJK COMPATIBILITY IDEOGRAPH-FAD9
    HanChar::Range(0x20000, 0x2A6D6),  // CJK UNIFIED IDEOGRAPH-20000, CJK UNIFIED IDEOGRAPH-2A6D6
    HanChar::Range(0x2F800, 0x2FA1D),  // CJK COMPATIBILITY IDEOGRAPH-2F800, CJK COMPATIBILITY IDEOGRAPH-2FA1D
];

/// Compiles and returns a regex that matches only Hanzi characters.
fn get_hanzi_only_regex_pattern() -> Regex {
    let mut pattern_list = String::new();

    for han_char in LHAN {
        match han_char {
            &HanChar::Range(from, to) => {
                pattern_list.push_str(&format!("{}-{}", char::from_u32(from).unwrap(), char::from_u32(to).unwrap()));
            }
            &HanChar::Single(val) => {
                pattern_list.push(char::from_u32(val).unwrap());
            }
        }
    }
    let pattern = format!("[{}]", pattern_list);

    Regex::new(&pattern).unwrap()
}

pub fn check_entries(conn: &Connection) -> Result<Vec<String>, SqliteError> {
    let mut errors = vec![];
    let mut stmt = conn
        .prepare(
            r#"
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
        "#,
        )
        .unwrap();

    let hanzi_pattern = get_hanzi_only_regex_pattern();
    let mut rows = stmt.query([])?;

    while let Some(row) = rows.next()? {
        let trad: String = row.get("trad")?;
        let simp: String = row.get("simp")?;
        let class_name: String = row.get("class_name")?;
        let pinyin_nums: Vec<String> = row.get::<_, String>("pinyin_nums")?
            .split(';')
            .map(|s| s.to_owned())
            .collect();

        // check if number of characters is the same in trad and simp
        if trad.chars().count() != simp.chars().count() {
            errors.push(format!("Validation Error: Different numbers of characters, traditional: {} simplified: {}", trad, simp));
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
            let expected_syllables = num_trad_chars - possible_erhuas..num_trad_chars + 1;
            for pinyin_num in pinyin_nums {
                let num_pinyin_syllables = count_syllables(&pinyin_num);
                if !expected_syllables.contains(&num_pinyin_syllables) {
                    errors.push(format!("Validation Error: pinyin syllables don't match number of characters, traditional: {} pinyin: {}", trad, pinyin_num));
                }
            }
        }

    }
    Ok(errors)
}