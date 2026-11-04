//! Plain-text access to TSWP.StorageArchive.
//!
//! A storage keeps its text next to attribute tables keyed by UTF-16 character index. Replacing
//! the text rebuilds those tables: paragraph-level ones (style, list level, bullets) are carried
//! over paragraph by paragraph, run-level ones keep only the formatting at the start, and inline
//! objects (attachments, fields, bookmarks, ...) are dropped along with the text they annotated.

use anyhow::Result;

use super::proto::Message;

////////////////////////////////////////////////////////////////////////////////
const TEXT: u32 = 3;

/// table_para_style, table_para_data, table_list_style, table_layout_style, table_para_starts,
/// table_para_bidi
const PARAGRAPH_TABLES: [u32; 6] = [5, 6, 7, 12, 14, 24];

/// table_char_style, table_section, table_language, table_dictation, table_drop_cap_style
const RUN_TABLES: [u32; 5] = [8, 17, 19, 20, 28];

/// table_attachment, table_smartfield, table_bookmark, table_footnote, table_rubyfield,
/// table_insertion, table_deletion, table_highlight, table_overlapping_highlight,
/// table_pencil_annotation, table_tatechuyoko
const INLINE_TABLES: [u32; 11] = [9, 11, 15, 16, 18, 21, 22, 23, 25, 26, 27];

const TABLE_ENTRIES: u32 = 1;
const ENTRY_CHARACTER_INDEX: u32 = 1;

const PARAGRAPH_SEPARATOR: char = '\u{2029}';
const LINE_SEPARATOR: char = '\u{2028}';
const OBJECT_REPLACEMENT: char = '\u{fffc}';

////////////////////////////////////////////////////////////////////////////////
/// Storage text with paragraphs separated by newlines and inline objects left out
pub fn storage_text(storage: &Message) -> String {
    let text = raw_text(storage);
    let text = text.replace([PARAGRAPH_SEPARATOR, LINE_SEPARATOR], "\n");
    text.replace(OBJECT_REPLACEMENT, "")
}

