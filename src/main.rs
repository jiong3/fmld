use fmld::db_check;
use fmld::db_edit;

use fmld::db_to_txt;
use fmld::txt_to_db;

use clap::Parser;
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, backup};

#[derive(Parser)]
#[command(name = "FMLD Tool")]
#[command(version = "0.0.1")]
#[command(about = "Free Mandarin Learners Dictionary Tool", long_about = None)]
struct Cli {
    /// Input file, .txt or .db (sqlite)
    input_file: PathBuf,

    /// Output as .db file (sqlite)
    #[arg(short, long)]
    db: Option<PathBuf>,

    /// Output as .txt file
    #[arg(short, long)]
    txt: Option<PathBuf>,

    /// Limit input or output in text format to all entries up to the provided word
    #[arg(short, long)]
    limit_to_word: Option<String>,

    /// Use tabs for indendation
    #[arg(long)]
    indent_with_tabs: bool,

    /// Do round trip check, which checks if the two text representations before and after the conversion to the sqlite DB are identical
    #[arg(long)]
    round_trip_check: Option<PathBuf>,
    // TODO create note ids
}

enum DbSource {
    Txt(Vec<String>),
    Db,
}

struct DictDb {
    source: DbSource,
    conn: Connection,
}

fn read_input(path: &PathBuf, limit_to_word: Option<&str>) -> DictDb {
    match path.extension().and_then(OsStr::to_str) {
        Some("db") => {
            let mut conn = Connection::open_in_memory().unwrap();
            // create in-memory copy of the source (source is never modified)
            let input_conn = Connection::open(path).unwrap_or_else(|_| {
                eprintln!("Error: Could not open sqlite file {}", path.display());
                std::process::exit(1);
            });
            {
                let backup = backup::Backup::new(&input_conn, &mut conn).unwrap();
                backup
                    .run_to_completion(-1, Duration::new(0, 0), None)
                    .unwrap();
            }
            DictDb {
                source: DbSource::Db,
                conn,
            }
        }
        Some("txt") => {
            let conn = Connection::open_in_memory().unwrap();
            let mut file = File::open(path).unwrap_or_else(|_| {
                eprintln!("Error: Could not open txt file {}", path.display());
                std::process::exit(1);
            });
            let errors = txt_to_db::txt_to_db(&mut file, &conn, limit_to_word);
            DictDb {
                source: DbSource::Txt(errors),
                conn,
            }
        }
        _ => {
            eprintln!("Error: Invalid input file {}", path.display());
            std::process::exit(1);
        }
    }
}

fn write_output(db_source: &DictDb, cli: &Cli) {
    if let Some(path_out) = &cli.txt {
        if *path_out == cli.input_file {
            eprintln!("Error: input file and output file must be different");
            std::process::exit(1);
        }
        let file_out = File::create(path_out).unwrap();
        let mut writer_out = BufWriter::new(file_out);
        db_to_txt::db_to_txt(
            &mut writer_out,
            &db_source.conn,
            cli.indent_with_tabs,
            cli.limit_to_word.as_deref(),
        )
        .unwrap();
    }

    if let Some(path_out) = &cli.db {
        if *path_out == cli.input_file {
            eprintln!("Error: input file and output file must be different");
            std::process::exit(1);
        }
        let mut db_out = Connection::open(path_out).unwrap();
        let backup = backup::Backup::new(&db_source.conn, &mut db_out).unwrap();
        backup
            .run_to_completion(-1, Duration::new(0, 0), None)
            .unwrap();
    }
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let mut db_source = read_input(&cli.input_file, cli.limit_to_word.as_deref());
    if let DbSource::Txt(errors) = &db_source.source {
        if !errors.is_empty() {
            for err in errors {
                eprintln!("{err}");
            }
        }
    }

    let check_result = db_check::check_entries(&db_source.conn);
    if let Ok(err_list) = check_result {
        for err in err_list {
            eprintln!("{err}");
        }
    }
    let tx = db_source.conn.transaction().unwrap();

    db_edit::add_missing_symmetric_references(&tx).unwrap();
    db_edit::add_missing_notes_and_tags_for_symmetric_references(&tx).unwrap();

    tx.commit();

    if let Some(txt_b_out_path) = &cli.round_trip_check {
        let txt_b = db_check::round_trip_check(&db_source.conn).unwrap();
        if !txt_b.is_empty() && txt_b_out_path.extension().and_then(OsStr::to_str) == Some("txt") {
            let file_out = File::create(txt_b_out_path).unwrap();
            let mut writer_out = BufWriter::new(file_out);
            writer_out.write(&txt_b);
        }
        if txt_b.is_empty() {
            eprintln!("Round trip check ok!");
        } else {
            eprintln!("Round trip check failed!");
        }
    }

    write_output(&db_source, &cli);

    Ok(())
}
