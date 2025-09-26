use rusqlite::{params, Connection, Row};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Result as IoResult, Write};

type SqliteId = i64;

#[derive(Debug)]
pub enum ExportError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
}

impl From<rusqlite::Error> for ExportError {
    fn from(e: rusqlite::Error) -> Self {
        ExportError::Sqlite(e)
    }
}
impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        ExportError::Io(e)
    }
}

#[derive(Debug, Clone)]
struct SharedMeta {
    ascii_tags: Vec<char>,
    full_tags: Vec<String>,
    note: Option<(u32, String)>, // (ext_note_id, note_text)
    comment: Option<String>,
}

#[derive(Debug, Clone)]
struct WordRow {
    id: SqliteId,
    shared_id: SqliteId,
    trad: String,
    simp: String,
}

#[derive(Debug, Clone)]
struct DefRow {
    id: SqliteId,
    shared_id: SqliteId,
    word_id: SqliteId,
    ext_def_id: u32,
    definition: String,
    class_id: SqliteId,
}

#[derive(Debug, Clone)]
struct ClassRow {
    id: SqliteId,
    name: String,
}

#[derive(Debug, Clone)]
struct HeaderComment {
    shared_id: SqliteId,
    comment: String,
}

#[derive(Debug, Clone)]
struct PronShared {
    pron_shared_id: SqliteId,
    rank: i64,
    rank_relative: i64,
    note_id: Option<SqliteId>,
    comment_id: Option<SqliteId>,
    tags: SharedMeta,
    pinyins: Vec<String>, // for the current definition
}

#[derive(Debug, Clone)]
struct RefTarget {
    dst_trad: String,
    dst_simp: String,
    dst_ext_def_id: Option<u32>,
}

#[derive(Debug, Clone)]
struct RefSharedGroup {
    shared_id: SqliteId,
    rank: i64,
    rank_relative: i64,
    tags: SharedMeta,
    refs: Vec<RefTarget>,
    note_id: Option<SqliteId>,
    comment_id: Option<SqliteId>,
}

#[derive(Debug, Clone)]
struct RefLineGroup {
    ref_type: char,
    groups: Vec<RefSharedGroup>, // all share same (note_id, comment_id)
    note_id: Option<SqliteId>,
    comment_id: Option<SqliteId>,
}

/// Public API
pub fn export_db_to_txt(conn: &Connection, mut out: impl Write) -> Result<(), ExportError> {
    let mut exporter = Exporter::new(conn, &mut out);
    exporter.run()?;
    Ok(())
}

struct Exporter<'a, W: Write> {
    conn: &'a Connection,
    out: &'a mut W,
    seen_notes: HashSet<u32>, // globally track printed notes by ext_note_id
    class_cache: HashMap<SqliteId, String>,
}

impl<'a, W: Write> Exporter<'a, W> {
    fn new(conn: &'a Connection, out: &'a mut W) -> Self {
        Exporter {
            conn,
            out,
            seen_notes: HashSet::new(),
            class_cache: HashMap::new(),
        }
    }

