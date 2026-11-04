//! Styled text in TSWP.StorageArchive: paragraphs of runs with character and paragraph
//! styling, bullets, list levels and hyperlinks.
//!
//! A storage keeps attribute tables keyed by UTF-16 offset (see text.rs). Styling goes through
//! style variations: a run's character properties become a variation of the theme's "None"
//! character style, a paragraph's alignment and spacing a variation of its paragraph style.
//! Entries without a style end a styled run. Links are a "Link" character style variation plus
//! a TSWP.HyperlinkFieldArchive in the smart field table, the way Keynote writes them.

use anyhow::{Result, anyhow, bail};

use super::document::KeynoteDocument;
use super::drawing::{Color, float};
use super::iwa::IwaObject;
use super::proto::{Message, make_reference, reference};
use super::style::{StyleProps, TYPE_CHARACTER_STYLE, TYPE_LIST_STYLE, char_props, para_props};
use super::text::set_storage_text;

////////////////////////////////////////////////////////////////////////////////
pub const TYPE_STORAGE: u32 = 2001;
const TYPE_HYPERLINK: u32 = 2032;

/// TSWP.StorageArchive
pub mod storage {
    pub const STYLESHEET: u32 = 2;
    pub const TEXT: u32 = 3;
    pub const PARA_STYLE: u32 = 5;
    pub const PARA_DATA: u32 = 6;
    pub const LIST_STYLE: u32 = 7;
    pub const CHAR_STYLE: u32 = 8;
    pub const IN_DOCUMENT: u32 = 10;
    pub const SMART_FIELD: u32 = 11;
    pub const PARA_STARTS: u32 = 14;
    pub const PARA_BIDI: u32 = 24;
    pub const DROP_CAP: u32 = 28;

    pub const ENTRIES: u32 = 1;
    pub const ENTRY_INDEX: u32 = 1;
    pub const ENTRY_OBJECT: u32 = 2;
    pub const ENTRY_FIRST: u32 = 2;
    pub const ENTRY_SECOND: u32 = 3;
}

/// TSWP.HyperlinkFieldArchive
mod hyperlink {
    pub const SMART_FIELD: u32 = 1;
    pub const URL: u32 = 2;
    pub const SMART_FIELD_IDENTIFIER: u32 = 1;
}

const LINE_SEPARATOR: char = '\u{2028}';

/// Monospace font of the `monospace` shortcut, present on every Mac
pub const MONOSPACE_FONT: &str = "Menlo";

////////////////////////////////////////////////////////////////////////////////
// Model
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    Normal,
    Super,
    Sub,
}

/// Character formatting, None leaves a property to the paragraph style
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunStyle {
    pub font: Option<String>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub color: Option<Color>,
    pub highlight: Option<Color>,
    pub baseline: Option<Baseline>,
    /// URL the run links to
    pub link: Option<String>,
}

