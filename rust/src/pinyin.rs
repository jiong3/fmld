use std::borrow::Borrow;
use std::hash::Hash;
use std::{collections::HashSet, usize};

#[must_use]
pub fn pinyin_mark_from_num(pinyin_num: &str) -> String {
    // TODO currently no unicode normalization for ê
    let split_pattern = |c: char| (c > '0') && (c < '6');
    let apostrophe_chars = &['a', 'e', 'ê', 'o'];
    let mut pinyin_mark_syllables = vec![];
    for pinyin_num_syllable in pinyin_num.split_inclusive(split_pattern) {
        if !pinyin_mark_syllables.is_empty()
            && pinyin_num_syllable
                .to_lowercase()
                .starts_with(apostrophe_chars)
        {
            pinyin_mark_syllables.push("'".to_owned());
        }
        pinyin_mark_syllables.push(pinyin_syllable_mark_from_num(pinyin_num_syllable));
    }
    pinyin_mark_syllables.join("")
}

/// Helper to separate the base pinyin from the tone number.
/// e.g. "ni3" -> ("ni", 3). If no digit is found, defaults to tone 5.
#[must_use]
pub fn pinyin_get_syllable_tone(syllable: &str) -> (&str, u8) {
    let bytes = syllable.as_bytes();
    if let Some(last) = bytes.last() {
        if last.is_ascii_digit() {
            let tone = last - b'0';
            let base = &syllable[0..syllable.len() - 1];
            return (base, tone);
        }
    }
    (syllable, 5)
}

/// Checks if two syllables match.
/// The base (e.g., "ni") must be identical.
/// The tones match if they are equal OR if at least one of them is the neutral tone (5).
#[must_use]
pub fn pinyin_match_excl_neutral_tone(s1: &str, s2: &str) -> bool {
    let (b1, t1) = pinyin_get_syllable_tone(s1);
    let (b2, t2) = pinyin_get_syllable_tone(s2);

    if b1 != b2 {
        return false;
    }

    t1 == t2 || t1 == 5 || t2 == 5
}

#[must_use]
pub fn count_syllables(pinyin_num: &str) -> usize {
    let pattern = ['1', '2', '3', '4', '5'];
    pinyin_num.chars().filter(|c| pattern.contains(c)).count()
}

#[must_use]
/// lowercase pinyin syllables without additional characters (. -,)
pub fn pinyin_num_normalized_syllables(pinyin_num: &str) -> Vec<String> {
    let strip_chars = &['.', ' ', '-', ','];
    let split_pattern = |c: char| (c > '0') && (c < '6');
    let mut pinyin_num_syllables = vec![];
    for pinyin_num_syllable in pinyin_num.split_inclusive(split_pattern) {
        pinyin_num_syllables.push(
            pinyin_num_syllable
                .to_lowercase()
                .trim_matches(strip_chars)
                .to_owned(),
        );
    }
    pinyin_num_syllables
}

#[must_use]
/// lowercase pinyin without additional characters (. -,)
pub fn pinyin_num_normalized(pinyin_num: &str) -> String {
    pinyin_num_normalized_syllables(pinyin_num).join("")
}