    fn run(&mut self) -> Result<(), ExportError> {
        // 1) Header comments (unreferenced shared rows with a comment)
        let headers = self.load_header_comments()?;
        for h in headers {
            self.write_comment_block(0, &h.comment)?;
        }

        // 2) Iterate definitions ordered by shared.rank and rank_relative
        let defs = self.load_definitions_in_shared_order()?;

        let mut cur_word_id: Option<SqliteId> = None;
        let mut cur_pron_set: Option<BTreeSet<SqliteId>> = None;
        let mut cur_class_id: Option<SqliteId> = None;

        // For each word, we will emit W line and word-level references only once
        for d in defs {
            if cur_word_id != Some(d.word_id) {
                // new word
                cur_word_id = Some(d.word_id);
                cur_pron_set = None;
                cur_class_id = None;

                let word = self.load_word(d.word_id)?;
                let word_meta = self.load_shared_meta(word.shared_id)?;

                // W line
                self.write_w_line(&word, &word_meta)?;

                // children (#, N)
                self.write_comment_child(1, &word_meta.comment)?;
                self.write_note_child(1, &word_meta.note)?;

                // word-level references under W
                let wrefs = self.load_references_for_word(word.id)?;
                self.write_reference_blocks(1, &wrefs)?;
            }

            // P lines for this definition when pron set differs
            let prons = self.load_pron_groups_for_definition(d.id)?;
            let pron_id_set: BTreeSet<SqliteId> =
                prons.iter().map(|p| p.pron_shared_id).collect();

            if cur_pron_set.as_ref() != Some(&pron_id_set) {
                // Emit P lines: first group at indent 1, others at indent 2
                // Group pron_shared by (note_id, comment_id)
                let mut group_map: BTreeMap<(Option<SqliteId>, Option<SqliteId>), Vec<PronShared>> =
                    BTreeMap::new();
                for p in prons.iter() {
                    group_map
                        .entry((p.note_id, p.comment_id))
                        .or_default()
                        .push(p.clone());
                }

                let mut is_first = true;
                for ((_nid, _cid), mut group_items) in group_map {
                    // already sorted by rank then rank_relative by query
                    let indent = if is_first { 1 } else { 2 };
                    is_first = false;

                    // Write P line: "P" + for each pron_shared a tag group with its pinyins
                    // All in this line share same note/comment; we take from the first
                    let line = self.build_p_line(&group_items)?;
                    self.write_line(indent, &line)?;

                    // child (#, N) for this P line (shared across all groups in this line)
                    if let Some(first) = group_items.first() {
                        self.write_comment_child(indent + 1, &first.tags.comment)?;
                        self.write_note_child(indent + 1, &first.tags.note)?;
                    }
                }

                cur_pron_set = Some(pron_id_set);
            }

            // Class line if changed
            if cur_class_id != Some(d.class_id) {
                let class_name = self.get_class_name(d.class_id)?;
                self.write_line(2, &format!("C|{}", class_name))?;
                cur_class_id = Some(d.class_id);
            }

            // Definition line
            let dmeta = self.load_shared_meta(d.shared_id)?;
            self.write_definition_line(3, &d, &dmeta)?;

            // children for definition: comment then note
            self.write_comment_child(4, &dmeta.comment)?;
            self.write_note_child(4, &dmeta.note)?;

            // definition-level references (under D)
            let drefs = self.load_references_for_definition(d.id)?;
            self.write_reference_blocks(4, &drefs)?;
        }

        Ok(())
    }

