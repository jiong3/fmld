use crate::common::{self, SqliteId};
use crate::db_read::{self, WordEntry};
use itertools::Itertools;
use rusqlite::{Connection, Error as SqliteError};
use serde::{Deserialize, Serialize};
use std::cmp::{max, min};
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
    user_prompts_meta: HashMap<String, Vec<String>>,
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

fn format_word_defs_for_word_id(
    conn: &Connection,
    word_id: SqliteId,
    exclude_tag_ids: &Vec<SqliteId>,
) -> (WordEntry, String) {
    let word = db_read::read_word(conn, word_id).unwrap();
    let word_defs =
        db_read::read_definitions_for_words(conn, &vec![word_id], &vec![], exclude_tag_ids)
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
    (word, formatted_defs)
}

fn format_word_defs_for_prompt_trad_word(
    conn: &Connection,
    trad: &str,
) -> Vec<(WordEntry, String)> {
    let exclude_tags = vec![db_read::Tag::Ascii('X'), db_read::Tag::Ascii('x')];
    let exclude_tag_ids: Vec<SqliteId> = db_read::get_tag_ids(conn, exclude_tags)
        .unwrap()
        .into_iter()
        .map(|t| t.unwrap())
        .collect();
    let word_ids = db_read::get_words(conn, None, Some(trad)).unwrap_or(vec![]);
    let mut word_ids = db_read::resolve_word_variants(conn, &word_ids).unwrap_or(vec![]);
    word_ids.sort();
    word_ids.dedup();
    let mut formatted_word_defs = vec![];
    for word_id in word_ids {
        let (word, formatted_defs) = format_word_defs_for_word_id(conn, word_id, &exclude_tag_ids);
        if !formatted_defs.is_empty() {
            formatted_word_defs.push((word, formatted_defs))
        }
    }
    formatted_word_defs
}

