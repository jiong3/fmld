use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use fmld::pinyin::pinyin_mark_from_num;

#[test]
fn test_pinyin_conversion_from_file() {
    // Integration tests are run from the crate's root directory,
    // so the path to the test file is relative to that root.
    let path = Path::new("tests/pinyin_pairs.txt");
    let file = File::open(&path).expect("Failed to open pinyin_pairs.txt");
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line.expect("Failed to read line");

        // Split the line into pinyin_num and pinyin_mark
        let parts: Vec<&str> = line.split(';').collect();
        if parts.len() != 2 {
            panic!("Invalid line format: {}", line);
        }

        let pinyin_num = parts[0];
        let expected_mark = parts[1];

        // Run the function and compare the result
        let result = pinyin_mark_from_num(pinyin_num);
        assert_eq!(result, expected_mark, "Failed on input: {}", pinyin_num);
    }
}
