pub fn pinyin_mark_from_num(pinyin_num: &str) -> String {
    // TODO currently no unicode normalization for ê and 
    let split_pattern = |c: char| (c > '0') && (c < '6');
    let apostrophe_chars = &['a', 'e', 'ê', 'o'];
    let mut pinyin_mark_syllables = vec![];
    for pinyin_num_syllable in pinyin_num.split_inclusive(split_pattern) {
        if !pinyin_mark_syllables.is_empty() && pinyin_num_syllable.to_lowercase().starts_with(apostrophe_chars) {
            pinyin_mark_syllables.push("'".to_owned());
        }
        pinyin_mark_syllables.push(pinyin_syllable_mark_from_num(pinyin_num_syllable))
    }
    pinyin_mark_syllables.join("")
}

fn pinyin_syllable_mark_from_num(pinyin_num: &str) -> String {
    // "normalize" pinyin, could be extended for handling of MDBG u:
    let pinyin = pinyin_num.replace("v", "ü").replace("V", "Ü");
    
    // Split off the final char (expected to be the tone number)
    let mut chars = pinyin.chars();
    let last = match chars.next_back() {
        Some(c) => c,
        None => return String::new(),
    };
    let Some(tone) = last.to_digit(10) else {
        return pinyin;
    };
    let mut pinyin: String = chars.collect();
    let pinyin_lower = pinyin.to_lowercase();

    if tone >= 1 && tone <= 4 {
        // Collect vowels from the lowercase sound, v as ü
        let mut pinyin_vowels = String::new();
        for c in pinyin_lower.chars() {
            match c {
                'a' | 'e' | 'ê' | 'i' | 'o' | 'u' | 'ü' => pinyin_vowels.push(c),
                _ => {}
            }
        }
        // Candidate target to mark ("a", "e", "ê", "ou", last vowel, or 'n'/'m' if no vowel)
        let mut target: Option<&str> = None;

        if !pinyin_vowels.is_empty() {
            for cand in ["a", "e", "ê", "ou"] {
                if pinyin_vowels.contains(cand) {
                    target = Some(cand);
                    break;
                }
            }
            if target.is_none() {
                // last vowel
                if let Some((i, _)) = pinyin_vowels.char_indices().next_back() {
                    target = Some(&pinyin_vowels[i..]);
                }
            }
        } else {
            if pinyin_lower.contains('n') {
                target = Some("n");
            } else if pinyin_lower.contains('m') {
                target = Some("m");
            }
        }

        if let Some(tgt) = target {
            if let Some(idx) = pinyin_lower.find(tgt) {
                // Char to be marked, from original-cased sound
                if let Some(ch_to_mark) = pinyin[idx..].chars().next() {
                    if let Some(marked) = tone_mark_char(ch_to_mark, tone) {
                        let needle = ch_to_mark.to_string();
                        pinyin = pinyin.replace(&needle, marked);
                    }
                }
            }
        }
    }

    pinyin
}

