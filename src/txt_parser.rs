/*
Format Description

- encoded in utf-8
- single space indentation creates a child element
- double space indentation relative to previous line continues previous line
- the first letter indicates the content of the line:
  * W: word
  * P: pronunciation in pinyin with tone marks, including 5 for neutral tone
  * C: class / part-of-speech
  * D: definition
  * X: cross-reference, the X is followed by another character indicating the type of reference (e.g. variant, measure word, collocation, ...)
  * #: comment (meta information etc. which should not be exposed to readers of the dictionary)
  * N: note, e.g. more detailed explanations
   * N->: direct reference to a note entry to avoid duplications in the text representation
- allowed child elements for each entry type:
  * W: P, X, #, N
  * P: P (one level), C, #, N
  * C: D
  * D: X, #, N
  * X: #, N
  * #: none
  * N: none

The grammar is more or less as follows:

entry_line = "W" word_tag_group {; word_tag_group}
pinyin_line = "P" pinyin_tag_group {; pinyin_tag_group}
class_line = "C|" ascii_word
definition_line = "D" id tags_full ...
cross_reference_line = "X" ascii_character reference_tag_group {; reference_tag_group}
comment_line = "#" ...
note_line = "N" id "|" ...
note_reference_line "N->" id ...

letter = A-Za-z | "-"
tag_letter = ascii character - "|" - whitespace
tag_word = letter {letter}
hanzi = chinese character | letter
hanzi_word = hanzi {hanzi}
id = number
tone = "1" | "2" | "3" | "4" | "5"
pinyin_syllable = letter {letter} tone
pinyin = pinyin_syllable {pinyin_syllable}
word_entry = hanzi_word [("／" | "/") hanzi_word]
reference = word_entry [letter id]
tags_ascii = "|" {tag_letter} "|"
tags_full = "|" {letter} {"#" tag_word} "|"
word_tag_group = tags_ascii word_entry {; word_entry}
pinyin_tag_group = tags_ascii pinyin {; pinyin}
reference_tag_group = tags_ascii reference

*/

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::{alphanumeric1, anychar, char, multispace0, none_of, u32},
    combinator::{all_consuming, map, opt, rest, value},
    error::Error,
    multi::{many0, many1, separated_list1},
    sequence::{delimited, pair, preceded, terminated},
};
use std::fmt;

#[derive(Debug, PartialEq)]
pub enum Tag {
    Ascii(char),
    Full(String),
}

pub type Tags = Vec<Tag>;

#[derive(Debug, PartialEq)]
pub struct PinyinTagGroup {
    pub tags: Tags,
    pub pinyins: Vec<String>,
}
#[derive(Debug, PartialEq, Clone)]
pub struct Word {
    pub trad: String,
    pub simp: Option<String>,
}
#[derive(Debug, PartialEq)]
pub struct WordTagGroup {
    pub tags: Tags,
    pub words: Vec<Word>,
}

#[derive(Debug, PartialEq)]
pub struct Reference {
    pub target_word: Word,
    pub target_id: Option<(char, u32)>,
}
#[derive(Debug, PartialEq)]
pub struct ReferenceTagGroup {
    pub ref_type: char,
    pub tags: Tags,
    pub references: Vec<Reference>,
}
#[derive(Debug, PartialEq)]
pub struct DefinitionTag {
    pub tags: Tags,
    pub id: u32,
    pub definition: String,
}

#[derive(Debug, PartialEq)]
pub struct Note {
    pub id: u32,
    pub is_link: bool,
    pub note: String,
}

#[derive(Debug, PartialEq)]
pub enum DictLine {
    Word(Vec<WordTagGroup>),
    Pinyin(Vec<PinyinTagGroup>),
    Class(String),
    Definition(DefinitionTag),
    CrossReference(Vec<ReferenceTagGroup>),
    Note(Note),
    Comment(String),
}

#[derive(Debug, PartialEq)]
pub struct ParsedLine {
    pub source_line_start: u32,
    pub source_line_num: u32,
    pub indentation: u32,
    pub line: Result<DictLine, String>, // TODO can probably done better with a proper error type
}

#[derive(Debug, PartialEq, Default)]
struct CurLine {
    line: String,
    source_line_start: u32,
    num_source_lines: u32,
}

#[derive(Debug, PartialEq, Default)]
pub struct ParserIterator<I>
where
    I: Iterator<Item = String>,
{
    inner: I,
    inner_line_count: u32,
    cur_line: Option<CurLine>,
    cur_indentation: usize,
}

