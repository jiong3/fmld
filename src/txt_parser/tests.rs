#[cfg(test)]
use super::*;

#[test]
fn test_parse_tags_simple_ascii() {
    let input = "|T|";
    let expected = Ok(("", vec![Tag::Ascii('T')]));
    assert_eq!(parse_tags(input), expected);
}

#[test]
fn test_parse_tags_multiple_ascii() {
    let input = "|AB C|";
    let expected = Ok(("", vec![Tag::Ascii('A'), Tag::Ascii('B'), Tag::Ascii('C')]));
    assert_eq!(parse_tags(input), expected);
}

#[test]
fn test_parse_tags_simple_full() {
    let input = "|#tag1|";
    let expected = Ok(("", vec![Tag::Full("tag1".to_owned())]));
    assert_eq!(parse_tags(input), expected);
}

#[test]
fn test_parse_tags_multiple_full() {
    let input = "|#tag1#tag2|";
    let expected = Ok((
        "",
        vec![Tag::Full("tag1".to_owned()), Tag::Full("tag2".to_owned())],
    ));
    assert_eq!(parse_tags(input), expected);
}

#[test]
fn test_parse_tags_mixed() {
    let input = "|A#tag1 B #tag2|";
    let expected = Ok((
        "",
        vec![
            Tag::Ascii('A'),
            Tag::Full("tag1 B".to_owned()),
            Tag::Full("tag2".to_owned()),
        ],
    ));
    assert_eq!(parse_tags(input), expected);
}

#[test]
fn test_parse_tags_no_tags() {
    let input = "||";
    let expected = Ok(("", vec![]));
    assert_eq!(parse_tags(input), expected);
}

#[test]
fn test_parse_tags_with_whitespace() {
    let input = "  |  A #tag1  |  ";
    let expected = Ok(("", vec![Tag::Ascii('A'), Tag::Full("tag1".to_owned())]));
    assert_eq!(parse_tags(input), expected);
}

#[test]
fn test_parse_tags_with_remainder() {
    let input = "|T| some other text";
    let expected = Ok(("some other text", vec![Tag::Ascii('T')]));
    assert_eq!(parse_tags(input), expected);
}

// Tests for parse_word_line
#[test]
fn test_parse_word_line_simple() {
    let input = "|T| TraditionalWord";
    let expected = Ok((
        "",
        vec![WordTagGroup {
            tags: vec![Tag::Ascii('T')],
            words: vec![Word {
                trad: "TraditionalWord".to_owned(),
                simp: None,
            }],
        }],
    ));
    assert_eq!(parse_word_line(input), expected);
}

#[test]
fn test_parse_word_line_with_simplified() {
    let input = "|T| Traditional/Simplified";
    let expected = Ok((
        "",
        vec![WordTagGroup {
            tags: vec![Tag::Ascii('T')],
            words: vec![Word {
                trad: "Traditional".to_owned(),
                simp: Some("Simplified".to_owned()),
            }],
        }],
    ));
    assert_eq!(parse_word_line(input), expected);
}

#[test]
fn test_parse_word_line_multiple_words() {
    let input = "|T| Word1; Word2/Simp2";
    let expected = Ok((
        "",
        vec![WordTagGroup {
            tags: vec![Tag::Ascii('T')],
            words: vec![
                Word {
                    trad: "Word1".to_owned(),
                    simp: None,
                },
                Word {
                    trad: "Word2".to_owned(),
                    simp: Some("Simp2".to_owned()),
                },
            ],
        }],
    ));
    assert_eq!(parse_word_line(input), expected);
}

#[test]
fn test_parse_word_line_multiple_tag_groups() {
    let input = "|T| Trad1/Simp1 |S| Simp2";
    let expected = Ok((
        "",
        vec![
            WordTagGroup {
                tags: vec![Tag::Ascii('T')],
                words: vec![Word {
                    trad: "Trad1".to_owned(),
                    simp: Some("Simp1".to_owned()),
                }],
            },
            WordTagGroup {
                tags: vec![Tag::Ascii('S')],
                words: vec![Word {
                    trad: "Simp2".to_owned(),
                    simp: None,
                }],
            },
        ],
    ));
    assert_eq!(parse_word_line(input), expected);
}

#[test]
fn test_parse_word_line_no_tags() {
    let input = "|| Word1";
    let expected = Ok((
        "",
        vec![WordTagGroup {
            tags: vec![],
            words: vec![Word {
                trad: "Word1".to_owned(),
                simp: None,
            }],
        }],
    ));
    assert_eq!(parse_word_line(input), expected);
}

// Tests for parse_pinyin_line
#[test]
fn test_parse_pinyin_line_simple() {
    let input = "|M| man2";
    let expected = Ok((
        "",
        vec![PinyinTagGroup {
            tags: vec![Tag::Ascii('M')],
            pinyins: vec!["man2".to_owned()],
        }],
    ));
    assert_eq!(parse_pinyin_line(input), expected);
}

#[test]
fn test_parse_pinyin_line_multiple_pinyins() {
    let input = "|M| man2; woman2";
    let expected = Ok((
        "",
        vec![PinyinTagGroup {
            tags: vec![Tag::Ascii('M')],
            pinyins: vec!["man2".to_owned(), "woman2".to_owned()],
        }],
    ));
    assert_eq!(parse_pinyin_line(input), expected);
}

#[test]
fn test_parse_pinyin_line_multiple_tag_groups() {
    let input = "|M| man2 |C| cha2";
    let expected = Ok((
        "",
        vec![
            PinyinTagGroup {
                tags: vec![Tag::Ascii('M')],
                pinyins: vec!["man2".to_owned()],
            },
            PinyinTagGroup {
                tags: vec![Tag::Ascii('C')],
                pinyins: vec!["cha2".to_owned()],
            },
        ],
    ));
    assert_eq!(parse_pinyin_line(input), expected);
}

#[test]
fn test_parse_pinyin_line_full_tags() {
    let input = "|M#tag1| man2";
    let expected = Ok((
        "",
        vec![PinyinTagGroup {
            tags: vec![Tag::Ascii('M'), Tag::Full("tag1".to_owned())],
            pinyins: vec!["man2".to_owned()],
        }],
    ));
    assert_eq!(parse_pinyin_line(input), expected);
}

#[test]
fn test_parse_pinyin_line_no_tags() {
    let input = "|| man2";
    let expected = Ok((
        "",
        vec![PinyinTagGroup {
            tags: vec![],
            pinyins: vec!["man2".to_owned()],
        }],
    ));
    assert_eq!(parse_pinyin_line(input), expected);
}
