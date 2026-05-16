use crate::config;
use crate::txt_parser::{DictLine, LineInfo, ParserIterator, Reference, SentenceWord, Tag, Tags};

use std::cmp::max;
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read};

/// Do a quick sanity check on the text file using the parser
pub fn quick_check(reader: &mut dyn Read) -> Vec<String> {
    let reader = BufReader::new(reader);
    let lines = reader.lines().map_while(io::Result::ok);
    let parser = ParserIterator::new(lines);
    const ERR_PREFIX: &str = "Quick check:";
    
    // track maximum definition ids in entries and references to later check if references are covered by entries
    let mut words_to_max_def_id: HashMap<String, u32> = HashMap::new();
    let mut refs_to_max_def_id: HashMap<String, u32> = HashMap::new();

    let mut add_ref = |r: &Reference| {
        let def_id = r.target_id.unwrap_or_default().1;
        for k in std::iter::once(&r.target_word.trad).chain(r.target_word.simp.as_ref()) {
            if let Some(id) = refs_to_max_def_id.get_mut(k) {
                *id = max(*id, def_id);
            } else {
                refs_to_max_def_id.insert(k.clone(), def_id);
            }
        }
    };

    let mut add_def_id = |t: &str, s: Option<&str>, def_id: u32| {
        for k in std::iter::once(t).chain(s) {
            if let Some(id) = words_to_max_def_id.get_mut(k) {
                *id = max(*id, def_id);
            } else {
                words_to_max_def_id.insert(k.to_owned(), def_id);
            }
        }
    };

    let format_line_error = |msg: &str, line: &LineInfo| -> String {
        if line.source_line_num > 1 {
            format!(
                "{ERR_PREFIX} {msg} in line {}-{}: {}",
                line.source_line_start,
                line.source_line_start + line.source_line_num,
                line.line
            )
        } else {
            format!(
                "{ERR_PREFIX} {msg} in line {}: {}",
                line.source_line_start, line.line
            )
        }
    };

    let check_tags = |tags: &Tags, line: &LineInfo, errors: &mut Vec<String>| {
        for t in tags {
            if let Tag::Ascii(t_a) = t {
                if config::tag_to_txt_ascii_common(*t_a).is_none() {
                    errors.push(format_line_error(&format!("unknown ascii tag {t_a}"), line));
                }
            }
        }
    };

    let mut errors: Vec<String> = vec![];
    let mut cur_trad = String::from("header");
    let mut cur_simp: Option<String> = None;
    let mut line_stack: Vec<char> = vec![];

    for line in parser.skip(2) {
        // Handle error gracefully and early to prevent rightward drift
        let parsed = match line.parsed_line {
            Ok(p) => p,
            Err(_) => {
                let mut msg = "parser_error".to_owned();
                if line.line.source_line_num > 1 {
                    msg.push_str(" (check indentation!)"); // spelling fixed
                }
                errors.push(format_line_error(&msg, &line.line));
                continue;
            }
        };

        // Match as an expression handles both assignment and branching
        let (cur_entry, allowed_parents) = match &parsed {
                    DictLine::Word(word_tag_groups) => {
                        if let Some(w) = word_tag_groups.first().and_then(|w| w.words.first()) {
                    cur_trad.clone_from(&w.trad);
                    cur_simp.clone_from(&w.simp);
                        } else {
                            // should never happen
                    cur_trad.clear();
                    cur_trad.push_str("unknown");
                            cur_simp = None;
                        }
                        for wt in word_tag_groups {
                    check_tags(&wt.tags, &line.line, &mut errors);
                        }
                ('W', "")
                    }
                    DictLine::Pinyin(pinyin_tag_groups) => {
                        for pt in pinyin_tag_groups {
                    check_tags(&pt.tags, &line.line, &mut errors);
                        }
                ('P', "PW")
                    }
            DictLine::Class(_) => ('C', "P"),
                    DictLine::Definition(definition_tag) => {
                        add_def_id(&cur_trad, cur_simp.as_deref(), definition_tag.id);
                check_tags(&definition_tag.tags, &line.line, &mut errors);
                ('D', "CD")
                    }
                    DictLine::CrossReference(reference_tag_groups) => {
                        if let Some(rt) = reference_tag_groups.first() {
                            if config::get_ref_type(rt.ref_type).is_none() {
                                errors.push(format_line_error(
                                    &format!("unknown reference type {}", rt.ref_type),
                                    &line.line,
                                ));
                            }
                        }
                        for rt in reference_tag_groups {
                            for r in &rt.references {
                                add_ref(r);
                            }
                    check_tags(&rt.tags, &line.line, &mut errors);
                        }
                ('X', "WD")
                    }
                    DictLine::Sentence(sentence_tag) => {
                check_tags(&sentence_tag.tags, &line.line, &mut errors);
                        for w in &sentence_tag.words {
                            if let SentenceWord::DictWord(dw) = w {
                                add_ref(&dw.0);
                                if let Some(r) = &dw.2 {
                                    add_ref(r);
                                }
                            }
                        }
                ('S', "D")
                    }
            DictLine::Note(_) => ('N', "WPDXS"),
            DictLine::Comment(_) => ('#', "WPDXS"),
        };

                if line.line.indentation > line_stack.len() {
                    // this branch is probably never triggered since the additional indentation would add it
                    // to the previous line, which should most likely have a parser error in that case
                    errors.push(format_line_error(&format!("wrong indendation"), &line.line));
                } else {
                    line_stack.truncate(line.line.indentation);
                    if !allowed_parents.is_empty()
                        && !allowed_parents.contains(line_stack.last().copied().unwrap_or_default())
                    {
                        errors.push(format_line_error(
                    &format!("allowed parent elements for {cur_entry} are {allowed_parents},"),
                            &line.line,
                        ));
                    }
                    line_stack.push(cur_entry);
                }
    }

    for (w, def_id) in refs_to_max_def_id {
        if let Some(max_def_id) = words_to_max_def_id.get(&w) {
            if def_id > *max_def_id {
                errors.push(format!("{ERR_PREFIX} referenced definition id {def_id} not found in dictionary for word: {w}"));
            }
        } else {
            errors.push(format!(
                "{ERR_PREFIX} referenced word not found in dictionary: {w}"
            ));
        }
    }

    errors
}