impl<I> ParserIterator<I>
where
    I: Iterator<Item = String>,
{
    pub fn new(inner: I) -> Self {
        ParserIterator {
            inner: inner,
            inner_line_count: 0,
            cur_line: None,
            cur_indentation: 0,
        }
    }
}

impl<I> Iterator for ParserIterator<I>
where
    I: Iterator<Item = String>,
{
    type Item = ParsedLine;

    fn next(&mut self) -> Option<Self::Item> {
        // read next line
        loop {
            if let Some(line) = self.inner.next() {
                self.inner_line_count += 1;

                // count and remove leading spaces
                let line_content = line.trim_start_matches(' '); // TODO detect indentation tab vs space based on header comment
                if line_content.len() < 2 {
                    break;
                }
                let indentation = line.len() - line_content.len();
                // TODO check if line.trim() removes tabs, error in case tabs are used

                // check if current line belongs to previous line (indentation +2)
                if indentation > self.cur_indentation + 1 {
                    if let Some(ref mut cur_line) = self.cur_line {
                        cur_line.line.push_str("\n");
                        cur_line.line.push_str(&line[self.cur_indentation + 2..]);
                        cur_line.num_source_lines += 1;
                    }
                    // TODO else would be an error
                    continue;
                }
                // new line, get current line so that it can be returned after storing the new line
                let return_line = self.cur_line.take();
                let cur_indentation = self.cur_indentation;

                self.cur_indentation = indentation;
                self.cur_line = Some(CurLine {
                    line: line_content.to_owned(),
                    source_line_start: self.inner_line_count,
                    num_source_lines: 1,
                });

                if let Some(return_line) = return_line {
                    return Some(ParsedLine {
                        source_line_start: return_line.source_line_start,
                        source_line_num: return_line.num_source_lines,
                        indentation: cur_indentation as u32,
                        line: parse_line(&return_line.line),
                    });
                }
                continue;
            }
            break;
        }
        if let Some(return_line) = self.cur_line.take() {
            return Some(ParsedLine {
                source_line_start: return_line.source_line_start,
                source_line_num: return_line.num_source_lines,
                indentation: self.cur_indentation as u32,
                line: parse_line(&return_line.line),
            });
        }
        None
    }
}

fn parse_line(line: &str) -> Result<DictLine, String> {
    let line_parser = alt((
        map(preceded(char('W'), parse_word_line), DictLine::Word),
        map(preceded(char('P'), parse_pinyin_line), DictLine::Pinyin),
        map(preceded(tag("C|"), parse_class_line), DictLine::Class),
        map(
            preceded(char('D'), parse_definition_line),
            DictLine::Definition,
        ),
        map(
            preceded(char('X'), parse_reference_line),
            DictLine::CrossReference,
        ),
        map(preceded(char('N'), parse_note_line), DictLine::Note),
        map(preceded(char('#'), parse_comment_line), DictLine::Comment),
    ));
    match all_consuming(line_parser).parse(line) {
        Ok((_remainder, dict_line)) => Ok(dict_line),
        Err(e) => Err(e.to_string()),
    }
}

fn parse_tags(tag_str: &str) -> IResult<&str, Tags> {
    let parse_ascii_tag = delimited(multispace0, none_of("#|"), multispace0);
    let parse_ascii_tags = many0(parse_ascii_tag);
    let parse_full_tag = preceded(char('#'), take_while1(|c: char| c != '|' && c != '#'));
    let parse_full_tags = delimited(multispace0, many0(parse_full_tag), multispace0);
    let parse_ascii_full_tags = pair(parse_ascii_tags, parse_full_tags);

    let (remainder, tags) = delimited(
        delimited(multispace0, char('|'), multispace0),
        parse_ascii_full_tags,
        delimited(multispace0, char('|'), multispace0),
    )
    .parse(tag_str)?;
    let mut all_tags: Vec<Tag> = tags.0.iter().map(|c| Tag::Ascii(*c)).collect();
    let full_tags: Vec<Tag> = tags
        .1
        .iter()
        .map(|s| Tag::Full(s.trim().to_owned()))
        .collect();
    all_tags.extend(full_tags);
    Ok((remainder, all_tags))
}

