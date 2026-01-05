use crate::common::SqliteId;
use crate::db_read;
use itertools::Itertools;
use rusqlite::{Connection, Error as SqliteError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use toml::{self, Table};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LlmPromptTemplate {
    system_prompt: String,
    user_prompt_shared: String,
    expected_schema: String,
}

#[derive(Clone, Debug, Serialize)]
struct LlmPromptResult {
    template: LlmPromptTemplate,
    user_prompts: HashMap<String, String>, // id, prompt add after system and shared user prompt
}

fn read_prompt_templates(prompt_templates_path: &str) -> HashMap<String, LlmPromptTemplate> {
    let content = std::fs::read_to_string(prompt_templates_path).unwrap();
    toml::from_str(&content).unwrap()
}

fn get_formatted_tags(conn: &Connection, shared_id: SqliteId) -> rusqlite::Result<String> {
    let mut stmt = conn.prepare_cached(
            "SELECT t.ascii_symbol, t.tag, t.type FROM dict_shared_tag st JOIN dict_tag t ON st.tag_id = t.id WHERE st.for_shared_id = ?1",
        )?;
    let mut rows = stmt.query([shared_id])?;
    let mut ascii_tags = vec![];
    let mut full_tags = vec![];

    while let Some(row) = rows.next()? {
        let ascii_symbol: Option<String> = row.get(0)?;
        let tag: String = row.get(1)?;

        if let Some(symbol) = ascii_symbol {
            if !symbol.is_empty() {
                ascii_tags.push(symbol);
            }
        } else {
            full_tags.push(tag);
        }
    }

    // skip output of ascii tags for now
    if full_tags.is_empty() {
        Ok("".to_owned())
    } else {
        Ok(format!("[{}]", full_tags.iter().join(", ")))
    }
}

fn format_word_defs_for_synonym_prompt(conn: &Connection, trad: &str) -> Vec<String> {
    let exclude_tags = vec![db_read::Tag::Ascii('X'), db_read::Tag::Ascii('x')];
    let exclude_tag_ids: Vec<SqliteId> = db_read::get_tag_ids(conn, exclude_tags)
        .unwrap()
        .into_iter()
        .map(|t| t.unwrap())
        .collect();
    let word_ids = db_read::get_words(conn, None, Some(trad)).unwrap_or(vec![]);
    let word_ids = db_read::resolve_word_variants(conn, &word_ids).unwrap_or(vec![]);
    let mut formatted_word_defs = vec![];
    for word_id in word_ids {
        let word_defs =
            db_read::read_definitions_for_words(conn, &vec![word_id], &vec![], &exclude_tag_ids)
                .unwrap();
        let mut formatted_defs = String::new();
        for (def, def_group) in word_defs {
            let mut tag_str = get_formatted_tags(conn, def.shared_id).unwrap();
            if !tag_str.is_empty() {
                tag_str.push_str(" ");
            }
            let indent = "    ".repeat(def.nested_level);
            let def_str = format!(
                "{}- D{} {}{}\n",
                indent, def.ext_def_id, tag_str, def.definition
            );
            formatted_defs.push_str(&def_str);
        }
        if !formatted_defs.is_empty() {
            formatted_word_defs.push(formatted_defs)
        }
    }
    formatted_word_defs
}

/// Create a json with all the prompts, using data from the dictionary
/// The input is expected to be a json file with key-value pairs, the keys are reused in the output, the values should be pairs of traditional characters
pub fn create_match_synonym_definitions(
    conn: &Connection,
    prompt_templates_path: &str,
    prompt_input_path: &str,
    prompt_output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let prompt_template = read_prompt_templates(prompt_templates_path)
        .get("match_synonym_definitions")
        .unwrap()
        .clone();
    let mut prompt_out = LlmPromptResult {
        template: prompt_template,
        user_prompts: HashMap::new(),
    };

    let file = File::open(prompt_input_path)?;
    let reader = BufReader::new(file);
    let input_pairs: HashMap<String, (String, String)> = serde_json::from_reader(reader)?;

    // for each pair, create prompt which contains the definitions
    for (key, (word_a, word_b)) in input_pairs.iter() {
        let word_a_defs = format_word_defs_for_synonym_prompt(conn, word_a);
        let word_b_defs = format_word_defs_for_synonym_prompt(conn, word_b);
        let mut num_prompts = 0; // should usually be 1, we don't expect many words to have more than one result in the queries
        for word_a_def in &word_a_defs {
            for word_b_def in &word_b_defs {
                num_prompts += 1;
                let prompt_txt =
                    format!("word_a: {word_a}\n{word_a_def}\n\nword_b: {word_b}:\n{word_b_def}");
                let prompt_key = format!("{key}_{num_prompts}");
                prompt_out.user_prompts.insert(prompt_key, prompt_txt);
            }
        }
    }

    let file = File::create(prompt_output_path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &prompt_out)?;
    writer.flush()?;

    Ok(())
}
