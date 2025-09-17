use std::fs::File;
use std::io::BufWriter;
use std::io::Write;

use rusqlite::Connection;

use fmld::db_check;
use fmld::db_edit;
use fmld::db_to_txt;
use fmld::txt_to_db;

#[test]
fn test_txt_to_db_to_txt() {
    let mut conn = Connection::open_in_memory().unwrap();
    let mut file = File::open("./tests/txt2db2txt_input.txt").unwrap();
    let errors = txt_to_db::txt_to_db(&mut file, &conn, None);

    // TODO add errors and check if they are reported

    let tx = conn.transaction().unwrap();

    db_edit::add_missing_symmetric_references(&tx).unwrap();
    db_edit::add_missing_notes_and_tags_for_symmetric_references(&tx).unwrap();

    tx.commit();

    let round_trip_out_txt = db_check::round_trip_check(&conn);
    assert_eq!(
        round_trip_out_txt.is_ok_and(|t| t.is_empty()),
        true,
        "round trip check failed"
    );

    let mut txt_out: Vec<u8> = Vec::with_capacity(20000000);
    db_to_txt::db_to_txt(&mut txt_out, &conn, false, None).unwrap();

    let txt_expected = std::fs::read("./tests/txt2db2txt_expected_output.txt").unwrap();
    if txt_expected != txt_out {
        let file_out = File::create("./tests/txt2db2txt_test_output.txt").unwrap();
        let mut writer_out = BufWriter::new(file_out);
        writer_out.write(&txt_out);
    }
    assert_eq!(txt_expected, txt_out, "output does not match expected file");
}
