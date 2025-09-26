use fmld::db_check;
use fmld::db_edit;

use fmld::db_to_txt;
use fmld::txt_to_db;

use clap::Parser;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::BufWriter;
use std::io::Write;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};

use rusqlite::{Connection, backup};

#[derive(Parser)]
#[command(name = "FMLD Tool")]
#[command(version = "0.0.1")]
#[command(about = "Free Mandarin Learner's Dictionary Tool", long_about = None)]
struct Cli {
    /// Input file, .txt or .db (sqlite)
    input_file: PathBuf,

    /// Output as .db file (sqlite)
    #[arg(short, long)]
    db: Option<PathBuf>,

    /// Output as .txt file
    #[arg(short, long)]
    txt: Option<PathBuf>,

    /// Meta data as .json file to create final note ids (used on server only, for final merge)
    #[arg(long)]
    finalize_with_meta: Option<PathBuf>,

    /// Limit input or output in text format to all entries up to the provided word
    #[arg(short, long)]
    limit_to_word: Option<String>,

    /// Use tabs for indendation
    #[arg(long)]
    indent_with_tabs: bool,

    /// Do round trip check, which checks if the two text representations before and after the conversion to the sqlite DB are identical
    #[arg(long)]
    round_trip_check: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug)]
struct DictMeta {
    #[serde(default)]
    num_words: u32,
    #[serde(default)]
    num_definitions: u32,
    #[serde(default)]
    num_references: u32,
    #[serde(default)]
    num_notes: u32,
    #[serde(default)]
    max_note_id: u32,
}

enum DbSource {
    Txt(Vec<String>),
    Db,
}

struct DictDb {
    source: DbSource,
    conn: Connection,
}

fn read_input(path: &Path, limit_to_word: Option<&str>) -> anyhow::Result<DictDb> {
    match path.extension().and_then(OsStr::to_str) {
        Some("db") => {
            let mut conn = Connection::open_in_memory()?;
            // create in-memory copy of the source (source is never modified)
            let input_conn = Connection::open(path)
                .context(format!("Could not open sqlite file {}", path.display()))?;
            {
                let backup = backup::Backup::new(&input_conn, &mut conn)?;
                backup.run_to_completion(4000, Duration::new(0, 0), None)?;
            }
            Ok(DictDb {
                source: DbSource::Db,
                conn,
            })
        }
        Some("txt") => {
            let conn = Connection::open_in_memory()?;
            let mut file =
                File::open(path).context(format!("Could not open txt file {}", path.display()))?;
            let errors = txt_to_db::txt_to_db(&mut file, &conn, limit_to_word);
            Ok(DictDb {
                source: DbSource::Txt(errors),
                conn,
            })
        }
        _ => Err(anyhow!("Invalid input file {}", path.display())),
    }
}

fn write_output(db_source: &DictDb, cli: &Cli) -> anyhow::Result<()> {
    if let Some(path_out) = &cli.txt {
        if *path_out == cli.input_file {
            bail!("Input file and output file must be different");
        }
        let file_out = File::create(path_out).context(format!(
            "Could not create output file {}",
            path_out.display()
        ))?;
        let mut writer_out = BufWriter::new(file_out);
        db_to_txt::db_to_txt(
            &mut writer_out,
            &db_source.conn,
            cli.indent_with_tabs,
            cli.limit_to_word.as_deref(),
        )?;
    }

    if let Some(path_out) = &cli.db {
        if *path_out == cli.input_file {
            bail!("Input file and output file must be different");
        }
        let mut db_out = Connection::open(path_out).context(format!(
            "Could not create output file {}",
            path_out.display()
        ))?;
        let backup = backup::Backup::new(&db_source.conn, &mut db_out)?;
        backup.run_to_completion(4000, Duration::new(0, 0), None)?;
    }
    Ok(())
}

fn finalize(db_source: &mut DictDb, meta_path: &Path) -> anyhow::Result<()> {
    let external_meta: Option<DictMeta> =
        if meta_path.extension().and_then(OsStr::to_str) == Some("json") {
            let s = fs::read_to_string(meta_path)?;
            let meta: DictMeta = serde_json::from_str(&s)?;
            Some(meta)
        } else {
            None
        };
    let max_ext_note_id = if let Some(m) = &external_meta {
        m.max_note_id
    } else {
        0
    };
    let tx = db_source.conn.transaction()?;
    let new_max_ext_note_id = db_edit::finalize_note_ids(&tx, max_ext_note_id)?;
    tx.commit()?;

    if let Some(mut m) = external_meta {
        let mut stmt = db_source.conn.prepare(
            "
        SELECT
            (SELECT COUNT(dict_definition.id) FROM dict_definition) AS num_defs ,
            (SELECT COUNT(dict_word.id) FROM dict_word) AS num_words,
            (SELECT COUNT(dict_note.id) FROM dict_note) AS num_notes,
            (SELECT COUNT(dict_reference.id) FROM dict_reference) AS num_refs;
        ",
        )?;
        let (num_words, num_defs, num_refs, num_notes) = stmt.query_row([], |row| {
            Ok((
                row.get("num_words")?,
                row.get("num_defs")?,
                row.get("num_refs")?,
                row.get("num_notes")?,
            ))
        })?;
        m.num_notes = num_notes;
        m.num_definitions = num_defs;
        m.num_references = num_refs;
        m.num_words = num_words;
        m.max_note_id = new_max_ext_note_id;
        let s = serde_json::to_string_pretty(&m)?;
        fs::write(meta_path, s)?;
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut status_ok = true;

    let mut db_source = read_input(&cli.input_file, cli.limit_to_word.as_deref())?;
    if let DbSource::Txt(errors) = &db_source.source {
        if !errors.is_empty() {
            status_ok = false;
            for err in errors {
                eprintln!("{err}");
            }
        }
    }

    let check_result = db_check::check_entries(&db_source.conn);
    if let Ok(err_list) = check_result {
        if !err_list.is_empty() {
            status_ok = false;
        }
        for err in err_list {
            eprintln!("{err}");
        }
    }
    let tx = db_source.conn.transaction()?;

    db_edit::add_missing_symmetric_references(&tx)?;
    db_edit::add_missing_notes_and_tags_for_symmetric_references(&tx)?;
    tx.commit()?;

    if let Some(meta_path) = &cli.finalize_with_meta {
        finalize(&mut db_source, meta_path)?;
    }

    if let Some(txt_b_out_path) = &cli.round_trip_check {
        let txt_b = db_check::round_trip_check(&db_source.conn)?;
        if !txt_b.is_empty() && txt_b_out_path.extension().and_then(OsStr::to_str) == Some("txt") {
            let file_out = File::create(txt_b_out_path)?;
            let mut writer_out = BufWriter::new(file_out);
            writer_out.write(&txt_b)?;
        }
        if txt_b.is_empty() {
            eprintln!("Round trip check ok!");
        } else {
            status_ok = false;
            eprintln!("Round trip check failed!");
        }
    }

    write_output(&db_source, &cli)?;

    if status_ok {
        Ok(())
    } else {
        Err(anyhow!("Failure!"))
    }
}
