use crate::config;
use crate::txt_parser::{DictLine, LineInfo, ParserIterator, Reference, SentenceWord, Tag, Tags};

use std::cmp::max;
use std::collections::HashMap;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;

pub fn quick_check(reader: &mut dyn Read) -> Vec<String> {
    let reader = BufReader::new(reader);
    let lines = reader.lines().map_while(io::Result::ok);
    let parser = ParserIterator::new(lines.into_iter());
    const ERR_PREFIX: &str = "Quick check:";
    // track maximum definition ids in entries and references to later check if references are covered by entries
    let mut words_to_max_def_id: HashMap<String, u32> = HashMap::new();
    let mut refs_to_max_def_id: HashMap<String, u32> = HashMap::new();

    let mut add_ref = |r: &Reference| {
        let def_id = r.target_id.unwrap_or_default().1;
        for k in Some(&r.target_word.trad)
            .into_iter()
            .chain(r.target_word.simp.as_ref().into_iter())
        {
            let max_def_id = max(def_id, refs_to_max_def_id.get(k).copied().unwrap_or(0));
            refs_to_max_def_id.insert(k.to_owned(), max_def_id);
        }
    };

    let mut add_def_id = |t: &str, s: Option<&str>, def_id: u32| {
        for k in Some(t).into_iter().chain(s.into_iter()) {
            let max_def_id = max(def_id, words_to_max_def_id.get(k).copied().unwrap_or(0));
            words_to_max_def_id.insert(k.to_owned(), max_def_id);
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

    let check_tags = |tags: &Tags, line: &LineInfo| -> Vec<String> {
        let mut e = vec![];
        for t in tags {
            if let Tag::Ascii(t_a) = t {
                if config::tag_to_txt_ascii_common(*t_a).is_none() {
                    e.push(format_line_error(&format!("unknown ascii tag {t_a}"), line));
                }
            }
        }
        e
    };

    let mut errors: Vec<String> = vec![];
    let mut cur_trad = "header".to_owned();
    let mut cur_simp: Option<String> = None;
    let mut line_stack: Vec<char> = vec![];

    for line in parser.skip(2) {
        // skip the first two entries (header comment and note reference)
        match line.parsed_line {
            Ok(parsed) => {
                let mut cur_entry = ' ';
                let mut allowed_parents: &str = "";
                match &parsed {
                    DictLine::Word(word_tag_groups) => {
                        if let Some(w) = word_tag_groups.first().and_then(|w| w.words.first()) {
                            cur_trad = w.trad.clone();
                            cur_simp = w.simp.clone();
                        } else {
                            // should never happen
                            cur_trad = "unknown".to_owned();
                            cur_simp = None;
                        }
                        cur_entry = 'W';
                        allowed_parents = "";
                        for wt in word_tag_groups {
                            errors.append(&mut check_tags(&wt.tags, &line.line));
                        }
                    }
                    DictLine::Pinyin(pinyin_tag_groups) => {
                        cur_entry = 'P';
                        allowed_parents = "PW";
                        for pt in pinyin_tag_groups {
                            errors.append(&mut check_tags(&pt.tags, &line.line));
                        }
                    }
                    DictLine::Class(_) => {
                        cur_entry = 'C';
                        allowed_parents = "P";
                    }
                    DictLine::Definition(definition_tag) => {
                        cur_entry = 'D';
                        allowed_parents = "CD";
                        add_def_id(&cur_trad, cur_simp.as_deref(), definition_tag.id);
                        errors.append(&mut check_tags(&definition_tag.tags, &line.line));
                    }
                    DictLine::CrossReference(reference_tag_groups) => {
                        cur_entry = 'X';
                        allowed_parents = "WD";
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
                            errors.append(&mut check_tags(&rt.tags, &line.line));
                        }
                    }
                    DictLine::Sentence(sentence_tag) => {
                        cur_entry = 'S';
                        allowed_parents = "D";
                        errors.append(&mut check_tags(&sentence_tag.tags, &line.line));
                        for w in &sentence_tag.words {
                            if let SentenceWord::DictWord(dw) = w {
                                add_ref(&dw.0);
                                if let Some(r) = &dw.2 {
                                    add_ref(r);
                                }
                            }
                        }
                    }
                    DictLine::Note(note) => {
                        cur_entry = 'N';
                        allowed_parents = "WPDXS";
                    }
                    DictLine::Comment(_) => {
                        cur_entry = '#';
                        allowed_parents = "WPDXS";
                    }
                }
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
                            &format!(
                                "allowed parent elements for {cur_entry} are {allowed_parents},"
                            ),
                            &line.line,
                        ));
                    }
                    line_stack.push(cur_entry);
                }
            }
            Err(_e) => {
                let mut msg = "parser_error".to_owned();
                if line.line.source_line_num > 1 {
                    msg.push_str(" (check indendation!)")
                }
                errors.push(format_line_error(&msg, &line.line));
            }
        }
    }

    // check if reference targets exist in dictionary
    for (w, def_id) in refs_to_max_def_id.into_iter() {
        if let Some(max_def_id) = words_to_max_def_id.get(&w) {
            if def_id > *max_def_id {
                errors.push(format!("{ERR_PREFIX} referenced definition id {def_id} not found in dictionary for word: {w}"))
            }
        } else {
            errors.push(format!(
                "{ERR_PREFIX} referenced word not found in dictionary: {w}"
            ))
        }
    }

    errors
}
