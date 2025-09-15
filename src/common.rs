use crate::config;

pub type SqliteId = i64;

pub fn format_word_def(trad: &str, simp: &str, ext_def_id: Option<u32>) -> String {
    if let Some(id) = ext_def_id {
        if trad == simp {
            format!("{}#D{}", trad, id)
        } else {
            format!("{}{}{}#D{}", trad, config::WORD_SEP, simp, id)
        }
    } else {
        if trad == simp {
            trad.to_owned()
        } else {
            format!("{}{}{}", trad, config::WORD_SEP, simp)
        }
    }
}