fn parse_word(word_str: &str) -> IResult<&str, Word> {
    let simp_trad = delimited(
        multispace0::<&str, _>,
        take_while1(|c: char| !"|#;/／".contains(c)),
        multispace0,
    );
    let simp = delimited(
        multispace0,
        take_while1(|c: char| !"#|;".contains(c)),
        multispace0,
    );

    map(
        pair(simp_trad, opt(preceded(alt((char('/'), char('／'))), simp))),
        |word_pair| Word {
            trad: word_pair.0.trim().to_owned(),
            simp: word_pair.1.map(|s| s.trim().to_owned()),
        },
    )
    .parse(word_str)
}

fn parse_word_list(word_list: &str) -> IResult<&str, Vec<Word>> {
    separated_list1(char(';'), parse_word).parse(word_list)
}

fn parse_word_tag_group(tag_group_str: &str) -> IResult<&str, WordTagGroup> {
    map(pair(parse_tags, parse_word_list), |tag_group| {
        WordTagGroup {
            tags: tag_group.0,
            words: tag_group.1,
        }
    })
    .parse(tag_group_str)
}

fn parse_word_line(word_line: &str) -> IResult<&str, Vec<WordTagGroup>> {
    all_consuming(many1(parse_word_tag_group)).parse(word_line)
}

fn parse_pinyin_list(pinyin_list: &str) -> IResult<&str, Vec<&str>> {
    let pinyin_parser = delimited(multispace0, take_while1(|c: char| !"|;".contains(c)), multispace0);
    separated_list1(char(';'), pinyin_parser).parse(pinyin_list)
}

fn parse_pinyin_tag_group(tag_group_str: &str) -> IResult<&str, PinyinTagGroup> {
    let (remainder, tag_group) = pair(parse_tags, parse_pinyin_list).parse(tag_group_str)?;
    let tags = tag_group.0;
    let pinyins = tag_group.1.iter().map(|s| s.to_string()).collect();
    Ok((remainder, PinyinTagGroup { tags, pinyins }))
}

fn parse_pinyin_line(pinyin_line: &str) -> IResult<&str, Vec<PinyinTagGroup>> {
    all_consuming(many1(parse_pinyin_tag_group)).parse(pinyin_line)
}

fn parse_class_line(class_line: &str) -> IResult<&str, String> {
    map(all_consuming(preceded(multispace0, rest)), |c: &str| {
        c.to_owned()
    })
    .parse(class_line)
}

fn parse_definition_line(definition_line: &str) -> IResult<&str, DefinitionTag> {
    let (remainder, (id, tags, definition)) =
        all_consuming((u32, parse_tags, rest)).parse(definition_line)?;
    Ok((
        remainder,
        DefinitionTag {
            tags,
            id,
            definition: definition.to_owned(),
        },
    ))
}

fn parse_comment_line(comment_line: &str) -> IResult<&str, String> {
    let (remainder, comment) = all_consuming(preceded(multispace0, rest)).parse(comment_line)?;
    Ok((remainder, comment.to_owned()))
}

fn parse_note_line(note_line: &str) -> IResult<&str, Note> {
    let (remainder, (is_link, id, note)) = all_consuming((
        opt(value(true, tag("->"))),
        u32,
        preceded(opt(delimited(multispace0, char('|'), multispace0)), rest),
    ))
    .parse(note_line)?;
    Ok((
        remainder,
        Note {
            id,
            is_link: is_link.is_some(),
            note: note.to_owned(),
        },
    ))
}

fn parse_reference(reference: &str) -> IResult<&str, Reference> {
    let (remainder, (word, id)) = pair(
        parse_word,
        opt(preceded(tag("#D"), terminated(u32, multispace0))),
    )
    .parse(reference)?;

    Ok((
        remainder,
        Reference {
            target_word: word,
            target_id: id.map(|i| ('D', i)),
        },
    ))
}

fn parse_reference_tag_group(tag_group_str: &str) -> IResult<&str, ReferenceTagGroup> {
    let ref_list_parse = separated_list1(char(';'), parse_reference);
    let (remainder, (ref_type, tags, references)) =
        (anychar, parse_tags, ref_list_parse).parse(tag_group_str)?;
    Ok((
        remainder,
        ReferenceTagGroup {
            ref_type,
            tags,
            references,
        },
    ))
}

fn parse_reference_line(reference_line: &str) -> IResult<&str, Vec<ReferenceTagGroup>> {
    all_consuming(many1(parse_reference_tag_group)).parse(reference_line)
}

#[cfg(test)]
mod tests;