fn tone_mark_char(ch: char, tone: u32) -> Option<&'static str> {
    let tone_idx = (tone - 1) as usize;
    Some(match ch {
        'a' => ["ā", "á", "ǎ", "à", "a"][tone_idx],
        'A' => ["Ā", "Á", "Ǎ", "À", "A"][tone_idx],
        'e' => ["ē", "é", "ě", "è", "e"][tone_idx],
        'E' => ["Ē", "É", "Ě", "È", "E"][tone_idx],
        'ê' => ["ê̄", "ế", "ê̌", "ề", "ê"][tone_idx],
        'Ê' => ["Ê̄", "Ế", "Ê̌", "Ề", "Ê"][tone_idx],
        'i' => ["ī", "í", "ǐ", "ì", "i"][tone_idx],
        'I' => ["Ī", "Í", "Ǐ", "Ì", "I"][tone_idx],
        'o' => ["ō", "ó", "ǒ", "ò", "o"][tone_idx],
        'O' => ["Ō", "Ó", "Ǒ", "Ò", "O"][tone_idx],
        'u' => ["ū", "ú", "ǔ", "ù", "u"][tone_idx],
        'U' => ["Ū", "Ú", "Ǔ", "Ù", "U"][tone_idx],
        'ü' => ["ǖ", "ǘ", "ǚ", "ǜ", "ü"][tone_idx],
        'Ü' => ["Ǖ", "Ǘ", "Ǚ", "Ǜ", "Ü"][tone_idx],
        'm' => ["m̄", "ḿ", "m̌", "m̀", "m"][tone_idx],
        'M' => ["M̄", "Ḿ", "M̌", "M̀", "M"][tone_idx],
        'n' => ["n̄", "ń", "ň", "ǹ", "n"][tone_idx],
        'N' => ["N̄", "Ń", "Ň", "Ǹ", "N"][tone_idx],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_mark() {
        assert_eq!(pinyin_mark_from_num("ni3hao3"), "nǐhǎo");
        assert_eq!(pinyin_mark_from_num("zhong1guo2"), "zhōngguó");
        assert_eq!(pinyin_mark_from_num("lü4"), "lǜ");
        assert_eq!(pinyin_mark_from_num("nv3"), "nǚ");
        assert_eq!(pinyin_mark_from_num("er2"), "ér");
        assert_eq!(pinyin_mark_from_num("hen3"), "hěn");
        assert_eq!(pinyin_mark_from_num("ma5"), "ma");
        assert_eq!(pinyin_mark_from_num("ma5li5"), "mali");
        assert_eq!(pinyin_mark_from_num("a1i5"), "āi");
        assert_eq!(pinyin_mark_from_num("quan2ai1"), "quán'āi");
        assert_eq!(pinyin_mark_from_num("ou3"), "ǒu");
        assert_eq!(pinyin_mark_from_num("m2"), "ḿ");
        assert_eq!(pinyin_mark_from_num("N4"), "Ǹ");
        assert_eq!(pinyin_mark_from_num("jue2"), "jué");
        assert_eq!(pinyin_mark_from_num("xiong2"), "xióng");
        assert_eq!(pinyin_mark_from_num("pinyin"), "pinyin"); // No tone number
        assert_eq!(pinyin_mark_from_num(""), ""); // Empty string
        assert_eq!(pinyin_mark_from_num("song4"), "sòng");
        assert_eq!(pinyin_mark_from_num("lian3"), "liǎn");
        assert_eq!(pinyin_mark_from_num("gui4"), "guì");
        assert_eq!(pinyin_mark_from_num("shui3"), "shuǐ");
        assert_eq!(pinyin_mark_from_num("cuan1"), "cuān");
        assert_eq!(pinyin_mark_from_num("jiong3"), "jiǒng");
        assert_eq!(pinyin_mark_from_num("fen1"), "fēn");
        assert_eq!(pinyin_mark_from_num("hong2"), "hóng");
        assert_eq!(pinyin_mark_from_num("yun2"), "yún");
        assert_eq!(pinyin_mark_from_num("wen3"), "wěn");
        assert_eq!(pinyin_mark_from_num("yuan4"), "yuàn");
        assert_eq!(pinyin_mark_from_num("nü3"), "nǚ"); // already has ü
        assert_eq!(pinyin_mark_from_num("qu2"), "qú");
        assert_eq!(pinyin_mark_from_num("xu4"), "xù");
        assert_eq!(pinyin_mark_from_num("yue4"), "yuè");
        assert_eq!(pinyin_mark_from_num("jiong1"), "jiōng");
        assert_eq!(pinyin_mark_from_num("juan4"), "juàn");
        assert_eq!(pinyin_mark_from_num("Qing1"), "Qīng");
        assert_eq!(pinyin_mark_from_num("Xi4"), "Xì");
        assert_eq!(pinyin_mark_from_num("LUO2"), "LUÓ");
        assert_eq!(pinyin_mark_from_num("BA5"), "BA");
        assert_eq!(pinyin_mark_from_num("De5"), "De");
        assert_eq!(pinyin_mark_from_num("N3"), "Ň");
        assert_eq!(pinyin_mark_from_num("M1"), "M̄");
        assert_eq!(pinyin_mark_from_num("r5"), "r");
        assert_eq!(pinyin_mark_from_num("zhe4"), "zhè");
        assert_eq!(pinyin_mark_from_num("chi1"), "chī");
        assert_eq!(pinyin_mark_from_num("shi2"), "shí");
        assert_eq!(pinyin_mark_from_num("ri4"), "rì");
        assert_eq!(pinyin_mark_from_num("zi3"), "zǐ");
        assert_eq!(pinyin_mark_from_num("ci2"), "cí");
        assert_eq!(pinyin_mark_from_num("si4"), "sì");
        assert_eq!(pinyin_mark_from_num("zhi1"), "zhī");
        assert_eq!(pinyin_mark_from_num("chang2"), "cháng");
        assert_eq!(pinyin_mark_from_num("liang3"), "liǎng");
        assert_eq!(pinyin_mark_from_num("dian3"), "diǎn");
        assert_eq!(pinyin_mark_from_num("gui1"), "guī");
        assert_eq!(pinyin_mark_from_num("juan1"), "juān");
        assert_eq!(pinyin_mark_from_num("qiang2"), "qiáng");
        assert_eq!(pinyin_mark_from_num("bing3"), "bǐng");
        assert_eq!(pinyin_mark_from_num("kuang4"), "kuàng");
        assert_eq!(pinyin_mark_from_num("ting1"), "tīng");
        assert_eq!(pinyin_mark_from_num("yu4"), "yù");
        assert_eq!(pinyin_mark_from_num("yin2"), "yín");
        assert_eq!(pinyin_mark_from_num("weng3"), "wěng");
        assert_eq!(pinyin_mark_from_num("yong4"), "yòng");
        assert_eq!(pinyin_mark_from_num("lve4"), "lüè");
        assert_eq!(pinyin_mark_from_num("jue2"), "jué");
        assert_eq!(pinyin_mark_from_num("xue3"), "xuě");
        assert_eq!(pinyin_mark_from_num("yue4"), "yuè");
        assert_eq!(pinyin_mark_from_num("quan2"), "quán");
        assert_eq!(pinyin_mark_from_num("nve4"), "nüè");
        assert_eq!(pinyin_mark_from_num("nv3"), "nǚ");
        assert_eq!(pinyin_mark_from_num("nv5"), "nü");
        assert_eq!(pinyin_mark_from_num("Nv3"), "Nǚ");
        assert_eq!(pinyin_mark_from_num("Nv5"), "Nü");
        assert_eq!(pinyin_mark_from_num("v3"), "ǚ");
        assert_eq!(pinyin_mark_from_num("V3"), "Ǚ");
    }
}