impl RunStyle {
    ////////////////////////////////////////////////////////////////////////////////
    /// `self` with every property `over` sets replaced
    pub fn merged(&self, over: &RunStyle) -> RunStyle {
        RunStyle {
            font: over.font.clone().or_else(|| self.font.clone()),
            size: over.size.or(self.size),
            bold: over.bold.or(self.bold),
            italic: over.italic.or(self.italic),
            underline: over.underline.or(self.underline),
            strike: over.strike.or(self.strike),
            color: over.color.or(self.color),
            highlight: over.highlight.or(self.highlight),
            baseline: over.baseline.or(self.baseline),
            link: over.link.clone().or_else(|| self.link.clone()),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// TSWP.CharacterStylePropertiesArchive with what this style sets, links aside
    pub fn properties(&self) -> Message {
        let mut message = Message::default();
        if let Some(bold) = self.bold {
            message.set_bool(char_props::BOLD, bold);
        }
        if let Some(italic) = self.italic {
            message.set_bool(char_props::ITALIC, italic);
        }
        if let Some(size) = self.size {
            message.set(char_props::SIZE, float(size));
        }
        if let Some(font) = &self.font {
            message.set_bytes(char_props::FONT, font.clone().into_bytes());
        }
        if let Some(color) = self.color {
            message.set_message(char_props::COLOR, &color.message());
            let mut fill = Message::default();
            fill.set_message(1, &color.message());
            message.set_message(char_props::FILL, &fill);
        }
        if let Some(baseline) = self.baseline {
            let value = match baseline {
                Baseline::Normal => 0,
                Baseline::Super => 1,
                Baseline::Sub => 2,
            };
            message.set_varint(char_props::SUPERSCRIPT, value);
        }
        if let Some(underline) = self.underline {
            message.set_varint(char_props::UNDERLINE, u64::from(underline));
        }
        if let Some(strike) = self.strike {
            message.set_varint(char_props::STRIKE, u64::from(strike));
        }
        if let Some(highlight) = self.highlight {
            message.set_message(char_props::BACKGROUND_COLOR, &highlight.message());
        }
        message
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn is_empty(&self) -> bool {
        self == &RunStyle::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub style: RunStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_lowercase().as_str() {
            "left" | "start" => Ok(Align::Left),
            "center" | "centre" | "middle" => Ok(Align::Center),
            "right" | "end" => Ok(Align::Right),
            "justify" | "justified" => Ok(Align::Justify),
            other => bail!("unknown alignment {other:?}, use left, center, right or justify"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bullet {
    None,
    Disc,
    Number,
    Letter,
}

impl Bullet {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_lowercase().as_str() {
            "none" => Ok(Bullet::None),
            "disc" | "bullet" | "•" | "-" => Ok(Bullet::Disc),
            "number" | "numbered" | "1" => Ok(Bullet::Number),
            "letter" | "lettered" | "a" => Ok(Bullet::Letter),
            other => bail!("unknown bullet {other:?}, use none, disc, number or letter"),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Name of the theme's list style
    fn list_style_name(&self) -> &'static str {
        match self {
            Bullet::None => "None",
            Bullet::Disc => "Bullet",
            Bullet::Number => "Numbered",
            Bullet::Letter => "Lettered",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSpacing {
    /// multiple of the font's line height
    Relative(f32),
    /// points from baseline to baseline
    Exact(f32),
}

/// Paragraph formatting, None leaves a property to the paragraph style
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Paragraph {
    pub runs: Vec<TextRun>,
    pub align: Option<Align>,
    pub bullet: Option<Bullet>,
    /// 0-based list level
    pub level: Option<u32>,
    pub line_spacing: Option<LineSpacing>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub left_indent: Option<f32>,
    pub first_line_indent: Option<f32>,
}

impl Paragraph {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn plain(text: &str) -> Self {
        Self {
            runs: vec![TextRun {
                text: text.to_string(),
                style: RunStyle::default(),
            }],
            ..Default::default()
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Text with line breaks inside the paragraph as U+2028
    fn text(&self) -> String {
        let text: String = self.runs.iter().map(|e| e.text.as_str()).collect();
        text.replace("\r\n", "\n").replace('\n', &LINE_SEPARATOR.to_string())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// TSWP.ParagraphStylePropertiesArchive with what this paragraph sets
    fn properties(&self) -> Message {
        let mut message = Message::default();
        if let Some(align) = self.align {
            let value = match align {
                Align::Left => para_props::ALIGN_LEFT,
                Align::Center => para_props::ALIGN_CENTER,
                Align::Right => para_props::ALIGN_RIGHT,
                Align::Justify => para_props::ALIGN_JUSTIFY,
            };
            message.set_varint(para_props::ALIGNMENT, value);
        }
        if let Some(indent) = self.first_line_indent {
            message.set(para_props::FIRST_LINE_INDENT, float(indent));
        }
        if let Some(indent) = self.left_indent {
            message.set(para_props::LEFT_INDENT, float(indent));
        }
        if let Some(spacing) = self.line_spacing {
            let (mode, amount) = match spacing {
                LineSpacing::Relative(e) => (para_props::MODE_RELATIVE, e),
                LineSpacing::Exact(e) => (para_props::MODE_EXACT, e),
            };
            let mut archive = Message::default();
            archive.set_varint(para_props::LINE_SPACING_MODE, mode);
            archive.set(para_props::LINE_SPACING_AMOUNT, float(amount));
            message.set_message(para_props::LINE_SPACING, &archive);
        }
        if let Some(space) = self.space_after {
            message.set(para_props::SPACE_AFTER, float(space));
        }
        if let Some(space) = self.space_before {
            message.set(para_props::SPACE_BEFORE, float(space));
        }
        message
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Plain text as paragraphs, one per line
pub fn plain_paragraphs(text: &str) -> Vec<Paragraph> {
    text.replace("\r\n", "\n").split('\n').map(Paragraph::plain).collect()
}

////////////////////////////////////////////////////////////////////////////////
// Markdown-lite
////////////////////////////////////////////////////////////////////////////////

/// Paragraphs of a Markdown-lite text, one per line: `**bold**`, `*italic*` or `_italic_`,
/// `~~strike~~`, `` `code` `` (monospace), `[text](url)`, list items starting with "- ", "* "
/// or "1. ", nested by two spaces per level. `base` styles every run.
pub fn markdown_paragraphs(text: &str, base: &RunStyle, code: &RunStyle) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    for line in text.replace("\r\n", "\n").split('\n') {
        let indent = line.chars().take_while(|e| *e == ' ').count();
        let trimmed = &line[indent..];
        let (bullet, content) = if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            (Some(Bullet::Disc), rest)
        } else if let Some(rest) = trimmed.strip_prefix("• ") {
            (Some(Bullet::Disc), rest)
        } else if let Some((number, rest)) = trimmed.split_once(". ")
            && !number.is_empty()
            && number.chars().all(|e| e.is_ascii_digit())
        {
            (Some(Bullet::Number), rest)
        } else {
            (None, trimmed)
        };

        let level = (bullet.is_some() && indent >= 2).then_some((indent / 2) as u32);
        let content = if bullet.is_some() { content } else { line };
        paragraphs.push(Paragraph {
            runs: markdown_runs(content, base, code),
            bullet,
            level,
            ..Default::default()
        });
    }
    paragraphs
}

////////////////////////////////////////////////////////////////////////////////
/// Inline Markdown-lite of one line
pub fn markdown_runs(text: &str, base: &RunStyle, code: &RunStyle) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut current = String::new();
    let (mut bold, mut italic, mut strike) = (false, false, false);
    let mut italic_marker = '*';

    let style = |bold: bool, italic: bool, strike: bool| RunStyle {
        bold: bold.then_some(true),
        italic: italic.then_some(true),
        strike: strike.then_some(true),
        ..Default::default()
    };
    let flush = |runs: &mut Vec<TextRun>, current: &mut String, style: RunStyle| {
        if current.is_empty() {
            return;
        }
        let style = base.merged(&style);
        match runs.last_mut() {
            Some(last) if last.style == style => last.text.push_str(current),
            _ => runs.push(TextRun {
                text: current.clone(),
                style,
            }),
        }
        current.clear();
    };

    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let rest: String = chars[index..].iter().collect();
        let character = chars[index];

        if character == '\\' && index + 1 < chars.len() {
            current.push(chars[index + 1]);
            index += 2;
            continue;
        }
        if rest.starts_with("**") || rest.starts_with("__") {
            flush(&mut runs, &mut current, style(bold, italic, strike));
            bold = !bold;
            index += 2;
            continue;
        }
        if rest.starts_with("~~") {
            flush(&mut runs, &mut current, style(bold, italic, strike));
            strike = !strike;
            index += 2;
            continue;
        }
        if (character == '*' || character == '_') && (!italic || character == italic_marker) {
            // an underscore inside a word is not emphasis (snake_case)
            let inside_word = character == '_'
                && index > 0
                && chars[index - 1].is_alphanumeric()
                && chars.get(index + 1).is_some_and(|e| e.is_alphanumeric());
            if !inside_word {
                flush(&mut runs, &mut current, style(bold, italic, strike));
                italic = !italic;
                italic_marker = character;
                index += 1;
                continue;
            }
        }
        if character == '`'
            && let Some(end) = chars[index + 1..].iter().position(|e| *e == '`')
        {
            flush(&mut runs, &mut current, style(bold, italic, strike));
            let code_text: String = chars[index + 1..index + 1 + end].iter().collect();
            runs.push(TextRun {
                text: code_text,
                style: base.merged(&style(bold, italic, strike)).merged(code),
            });
            index += end + 2;
            continue;
        }
        if character == '['
            && let Some(close) = chars[index..].iter().position(|e| *e == ']')
            && chars.get(index + close + 1) == Some(&'(')
            && let Some(end) = chars[index + close + 1..].iter().position(|e| *e == ')')
        {
            flush(&mut runs, &mut current, style(bold, italic, strike));
            let label: String = chars[index + 1..index + close].iter().collect();
            let url: String = chars[index + close + 2..index + close + 1 + end].iter().collect();
            let link = RunStyle {
                link: Some(url.trim().to_string()),
                ..style(bold, italic, strike)
            };
            runs.push(TextRun {
                text: label,
                style: base.merged(&link),
            });
            index += close + 1 + end + 1;
            continue;
        }

        current.push(character);
        index += 1;
    }
    flush(&mut runs, &mut current, style(bold, italic, strike));
    if runs.is_empty() {
        runs.push(TextRun {
            text: String::new(),
            style: base.clone(),
        });
    }
    runs
}

////////////////////////////////////////////////////////////////////////////////
// Reading
////////////////////////////////////////////////////////////////////////////////

/// A paragraph as stored, with its styles for rendering
#[derive(Debug, Clone)]
pub struct StoredParagraph {
    pub text: String,
    pub paragraph_style: Option<u64>,
    pub list_style: Option<u64>,
    pub level: u32,
    /// (UTF-16 range within the paragraph, character style)
    pub runs: Vec<(usize, usize, Option<u64>)>,
}

/// An attribute table entry: UTF-16 offset, object, first and second value
type TableEntry = (u64, Option<u64>, Option<u64>, Option<u64>);

/// Attribute table entries sorted by offset
fn table_entries(storage_message: &Message, table: u32) -> Result<Vec<TableEntry>> {
    let Some(table) = storage_message.message(table)? else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for entry in table.messages(storage::ENTRIES)? {
        let index = entry.varint(storage::ENTRY_INDEX).unwrap_or(0);
        entries.push((
            index,
            reference(&entry, storage::ENTRY_OBJECT).unwrap_or(None),
            entry.varint(storage::ENTRY_FIRST),
            entry.varint(storage::ENTRY_SECOND),
        ));
    }
    entries.sort_by_key(|e| e.0);
    Ok(entries)
}

////////////////////////////////////////////////////////////////////////////////
/// The entry in effect at `index`: the last one starting at or before it
fn entry_at(entries: &[TableEntry], index: u64) -> Option<&TableEntry> {
    entries.iter().rev().find(|e| e.0 <= index)
}

////////////////////////////////////////////////////////////////////////////////
/// The object of the last entry at or before `index` that has one ("no change" entries skipped)
fn object_at(entries: &[TableEntry], index: u64) -> Option<u64> {
    entries.iter().rev().filter(|e| e.0 <= index).find_map(|e| e.1)
}

////////////////////////////////////////////////////////////////////////////////
/// The object of the entry in effect at `index`, None when that entry has none
fn run_object_at(entries: &[TableEntry], index: u64) -> Option<u64> {
    entry_at(entries, index).and_then(|e| e.1)
}


////////////////////////////////////////////////////////////////////////////////
/// Paragraphs of a storage with their styles, inline objects left out
pub fn stored_paragraphs(storage_message: &Message) -> Result<Vec<StoredParagraph>> {
    let text = storage_message.strings(storage::TEXT).concat();
    let para_styles = table_entries(storage_message, storage::PARA_STYLE)?;
    let list_styles = table_entries(storage_message, storage::LIST_STYLE)?;
    let para_data = table_entries(storage_message, storage::PARA_DATA)?;
    let char_styles = table_entries(storage_message, storage::CHAR_STYLE)?;

    let mut paragraphs = Vec::new();
    let mut start = 0u64;
    for piece in text.split(['\n', '\u{2029}']) {
        let length: u64 = piece.chars().map(|e| e.len_utf16() as u64).sum();
        let level = entry_at(&para_data, start).and_then(|e| e.2).unwrap_or(0) as u32;

        // run boundaries within the paragraph
        let mut boundaries: Vec<u64> = char_styles
            .iter()
            .map(|e| e.0)
            .filter(|e| *e > start && *e < start + length)
            .collect();
        boundaries.insert(0, start);
        boundaries.push(start + length);
        boundaries.dedup();
        let mut runs = Vec::new();
        for pair in boundaries.windows(2) {
            runs.push((
                (pair[0] - start) as usize,
                (pair[1] - start) as usize,
                run_object_at(&char_styles, pair[0]),
            ));
        }

        paragraphs.push(StoredParagraph {
            text: piece.replace('\u{fffc}', " "),
            paragraph_style: object_at(&para_styles, start),
            list_style: object_at(&list_styles, start),
            level,
            runs,
        });
        start += length + 1;
    }
    Ok(paragraphs)
}

////////////////////////////////////////////////////////////////////////////////
// Writing
////////////////////////////////////////////////////////////////////////////////

fn entry(index: u64, object: Option<u64>) -> Message {
    let mut entry = Message::default();
    entry.set_varint(storage::ENTRY_INDEX, index);
    if let Some(object) = object {
        entry.set_message(storage::ENTRY_OBJECT, &make_reference(object));
    }
    entry
}

fn data_entry(index: u64, first: u64, second: u64) -> Message {
    let mut entry = Message::default();
    entry.set_varint(storage::ENTRY_INDEX, index);
    entry.set_varint(storage::ENTRY_FIRST, first);
    entry.set_varint(storage::ENTRY_SECOND, second);
    entry
}

fn table(entries: &[Message]) -> Message {
    let mut table = Message::default();
    table.set_messages(storage::ENTRIES, entries);
    table
}

////////////////////////////////////////////////////////////////////////////////
/// A new, empty storage message for a text box or shape, in the theme's `paragraph_style`
pub fn new_storage_message(stylesheet: u64, paragraph_style: u64, list_style: Option<u64>) -> Message {
    let mut message = Message::default();
    message.set_message(storage::STYLESHEET, &make_reference(stylesheet));
    message.set_bytes(storage::TEXT, Vec::new());
    message.set_message(storage::PARA_STYLE, &table(&[entry(0, Some(paragraph_style))]));
    message.set_message(storage::PARA_DATA, &table(&[data_entry(0, 0, 0)]));
    if let Some(list_style) = list_style {
        message.set_message(storage::LIST_STYLE, &table(&[entry(0, Some(list_style))]));
    }
    message.set_bool(storage::IN_DOCUMENT, true);
    message.set_message(storage::PARA_STARTS, &table(&[data_entry(0, 0, 0)]));
    message.set_message(storage::PARA_BIDI, &table(&[data_entry(0, 0, 0)]));
    let mut drop_cap = Message::default();
    drop_cap.set_varint(storage::ENTRY_INDEX, 0);
    message.set_message(storage::DROP_CAP, &table(&[drop_cap]));
    message
}

impl KeynoteDocument {
    ////////////////////////////////////////////////////////////////////////////////
    /// Replaces a storage's text with styled paragraphs. Paragraph n keeps the paragraph and
    /// list style paragraph n had (the last one's for extra paragraphs) unless it sets its own.
    pub(super) fn set_rich_text(&mut self, storage_id: u64, paragraphs: &[Paragraph]) -> Result<()> {
        let paragraphs: Vec<Paragraph> = if paragraphs.is_empty() {
            vec![Paragraph::default()]
        } else {
            paragraphs.to_vec()
        };
        let texts: Vec<String> = paragraphs.iter().map(Paragraph::text).collect();
        let text = texts.join("\n");

        // carry paragraph styles over like a plain text edit does
        let mut message = self.package.message(storage_id)?;
        set_storage_text(&mut message, &text)?;
        let separator_length = 1u64;

        let mut starts = Vec::new();
        let mut offset = 0u64;
        for text in &texts {
            starts.push(offset);
            offset += text.chars().map(|e| e.len_utf16() as u64).sum::<u64>() + separator_length;
        }

        let para_entries = table_entries(&message, storage::PARA_STYLE)?;
        let list_entries = table_entries(&message, storage::LIST_STYLE)?;
        let data_entries = table_entries(&message, storage::PARA_DATA)?;

        // paragraph styles
        let mut added = Vec::new();
        let mut para_table = Vec::new();
        for (paragraph, start) in paragraphs.iter().zip(&starts) {
            let base = object_at(&para_entries, *start)
                .ok_or_else(|| anyhow!("the text has no paragraph style to start from"))?;
            let overrides = paragraph.properties();
            let style = if overrides.fields.is_empty() {
                base
            } else {
                let props = StyleProps {
                    groups: vec![Message::default(), overrides],
                };
                self.style_variation(base, &props)?
            };
            added.push(style);
            para_table.push(entry(*start, Some(style)));
        }
        message.set_message(storage::PARA_STYLE, &table(&para_table));

        // bullets and levels
        if paragraphs.iter().any(|e| e.bullet.is_some()) {
            let mut list_table = Vec::new();
            for (paragraph, start) in paragraphs.iter().zip(&starts) {
                let style = match paragraph.bullet {
                    Some(bullet) => Some(self.list_style(bullet)?),
                    None => object_at(&list_entries, *start),
                };
                added.extend(style);
                list_table.push(entry(*start, style));
            }
            message.set_message(storage::LIST_STYLE, &table(&list_table));
        }
        if paragraphs.iter().any(|e| e.level.is_some()) {
            let mut data_table = Vec::new();
            for (paragraph, start) in paragraphs.iter().zip(&starts) {
                let current = entry_at(&data_entries, *start);
                let level = paragraph
                    .level
                    .map(u64::from)
                    .or_else(|| current.and_then(|e| e.2))
                    .unwrap_or(0);
                let second = current.and_then(|e| e.3).unwrap_or(0);
                data_table.push(data_entry(*start, level.min(8), second));
            }
            message.set_message(storage::PARA_DATA, &table(&data_table));
        }

        // runs
        let styled = paragraphs.iter().flat_map(|e| &e.runs).any(|e| !e.style.is_empty());
        let mut links = Vec::new();
        if styled {
            let mut char_table: Vec<Message> = Vec::new();
            let mut last: Option<Option<u64>> = None;
            for (paragraph, start) in paragraphs.iter().zip(&starts) {
                let mut offset = *start;
                for run in &paragraph.runs {
                    let length: u64 = run.text.chars().map(|e| e.len_utf16() as u64).sum();
                    if length == 0 {
                        continue;
                    }
                    let style = self.character_style(&run.style)?;
                    if last != Some(style) {
                        char_table.push(entry(offset, style));
                        last = Some(style);
                    }
                    added.extend(style);
                    if let Some(url) = &run.style.link {
                        links.push((offset, offset + length, url.clone()));
                    }
                    offset += length;
                }
            }
            if char_table.first().and_then(|e| e.varint(storage::ENTRY_INDEX)) != Some(0) {
                char_table.insert(0, entry(0, None));
            }
            message.set_message(storage::CHAR_STYLE, &table(&char_table));
        }

        if !links.is_empty() {
            let component = self.package.component_of(storage_id)?.to_string();
            let version = self.object_version(TYPE_HYPERLINK).unwrap_or_else(|| {
                self.package.object(storage_id).map(|e| e.version()).unwrap_or_default()
            });
            let mut field_table = vec![entry(0, None)];
            for (start, end, url) in links {
                let identifier = self.package.last_object_identifier()? + 1;
                let mut smart_field = Message::default();
                let uuid = super::document::format_uuid(rand::random(), rand::random());
                smart_field.set_bytes(hyperlink::SMART_FIELD_IDENTIFIER, uuid.into_bytes());
                let mut link = Message::default();
                link.set_message(hyperlink::SMART_FIELD, &smart_field);
                link.set_bytes(hyperlink::URL, url.into_bytes());
                self.package
                    .insert(&component, IwaObject::new(identifier, TYPE_HYPERLINK, &version, &link, &[]))?;
                self.package.set_last_object_identifier(identifier)?;

                if field_table.last().and_then(|e| e.varint(storage::ENTRY_INDEX)) == Some(start) {
                    field_table.pop();
                }
                field_table.push(entry(start, Some(identifier)));
                field_table.push(entry(end, None));
                added.push(identifier);
            }
            message.set_message(storage::SMART_FIELD, &table(&field_table));
        }

        added.sort_unstable();
        added.dedup();
        self.package.update(storage_id, &added, |e| {
            *e = message;
            Ok(())
        })?;

        let mut infos = self.package.component_infos()?;
        self.add_external_references(&mut infos, &[storage_id])?;
        self.package.set_component_infos(&infos)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Character style for a run: None for unstyled text, a variation of "None" (or of "Link"
    /// for links) otherwise
    fn character_style(&mut self, style: &RunStyle) -> Result<Option<u64>> {
        if style.is_empty() {
            return Ok(None);
        }
        let base = match &style.link {
            Some(_) => self
                .named_style("character-style-hyperlink")?
                .or(self.style_named(TYPE_CHARACTER_STYLE, "Link")?),
            None => None,
        };
        let base = match base {
            Some(e) => e,
            None => self
                .named_style("character-style-null")?
                .or(self.style_named(TYPE_CHARACTER_STYLE, "None")?)
                .ok_or_else(|| anyhow!("the theme has no \"None\" character style to vary"))?,
        };
        let properties = style.properties();
        if properties.fields.is_empty() {
            return Ok(Some(base));
        }
        let props = StyleProps {
            groups: vec![properties],
        };
        Ok(Some(self.style_variation(base, &props)?))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn list_style(&self, bullet: Bullet) -> Result<u64> {
        let name = bullet.list_style_name();
        self.style_named(TYPE_LIST_STYLE, name)?
            .ok_or_else(|| anyhow!("the theme has no {name:?} list style for {bullet:?} bullets"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Archive version Keynote wrote objects of a type with, None when there's none to look at
    pub(super) fn object_version(&self, message_type: u32) -> Option<Vec<u64>> {
        self.package
            .objects()
            .find(|e| e.message_type() == message_type)
            .map(IwaObject::version)
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    fn texts(runs: &[TextRun]) -> Vec<(&str, RunStyle)> {
        runs.iter().map(|e| (e.text.as_str(), e.style.clone())).collect()
    }

    #[test]
    fn parses_markdown_lite() {
        let code = RunStyle {
            font: Some(MONOSPACE_FONT.to_string()),
            ..Default::default()
        };
        let runs = markdown_runs("**76%** of *bugs*, `unsafe` in snake_case [docs](https://x.y)", &RunStyle::default(), &code);
        let bold = RunStyle {
            bold: Some(true),
            ..Default::default()
        };
        let italic = RunStyle {
            italic: Some(true),
            ..Default::default()
        };
        let link = RunStyle {
            link: Some("https://x.y".to_string()),
            ..Default::default()
        };
        assert_eq!(
            texts(&runs),
            vec![
                ("76%", bold),
                (" of ", RunStyle::default()),
                ("bugs", italic),
                (", ", RunStyle::default()),
                ("unsafe", code.clone()),
                (" in snake_case ", RunStyle::default()),
                ("docs", link),
            ]
        );

        let base = RunStyle {
            size: Some(30.0),
            ..Default::default()
        };
        let runs = markdown_runs("are **memory** bugs", &base, &code);
        let text: String = runs.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(text, "are memory bugs", "{runs:?}");

        let paragraphs = markdown_paragraphs("Intro\n- one\n  - nested\n1. first", &RunStyle::default(), &code);
        assert_eq!(paragraphs.len(), 4);
        assert_eq!(paragraphs[0].bullet, None);
        assert_eq!(paragraphs[1].bullet, Some(Bullet::Disc));
        assert_eq!(paragraphs[2].level, Some(1));
        assert_eq!(paragraphs[3].bullet, Some(Bullet::Number));
        assert_eq!(paragraphs[3].runs[0].text, "first");
    }

    #[test]
    fn reads_stored_runs() {
        let mut message = Message::default();
        message.set_bytes(storage::TEXT, b"ab\ncd".to_vec());
        message.set_message(storage::PARA_STYLE, &table(&[entry(0, Some(7)), entry(3, None)]));
        message.set_message(storage::CHAR_STYLE, &table(&[entry(0, Some(9)), entry(1, None)]));

        let paragraphs = stored_paragraphs(&message).unwrap();
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].runs, vec![(0, 1, Some(9)), (1, 2, None)]);
        assert_eq!(paragraphs[1].paragraph_style, Some(7));
        assert_eq!(paragraphs[1].runs, vec![(0, 2, None)]);
    }
}