#[must_use]
fn pinyin_syllable_mark_from_num(pinyin_num: &str) -> String {
    // "normalize" pinyin, could be extended for handling of MDBG u:
    let pinyin = pinyin_num.replace("v", "ü").replace("V", "Ü");

    // Split off the final char (expected to be the tone number)
    let mut chars = pinyin.chars();
    let Some(last) = chars.next_back() else {
        return String::new();
    };
    let Some(tone) = last.to_digit(10) else {
        return pinyin;
    };
    let mut pinyin: String = chars.collect();
    let pinyin_lower = pinyin.to_lowercase();

    if (1..=4).contains(&tone) {
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

        if pinyin_vowels.is_empty() {
            if pinyin_lower.contains('n') {
                target = Some("n");
            } else if pinyin_lower.contains('m') {
                target = Some("m");
            }
        } else {
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

#[must_use]
const fn tone_mark_char(ch: char, tone: u32) -> Option<&'static str> {
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

pub trait StringContainer {
    fn contains(&self, target: &str) -> bool;
}

impl<T> StringContainer for HashSet<T>
where
    T: Hash + Eq + Borrow<str>,
{
    fn contains(&self, target: &str) -> bool {
        self.contains(target)
    }
}

/// Split str into substrings, with a prioritizing those in `good_strings`
///
/// 1st priority: maximize number of characters which are part of a `good_strings`
/// 2nd priority: minimize number of substrings which are not in `good_strings`
///
/// Return all possible segmentations with equal priority, in order of the number of substrings (less to more)
pub fn split_into_substrings<'a, S: StringContainer>(
    s: &'a str,
    good_strings: &S,
) -> Vec<Vec<&'a str>> {
    #[derive(Clone, Debug)]
    struct Score {
        bytes_covered: usize, // 1st priority: bytes covered by good strings, larger is better
        substr_uncovered: usize, // 2nd priority: number of substrings not in good strings, smaller is better
        start_idxs: Vec<usize>,  // start indexes of the substrings
    }

    if s.is_empty() {
        return vec![];
    }

    let mut scores: Vec<Score> = vec![
        Score {
            bytes_covered: 0,
            substr_uncovered: 0,
            start_idxs: vec![],
        };
        s.len() + 1
    ];
    let mut end_idxs: Vec<usize> = s.char_indices().skip(1).map(|i| i.0).collect();
    end_idxs.push(s.len()); // add end index of last char

    // find best substring segmentation up to each byte index
    for idx_end in end_idxs {
        for idx_start in s[..idx_end].char_indices().map(|i| i.0) {
            let sub = &s[idx_start..idx_end];

            // check score for segmentation with current substring
            let is_good = good_strings.contains(sub);
            let bytes_covered = if is_good {
                scores[idx_start].bytes_covered + sub.len()
            } else {
                scores[idx_start].bytes_covered
            };
            let substr_uncovered = if is_good {
                scores[idx_start].substr_uncovered
            } else {
                scores[idx_start].substr_uncovered + 1
            };

            let current_best = scores[idx_end].start_idxs.is_empty()
                || bytes_covered > scores[idx_end].bytes_covered
                || (bytes_covered == scores[idx_end].bytes_covered
                    && substr_uncovered < scores[idx_end].substr_uncovered);

            let current_equal = bytes_covered == scores[idx_end].bytes_covered
                && substr_uncovered == scores[idx_end].substr_uncovered;

            if current_best {
                // replace previous best
                scores[idx_end] = Score {
                    bytes_covered,
                    substr_uncovered,
                    start_idxs: vec![idx_start],
                };
            }
            if current_equal {
                // extend equally good segmentations
                scores[idx_end].start_idxs.push(idx_start);
            }
        }
    }

    // build up substrings
    let mut substrings = vec![];
    let mut stack: Vec<(usize, Vec<&'a str>)> = vec![(s.len(), vec![])];
    while let Some((end_idx, mut segmentation)) = stack.pop() {
        if end_idx == 0 {
            segmentation.reverse();
            substrings.push(segmentation);
            continue;
        }
        for &start_idx in &scores[end_idx].start_idxs {
            let mut new_segmentation = segmentation.clone();
            new_segmentation.push(&s[start_idx..end_idx]); // Prepend
            stack.push((start_idx, new_segmentation));
        }
    }
    substrings.sort_by_key(std::vec::Vec::len);

    substrings
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a HashSet from a list of string slices
    fn set(list: &[&'static str]) -> HashSet<&'static str> {
        list.iter().cloned().collect()
    }

    #[test]
    fn test_pinyin_match_excl_neutral_tone() {
        let a = "jie1";
        let b = "jie5";
        let result = pinyin_match_excl_neutral_tone(a, b);
        assert!(result);
    }

    #[test]
    fn test_split_into_substrings() {
        // empty string
        let s = "";
        let good = set(&["a", "b"]);
        let result = split_into_substrings(s, &good);
        assert!(result.is_empty());

        assert_eq!(
            split_into_substrings("xyz", &HashSet::from(["abc"])),
            vec![vec!["xyz"]]
        );
        assert_eq!(
            split_into_substrings("a", &HashSet::from(["abc"])),
            vec![vec!["a"]]
        );
        assert_eq!(
            split_into_substrings("ab", &HashSet::from(["a", "ab", "c"])),
            vec![vec!["ab"]]
        );
        assert_eq!(
            split_into_substrings("ab", &HashSet::from(["a", "b", "ab", "c"])),
            vec![vec!["ab"], vec!["a", "b"]]
        );

        let s = "abc";
        let good = set(&["ab", "bc"]);
        let result = split_into_substrings(s, &good);
        assert_eq!(result.len(), 2);
        // Order between these two relies on HashSet iteration order, check existence
        assert!(result.contains(&vec!["ab", "c"]));
        assert!(result.contains(&vec!["a", "bc"]));

        let s = "ni好";
        let good = set(&["ni", "好"]);
        let result = split_into_substrings(s, &good);
        assert_eq!(result, vec![vec!["ni", "好"]]);

        let s = "A愛B";
        let good = set(&["愛"]);
        let result = split_into_substrings(s, &good);
        assert_eq!(result, vec![vec!["A", "愛", "B"]]);

        let s = "foobar";
        let good = set(&["foo", "bar", "foobar"]);
        let result = split_into_substrings(s, &good);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], vec!["foobar"]);
        assert_eq!(result[1], vec!["foo", "bar"]);

        let s = "ABCde";
        let good = set(&["AB", "C", "ABC"]);
        let result = split_into_substrings(s, &good);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], vec!["ABC", "de"]);
        assert_eq!(result[1], vec!["AB", "C", "de"]);
    }

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
