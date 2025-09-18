pub const WORD_SEP: &str = "／";
pub const ITEMS_SEP: &str = ";";

pub const APPROX_TXT_FILE_SIZE: usize = 16_000_000;

pub const DB_SCHEMA: &str = r#"

PRAGMA user_version = 1;
/* ext_def_id is a constant unique id within the scope of all definitions for the same word. It is used for references or internal and external links, similar to ext_note_id */
CREATE TABLE IF NOT EXISTS "dict_definition" (
	"id" INTEGER NOT NULL UNIQUE,
	"shared_id" INTEGER NOT NULL,
	"word_id" INTEGER NOT NULL,
	"definition" TEXT NOT NULL, -- definition of the word
	"ext_def_id" INTEGER NOT NULL, -- constant id, used for referencing definitions in the text representation of from external sources
	"class_id" INTEGER NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY ("word_id") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("shared_id") REFERENCES "dict_shared"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("class_id") REFERENCES "dict_class"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_definition_index_0"
ON "dict_definition" ("word_id", "ext_def_id");
/* tags allow a flexible assignment of entries to classes, which includes parts-of-speech, spoken vs written language, usage in Taiwan vs China etc. */
CREATE TABLE IF NOT EXISTS "dict_tag" (
	"id" INTEGER NOT NULL UNIQUE,
	"tag" TEXT NOT NULL,
	"type" TEXT NOT NULL,
	"ascii_symbol" TEXT,
	PRIMARY KEY("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_tag_index_0"
ON "dict_tag" ("tag", "type");
CREATE TABLE IF NOT EXISTS "dict_word" (
	"id" INTEGER NOT NULL UNIQUE,
	"shared_id" INTEGER NOT NULL,
	"trad" TEXT NOT NULL,
	"simp" TEXT NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY ("shared_id") REFERENCES "dict_shared"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_word_index_0"
ON "dict_word" ("trad", "simp");
CREATE TABLE IF NOT EXISTS "dict_pron" (
	"id" INTEGER NOT NULL UNIQUE,
	"pinyin_num" TEXT NOT NULL,
	"pinyin_mark" TEXT NOT NULL,
	PRIMARY KEY("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_pron_index_0"
ON "dict_pron" ("pinyin_num");
CREATE TABLE IF NOT EXISTS "dict_pron_definition" (
	"id" INTEGER NOT NULL UNIQUE,
	"shared_pron_id" INTEGER NOT NULL,
	"definition_id" INTEGER NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY ("definition_id") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("shared_pron_id") REFERENCES "dict_shared_pron"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE INDEX IF NOT EXISTS "dict_pron_definition_index_0"
ON "dict_pron_definition" ("definition_id");
/* Relationship from a to b, e.g. measureword, antonym, synonym or variant. */
CREATE TABLE IF NOT EXISTS "dict_reference" (
	"id" INTEGER NOT NULL UNIQUE,
	"shared_id" INTEGER NOT NULL,
	"ref_type_id" INTEGER NOT NULL,
	"word_id_src" INTEGER NOT NULL,
	"definition_id_src" INTEGER,
	"word_id_dst" INTEGER NOT NULL,
	"definition_id_dst" INTEGER,
	PRIMARY KEY("id"),
	FOREIGN KEY ("shared_id") REFERENCES "dict_shared"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("word_id_dst") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("word_id_src") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("definition_id_src") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("definition_id_dst") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("ref_type_id") REFERENCES "dict_ref_type"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE INDEX IF NOT EXISTS "dict_reference_index_0"
ON "dict_reference" ("word_id_src", "definition_id_src");
/* dict_shared enables linking tags, notes or references to different entries in other tables
rank indicates the order of the element, it is a continuous counter
rank_relative can be used to add new elements with a certain order between two successive ranks */
CREATE TABLE IF NOT EXISTS "dict_shared" (
	"id" INTEGER NOT NULL UNIQUE,
	"rank" INTEGER NOT NULL,
	"rank_relative" INTEGER,
	"note_id" INTEGER,
	"comment_id" INTEGER,
	PRIMARY KEY("id"),
	FOREIGN KEY ("comment_id") REFERENCES "dict_comment"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("note_id") REFERENCES "dict_note"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE INDEX IF NOT EXISTS "dict_shared_index_0"
ON "dict_shared" ("rank", "rank_relative");
CREATE TABLE IF NOT EXISTS "dict_shared_tag" (
	"for_shared_id" INTEGER NOT NULL,
	"tag_id" INTEGER NOT NULL,
	PRIMARY KEY("for_shared_id", "tag_id"),
	FOREIGN KEY ("tag_id") REFERENCES "dict_tag"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("for_shared_id") REFERENCES "dict_shared"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_shared_tag_index_0"
ON "dict_shared_tag" ("for_shared_id", "tag_id");
/* ext_note_id is a globally unique id for each note (but same id for different translations), exported into txt format */
CREATE TABLE IF NOT EXISTS "dict_note" (
	"id" INTEGER NOT NULL UNIQUE,
	"note" TEXT NOT NULL,
	"ext_note_id" INTEGER NOT NULL,
	PRIMARY KEY("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_note_index_0"
ON "dict_note" ("ext_note_id");
CREATE TABLE IF NOT EXISTS "dict_comment" (
	"id" INTEGER NOT NULL UNIQUE,
	"comment" TEXT NOT NULL,
	PRIMARY KEY("id")
);

/* part of speech */
CREATE TABLE IF NOT EXISTS "dict_class" (
	"id" INTEGER NOT NULL UNIQUE,
	"name" TEXT NOT NULL,
	PRIMARY KEY("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_class_index_0"
ON "dict_class" ("name");
CREATE TABLE IF NOT EXISTS "dict_ref_type" (
	"id" INTEGER NOT NULL UNIQUE,
	"type" TEXT NOT NULL,
	"ascii_symbol" TEXT NOT NULL,
	"is_symmetric" INTEGER NOT NULL,
	PRIMARY KEY("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_ref_type_index_0"
ON "dict_ref_type" ("type");
CREATE TABLE IF NOT EXISTS "dict_shared_pron" (
	"id" INTEGER NOT NULL UNIQUE,
	"shared_id" INTEGER NOT NULL,
	"pron_id" INTEGER NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY ("shared_id") REFERENCES "dict_shared"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("pron_id") REFERENCES "dict_pron"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

/* Views (for manual browsing) */
CREATE VIEW trad_simp_class_pinyin_def AS
SELECT
    w.trad,
    w.simp,
    c.name AS class_name,
    GROUP_CONCAT(p.pinyin_mark ORDER BY p_s.rank, p_s.rank_relative),
    def.ext_def_id,
    def.definition
FROM dict_definition def
JOIN dict_shared s ON def.shared_id = s.id
JOIN dict_word w ON def.word_id = w.id
JOIN dict_class c ON def.class_id = c.id
LEFT JOIN dict_pron_definition pdp ON def.id = pdp.definition_id
LEFT JOIN dict_shared_pron sp ON pdp.shared_pron_id = sp.id
LEFT JOIN dict_pron p ON sp.pron_id = p.id
LEFT JOIN dict_shared p_s ON sp.shared_id = p_s.id
GROUP BY def.id
ORDER BY s.rank, s.rank_relative;

"#;

/// Get (full reference type name, is symmetric?) for the given reference type
/// A symmetric reference should exist in both directions
pub const fn get_ref_type(ref_type_char: char) -> Option<(&'static str, bool)> {
    Some(match ref_type_char {
        '=' => ("synonym-equal", true),
        '~' => ("synonym-similar", true),
        '!' => ("antonym", true),
        '?' => ("could-be-confused-with", true),
        '<' => ("part-of", false),
        '>' => ("contains", false),
        'V' => ("word-variant-of", false),
        'v' => ("character-variant-of", false),
        'M' => ("used-with-measure-word", false),
        '&' => ("collocation", false),
        'G' => ("word-group", false),
        _ => {
            return None;
        }
    })
}

/// Get (name, category, rank) of a tag, there shall not be several tags with the same rank applied to the same item
pub const fn tag_to_txt_ascii_common(ascii_tag: char) -> Option<(&'static str, &'static str, u8)> {
    Some(match ascii_tag {
        'T' => ("taiwan-only", "country", 10),
        't' => ("taiwan-chiefly", "country", 10),
        'C' => ("china-only", "country", 10),
        'c' => ("china-chiefly", "country", 10),
        '&' => ("bound-form", "bound-form", 8), // not used for references, use b (chiefly bound) and B (only as bound form)?
        'A' => ("ai-only", "ai", 6),
        'a' => ("ai-human", "ai", 6),
        'w' => ("wiktionary", "source", 3),
        'm' => ("mdbg", "source", 2),
        '+' => ("high-relevance", "relevance", 1),
        '-' => ("low-relevance", "relevance", 1),
        'x' => ("irrelevant", "relevance", 1),
        'X' => ("deleted", "relevance", 1),
        _ => {
            return None;
        }
    })
}