////////////////////////////////////////////////////////////////////////////////
/// Replaces the storage text, newlines start new paragraphs
pub fn set_storage_text(storage: &mut Message, text: &str) -> Result<()> {
    let old_text = raw_text(storage);
    // keep whichever paragraph break the storage already uses
    let separator = if old_text.contains(PARAGRAPH_SEPARATOR) {
        PARAGRAPH_SEPARATOR
    } else {
        '\n'
    };
    let new_text: String = text
        .replace("\r\n", "\n")
        .chars()
        .map(|e| if e == '\n' { separator } else { e })
        .collect();

    let old_starts = paragraph_starts(&old_text, separator);
    let new_starts = paragraph_starts(&new_text, separator);

    storage.remove(TEXT);
    storage.set_bytes(TEXT, new_text.into_bytes());

    for table in PARAGRAPH_TABLES {
        if let Some(mut attributes) = storage.message(table)? {
            let entries = attributes.messages(TABLE_ENTRIES)?;
            attributes.set_messages(TABLE_ENTRIES, &remap_paragraphs(&entries, &old_starts, &new_starts));
            storage.set_message(table, &attributes);
        }
    }

    for table in RUN_TABLES {
        if let Some(mut attributes) = storage.message(table)? {
            let entries = attributes.messages(TABLE_ENTRIES)?;
            let first = effective_entry(&entries, 0).map(|e| at_index(e, 0));
            attributes.set_messages(TABLE_ENTRIES, first.as_slice());
            storage.set_message(table, &attributes);
        }
    }

    for table in INLINE_TABLES {
        storage.remove(table);
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
fn raw_text(storage: &Message) -> String {
    storage.strings(TEXT).concat()
}

////////////////////////////////////////////////////////////////////////////////
/// UTF-16 offsets where each paragraph starts
fn paragraph_starts(text: &str, separator: char) -> Vec<u64> {
    let mut starts = vec![0];
    let mut offset = 0;
    for character in text.chars() {
        offset += character.len_utf16() as u64;
        if character == separator {
            starts.push(offset);
        }
    }
    starts
}

////////////////////////////////////////////////////////////////////////////////
/// New paragraph `n` takes the attributes old paragraph `n` had, extra paragraphs repeat the last one
fn remap_paragraphs(entries: &[Message], old_starts: &[u64], new_starts: &[u64]) -> Vec<Message> {
    let mut remapped = Vec::new();
    for (index, start) in new_starts.iter().enumerate() {
        let old_start = old_starts[index.min(old_starts.len() - 1)];
        if let Some(entry) = effective_entry(entries, old_start) {
            remapped.push(at_index(entry, *start));
        }
    }
    remapped
}

////////////////////////////////////////////////////////////////////////////////
/// The entry in effect at a character index: the last one starting at or before it
fn effective_entry(entries: &[Message], index: u64) -> Option<&Message> {
    let entries = entries
        .iter()
        .filter(|e| e.varint(ENTRY_CHARACTER_INDEX).unwrap_or_default() <= index);
    entries.max_by_key(|e| e.varint(ENTRY_CHARACTER_INDEX).unwrap_or_default())
}

////////////////////////////////////////////////////////////////////////////////
fn at_index(entry: &Message, index: u64) -> Message {
    let mut entry = entry.clone();
    entry.set_varint(ENTRY_CHARACTER_INDEX, index);
    entry
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::keynote::format::proto::{make_reference, reference};

    fn table(entries: &[(u64, u64)]) -> Message {
        let mut table = Message::default();
        for (index, style) in entries {
            let mut entry = Message::default();
            entry.set_varint(ENTRY_CHARACTER_INDEX, *index);
            entry.set_message(2, &make_reference(*style));
            table.push_message(TABLE_ENTRIES, &entry);
        }
        table
    }

    fn entries(storage: &Message, number: u32) -> Vec<(u64, u64)> {
        let table = storage.message(number).unwrap().unwrap();
        let entries = table.messages(TABLE_ENTRIES).unwrap();
        let entries = entries.iter().map(|e| {
            let index = e.varint(ENTRY_CHARACTER_INDEX).unwrap();
            (index, reference(e, 2).unwrap().unwrap())
        });
        entries.collect()
    }

    #[test]
    fn carries_paragraph_styles_over() {
        let mut storage = Message::default();
        storage.set_bytes(TEXT, "Level one\nLevel two".as_bytes().to_vec());
        storage.set_message(5, &table(&[(0, 100), (10, 200)]));
        storage.set_message(8, &table(&[(0, 300), (3, 400)]));
        storage.set_message(9, &table(&[(4, 500)]));

        set_storage_text(&mut storage, "A\nB\nC").unwrap();

        assert_eq!(storage_text(&storage), "A\nB\nC");
        assert_eq!(entries(&storage, 5), vec![(0, 100), (2, 200), (4, 200)]);
        assert_eq!(entries(&storage, 8), vec![(0, 300)]);
        assert!(!storage.has(9));
    }

    #[test]
    fn keeps_the_paragraph_separator_in_use() {
        let mut storage = Message::default();
        storage.set_bytes(TEXT, "One\u{2029}Two".as_bytes().to_vec());
        storage.set_message(5, &table(&[(0, 1), (4, 2)]));

        set_storage_text(&mut storage, "Ünï\ncödé 🙂\nend").unwrap();

        assert_eq!(storage.string(TEXT).unwrap(), "Ünï\u{2029}cödé 🙂\u{2029}end");
        // offsets are UTF-16 code units, the emoji takes two
        assert_eq!(entries(&storage, 5), vec![(0, 1), (4, 2), (12, 2)]);
    }

    #[test]
    fn empty_text_keeps_the_first_paragraph_style() {
        let mut storage = Message::default();
        storage.set_bytes(TEXT, b"Title".to_vec());
        storage.set_message(5, &table(&[(0, 7)]));

        set_storage_text(&mut storage, "").unwrap();

        assert_eq!(storage_text(&storage), "");
        assert_eq!(entries(&storage, 5), vec![(0, 7)]);
    }
}
