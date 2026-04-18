pub const WORD_SEP: &str = "／";
pub const ITEMS_SEP: &str = ";";

pub const APPROX_TXT_FILE_SIZE: usize = 20_000_000;

// TODO ? PRAGMA OPTIMIZE; ANALYZE; ?

pub const DB_SCHEMA: &str = r#"

PRAGMA user_version = 4;

/* ------------------- generated start ---------------------- */

/* Schema of a dictionary for Mandarin Chinese. The same data can also be represented as a text file. Some fields in this table exist mainly in order to preserve information of the text representation or make the conversions more convenient.

Each entry consists of a word (dict_word), which can have several definitions (dict_definition). Each definition must have one or more pronunciations (dict_pron) and a class (dict_class), which corresponds to the part of speech.
Words and definitions can be linked (dict_reference), e.g. to indicate synonyms, antonyms etc..
All words, definitions, pronunciations and references can have zero or more tags (dict_tag), and zero or one comment or note. A comment is for meta data, not for a user of the dictionary. A note can provide additional information to the user of the dictionary.

The order of the text file is preserved using the rank field (dict_shared). New items can be inserted using rank_relative. The fields ascii_symbol (dict_ref_type, dict_tag) refer to the symbol used in the text representation.



ext_def_id is a constant unique id within the scope of all definitions for the same word. It is used for references or internal and external links, similar to ext_note_id */
CREATE TABLE IF NOT EXISTS "dict_definition" (
	"id" INTEGER NOT NULL UNIQUE,
	-- constant id, used for referencing definitions in the text representation of from external sources
	"ext_def_id" INTEGER NOT NULL,
	"shared_id" INTEGER NOT NULL,
	"word_id" INTEGER NOT NULL,
	"definition" TEXT NOT NULL,
	"class_id" INTEGER NOT NULL,
	"parent_id" INTEGER,
	PRIMARY KEY("id"),
	FOREIGN KEY ("word_id") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("shared_id") REFERENCES "dict_shared"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("class_id") REFERENCES "dict_class"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("parent_id") REFERENCES "dict_definition"("id")
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
	-- word in traditional characters
	"trad" TEXT NOT NULL,
	-- word in simplified characters
	"simp" TEXT NOT NULL,
	-- link to the main variant if not NULL, the entry will have the same shared_id as the main variant and no definitions should link to this entry, only to the main variant
	"variant_of" INTEGER,
	PRIMARY KEY("id"),
	FOREIGN KEY ("shared_id") REFERENCES "dict_shared"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("variant_of") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_word_index_0"
ON "dict_word" ("trad", "simp");

CREATE INDEX IF NOT EXISTS "dict_word_index_1"
ON "dict_word" ("variant_of");
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
/* Relationship from a to b, e.g. measureword, antonym, synonym or variant.
Link to dict_ref_type uses the ascii_symbol so that the symbol is available in dict_reference directly to enable a partial unique index, exluding certain reference types from the unique constraint */
CREATE TABLE IF NOT EXISTS "dict_reference" (
	"id" INTEGER NOT NULL UNIQUE,
	"shared_id" INTEGER NOT NULL,
	"ascii_symbol" TEXT NOT NULL,
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
	FOREIGN KEY ("ascii_symbol") REFERENCES "dict_ref_type"("ascii_symbol")
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
ON "dict_ref_type" ("ascii_symbol");
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

/* Example sentence for a definition, word_id is included to ensure uniqueness of word + ext_sen_id */
CREATE TABLE IF NOT EXISTS "dict_sentence" (
	"id" INTEGER NOT NULL UNIQUE,
	"ext_sent_id" INTEGER NOT NULL,
	"shared_id" INTEGER NOT NULL,
	"for_word_id" INTEGER NOT NULL,
	"for_definition_id" INTEGER NOT NULL,
	"translation" TEXT NOT NULL,
	PRIMARY KEY("id"),
	FOREIGN KEY ("shared_id") REFERENCES "dict_shared"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("for_word_id") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("for_definition_id") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_example_index_0"
ON "dict_sentence" ("word_id", "ext_sen_id");

CREATE INDEX IF NOT EXISTS "dict_sentence_index_1"
ON "dict_sentence" ("for_definition_id");
/* Translations for any string in the dictionary which is seen by the user and in English in the original dictionary. */
CREATE TABLE IF NOT EXISTS "dict_translation" (
	"id" INTEGER NOT NULL UNIQUE,
	"revision_id" INTEGER NOT NULL,
	"txt" TEXT NOT NULL,
	"edited_on" DATE,
	"dict_note_id" INTEGER,
	"dict_def_id" INTEGER,
	"dict_sent_id" INTEGER,
	"dict_tag_id" INTEGER,
	"dict_ref_type_id" INTEGER,
	"dict_class_id" INTEGER,
	PRIMARY KEY("id"),
	FOREIGN KEY ("dict_ref_type_id") REFERENCES "dict_ref_type"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("dict_note_id") REFERENCES "dict_note"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("dict_def_id") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("dict_sent_id") REFERENCES "dict_sentence"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("dict_tag_id") REFERENCES "dict_tag"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("dict_class_id") REFERENCES "dict_class"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("revision_id") REFERENCES "dict_translation_revision"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_note_index_0"
ON "dict_translation" ("dict_note_id");

CREATE INDEX IF NOT EXISTS "dict_translation_index_1"
ON "dict_translation" ("dict_def_id");

CREATE INDEX IF NOT EXISTS "dict_translation_index_2"
ON "dict_translation" ("dict_sen_id");

CREATE INDEX IF NOT EXISTS "dict_translation_index_3"
ON "dict_translation" ("dict_tag_id");

CREATE INDEX IF NOT EXISTS "dict_translation_index_4"
ON "dict_translation" ("dict_ref_type_id");

CREATE INDEX IF NOT EXISTS "dict_translation_index_5"
ON "dict_translation" ("dict_class_id");
/* In case of separable words, the part_of_word/definition_id is used to point to the word/definition of another word than the word_id, in that case the word_id indicates the word as it should be shown in the sentence and it must be a subset of the word indicated by the part_of_word/definition_id. The definition_id can still point to a definition of the word_id, e.g. in order to easily find the correct pronunciation.
The word_rank is the position of the word in the sentence, to get a correct sentence the words should be read ordered by word_rank.
ascii_txt is used for: spaces between words, numbers, English names, etc.
Only one of word_id and ascii_txt must be NOT NULL for each row. */
CREATE TABLE IF NOT EXISTS "dict_sentence_word" (
	"sentence_id" INTEGER NOT NULL,
	"word_rank" INTEGER NOT NULL,
	"word_id" INTEGER,
	"definition_id" INTEGER CHECK(definition_id IS NULL OR word_id IS NOT NULL),
	"part_of_word_id" INTEGER,
	"part_of_definition_id" INTEGER,
	"ascii_txt" TEXT CHECK((word_id IS NULL) != (ascii_txt IS NULL)),
	PRIMARY KEY("sentence_id", "word_rank"),
	FOREIGN KEY ("sentence_id") REFERENCES "dict_sentence"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("word_id") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("definition_id") REFERENCES "dict_definition"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION,
	FOREIGN KEY ("part_of_word_id") REFERENCES "dict_word"("id")
	ON UPDATE NO ACTION ON DELETE NO ACTION
);

CREATE UNIQUE INDEX IF NOT EXISTS "dict_sen_word_index_0"
ON "dict_sentence_word" ("sentence_id", "word_rank");
CREATE TABLE IF NOT EXISTS "dict_translation_revision" (
	"id" INTEGER NOT NULL UNIQUE,
	"lang" TEXT NOT NULL,
	"date" DATE NOT NULL,
	"info_json" TEXT NOT NULL,
	PRIMARY KEY("id")
);


/* ------------------- generated end ---------------------- */

/* don't enforce unique contraint on decomposition references, since the same component might appear multiple times */
CREATE UNIQUE INDEX IF NOT EXISTS "dict_reference_index_unique"
ON "dict_reference" ("ascii_symbol", "word_id_src", "definition_id_src", "word_id_dst", "definition_id_dst")
WHERE ascii_symbol != ">";


"#;


/// Get (full reference type name, is symmetric, sort by destination rank, relative rank of reference type) for the given reference type
/// A symmetric reference should exist in both directions
pub const fn get_ref_type(ref_type_char: char) -> Option<(&'static str, bool, bool, u8)> {
    Some(match ref_type_char {
		'>' => ("contains", false, false, 1), // source word contains destination word (or definition), duplicates allowed
		'~' => ("definition-suffix", false, false, 2), // used instead of the (～X) convention in the wiktionary definitions
        'C' => ("used-with-classifier", false, false, 3), // source word is used with destination classifier / measure word
		'=' => ("synonym", true, true, 4), // source has the same or a very similar meaning as destination
		'%' => ("cross-strait", true, true, 6), // links two definitions with the same meaning but one is used in Taiwan, the other in China
		'?' => ("could-be-confused-with", true, true, 8), // words which are easily mixed up but not synonyms
        '!' => ("antonym", true, true, 10), // links words and definitions with opposite meanings
        'V' => ("word-variant-of", false, false, 12), // source is a variant of a destination, with some differences in pronunciations or definitions
        'v' => ("character-variant-of", false, false, 14),
        '<' => ("part-of", false, true, 18), // source word is part of destination word (or definition)
		'{' => ("collocation-before", false, true, 20), // destination words usually appear before source word
		'.' => ("collocation-within", false, true, 21), // destination words usually appear within source word (e.g. separable verbs, grammar patterns)
		'}' => ("collocation-after", false, true, 22), // destination words usually appear after source word
		'R' => ("relevant-reference", true, false, 23), // anything particularly relevant
        'G' => ("word-group", true, false, 24), // groups like North, South, East, West etc.
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
        '&' => ("in-compounds", "in-compounds", 8),
        'i' => ("irregular", "checks", 7), // skip automatic checks
        'A' => ("ai-only", "ai", 6), // content was generated/checked by multiple LLMs
        'a' => ("ai-partly", "ai", 6), // content was partly generated/checked by LLMs (e.g. matching word definitions for references)
		'E' => ("explanation-only", "explanation", 9), // the definition is not a translation but an explanation
		'e' => ("explanation-partly", "explanation", 9), // the definition contains both a translation and an explanation
        'w' => ("wiktionary", "source", 3),
        'm' => ("mdbg", "source", 2),
        '*' => ("active-candidate", "relevance", 1), // candidate for a + tag (involving LLMs)
		'+' => ("active", "relevance", 1), // definition/pronunciation/... can be used in active vocabulary
        '-' => ("extended", "relevance", 1), // extended (passive) vocabulary
        'x' => ("excluded", "relevance", 1), // excluded from the dictionary
        'X' => ("deleted", "relevance", 1),
        _ => {
            return None;
        }
    })
}
