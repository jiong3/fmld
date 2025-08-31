use fmld::txt_to_db::*;
use fmld::db_to_txt::*;
use fmld::db_check::*;

use std::fs::File;
use std::fs::remove_file;
use std::io::BufWriter;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use rusqlite::{Connection, Error as SqliteError};

fn main() -> io::Result<()> {
    let path = Path::new("./tests/tst_dict2.txt");
    let path = Path::new("/Users/wibr/Daten/Dropbox/j3/dict/output/dict_test.txt");
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    remove_file("test.db").unwrap_or(());
    let db_conn = Connection::open("test.db").unwrap();
    // Create an iterator that yields Strings and stops on the first error.
    let lines_iterator = reader.lines().map_while(io::Result::ok); // TODO move this into TxtToDb??
    let mut txt2db = TxtToDb::new(&db_conn);
    txt2db.txt_to_db(lines_iterator);
    txt2db.print_errors();

    let check_result = check_entries(&db_conn);
    if let Ok(err_list) = check_result {
        for err in err_list {
            println!("{}", err);
        }
    }

    let path_out = Path::new("test.txt");
    let file_out = File::create(path_out)?;
    let mut writer_out = BufWriter::new(file_out);

    
    let mut db2txt = DbToTxt::new(&db_conn, &mut writer_out);
    println!("generating txt");
    db2txt.generate_txt_file();
    Ok(())
}