    fn load_header_comments(&self) -> Result<Vec<HeaderComment>, ExportError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT s.id, c.comment
            FROM dict_shared s
            JOIN dict_comment c ON c.id = s.comment_id
            LEFT JOIN dict_word w ON w.shared_id = s.id
            LEFT JOIN dict_definition d ON d.shared_id = s.id
            LEFT JOIN dict_reference r ON r.shared_id = s.id
            LEFT JOIN dict_pron_definition pd ON pd.pron_shared_id = s.id
            WHERE w.id IS NULL AND d.id IS NULL AND r.id IS NULL AND pd.id IS NULL
            ORDER BY s.rank ASC, COALESCE(s.rank_relative,0) ASC, s.id ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HeaderComment {
                shared_id: row.get(0)?,
                comment: row.get::<_, String>(1)?,
            })
        })?;
        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    fn load_definitions_in_shared_order(&self) -> Result<Vec<DefRow>, ExportError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT d.id, d.shared_id, d.word_id, d.ext_def_id, d.definition, d.class_id
            FROM dict_definition d
            JOIN dict_shared s ON s.id = d.shared_id
            ORDER BY s.rank ASC, COALESCE(s.rank_relative,0) ASC, d.id ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DefRow {
                id: row.get(0)?,
                shared_id: row.get(1)?,
                word_id: row.get(2)?,
                ext_def_id: row.get::<_, i64>(3)? as u32,
                definition: row.get(4)?,
                class_id: row.get(5)?,
            })
        })?;
        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    fn load_word(&self, word_id: SqliteId) -> Result<WordRow, ExportError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, shared_id, trad, simp FROM dict_word WHERE id=?1
            "#,
        )?;
        let row = stmt.query_row([word_id], |row| {
            Ok(WordRow {
                id: row.get(0)?,
                shared_id: row.get(1)?,
                trad: row.get(2)?,
                simp: row.get(3)?,
            })
        })?;
        Ok(row)
    }

    fn get_class_name(&mut self, class_id: SqliteId) -> Result<String, ExportError> {
        if let Some(n) = self.class_cache.get(&class_id) {
            return Ok(n.clone());
        }
        let mut stmt = self.conn.prepare(
            r#"
            SELECT name FROM dict_class WHERE id = ?1
            "#,
        )?;
        let name: String = stmt.query_row([class_id], |row| row.get(0))?;
        self.class_cache.insert(class_id, name.clone());
        Ok(name)
    }

    fn load_shared_meta(&self, shared_id: SqliteId) -> Result<SharedMeta, ExportError> {
        // tags
        let mut stmt_tags = self.conn.prepare(
            r#"
            SELECT t.ascii_symbol, t.tag
            FROM dict_shared_tag st
            JOIN dict_tag t ON t.id = st.tag_id
            WHERE st.for_shared_id = ?1
            ORDER BY (t.ascii_symbol IS NULL) ASC, t.ascii_symbol ASC, t.tag ASC
            "#,
        )?;
        let tag_rows = stmt_tags.query_map([shared_id], |row| {
            let ascii: Option<String> = row.get(0)?;
            let tag: String = row.get(1)?;
            Ok((ascii, tag))
        })?;

        let mut ascii_tags: Vec<char> = Vec::new();
        let mut full_tags: Vec<String> = Vec::new();
        for r in tag_rows {
            let (ascii_opt, tag) = r?;
            if let Some(a) = ascii_opt {
                if let Some(ch) = a.chars().next() {
                    ascii_tags.push(ch);
                }
            } else {
                full_tags.push(tag);
            }
        }
        ascii_tags.sort();
        full_tags.sort();

        // note/comment
        let mut stmt_nc = self.conn.prepare(
            r#"
            SELECT s.note_id, s.comment_id FROM dict_shared s WHERE s.id = ?1
            "#,
        )?;
        let (note_id_opt, comment_id_opt): (Option<SqliteId>, Option<SqliteId>) =
            stmt_nc.query_row([shared_id], |row| Ok((row.get(0)?, row.get(1)?)))?;

        let note = if let Some(nid) = note_id_opt {
            let mut stmt_n = self.conn.prepare(
                r#"
                SELECT ext_note_id, note FROM dict_note WHERE id = ?1
                "#,
            )?;
            let (eid, txt): (i64, String) = stmt_n.query_row([nid], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Some((eid as u32, txt))
        } else {
            None
        };

        let comment = if let Some(cid) = comment_id_opt {
            let mut stmt_c = self.conn.prepare(
                r#"
                SELECT comment FROM dict_comment WHERE id = ?1
                "#,
            )?;
            let c: String = stmt_c.query_row([cid], |row| row.get(0))?;
            Some(c)
        } else {
            None
        };

        Ok(SharedMeta {
            ascii_tags,
            full_tags,
            note,
            comment,
        })
    }

    fn load_pron_groups_for_definition(
        &self,
        def_id: SqliteId,
    ) -> Result<Vec<PronShared>, ExportError> {
        // Get pron_shared_id in order with their meta
        let mut stmt = self.conn.prepare(
            r#"
            SELECT pd.pron_shared_id, s.rank, COALESCE(s.rank_relative,0), s.note_id, s.comment_id
            FROM dict_pron_definition pd
            JOIN dict_shared s ON s.id = pd.pron_shared_id
            WHERE pd.definition_id = ?1
            GROUP BY pd.pron_shared_id
            ORDER BY s.rank ASC, COALESCE(s.rank_relative,0) ASC, pd.pron_shared_id ASC
            "#,
        )?;
        let rows = stmt.query_map([def_id], |row| {
            Ok((
                row.get::<_, SqliteId>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<SqliteId>>(3)?,
                row.get::<_, Option<SqliteId>>(4)?,
            ))
        })?;

        let mut res = Vec::new();
        for r in rows {
            let (psid, rank, rrel, note_id, comment_id) = r?;
            let tags = self.load_shared_meta(psid)?;

            // pinyins for this pron_shared_id and definition
            let mut stmt_p = self.conn.prepare(
                r#"
                SELECT DISTINCT p.pinyin_num
                FROM dict_pron_definition pd
                JOIN dict_pron p ON p.id = pd.pron_id
                WHERE pd.pron_shared_id = ?1 AND pd.definition_id = ?2
                ORDER BY p.pinyin_num ASC
                "#,
            )?;
            let pinyins_iter =
                stmt_p.query_map(params![psid, def_id], |row| Ok(row.get::<_, String>(0)?))?;
            let mut pinyins: Vec<String> = Vec::new();
            for pp in pinyins_iter {
                pinyins.push(pp?);
            }

            res.push(PronShared {
                pron_shared_id: psid,
                rank,
                rank_relative: rrel,
                note_id,
                comment_id,
                tags,
                pinyins,
            });
        }
        Ok(res)
    }

    fn load_references_for_word(
        &self,
        word_id: SqliteId,
    ) -> Result<Vec<RefLineGroup>, ExportError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT r.shared_id, s.rank, COALESCE(s.rank_relative,0),
                   rt.ascii_symbol,
                   s.note_id, s.comment_id,
                   wd.trad, wd.simp, dd.ext_def_id
            FROM dict_reference r
            JOIN dict_shared s ON s.id = r.shared_id
            JOIN dict_ref_type rt ON rt.id = r.ref_type_id
            JOIN dict_word wd ON wd.id = r.word_id_dst
            LEFT JOIN dict_definition dd ON dd.id = r.definition_id_dst
            WHERE r.word_id_src = ?1 AND r.definition_id_src IS NULL
            ORDER BY s.rank ASC, COALESCE(s.rank_relative,0) ASC, r.id ASC
            "#,
        )?;
        let rows = stmt.query_map([word_id], |row| self.map_ref_row(row))?;
        self.group_references(rows)
    }

    fn load_references_for_definition(
        &self,
        def_id: SqliteId,
    ) -> Result<Vec<RefLineGroup>, ExportError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT r.shared_id, s.rank, COALESCE(s.rank_relative,0),
                   rt.ascii_symbol,
                   s.note_id, s.comment_id,
                   wd.trad, wd.simp, dd.ext_def_id
            FROM dict_reference r
            JOIN dict_shared s ON s.id = r.shared_id
            JOIN dict_ref_type rt ON rt.id = r.ref_type_id
            JOIN dict_word wd ON wd.id = r.word_id_dst
            LEFT JOIN dict_definition dd ON dd.id = r.definition_id_dst
            WHERE r.definition_id_src = ?1
            ORDER BY s.rank ASC, COALESCE(s.rank_relative,0) ASC, r.id ASC
            "#,
        )?;
        let rows = stmt.query_map([def_id], |row| self.map_ref_row(row))?;
        self.group_references(rows)
    }

    fn map_ref_row(&self, row: &Row) -> rusqlite::Result<(RefSharedGroup, char)> {
        let shared_id: SqliteId = row.get(0)?;
        let rank: i64 = row.get(1)?;
        let rrel: i64 = row.get(2)?;
        let ref_type_s: String = row.get(3)?;
        let ref_type = ref_type_s.chars().next().unwrap_or('?');

        let note_id: Option<SqliteId> = row.get(4)?;
        let comment_id: Option<SqliteId> = row.get(5)?;

        let dst_trad: String = row.get(6)?;
        let dst_simp: String = row.get(7)?;
        let dst_ext_def_id: Option<i64> = row.get(8)?;

        let tags = self.load_shared_meta(shared_id).unwrap_or(SharedMeta {
            ascii_tags: vec![],
            full_tags: vec![],
            note: None,
            comment: None,
        });

        Ok((
            RefSharedGroup {
                shared_id,
                rank,
                rank_relative: rrel,
                tags,
                refs: vec![RefTarget {
                    dst_trad,
                    dst_simp: dst_simp,
                    dst_ext_def_id: dst_ext_def_id.map(|v| v as u32),
                }],
                note_id,
                comment_id,
            },
            ref_type,
        ))
    }

    fn group_references<I>(
        &self,
        rows: I,
    ) -> Result<Vec<RefLineGroup>, ExportError>
    where
        I: IntoIterator<Item = rusqlite::Result<(RefSharedGroup, char)>>,
    {
        // First group rows by (ref_type, note_id, comment_id)
        // Then within each line group, collect groups per shared_id (preserve order by rank)
        let mut groups_map: BTreeMap<(char, Option<SqliteId>, Option<SqliteId>), Vec<RefSharedGroup>> =
            BTreeMap::new();

        for rr in rows {
            let (mut g, ref_type) = rr?;
            let key = (ref_type, g.note_id, g.comment_id);
            if let Some(list) = groups_map.get_mut(&key) {
                // try to merge into existing same shared_id
                if let Some(existing) = list.iter_mut().find(|x| x.shared_id == g.shared_id) {
                    existing.refs.extend(g.refs.into_iter());
                } else {
                    list.push(g);
                }
            } else {
                groups_map.insert(key, vec![g]);
            }
        }

        let mut result: Vec<RefLineGroup> = Vec::new();
        for ((ref_type, n, c), mut v) in groups_map {
            v.sort_by_key(|x| (x.rank, x.rank_relative, x.shared_id));
            result.push(RefLineGroup {
                ref_type,
                groups: v,
                note_id: n,
                comment_id: c,
            });
        }
        Ok(result)
    }

    // ---------- Writing helpers ----------

    fn write_line(&mut self, indent: usize, line: &str) -> Result<(), ExportError> {
        for _ in 0..indent {
            self.out.write_all(b" ")?;
        }
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\n")?;
        Ok(())
    }

    fn write_continuations(
        &mut self,
        prev_indent: usize,
        text: &str,
    ) -> Result<(), ExportError> {
        // For multi-line continuation: indent = prev_indent + 2 spaces
        let mut lines = text.split('\n');
        // skip the first because it's already printed by caller
        if let Some(_) = lines.next() {}
        for l in lines {
            for _ in 0..(prev_indent + 2) {
                self.out.write_all(b" ")?;
            }
            self.out.write_all(l.as_bytes())?;
            self.out.write_all(b"\n")?;
        }
        Ok(())
    }

    fn write_comment_block(&mut self, indent: usize, comment: &str) -> Result<(), ExportError> {
        let mut lines = comment.split('\n');
        if let Some(first) = lines.next() {
            self.write_line(indent, &format!("# {}", first))?;
            for l in lines {
                // continuation
                for _ in 0..(indent + 2) {
                    self.out.write_all(b" ")?;
                }
                self.out.write_all(l.as_bytes())?;
                self.out.write_all(b"\n")?;
            }
        }
        Ok(())
    }

    fn write_comment_child(
        &mut self,
        indent: usize,
        comment: &Option<String>,
    ) -> Result<(), ExportError> {
        if let Some(c) = comment {
            self.write_comment_block(indent, c)?;
        }
        Ok(())
    }

    fn write_note_child(
        &mut self,
        indent: usize,
        note: &Option<(u32, String)>,
    ) -> Result<(), ExportError> {
        if let Some((ext_id, txt)) = note {
            // print N-> if seen before; otherwise N id| text
            if self.seen_notes.contains(ext_id) {
                self.write_line(indent, &format!("N->{}", ext_id))?;
            } else {
                // N{id}| {firstline}
                let mut lines = txt.split('\n');
                if let Some(first) = lines.next() {
                    self.write_line(indent, &format!("N{}| {}", ext_id, first))?;
                    for l in lines {
                        for _ in 0..(indent + 2) {
                            self.out.write_all(b" ")?;
                        }
                        self.out.write_all(l.as_bytes())?;
                        self.out.write_all(b"\n")?;
                    }
                } else {
                    // empty note text case
                    self.write_line(indent, &format!("N{}|", ext_id))?;
                }
                self.seen_notes.insert(*ext_id);
            }
        }
        Ok(())
    }

    fn tags_to_string(&self, meta: &SharedMeta) -> String {
        // compose |<ascii...>#tag1#tag2|
        let mut s = String::new();
        s.push('|');
        for ch in meta.ascii_tags.iter() {
            s.push(*ch);
        }
        for t in meta.full_tags.iter() {
            s.push('#');
            s.push_str(t);
        }
        s.push('|');
        s
    }

    fn build_word_str(&self, word: &WordRow) -> String {
        if word.trad == word.simp {
            word.trad.clone()
        } else {
            format!("{}/{}", word.trad, word.simp)
        }
    }

    fn write_w_line(&mut self, word: &WordRow, meta: &SharedMeta) -> Result<(), ExportError> {
        let tags = self.tags_to_string(meta);
        let w = self.build_word_str(word);
        self.write_line(0, &format!("W{} {}", tags, w))
    }

    fn build_p_line(&self, group_items: &Vec<PronShared>) -> Result<String, ExportError> {
        // "P" + each pron_shared as a tag group with its pinyins (comma-separated)
        // Separate groups with a single space
        let mut parts: Vec<String> = Vec::new();
        for ps in group_items {
            let tags = self.tags_to_string(&ps.tags);
            let pin = ps.pinyins.join(", ");
            parts.push(format!("{} {}", tags, pin));
        }
        Ok(format!("P{}", if parts.is_empty() { String::new() } else { format!(" {}", parts.join(" ")) }))
    }

    fn write_definition_line(
        &mut self,
        indent: usize,
        d: &DefRow,
        meta: &SharedMeta,
    ) -> Result<(), ExportError> {
        let tags = self.tags_to_string(meta);
        // definition text may be multiline -> use continuation
        // First line:
        let mut lines = d.definition.split('\n');
        if let Some(first) = lines.next() {
            self.write_line(
                indent,
                &format!("D{}{} {}", d.ext_def_id, tags, first),
            )?;
            // continuations
            for l in lines {
                for _ in 0..(indent + 2) {
                    self.out.write_all(b" ")?;
                }
                self.out.write_all(l.as_bytes())?;
                self.out.write_all(b"\n")?;
            }
        } else {
            self.write_line(indent, &format!("D{}{}", d.ext_def_id, tags))?;
        }
        Ok(())
    }

    fn format_reference_target(&self, r: &RefTarget) -> String {
        let mut s = String::new();
        if r.dst_trad == r.dst_simp {
            s.push_str(&r.dst_trad);
        } else {
            s.push_str(&r.dst_trad);
            s.push('/');
            s.push_str(&r.dst_simp);
        }
        if let Some(ext_id) = r.dst_ext_def_id {
            s.push_str(&format!("#D{}", ext_id));
        }
        s
    }

    fn write_reference_blocks(
        &mut self,
        indent: usize,
        lines: &Vec<RefLineGroup>,
    ) -> Result<(), ExportError> {
        for line in lines {
            // Build "X{ref_type}" + multiple tag groups (each for one shared entry)
            let mut groups_txt: Vec<String> = Vec::new();
            for g in &line.groups {
                let tags = self.tags_to_string(&g.tags);
                let mut refs: Vec<String> = Vec::new();
                for r in &g.refs {
                    refs.push(self.format_reference_target(r));
                }
                let refs_join = refs.join(", ");
                groups_txt.push(format!("{} {}", tags, refs_join));
            }
            let line_txt = if groups_txt.is_empty() {
                format!("X{}", line.ref_type)
            } else {
                format!("X{} {}", line.ref_type, groups_txt.join(" "))
            };
            self.write_line(indent, &line_txt)?;

            // Children comment/note for this X line (shared across all groups in the line).
            // Use the first group's meta to get comment/note.
            if let Some(first_group) = line.groups.first() {
                self.write_comment_child(indent + 1, &first_group.tags.comment)?;
                self.write_note_child(indent + 1, &first_group.tags.note)?;
            }
        }
        Ok(())
    }
}