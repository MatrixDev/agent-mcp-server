//! Styles: reading a style's effective properties and creating style variations.
//!
//! Every object points at a named style of the theme or at a *variation*: an anonymous style
//! with a parent, overriding a few properties. A variation's properties are complete
//! messages per property (an empty message means "none", eg no fill), anything it doesn't set
//! comes from its parent. Keynote lists variations in the stylesheet's style list and under
//! their parent in the parent-to-children map.

use anyhow::{Result, anyhow, bail};

use super::document::KeynoteDocument;
use super::iwa::IwaObject;
use super::proto::{Message, ReferenceAction, make_reference, reference, rewrite_references};

////////////////////////////////////////////////////////////////////////////////
pub const TYPE_SLIDE_STYLE: u32 = 9;
pub const TYPE_CHARACTER_STYLE: u32 = 2021;
pub const TYPE_PARAGRAPH_STYLE: u32 = 2022;
pub const TYPE_LIST_STYLE: u32 = 2023;
/// TSWP.ShapeStyleArchive, styles of shapes and text boxes (TSD shape style plus text layout)
pub const TYPE_SHAPE_STYLE: u32 = 2025;
pub const TYPE_TSD_SHAPE_STYLE: u32 = 3015;
pub const TYPE_MEDIA_STYLE: u32 = 3016;

/// KN.ShowArchive.stylesheet
const SHOW_STYLESHEET: u32 = 5;

/// TSS.StyleArchive
mod style_archive {
    pub const NAME: u32 = 1;
    pub const IDENTIFIER: u32 = 2;
    pub const PARENT: u32 = 3;
    pub const IS_VARIATION: u32 = 4;
    pub const STYLESHEET: u32 = 5;
}

/// TSS.StylesheetArchive
mod stylesheet {
    pub const STYLES: u32 = 1;
    pub const IDENTIFIER_MAP: u32 = 2;
    pub const CHILDREN_MAP: u32 = 5;
    pub const ENTRY_IDENTIFIER: u32 = 1;
    pub const ENTRY_STYLE: u32 = 2;
    pub const CHILDREN_PARENT: u32 = 1;
    pub const CHILDREN_STYLES: u32 = 2;
}

/// TSWP.CharacterStylePropertiesArchive
pub mod char_props {
    pub const BOLD: u32 = 1;
    pub const ITALIC: u32 = 2;
    pub const SIZE: u32 = 3;
    pub const FONT: u32 = 5;
    pub const COLOR: u32 = 7;
    pub const SUPERSCRIPT: u32 = 10;
    pub const UNDERLINE: u32 = 11;
    pub const STRIKE: u32 = 12;
    pub const BACKGROUND_COLOR: u32 = 26;
    pub const FILL: u32 = 46;
}

/// TSWP.ParagraphStylePropertiesArchive
pub mod para_props {
    pub const ALIGNMENT: u32 = 1;
    pub const FIRST_LINE_INDENT: u32 = 7;
    pub const LEFT_INDENT: u32 = 11;
    pub const LINE_SPACING: u32 = 13;
    pub const SPACE_AFTER: u32 = 20;
    pub const SPACE_BEFORE: u32 = 21;
    pub const LIST_STYLE: u32 = 40;

    pub const LINE_SPACING_MODE: u32 = 1;
    pub const LINE_SPACING_AMOUNT: u32 = 2;
    pub const MODE_RELATIVE: u64 = 0;
    pub const MODE_EXACT: u64 = 2;

    pub const ALIGN_LEFT: u64 = 0;
    pub const ALIGN_RIGHT: u64 = 1;
    pub const ALIGN_CENTER: u64 = 2;
    pub const ALIGN_JUSTIFY: u64 = 3;
}

/// TSD.ShapeStylePropertiesArchive (and TSD.MediaStylePropertiesArchive, see below)
pub mod shape_props {
    pub const FILL: u32 = 1;
    pub const STROKE: u32 = 2;
    pub const OPACITY: u32 = 3;
    pub const SHADOW: u32 = 4;
    pub const HEAD_LINE_END: u32 = 6;
    pub const TAIL_LINE_END: u32 = 7;
}

/// TSD.MediaStylePropertiesArchive
pub mod media_props {
    pub const STROKE: u32 = 1;
    pub const OPACITY: u32 = 2;
    pub const SHADOW: u32 = 3;
}

/// TSWP.ShapeStylePropertiesArchive
pub mod text_layout_props {
    pub const SHRINK_TO_FIT: u32 = 1;
    pub const VERTICAL_ALIGNMENT: u32 = 2;
    pub const COLUMNS: u32 = 4;
    pub const PADDING: u32 = 6;
    pub const PARAGRAPH_STYLE: u32 = 10;