/// Create a json with all the prompts, using data from the dictionary
/// The input is expected to be a json file with key-value pairs, the keys are reused in the output, the values should be pairs of traditional characters
pub fn create_prompts_match_definitions(
    conn: &Connection,
    prompt_templates_path: &str,
    prompt_template_name: &str,
    prompt_input_path: &str,
    prompt_output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let prompt_template = read_prompt_templates(prompt_templates_path)
        .get(prompt_template_name)
        .unwrap()
        .clone();
    let mut prompt_out = LlmPromptResult {
        template: prompt_template,
        user_prompts: HashMap::new(),
        user_prompts_meta: HashMap::new(),
    };

    let file = File::open(prompt_input_path)?;
    let reader = BufReader::new(file);
    let input_pairs: HashMap<String, (String, String)> = serde_json::from_reader(reader)?;

    // for each pair, create prompt which contains the definitions
    for (key, (word_a, word_b)) in input_pairs.iter() {
        let word_a_defs = format_word_defs_for_prompt_trad_word(conn, word_a);
        let word_b_defs = format_word_defs_for_prompt_trad_word(conn, word_b);
        let mut num_prompts = 0; // should usually be 1, we don't expect many words to have more than one result in the queries
        for (word_a, word_a_def) in &word_a_defs {
            let word_a_str = common::format_word_def(&word_a.trad, &word_a.simp, None);
            for (word_b, word_b_def) in &word_b_defs {
                let word_b_str = common::format_word_def(&word_b.trad, &word_b.simp, None);
                num_prompts += 1;
                let prompt_txt = format!(
                    "word_a: {word_a_str}\n{word_a_def}\nword_b: {word_b_str}\n{word_b_def}"
                );
                let prompt_key = format!(
                    "{key}_{num_prompts};{};{};{};{}",
                    word_a.trad, word_a.simp, word_b.trad, word_b.simp
                );
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

pub fn get_subtitle_snippets(sub_db_conn: &Connection, trad: &str) -> Result<Vec<String>, SqliteError> {
    let mut stmt = sub_db_conn.prepare_cached(
            r#"
            SELECT 
                L1.line_trad_raw || CHAR(10) || L2.line_trad_raw || CHAR(10) || L3.line_trad_raw AS chunk
            FROM line AS L2
            -- 1. Use FTS to quickly find the 'center' line (Line 2)
            JOIN fts_idx ON fts_idx.rowid = L2.id
            -- 2. Join the previous line (Line 1) ensuring same Show ID
            JOIN line AS L1 ON L1.id = (L2.id - 1) AND L1.show_id = L2.show_id
            -- 3. Join the next line (Line 3) ensuring same Show ID
            JOIN line AS L3 ON L3.id = (L2.id + 1) AND L3.show_id = L2.show_id
            -- 4. Join the Show table to filter genres
            JOIN "show" S ON L2.show_id = S.id
            WHERE 
                fts_idx.words_trad MATCH ?1
                -- 5. Pause constraint: Sum of Line 2 and Line 3 pause < 10
                AND (L2.pause + L3.pause) < 10
                AND L1.line_trad_raw IS NOT NULL
                AND L2.line_trad_raw IS NOT NULL
                AND L3.line_trad_raw IS NOT NULL
                -- 6. Genre exclusion logic
                AND NOT EXISTS (
                    SELECT 1 
                    FROM json_each(S.info, '$.douban_genres')
                    WHERE value IN ('Wuxia', 'Costume')
                )
                AND NOT S.name in ('QingChunYouNi3')
            ORDER BY L1.id;
            "#,
        )?;
    let mut rows = stmt.query([trad])?;

    let mut snippets: Vec<String> = vec![];

    while let Some(row) = rows.next()? {
        snippets.push(row.get(0)?);
    }
    Ok(snippets)
}

pub fn create_prompts_find_collocations(
    conn: &Connection,
    prompt_templates_path: &str,
    prompt_template_name: &str,
    prompt_output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let prompt_template = read_prompt_templates(prompt_templates_path)
        .get(prompt_template_name)
        .unwrap()
        .clone();
    let mut prompt_out = LlmPromptResult {
        template: prompt_template,
        user_prompts: HashMap::new(),
        user_prompts_meta: HashMap::new(),
    };
    let relevant_classes = vec!["noun", "verb", "verb-object", "adj", "adv", "phrase"];

    let words = db_read::read_words_with_classes(conn, relevant_classes).unwrap();
    let mut trads: Vec<&str> = vec![];
    for word in &words {
        if word.trad == "%" {
            break;
        }
        trads.push(&word.trad);
    }
    // chunk words into groups of 50
    let mut chunk_i = 0;
    for trad_chunk in trads.chunks(50) {
        chunk_i += 1;
        let mut prompt_txt = trad_chunk.join(":\n");
        prompt_txt.push_str(":\n");
        let prompt_key = format!("chunk_{chunk_i}");
        prompt_out.user_prompts.insert(prompt_key, prompt_txt);
    }
    // request collocations again, with a different order
    trads.sort();
    for trad_chunk in trads.chunks(50) {
        chunk_i += 1;
        let mut prompt_txt = trad_chunk.join(":\n");
        prompt_txt.push_str(":\n");
        let prompt_key = format!("chunk_{chunk_i}");
        prompt_out.user_prompts.insert(prompt_key, prompt_txt);
    }

    let file = File::create(prompt_output_path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &prompt_out)?;
    writer.flush()?;
    Ok(())
}

/// Create a json with all the prompts, using data from the dictionary
/// The input is expected to be a json file with key-value pairs, the keys are a word and the values a list of collocations for the word
pub fn create_prompts_match_collocation_definitions(
    conn: &Connection,
    prompt_templates_path: &str,
    prompt_template_name: &str,
    prompt_input_path: &str,
    prompt_output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let prompt_template = read_prompt_templates(prompt_templates_path)
        .get(prompt_template_name)
        .unwrap()
        .clone();
    let mut prompt_out = LlmPromptResult {
        template: prompt_template,
        user_prompts: HashMap::new(),
        user_prompts_meta: HashMap::new(),
    };

    let file = File::open(prompt_input_path)?;
    let reader = BufReader::new(file);
    let word_collocs: HashMap<String, Vec<String>> = serde_json::from_reader(reader)?;

    // for each pair, create prompt which contains the definitions
    for (word_trad, collocs) in word_collocs.iter() {
        let word_defs = format_word_defs_for_prompt_trad_word(conn, word_trad);

        let mut num_prompts = 0; // should usually be 1, we don't expect many words to have more than one result in the queries
        for (word, word_def) in &word_defs {
            num_prompts += 1;
            let prompt_key = format!("{word_trad}_{}_{num_prompts}", collocs.join(";"));
            let word_str = common::format_word_def(&word.trad, &word.simp, None);

            let mut prompt_txt = format!("word:\n\n{word_str}\n{word_def}\n\ncollocations:\n\n");
            let mut prompt_meta = vec![format!("{};{}", word.trad, word.simp)];

            for colloc in collocs {
                let colloc_defs = format_word_defs_for_prompt_trad_word(conn, colloc);
                if colloc_defs.is_empty() {
                    println!("no definitions for {colloc}");
                }
                for (col_word, col_def) in &colloc_defs {
                    let col_str = common::format_word_def(&col_word.trad, &col_word.simp, None);
                    let col_def = format!("{col_str}\n{col_def}\n");
                    prompt_txt.push_str(&col_def);
                    prompt_meta.push(format!("{};{}", col_word.trad, col_word.simp));
                }
            }

            prompt_out
                .user_prompts
                .insert(prompt_key.clone(), prompt_txt);
            prompt_out.user_prompts_meta.insert(prompt_key, prompt_meta);
        }
    }

    let file = File::create(prompt_output_path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &prompt_out)?;
    writer.flush()?;

    Ok(())
}

pub fn create_prompts_identify_definitions_in_subtitles(
    conn: &Connection,
    sub_db_conn: &Connection,
    prompt_templates_path: &str,
    prompt_template_name: &str,
    prompt_output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let max_snippets = 100;
    let prompt_template = read_prompt_templates(prompt_templates_path)
        .get(prompt_template_name)
        .unwrap()
        .clone();
    let mut prompt_out = LlmPromptResult {
        template: prompt_template,
        user_prompts: HashMap::new(),
        user_prompts_meta: HashMap::new(),
    };
    
    // get all words in the learners section
    let mut words_no_snippets = vec![];
    let words = db_read::read_words(conn).unwrap();

    for word in &words {
        if word.trad == "%" {
            break;
        }
        let word_str = common::format_word_def(&word.trad, &word.simp, None);
        let (_, word_defs) = format_word_defs_for_word_id(conn, word.id, &vec![]);
        let num_defs = word_defs.chars().filter(|c| *c == '\n').count();
        let mut prompt_txt =
            format!("dictionary entry:\n{word_str}\n{word_defs}\n\nsubtitle excerpts:\n\n");
        let prompt_meta = vec![format!("{};{}", word.trad, word.simp)];

        // try to get subtitle snippets
        let snippets = get_subtitle_snippets(sub_db_conn, &word.trad)?;
        if snippets.is_empty() {
            words_no_snippets.push(&word.trad);
            continue;
        }
        let mut snippet_count = 0;
        let step_size = max(1, snippets.len() / (min(num_defs * 10, max_snippets)));
        for snippet in snippets.iter().step_by(step_size) {
            snippet_count += 1;

            // TODO exclude snippets which contain multiple occurances
            let snippet_str = format!("{snippet_count}\n{snippet}\n\n");
            prompt_txt.push_str(&snippet_str);
        }

        prompt_out.user_prompts.insert(word_str.clone(), prompt_txt);
        prompt_out.user_prompts_meta.insert(word_str, prompt_meta);
    }

    let file = File::create(prompt_output_path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &prompt_out)?;
    writer.flush()?;


    for trad in words_no_snippets {
        println!("no snippets for: {trad}");
    }
    Ok(())
}

pub fn create_prompts_identify_stand_alone_definitions_of_chars(
    conn: &Connection,
    prompt_templates_path: &str,
    prompt_template_name: &str,
    prompt_output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let prompt_template = read_prompt_templates(prompt_templates_path)
        .get(prompt_template_name)
        .unwrap()
        .clone();
    let mut prompt_out = LlmPromptResult {
        template: prompt_template,
        user_prompts: HashMap::new(),
        user_prompts_meta: HashMap::new(),
    };
    
    // get all chars in the learners section
    let words = db_read::read_words(conn).unwrap();

    for word in &words {
        if word.trad == "%" {
            break;
        }
        if word.trad.chars().count() > 1 {
            continue;
        }
        let word_str = common::format_word_def(&word.trad, &word.simp, None);
        let (_, word_defs) = format_word_defs_for_word_id(conn, word.id, &vec![]);
        let prompt_txt =
            format!("dictionary entry:\n{word_str}\n{word_defs}");
        let prompt_meta = vec![format!("{};{}", word.trad, word.simp)];

        prompt_out.user_prompts.insert(word_str.clone(), prompt_txt);
        prompt_out.user_prompts_meta.insert(word_str, prompt_meta);
    }

    let file = File::create(prompt_output_path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &prompt_out)?;
    writer.flush()?;

    Ok(())
}

pub fn create_prompts_identify_spoken_definitions_of_words(
    conn: &Connection,
    prompt_templates_path: &str,
    prompt_template_name: &str,
    prompt_output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let prompt_template = read_prompt_templates(prompt_templates_path)
        .get(prompt_template_name)
        .unwrap()
        .clone();
    let mut prompt_out = LlmPromptResult {
        template: prompt_template,
        user_prompts: HashMap::new(),
        user_prompts_meta: HashMap::new(),
    };
    
    // get all words, but not characters, in the learners section
    let words = db_read::read_words(conn).unwrap();

    for word in &words {
        if word.trad == "%" {
            break;
        }
        if word.trad.chars().count() == 1 {
            continue;
        }
        let word_str = common::format_word_def(&word.trad, &word.simp, None);
        let (_, word_defs) = format_word_defs_for_word_id(conn, word.id, &vec![]);
        let prompt_txt =
            format!("dictionary entry:\n{word_str}\n{word_defs}");
        let prompt_meta = vec![format!("{};{}", word.trad, word.simp)];

        prompt_out.user_prompts.insert(word_str.clone(), prompt_txt);
        prompt_out.user_prompts_meta.insert(word_str, prompt_meta);
    }

    let file = File::create(prompt_output_path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &prompt_out)?;
    writer.flush()?;

    Ok(())
}

pub fn create_prompts_match_decomposition(
    conn: &Connection,
    prompt_templates_path: &str,
    prompt_template_name: &str,
    prompt_output_path: &str,
) -> Result<(), Box<dyn Error>> {
    let prompt_template = read_prompt_templates(prompt_templates_path)
        .get(prompt_template_name)
        .unwrap()
        .clone();
    let mut prompt_out = LlmPromptResult {
        template: prompt_template,
        user_prompts: HashMap::new(),
        user_prompts_meta: HashMap::new(),
    };

    let words = db_read::read_words(conn).unwrap();

    for word in &words {
        if word.trad == "%" {
            break;
        }
        // skip word if it's a single character
        if word.trad.chars().count() == 1 {
            continue;
        }
        let word_str = common::format_word_def(&word.trad, &word.simp, None);
        let (_, word_defs) = format_word_defs_for_word_id(conn, word.id, &vec![]);
        let mut prompt_txt =
            format!("word:\n\n{word_str}\n{word_defs}\n\npossible components:\n\n");
        let mut prompt_meta = vec![format!("{};{}", word.trad, word.simp)];

        // decompose word and add all possible components with their definitions to prompt
        let (comp_word_ids, _unknown_chars) =
            db_read::get_words_in_str(conn, &word.trad, db_read::SimpTrad::Trad)?;
        for (comp_id, _) in comp_word_ids {
            if comp_id == word.id {
                continue;
            }
            let (comp_word, comp_defs) = format_word_defs_for_word_id(conn, comp_id, &vec![]);
            let comp_str = common::format_word_def(&comp_word.trad, &comp_word.simp, None);
            let comp_def = format!("{comp_str}\n{comp_defs}\n");
            prompt_txt.push_str(&comp_def);
            prompt_meta.push(format!("{};{}", comp_word.trad, comp_word.simp))
        }

        prompt_out.user_prompts.insert(word_str.clone(), prompt_txt);
        prompt_out.user_prompts_meta.insert(word_str, prompt_meta);
    }

    let file = File::create(prompt_output_path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &prompt_out)?;
    writer.flush()?;
    Ok(())
}
