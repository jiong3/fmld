// LLM generated with minor modifications
// LLM input: parser file with grammar description

#[cfg(test)]
use super::*;

// Individual component parsers

#[test]
fn test_parse_tags() {
    assert_eq!(parse_tags("|a|"), Ok(("", vec![Tag::Ascii('a')])));
    assert_eq!(
        parse_tags("| a b |"),
        Ok(("", vec![Tag::Ascii('a'), Tag::Ascii('b')]))
    );
    assert_eq!(
        parse_tags("| a b #tag1 #tag2-long |"),
        Ok((
            "",
            vec![
                Tag::Ascii('a'),
                Tag::Ascii('b'),
                Tag::Full("tag1".to_string()),
                Tag::Full("tag2-long".to_string())
            ]
        ))
    );
    assert_eq!(parse_tags("||"), Ok(("", vec![])));
}

#[test]
fn test_parse_word() {
    assert_eq!(
        parse_word("傳統"),
        Ok((
            "",
            Word {
                trad: "傳統".to_string(),
                simp: None
            }
        ))
    );
    assert_eq!(
        parse_word("傳統/传统"),
        Ok((
            "",
            Word {
                trad: "傳統".to_string(),
                simp: Some("传统".to_string())
            }
        ))
    );
    assert_eq!(
        parse_word(" 傳統 ／ 传统 "),
        Ok((
            "",
            Word {
                trad: "傳統".to_string(),
                simp: Some("传统".to_string())
            }
        ))
    );
}

#[test]
fn test_parse_word_list() {
    assert_eq!(
        parse_word_list("單詞"),
        Ok((
            "",
            vec![Word {
                trad: "單詞".to_string(),
                simp: None
            }]
        ))
    );
    assert_eq!(
        parse_word_list("單詞; 詞語/词语"),
        Ok((
            "",
            vec![
                Word {
                    trad: "單詞".to_string(),
                    simp: None
                },
                Word {
                    trad: "詞語".to_string(),
                    simp: Some("词语".to_string())
                }
            ]
        ))
    );
}

#[test]
fn test_parse_pinyin_list() {
    assert_eq!(parse_pinyin_list("dan1ci2"), Ok(("", vec!["dan1ci2"])));
    assert_eq!(
        parse_pinyin_list("dan1ci2; ci2yu3"),
        Ok(("", vec!["dan1ci2", "ci2yu3"]))
    );
}

#[test]
fn test_parse_reference() {
    assert_eq!(
        parse_reference("近義詞"),
        Ok((
            "",
            Reference {
                target_word: Word {
                    trad: "近義詞".to_string(),
                    simp: None
                },
                target_id: None
            }
        ))
    );
    assert_eq!(
        parse_reference("近義詞#D2"),
        Ok((
            "",
            Reference {
                target_word: Word {
                    trad: "近義詞".to_string(),
                    simp: None
                },
                target_id: Some(('D', 2))
            }
        ))
    );
}

// Full line parsers

#[test]
fn test_parse_word_line_full() {
    let line = "|t|傳統/传统";
    let expected = Ok((
        "",
        vec![WordTagGroup {
            tags: vec![Tag::Ascii('t')],
            words: vec![Word {
                trad: "傳統".to_string(),
                simp: Some("传统".to_string()),
            }],
        }],
    ));
    assert_eq!(parse_word_line(line), expected);
}

#[test]
fn test_parse_word_line_multiple_groups() {
    let line = "|t|單詞;詞語/词语|s|单词;词语";
    let expected = Ok((
        "",
        vec![
            WordTagGroup {
                tags: vec![Tag::Ascii('t')],
                words: vec![
                    Word {
                        trad: "單詞".to_string(),
                        simp: None,
                    },
                    Word {
                        trad: "詞語".to_string(),
                        simp: Some("词语".to_string()),
                    },
                ],
            },
            WordTagGroup {
                tags: vec![Tag::Ascii('s')],
                words: vec![
                    Word {
                        trad: "单词".to_string(),
                        simp: None,
                    },
                    Word {
                        trad: "词语".to_string(),
                        simp: None,
                    },
                ],
            },
        ],
    ));
    assert_eq!(parse_word_line(line), expected);
}