    pub const PADDING_LEFT: u32 = 1;
    pub const PADDING_TOP: u32 = 2;
    pub const PADDING_RIGHT: u32 = 3;
    pub const PADDING_BOTTOM: u32 = 4;

    pub const COLUMNS_EQUAL: u32 = 1;
    pub const COLUMNS_COUNT: u32 = 1;
    pub const COLUMNS_GAP: u32 = 2;
}

/// KN.SlideStylePropertiesArchive
pub mod slide_props {
    pub const FILL: u32 = 1;
    pub const TITLE_VISIBLE: u32 = 4;
    pub const BODY_VISIBLE: u32 = 5;
    pub const SLIDE_NUMBER_VISIBLE: u32 = 6;
}

const OVERRIDE_COUNT: u32 = 10;

////////////////////////////////////////////////////////////////////////////////
/// Where a style archive keeps its TSS.StyleArchive, override counts and property groups
struct StyleLayout {
    style: &'static [u32],
    counts: &'static [&'static [u32]],
    groups: &'static [&'static [u32]],
}

fn style_layout(message_type: u32) -> Option<StyleLayout> {
    match message_type {
        TYPE_SLIDE_STYLE | TYPE_CHARACTER_STYLE | TYPE_TSD_SHAPE_STYLE | TYPE_MEDIA_STYLE => Some(StyleLayout {
            style: &[1],
            counts: &[&[OVERRIDE_COUNT]],
            groups: &[&[11]],
        }),
        TYPE_PARAGRAPH_STYLE => Some(StyleLayout {
            style: &[1],
            counts: &[&[OVERRIDE_COUNT]],
            groups: &[&[11], &[12]],
        }),
        // TSD properties in the nested super archive, text layout ones at the top
        TYPE_SHAPE_STYLE => Some(StyleLayout {
            style: &[1, 1],
            counts: &[&[1, OVERRIDE_COUNT], &[OVERRIDE_COUNT]],
            groups: &[&[1, 11], &[11]],
        }),
        TYPE_LIST_STYLE => Some(StyleLayout {
            style: &[1],
            counts: &[],
            groups: &[],
        }),
        _ => None,
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Property groups of a style, in the order its layout lists them:
/// - character style: [character properties]
/// - paragraph style: [character properties, paragraph properties]
/// - shape style: [TSD shape properties, text layout properties]
/// - slide / media style: [their properties]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleProps {
    pub groups: Vec<Message>,
}

impl StyleProps {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn with_groups(count: usize) -> Self {
        Self {
            groups: vec![Message::default(); count],
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn group(&self, index: usize) -> Message {
        self.groups.get(index).cloned().unwrap_or_default()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn group_mut(&mut self, index: usize) -> &mut Message {
        if self.groups.len() <= index {
            self.groups.resize(index + 1, Message::default());
        }
        &mut self.groups[index]
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn is_empty(&self) -> bool {
        self.groups.iter().all(|e| e.fields.is_empty())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Properties of `other` replace these, property by property
    pub fn merge(&mut self, other: &StyleProps) {
        for (index, group) in other.groups.iter().enumerate() {
            merge_properties(self.group_mut(index), group);
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Every field number of `over` replaces that field of `base`
pub fn merge_properties(base: &mut Message, over: &Message) {
    let mut numbers: Vec<u32> = over.fields.iter().map(|e| e.number).collect();
    numbers.dedup();
    for number in &numbers {
        base.remove(*number);
    }
    base.fields.extend(over.fields.iter().cloned());
}

////////////////////////////////////////////////////////////////////////////////
/// Number of properties a group overrides, Keynote counts a text colour (colour and fill) once
fn property_count(group: &Message) -> u64 {
    let mut numbers: Vec<u32> = group.fields.iter().map(|e| e.number).collect();
    numbers.sort_unstable();
    numbers.dedup();
    let duplicate_color = numbers.contains(&char_props::COLOR) && numbers.contains(&char_props::FILL);
    numbers.len() as u64 - u64::from(duplicate_color)
}

////////////////////////////////////////////////////////////////////////////////
pub fn message_at(message: &Message, path: &[u32]) -> Result<Option<Message>> {
    let mut current = message.clone();
    for number in path {
        match current.message(*number)? {
            Some(e) => current = e,
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}

////////////////////////////////////////////////////////////////////////////////
pub fn set_message_at(message: &mut Message, path: &[u32], value: &Message) -> Result<()> {
    match path {
        [] => *message = value.clone(),
        [number] => message.set_message(*number, value),
        [number, rest @ ..] => {
            let mut nested = message.message(*number)?.unwrap_or_default();
            set_message_at(&mut nested, rest, value)?;
            message.set_message(*number, &nested);
        }
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
fn set_varint_at(message: &mut Message, path: &[u32], value: u64) -> Result<()> {
    match path {
        [] => bail!("empty field path"),
        [number] => message.set_varint(*number, value),
        [number, rest @ ..] => {
            let mut nested = message.message(*number)?.unwrap_or_default();
            set_varint_at(&mut nested, rest, value)?;
            message.set_message(*number, &nested);
        }
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
/// Ids of every reference-shaped value in a message
pub fn embedded_references(message: &Message) -> Vec<u64> {
    let mut found = Vec::new();
    let mut message = message.clone();
    rewrite_references(&mut message, &mut |e| {
        if !found.contains(&e) {
            found.push(e);
        }
        ReferenceAction::Keep
    });
    found
}

impl KeynoteDocument {
    ////////////////////////////////////////////////////////////////////////////////
    /// The document stylesheet every style of the presentation belongs to
    pub(super) fn stylesheet(&self) -> Result<u64> {
        let (_, show) = self.show()?;
        reference(&show, SHOW_STYLESHEET)?
            .filter(|e| self.package.contains(*e))
            .ok_or_else(|| anyhow!("the presentation has no stylesheet"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn stylesheet_component(&self) -> Option<String> {
        let stylesheet = self.stylesheet().ok()?;
        self.package.component_of(stylesheet).ok().map(str::to_string)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// A named style of the theme by its identifier (eg "shape-0-shapestyle") or by its name
    pub(super) fn named_style(&self, identifier: &str) -> Result<Option<u64>> {
        let stylesheet = self.package.message(self.stylesheet()?)?;
        for entry in stylesheet.messages(stylesheet::IDENTIFIER_MAP)? {
            if entry.string(stylesheet::ENTRY_IDENTIFIER).as_deref() == Some(identifier)
                && let Some(style) = reference(&entry, stylesheet::ENTRY_STYLE)?.filter(|e| self.package.contains(*e))
            {
                return Ok(Some(style));
            }
        }
        Ok(None)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// A named style of a given type by its display name, eg the "None" character style
    pub(super) fn style_named(&self, message_type: u32, name: &str) -> Result<Option<u64>> {
        let stylesheet = self.package.message(self.stylesheet()?)?;
        for entry in stylesheet.messages(stylesheet::STYLES)? {
            let Some(style) = entry.varint(1).filter(|e| self.package.contains(*e)) else {
                continue;
            };
            if self.package.object(style)?.message_type() != message_type {
                continue;
            }
            if self.style_archive(style)?.string(style_archive::NAME).as_deref() == Some(name) {
                return Ok(Some(style));
            }
        }
        Ok(None)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// TSS.StyleArchive of a style
    fn style_archive(&self, style: u64) -> Result<Message> {
        let message_type = self.package.object(style)?.message_type();
        let layout = style_layout(message_type).ok_or_else(|| anyhow!("object {style} is not a style"))?;
        Ok(message_at(&self.package.message(style)?, layout.style)?.unwrap_or_default())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Display name of a named style, or of the named style a variation derives from
    pub(super) fn style_name(&self, style: u64) -> Result<Option<String>> {
        let mut current = style;
        for _ in 0..32 {
            let archive = self.style_archive(current)?;
            if let Some(name) = archive.string(style_archive::NAME).filter(|e| !e.is_empty()) {
                return Ok(Some(name));
            }
            match reference(&archive, style_archive::PARENT)?.filter(|e| self.package.contains(*e)) {
                Some(parent) => current = parent,
                None => return Ok(archive.string(style_archive::IDENTIFIER)),
            }
        }
        Ok(None)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn style_parent(&self, style: u64) -> Result<Option<u64>> {
        let archive = self.style_archive(style)?;
        Ok(reference(&archive, style_archive::PARENT)?.filter(|e| self.package.contains(*e)))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Properties a style sets itself
    pub(super) fn own_properties(&self, style: u64) -> Result<StyleProps> {
        let message_type = self.package.object(style)?.message_type();
        let layout = style_layout(message_type).ok_or_else(|| anyhow!("object {style} is not a style"))?;
        let message = self.package.message(style)?;
        let mut props = StyleProps::default();
        for path in layout.groups {
            props.groups.push(message_at(&message, path)?.unwrap_or_default());
        }
        Ok(props)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Effective properties of a style: its own over its parents'
    pub(super) fn resolved_properties(&self, style: u64) -> Result<StyleProps> {
        let mut chain = vec![style];
        while let Some(parent) = self.style_parent(*chain.last().unwrap())? {
            if chain.contains(&parent) || chain.len() > 32 {
                break;
            }
            chain.push(parent);
        }

        let mut props = StyleProps::default();
        for style in chain.iter().rev() {
            props.merge(&self.own_properties(*style)?);
        }
        Ok(props)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// A variation of `base` with `overrides` on top of what `base` sets. A variation of a
    /// variation derives from the same named style, and an identical existing variation is
    /// reused. Returns the named style itself when nothing is overridden.
    pub(super) fn style_variation(&mut self, base: u64, overrides: &StyleProps) -> Result<u64> {
        let object = self.package.object(base)?;
        let message_type = object.message_type();
        let version = object.version();
        let layout = style_layout(message_type).ok_or_else(|| anyhow!("object {base} is not a style"))?;
        if layout.groups.is_empty() {
            bail!("styles of type {message_type} can't be varied");
        }

        let archive = self.style_archive(base)?;
        let parent = reference(&archive, style_archive::PARENT)?.filter(|e| self.package.contains(*e));
        let (parent, mut props) = match parent {
            Some(parent) if archive.bool(style_archive::IS_VARIATION).unwrap_or(false) => {
                (parent, self.own_properties(base)?)
            }
            _ => (base, StyleProps::with_groups(layout.groups.len())),
        };
        props.merge(overrides);
        props.groups.resize(layout.groups.len(), Message::default());
        if props.is_empty() {
            return Ok(parent);
        }

        for child in self.style_children(parent)? {
            if self.package.object(child)?.message_type() == message_type && self.own_properties(child)? == props {
                return Ok(child);
            }
        }

        let stylesheet = match reference(&archive, style_archive::STYLESHEET)? {
            Some(e) if self.package.contains(e) => e,
            _ => self.stylesheet()?,
        };
        let mut style = Message::default();
        style.set_message(style_archive::PARENT, &make_reference(parent));
        style.set_bool(style_archive::IS_VARIATION, true);
        style.set_message(style_archive::STYLESHEET, &make_reference(stylesheet));

        let count: u64 = props.groups.iter().map(property_count).sum();
        let mut message = Message::default();
        set_message_at(&mut message, layout.style, &style)?;
        for (index, path) in layout.groups.iter().enumerate() {
            if let Some(count_path) = layout.counts.get(index) {
                set_varint_at(&mut message, count_path, count)?;
            }
            set_message_at(&mut message, path, &props.groups[index])?;
        }

        // like Keynote, the header lists the parent and what the properties point at, not the stylesheet
        let mut references = vec![parent];
        for group in &props.groups {
            references.extend(embedded_references(group).into_iter().filter(|e| self.package.contains(*e)));
        }
        references.dedup();

        let identifier = self.package.last_object_identifier()? + 1;
        let component = self.package.component_of(stylesheet)?.to_string();
        let object = IwaObject::new(identifier, message_type, &version, &message, &references);
        self.package.insert(&component, object)?;
        self.package.set_last_object_identifier(identifier)?;

        self.package.update(stylesheet, &[identifier], |e| {
            e.push_message(stylesheet::STYLES, &make_reference(identifier));
            let mut entries = e.messages(stylesheet::CHILDREN_MAP)?;
            let mut found = false;
            for entry in &mut entries {
                if reference(entry, stylesheet::CHILDREN_PARENT)? == Some(parent) {
                    entry.push_message(stylesheet::CHILDREN_STYLES, &make_reference(identifier));
                    found = true;
                }
            }
            if !found {
                let mut entry = Message::default();
                entry.set_message(stylesheet::CHILDREN_PARENT, &make_reference(parent));
                entry.push_message(stylesheet::CHILDREN_STYLES, &make_reference(identifier));
                entries.push(entry);
            }
            e.set_messages(stylesheet::CHILDREN_MAP, &entries);
            Ok(())
        })?;

        // the stylesheet and the style share a component unless the theme is unusual
        let mut infos = self.package.component_infos()?;
        self.add_external_references(&mut infos, &[identifier, stylesheet])?;
        self.package.set_component_infos(&infos)?;
        Ok(identifier)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn style_children(&self, parent: u64) -> Result<Vec<u64>> {
        let stylesheet = self.package.message(self.stylesheet()?)?;
        let mut children = Vec::new();
        for entry in stylesheet.messages(stylesheet::CHILDREN_MAP)? {
            if reference(&entry, stylesheet::CHILDREN_PARENT)? == Some(parent) {
                let styles = super::proto::references(&entry, stylesheet::CHILDREN_STYLES)?;
                children.extend(styles.into_iter().filter(|e| self.package.contains(*e)));
            }
        }
        Ok(children)
    }
}
