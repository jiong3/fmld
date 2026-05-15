use nom::{
    AsChar, Err, IResult, Parser, branch::alt, bytes::complete::{tag, take_while1}, error::{Error, ErrorKind}, character::complete::{anychar, char, newline, none_of, space0, u32}, combinator::{all_consuming, fail, map, opt, rest, success, value}, multi::{many0, many1, separated_list1}, sequence::{delimited, pair, preceded, separated_pair}
};

use crate::common;
use std::fmt;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Tag {
    Ascii(char),
    Full(String),
}

pub type Tags = Vec<Tag>;

#[derive(Debug, PartialEq, Eq)]
pub struct PinyinTagGroup {
    pub tags: Tags,
    pub pinyins: Vec<String>,
}
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Word {
    pub trad: String,
    pub simp: Option<String>,
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            common::format_word_def(&self.trad, self.simp.as_ref().unwrap_or(&self.trad), None)
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct WordTagGroup {
    pub tags: Tags,
    pub words: Vec<Word>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Reference {
    pub target_word: Word,
    pub target_id: Option<(char, u32)>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ReferenceTagGroup {
    pub ref_type: char,
    pub tags: Tags,
    pub references: Vec<Reference>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct DefinitionTag {
    pub tags: Tags,
    pub id: u32,
    pub definition: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Note {
    pub id: Option<u32>,
    pub is_link: bool,
    pub txt: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SentenceWord {
    DictWord((Reference, bool, Option<Reference>)), // bool indicates whether reference 1 is part of another word
    AsciiWord(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct SentenceTag {
    pub tags: Tags,
    pub id: u32,
    pub words: Vec<SentenceWord>,
    pub translation: String,
}

#[derive(Debug, PartialEq)]
pub enum DictLine {
    Word(Vec<WordTagGroup>),
    Pinyin(Vec<PinyinTagGroup>),
    Class(String),
    Definition(DefinitionTag),
    CrossReference(Vec<ReferenceTagGroup>),
    Sentence(SentenceTag),
    Note(Note),
    Comment(String),
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct LineInfo {
    pub source_line_start: u32,
    pub source_line_num: u32,
    pub indentation: usize,
    pub line: String,
}

#[derive(Debug, PartialEq)]
pub struct ParsedLine {
    pub line: LineInfo,
    pub parsed_line: Result<DictLine, ()>,
}

#[derive(Debug, PartialEq, Eq, Default)]
pub struct ParserIterator<I>
where
    I: Iterator<Item = String>,
{
    inner: I,
    inner_line_count: u32,
    cur_line: Option<LineInfo>,
}

impl<I> ParserIterator<I>
where
    I: Iterator<Item = String>,
{
    pub const fn new(inner: I) -> Self {
        Self {
            inner,
            inner_line_count: 0,
            cur_line: None,
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

                // count and remove leading spaces or tabs, currently no check if they are mixed
                let line_content = line.trim_start();
                // skip empty lines unless they belong to a current line
                if self.cur_line.is_none() && line_content.len() < 2 {
                    break;
                }
                let indentation = line.len() - line_content.len();

                // check if current line belongs to previous line (indentation +2)
                if let Some(ref mut cur_line) = self.cur_line {
                    if indentation > cur_line.indentation + 1 {
                        cur_line.line.push('\n');
                        cur_line.line.push_str(&line[cur_line.indentation + 2..]);
                        cur_line.source_line_num += 1;
                        continue;
                    }
                    // new line with no content and no indentation, still belongs to current line
                    if line_content.is_empty() {
                        cur_line.line.push('\n');
                        cur_line.source_line_num += 1;
                        continue;
                    }
                }
                // new line, get current line so that it can be returned after storing the new line
                let return_line = self.cur_line.take();

                self.cur_line = Some(LineInfo {
                    line: line_content.to_owned(),
                    source_line_start: self.inner_line_count,
                    source_line_num: 1,
                    indentation,
                });

                if let Some(return_line) = return_line {
                    let parsed_line = parse_line(&return_line.line);
                    return Some(ParsedLine {
                        line: return_line,
                        parsed_line,
                    });
                }
                continue;
            }
            break;
        }
        if let Some(return_line) = self.cur_line.take() {
            let parsed_line = parse_line(&return_line.line);
            return Some(ParsedLine {
                line: return_line,
                parsed_line,
            });
        }
        None
    }
}

fn parse_line(line: &str) -> Result<DictLine, ()> {
    let line_parser = alt((
        map(preceded(char('W'), parse_word_line), DictLine::Word),
        map(preceded(char('P'), parse_pinyin_line), DictLine::Pinyin),
        map(preceded(char('C'), parse_class_line), DictLine::Class),
        map(
            preceded(char('D'), parse_definition_line),
            DictLine::Definition,
        ),
        map(
            preceded(char('X'), parse_reference_line),
            DictLine::CrossReference,
        ),
        map(preceded(char('S'), parse_sentence_line), DictLine::Sentence),
        map(preceded(char('N'), parse_note_line), DictLine::Note),
        map(preceded(char('#'), parse_comment_line), DictLine::Comment),
    ));
    let line = line.trim_end_matches(&[';', ' ']);
    match all_consuming(line_parser).parse(line) {
        Ok((_remainder, dict_line)) => Ok(dict_line),
        Err(_e) => Err(()),
    }
}

fn parse_tags(tag_str: &str) -> IResult<&str, Tags> {
    let parse_ascii_tag = delimited(space0, none_of("#|"), space0);
    let parse_ascii_tags = many0(parse_ascii_tag);
    let parse_full_tag = delimited(
        space0,
        preceded(
            char('#'),
            take_while1(|c: char| c.is_ascii_alphanumeric() || c == '-'),
        ),
        space0,
    );
    let parse_full_tags = many0(parse_full_tag);
    let parse_ascii_full_tags = pair(parse_ascii_tags, parse_full_tags);

    let (remainder, tags) = delimited(
        delimited(space0, char('|'), space0),
        parse_ascii_full_tags,
        delimited(space0, char('|'), space0),
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
    let simp_trad = take_while1(|c: char| !"|#;/／< \n\t\r".contains(c));
    let simp = take_while1(|c: char| !"#|;< \n\t\r".contains(c));

    map(
        pair(
            preceded(space0::<&str, _>, simp_trad),
            opt(preceded(
                delimited(space0, alt((char('/'), char('／'))), space0),
                simp,
            )),
        ),
        |word_pair| Word {
            trad: word_pair.0.to_owned(),
            simp: word_pair.1.map(std::borrow::ToOwned::to_owned),
        },
    )
    .parse(word_str)
}

fn parse_word_list(word_list: &str) -> IResult<&str, Vec<Word>> {
    separated_list1(delimited(space0, char(';'), space0), parse_word).parse(word_list)
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
    let pinyin_parser = delimited(
        space0,
        take_while1(|c: char| c.is_ascii_alphanumeric() || "ê. -,".contains(c)),
        space0,
    );
    separated_list1(char(';'), pinyin_parser).parse(pinyin_list)
}

fn parse_pinyin_tag_group(tag_group_str: &str) -> IResult<&str, PinyinTagGroup> {
    let (remainder, tag_group) = pair(parse_tags, parse_pinyin_list).parse(tag_group_str)?;
    let tags = tag_group.0;
    let pinyins = tag_group.1.iter().map(|s| s.trim().to_string()).collect();
    Ok((remainder, PinyinTagGroup { tags, pinyins }))
}

fn parse_pinyin_line(pinyin_line: &str) -> IResult<&str, Vec<PinyinTagGroup>> {
    all_consuming(many1(parse_pinyin_tag_group)).parse(pinyin_line)
}

fn parse_class_line(class_line: &str) -> IResult<&str, String> {
    map(all_consuming(preceded(space0, rest)), |c: &str| {
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
    let (remainder, comment) = all_consuming(preceded(space0, rest)).parse(comment_line)?;
    Ok((remainder, comment.to_owned()))
}

fn parse_note_line(note_line: &str) -> IResult<&str, Note> {
    let (remainder, (is_link, id, note)) = all_consuming(
        // reference with note id or note with id or ? as a placeholder for new ids
        alt((
            (opt(value(true, tag("->"))), u32, preceded(space0, rest)),
            (
                opt(fail()),
                alt((u32, value(0, char('?')))),
                preceded(space0, rest),
            ),
        )),
    )
    .parse(note_line)?;
    Ok((
        remainder,
        Note {
            id: (id > 0).then_some(id),
            is_link: is_link.is_some(),
            txt: note.to_owned(),
        },
    ))
}

fn parse_reference(reference: &str) -> IResult<&str, Reference> {
    let (remainder, (word, id)) =
        pair(parse_word, opt(preceded(tag("#D"), u32))).parse(reference)?;

    Ok((
        remainder,
        Reference {
            target_word: word,
            target_id: id.map(|i| ('D', i)),
        },
    ))
}

fn parse_reference_tag_group(
    tag_group_str: &str,
    ref_type: char,
) -> IResult<&str, ReferenceTagGroup> {
    let ref_list_parse = separated_list1(delimited(space0, char(';'), space0), parse_reference);
    let (remainder, (tags, references)) = (parse_tags, ref_list_parse).parse(tag_group_str)?;

    Ok((
        remainder,
        ReferenceTagGroup {
            ref_type, // passed-in ref_type
            tags,
            references,
        },
    ))
}

fn parse_reference_line(reference_line: &str) -> IResult<&str, Vec<ReferenceTagGroup>> {
    let (input, ref_type) = anychar(reference_line)?;
    all_consuming(many1(|i| parse_reference_tag_group(i, ref_type))).parse(input)
}

fn parse_sentence_word_ascii(sentence_word: &str) -> IResult<&str, SentenceWord> {
    let (r, w) = take_while1(|c: char| c.is_ascii() && !c.is_newline()).parse(sentence_word)?;
    if w.ends_with(' ') {
        // leave the trailing space separator
        Ok((&sentence_word[w.len() - 1..], SentenceWord::AsciiWord(w.strip_suffix(' ').expect("checked above").to_owned())))
    } else if r.chars().next().is_none_or(|c| c.is_newline()) {
        // ascii word at the end of the line
        Ok((r, SentenceWord::AsciiWord(w.to_owned())))
    } else {
        // not an ascii-only word since it's not separated by a space or at the end of the line
        Err(Err::Error(Error::new(sentence_word, ErrorKind::Fail)))
    }
}

fn parse_sentence_word_hanzi(sentence_word: &str) -> IResult<&str, SentenceWord> {
    // complete_word is only relevant for separated words
    let (r, (word, (is_part_of, complete_word))) =
        (parse_reference,
            alt((
                (value(true, tag("<#D")), opt(fail())),
                (value(true, char('<')), opt(parse_reference)),
                (success(false), opt(fail())),
            ))
        ).parse(sentence_word)?;
    Ok((r, SentenceWord::DictWord((word, is_part_of, complete_word))))
}

fn parse_sentence_word(sentence_word: &str) -> IResult<&str, SentenceWord> {
    alt((
        parse_sentence_word_ascii,
        parse_sentence_word_hanzi,
    ))
    .parse(sentence_word)
}

fn parse_sentence_words(sentence_words: &str) -> IResult<&str, Vec<SentenceWord>> {
    separated_list1(char(' '), parse_sentence_word).parse(sentence_words)
}

fn parse_sentence(sentence: &str) -> IResult<&str, (Vec<SentenceWord>, String)> {
    let (remainder, (words, translation)) =
        separated_pair(parse_sentence_words, newline, rest).parse(sentence)?;
    Ok((remainder, (words, translation.to_owned())))
}

fn parse_sentence_line(sentence_line: &str) -> IResult<&str, SentenceTag> {
    let (remainder, (id, tags, (words, translation))) =
        (u32, parse_tags, parse_sentence).parse(sentence_line)?;
    Ok((
        remainder,
        SentenceTag {
            tags,
            id,
            words,
            translation,
        },
    ))
}

#[cfg(test)]
mod tests;