#[test]
fn test_parse_pinyin_line_full() {
    let line = "| |dan1ci2; ci2yu3";
    let expected = Ok((
        "",
        vec![PinyinTagGroup {
            tags: vec![],
            pinyins: vec!["dan1ci2".to_string(), "ci2yu3".to_string()],
        }],
    ));
    assert_eq!(parse_pinyin_line(line), expected);
}

#[test]
fn test_parse_class_line_full() {
    let line = " n.";
    let expected = Ok(("", "n.".to_string()));
    assert_eq!(parse_class_line(line), expected);
}

#[test]
fn test_parse_definition_line_full() {
    let line = "1|g #grammar| a word or a combination of words";
    let expected = Ok((
        "",
        DefinitionTag {
            tags: vec![Tag::Ascii('g'), Tag::Full("grammar".to_string())],
            id: 1,
            definition: "a word or a combination of words".to_string(),
        },
    ));
    assert_eq!(parse_definition_line(line), expected);
}

#[test]
fn test_parse_note_line_full() {
    let line = "1 This is a note.";
    let expected = Ok((
        "",
        Note {
            id: Some(1),
            is_link: false,
            txt: "This is a note.".to_string(),
        },
    ));
    assert_eq!(parse_note_line(line), expected);
}

#[test]
fn test_parse_note_link_line_full() {
    let line = "->2";
    let expected = Ok((
        "",
        Note {
            id: Some(2),
            is_link: true,
            txt: "".to_string(),
        },
    ));
    assert_eq!(parse_note_line(line), expected);
}

#[test]
fn test_parse_comment_line_full() {
    let line = " some metadata";
    let expected = Ok(("", "some metadata".to_string()));
    assert_eq!(parse_comment_line(line), expected);
}

#[test]
fn test_parse_reference_line_full() {
    let line = "=|s|同義詞/同义词; 相等詞#D5";
    let expected = Ok((
        "",
        vec![ReferenceTagGroup {
            ref_type: '=',
            tags: vec![Tag::Ascii('s')],
            references: vec![
                Reference {
                    target_word: Word {
                        trad: "同義詞".to_string(),
                        simp: Some("同义词".to_string()),
                    },
                    target_id: None,
                },
                Reference {
                    target_word: Word {
                        trad: "相等詞".to_string(),
                        simp: None,
                    },
                    target_id: Some(('D', 5)),
                },
            ],
        }],
    ));
    assert_eq!(parse_reference_line(line), expected);
}

// Top-level parse_line dispatcher
#[test]
fn test_parse_line_dispatcher() {
    assert_eq!(
        parse_line("W| |單詞"),
        Ok(DictLine::Word(vec![WordTagGroup {
            tags: vec![],
            words: vec![Word {
                trad: "單詞".to_string(),
                simp: None
            }]
        }]))
    );
    assert_eq!(
        parse_line("P| |dan1ci2"),
        Ok(DictLine::Pinyin(vec![PinyinTagGroup {
            tags: vec![],
            pinyins: vec!["dan1ci2".to_string()]
        }]))
    );
    assert_eq!(
        parse_line("C noun"),
        Ok(DictLine::Class("noun".to_string()))
    );
    assert_eq!(
        parse_line("D1| |a word"),
        Ok(DictLine::Definition(DefinitionTag {
            tags: vec![],
            id: 1,
            definition: "a word".to_string()
        }))
    );
    assert_eq!(
        parse_line("# a comment"),
        Ok(DictLine::Comment("a comment".to_string()))
    );
    assert_eq!(
        parse_line("N1 a note"),
        Ok(DictLine::Note(Note {
            id: Some(1),
            is_link: false,
            txt: "a note".to_string()
        }))
    );
    assert_eq!(
        parse_line("X=|s|同義詞"),
        Ok(DictLine::CrossReference(vec![ReferenceTagGroup {
            ref_type: '=',
            tags: vec![Tag::Ascii('s')],
            references: vec![Reference {
                target_word: Word {
                    trad: "同義詞".to_string(),
                    simp: None
                },
                target_id: None
            }]
        }]))
    );
    // Test invalid line
    assert!(parse_line("Z invalid line").is_err());
}
