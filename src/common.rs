use crate::config::WORD_SEP;

pub type SqliteId = i64;

pub fn format_word_def(trad: &str, simp: &str, ext_def_id: Option<u32>) -> String {
    #[allow(clippy::collapsible_else_if, reason = "maintain symmetry")]
    #[allow(clippy::option_if_let_else, reason= "readability")]
    if let Some(id) = ext_def_id {
        if trad == simp {
            format!("{trad}#D{id}")
        } else {
            format!("{trad}{WORD_SEP}{simp}#D{id}")
        }
    } else {
        if trad == simp {
            trad.to_owned()
        } else {
            format!("{trad}{WORD_SEP}{simp}")
        }
    }
}
