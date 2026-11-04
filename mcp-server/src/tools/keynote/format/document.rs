//! Keynote document model on top of the package: slides, layouts, text and slide copies.
//!
//! Object graph (Keynote 14 field numbers, from psobot/keynote-parser):
//!
//! ```text
//! KN.ShowArchive ── slideTree.slides ──> KN.SlideNodeArchive ── slide ──> KN.SlideArchive
//!       └─ theme ──> KN.ThemeArchive ── templates ──> template slide nodes     ├─ titlePlaceholder / bodyPlaceholder
//!                                                                            │     └─> KN.PlaceholderArchive.super.owned_storage ──> TSWP.StorageArchive
//!                                                                            ├─ note ──> KN.NoteArchive.containedStorage ──> TSWP.StorageArchive
//!                                                                            └─ template_slide ──> KN.SlideArchive (layout, has the name)
//! ```
//!
//! New slides are deep copies of existing ones. The copy covers every object only the source
//! slide can reach; shared objects (styles, layouts, theme, media) stay shared.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use super::iwa::IwaObject;
use super::objects::{BackgroundSpec, ObjectInfo, PlaceholderKind};
use super::package::{KeyPackage, TYPE_PACKAGE_METADATA, component_info, component_path};
use super::proto::{Message, Value, make_reference, reference, references};
use super::rich_text::Paragraph;
use super::text::{set_storage_text, storage_text};

////////////////////////////////////////////////////////////////////////////////
const TYPE_DOCUMENT: u32 = 1;
const TYPE_SHOW: u32 = 2;
const TYPE_SLIDE_NODE: u32 = 4;
const TYPE_PLACEHOLDER: u32 = 7;
const TYPE_BUILD: u32 = 8;
const TYPE_THEME: u32 = 10;
const TYPE_BUILD_CHUNK: u32 = 153;
const TYPE_SHAPE_INFO: u32 = 2011;

/// UI and selection state, it may point at slide objects without owning them
const VIEW_STATE_TYPES: &[u32] = &[
    3, 23, 24, 25, 164, 165, 166, 167, 168, 169, 180, 181, 210, 2002, 2409, 2412, 2413, 3045, 3061, 3062, 3063, 3071,
    3072, 3073, 3074, 3089, 3092, 5145, 6030, 6271, 6283, 6284, 6295, 6311, 6312, 11009,
];

/// Types a slide copy never descends into, they belong to the document rather than a slide
const BOUNDARY_TYPES: &[u32] = &[
    TYPE_DOCUMENT,
    TYPE_SHOW,
    TYPE_SLIDE_NODE,
    TYPE_THEME,
    TYPE_PACKAGE_METADATA,
];

const DOCUMENT_IDENTIFIER_FILE: &str = "Metadata/DocumentIdentifier";

////////////////////////////////////////////////////////////////////////////////
pub(super) mod show {
    pub const THEME: u32 = 2;
    pub const SLIDE_TREE: u32 = 3;
    pub const SIZE: u32 = 4;
}

mod slide_tree {
    pub const SLIDES: u32 = 2;
}

pub(super) mod size {
    pub const WIDTH: u32 = 1;
    pub const HEIGHT: u32 = 2;
}

pub(super) mod slide_node {
    pub const SLIDE: u32 = 2;
    pub const IS_SKIPPED: u32 = 4;
    pub const HAS_TRANSITION: u32 = 7;
    pub const HAS_NOTE: u32 = 8;
    pub const UNIQUE_IDENTIFIER: u32 = 11;
    pub const COPY_FROM_SLIDE_IDENTIFIER: u32 = 12;
    pub const THUMBNAILS_ARE_DIRTY: u32 = 14;
    pub const BUILD_EVENT_COUNT: u32 = 15;
    pub const HAS_EXPLICIT_BUILDS: u32 = 20;
    pub const HYPERLINK_MAP: u32 = 24;
    pub const DIGESTS_FOR_THUMBNAIL: u32 = 25;
    pub const TEMPLATE_SLIDE_ID: u32 = 29;
    pub const LIVE_VIDEO_SOURCE_IDS: u32 = 30;
    pub const LIVE_VIDEO_SOURCE_USAGE: u32 = 31;
}

pub(super) mod slide {
    pub const STYLE: u32 = 1;
    pub const BUILDS: u32 = 2;
    pub const TRANSITION: u32 = 4;
    pub const TITLE_PLACEHOLDER: u32 = 5;
    pub const BODY_PLACEHOLDER: u32 = 6;
    pub const OWNED_DRAWABLES: u32 = 7;
    pub const NAME: u32 = 10;
    pub const TEMPLATE_SLIDE: u32 = 17;
    pub const SLIDE_NUMBER_PLACEHOLDER: u32 = 20;
    pub const SAGE_TAG_MAP: u32 = 28;
    pub const NOTE: u32 = 27;
    pub const CLASSIC_STYLESHEET_RECORD: u32 = 29;
    pub const OBJECT_PLACEHOLDER: u32 = 30;
    pub const BODY_PARAGRAPH_STYLES: u32 = 31;
    pub const BODY_LIST_STYLES: u32 = 35;
    pub const USER_GUIDES: u32 = 36;
    pub const THUMBNAIL_TEXT_FOR_TITLE: u32 = 37;
    pub const THUMBNAIL_TEXT_FOR_BODY: u32 = 38;
    pub const OBJECTS_LAYER_WITH_TEMPLATE: u32 = 41;
    pub const DRAWABLES_Z_ORDER: u32 = 42;
    pub const BUILD_CHUNKS: u32 = 43;
    pub const INFOS_USING_OBJECT_PLACEHOLDER: u32 = 44;
    pub const INSTRUCTIONAL_TEXT_MAP: u32 = 45;
}

/// Slide archive fields only layouts have
const LAYOUT_ONLY_FIELDS: [u32; 5] = [
    slide::NAME,
    slide::CLASSIC_STYLESHEET_RECORD,
    slide::BODY_PARAGRAPH_STYLES,
    slide::BODY_LIST_STYLES,
    slide::OBJECTS_LAYER_WITH_TEMPLATE,
];

/// Transition effect Keynote stores for "no transition"
const NO_TRANSITION: &str = "none";

mod placeholder {
    pub const SUPER: u32 = 1;
}

mod shape_info {
    pub const OWNED_STORAGE: u32 = 4;
}

mod note {
    pub const CONTAINED_STORAGE: u32 = 1;
}

mod theme {
    pub const TEMPLATES: u32 = 2;
}

/// KN.TransitionArchive
mod transition {
    pub const ATTRIBUTES: u32 = 2;
}

/// KN.TransitionAttributesArchive
mod transition_attributes {
    pub const ANIMATION: u32 = 8;
}

/// KN.AnimationAttributesArchive
mod animation {
    pub const TYPE: u32 = 1;
    pub const EFFECT: u32 = 2;
    pub const DURATION: u32 = 3;
    pub const DELAY: u32 = 5;
    pub const IS_AUTOMATIC: u32 = 6;
    pub const RANDOM_SEED: u32 = 11;
    pub const WRITING_DIRECTION_IS_RTL: u32 = 16;
}

/// KN.BuildArchive
pub(super) mod build {
    pub const DRAWABLE: u32 = 1;
    pub const DELIVERY: u32 = 2;
    pub const DURATION: u32 = 3;
    pub const ATTRIBUTES: u32 = 4;
    pub const CHUNK_ID_SEED: u32 = 5;
}

/// KN.BuildAttributesArchive
mod build_attributes {
    pub const EVENT_TRIGGER: u32 = 4;
    pub const CHART_ROTATION_3D: u32 = 17;
    pub const ANIMATION: u32 = 18;
    pub const TEXT_DELIVERY: u32 = 20;
    pub const DELIVERY_OPTION: u32 = 21;
}

/// KN.BuildChunkArchive
pub(super) mod build_chunk {
    pub const BUILD: u32 = 1;
    pub const DELAY: u32 = 3;
    pub const DURATION: u32 = 4;
    pub const AUTOMATIC: u32 = 5;
    pub const REFERENT: u32 = 6;
    pub const IDENTIFIER: u32 = 7;
    pub const BUILD_ID: u32 = 8;
}

/// KN.BuildChunkIdentifierArchive
mod chunk_identifier {
    pub const BUILD_ID: u32 = 1;
    pub const CHUNK_ID: u32 = 2;
}

mod external_reference {
    pub const COMPONENT: u32 = 1;
    pub const OBJECT: u32 = 2;
}

mod data_reference {
    pub const DATA: u32 = 1;
    pub const OBJECTS: u32 = 2;
}

mod data_object {
    pub const OBJECT: u32 = 1;
}

mod uuid_entry {
    pub const IDENTIFIER: u32 = 1;
    pub const UUID: u32 = 2;
}

mod uuid {
    pub const LOWER: u32 = 1;
    pub const UPPER: u32 = 2;
}

/// TSP.ComponentInfo.ambiguous_object_identifiers, meaningless for a copied component
const COMPONENT_AMBIGUOUS_OBJECT_IDENTIFIERS: u32 = 20;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Serialize)]
pub struct DocumentInfo {
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// every layout of the theme
    pub layouts: Vec<LayoutInfo>,
    /// total number of slides, `slides` may list only some
    pub slide_count: usize,
    pub slides: Vec<SlideInfo>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutInfo {
    pub name: String,
    /// placeholders the layout shows: "title", "body", "media", "slide number"
    pub placeholders: Vec<String>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Serialize)]
pub struct SlideInfo {
    pub slide: usize,
    pub layout: Option<String>,
    /// fill behind the slide: "#RRGGBB", "gradient …", "image <file>" or "none"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// null when the slide has no title placeholder
    pub title: Option<String>,
    /// null when the slide has no body placeholder
    pub body: Option<String>,
    pub notes: Option<String>,
    pub skipped: bool,
    /// transition played when leaving the slide
    pub transition: Option<TransitionInfo>,
    /// object animations, in the order the slide lists them
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub builds: Vec<BuildInfo>,
    /// playback steps, each advancing one build
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub build_order: Vec<BuildStep>,
    /// the slide's objects back to front; builds and object tools take them as "id:N"
    /// (stable) or "object N" (1-based position in this list)
    pub objects: Vec<ObjectInfo>,
}

////////////////////////////////////////////////////////////////////////////////
/// One line per slide, for a quick overview
#[derive(Debug, Serialize)]
pub struct SlideSummary {
    pub slide: usize,
    pub layout: Option<String>,
    pub title: Option<String>,
    pub objects: usize,
    pub has_notes: bool,
    pub builds: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
}

impl From<&SlideInfo> for SlideSummary {
    fn from(slide: &SlideInfo) -> Self {
        Self {
            slide: slide.slide,
            layout: slide.layout.clone(),
            title: slide.title.clone(),
            objects: slide.objects.len(),
            has_notes: slide.notes.is_some(),
            builds: slide.builds.len(),
            skipped: slide.skipped,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransitionInfo {
    /// eg "apple:dissolve"
    pub effect: String,
    /// seconds
    pub duration: Option<f64>,
    /// seconds
    pub delay: Option<f64>,
    /// starts on its own rather than on click
    pub automatic: bool,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BuildInfo {
    pub target: BuildTarget,
    /// "In", "Out" or "Action"
    pub kind: Option<String>,
    /// eg "apple:dissolve character"
    pub effect: Option<String>,
    /// eg "All at Once", "By Bullet"
    pub delivery: Option<String>,
    /// seconds
    pub duration: Option<f64>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BuildTarget {
    /// object id, see SlideInfo.objects
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// "title", "body", "placeholder", "text box", "shape", "image", "type N" or "missing"
    pub kind: String,
    pub text: Option<String>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BuildStep {
    /// 1-based index into the slide's builds
    pub build: usize,
    /// starts after the previous step rather than on click
    pub automatic: bool,
    /// seconds
    pub delay: Option<f64>,
    /// seconds
    pub duration: Option<f64>,
}

////////////////////////////////////////////////////////////////////////////////
/// Changes to a slide, None leaves that part as it is
#[derive(Debug, Default, Clone)]
pub struct SlideContent {
    pub title: Option<String>,
    pub body: Option<String>,
    /// styled title, instead of `title`
    pub title_rich: Option<Vec<Paragraph>>,
    /// styled body, instead of `body`
    pub body_rich: Option<Vec<Paragraph>>,
    pub notes: Option<String>,
    pub skipped: Option<bool>,
    pub background: Option<BackgroundSpec>,
    pub show_title: Option<bool>,
    pub show_body: Option<bool>,
    pub show_slide_number: Option<bool>,
    pub transition: Option<TransitionSpec>,
    /// replaces every build of the slide, in playback order
    pub builds: Option<Vec<BuildSpec>>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionSpec {
    /// Keynote effect identifier, "none" removes the transition
    pub effect: String,
    /// seconds
    pub duration: f64,
    /// seconds before an automatic transition starts
    pub delay: f64,
    /// advances on its own rather than on click
    pub automatic: bool,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, PartialEq)]
pub struct BuildSpec {
    /// "title", "body", "id:N" or "object N" (1-based index into SlideInfo.objects)
    pub target: String,
    /// "In" or "Out"
    pub kind: String,
    /// Keynote effect identifier, eg "apple:dissolve"
    pub effect: String,
    /// "All at Once", or "By Bullet"-like deliveries that take a step per paragraph
    pub delivery: String,
    /// seconds
    pub duration: f64,
    /// seconds before the (first) step starts
    pub delay: f64,
    /// the (first) step starts after the previous one rather than on click
    pub automatic: bool,
}

////////////////////////////////////////////////////////////////////////////////
pub struct KeynoteDocument {
    pub(super) package: KeyPackage,
}

impl KeynoteDocument {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let package = KeyPackage::open(path)?;
        let document = Self { package };
        document.show().context("not a Keynote presentation")?;
        Ok(document)
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[cfg(test)]
    pub fn package(&self) -> &KeyPackage {
        &self.package
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[cfg(test)]
    pub fn node_message(&self, number: usize) -> Message {
        self.package.message(self.slide_node(number).unwrap()).unwrap()
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[cfg(test)]
    pub fn slide_message(&self, number: usize) -> Message {
        let slide = self.slide_of(self.slide_node(number).unwrap()).unwrap();
        self.package.message(slide).unwrap()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        self.package.save(path)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Gives the document a new identity, so a copy isn't mistaken for its source
    pub fn reset_identity(&mut self) {
        if self.package.has_file(DOCUMENT_IDENTIFIER_FILE) {
            let identifier = format_uuid(rand::random(), rand::random());
            self.package.set_file(DOCUMENT_IDENTIFIER_FILE, identifier.into_bytes());
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Reading
    ////////////////////////////////////////////////////////////////////////////////

    pub fn info(&self) -> Result<DocumentInfo> {
        self.info_range(None)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Document info with the slides of a 1-based inclusive range only
    pub fn info_range(&self, range: Option<(usize, usize)>) -> Result<DocumentInfo> {
        let (_, show) = self.show()?;
        let size = show.message(show::SIZE)?;
        let nodes = self.slide_nodes()?;

        let (from, to) = range.unwrap_or((1, nodes.len()));
        let mut slides = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            if (from..=to).contains(&(index + 1)) {
                slides.push(self.slide_info(index + 1, *node)?);
            }
        }

        Ok(DocumentInfo {
            width: size.as_ref().and_then(|e| e.float(size::WIDTH)),
            height: size.as_ref().and_then(|e| e.float(size::HEIGHT)),
            layouts: self.layout_infos()?,
            slide_count: nodes.len(),
            slides,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn slide_count(&self) -> Result<usize> {
        Ok(self.slide_nodes()?.len())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Info of one slide by its 1-based number
    pub fn slide(&self, number: usize) -> Result<SlideInfo> {
        self.slide_info(number, self.slide_node(number)?)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn slide_info(&self, number: usize, node: u64) -> Result<SlideInfo> {
        let node_message = self.package.message(node)?;
        let slide = self.slide_of(node)?;
        let slide_message = self.package.message(slide)?;

        let title = self.placeholder_storage(&slide_message, slide::TITLE_PLACEHOLDER)?;
        let body = self.placeholder_storage(&slide_message, slide::BODY_PLACEHOLDER)?;
        let notes = self.note_storage(&slide_message)?;
        let (builds, build_order) = self.builds(&slide_message)?;

        Ok(SlideInfo {
            slide: number,
            layout: self.layout_name(&slide_message)?,
            background: self.background_info(&slide_message)?,
            title: title.map(|e| self.text(e)).transpose()?,
            body: body.map(|e| self.text(e)).transpose()?,
            notes: notes.map(|e| self.text(e)).transpose()?.filter(|e| !e.is_empty()),
            skipped: node_message.bool(slide_node::IS_SKIPPED).unwrap_or(false),
            transition: transition(&slide_message)?,
            builds,
            build_order,
            objects: self.object_infos(&slide_message)?,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Builds in the order the slide lists them, and the order they play in
    fn builds(&self, slide_message: &Message) -> Result<(Vec<BuildInfo>, Vec<BuildStep>)> {
        let build_ids: Vec<u64> = references(slide_message, slide::BUILDS)?
            .into_iter()
            .filter(|e| self.package.contains(*e))
            .collect();

        let mut builds = Vec::new();
        for build in &build_ids {
            let message = self.package.message(*build)?;
            let attributes = message.message(build::ATTRIBUTES)?.unwrap_or_default();
            let animation = attributes.message(build_attributes::ANIMATION)?.unwrap_or_default();
            let target = match reference(&message, build::DRAWABLE)? {
                Some(drawable) => self.build_target(slide_message, drawable)?,
                None => BuildTarget::default(),
            };

            builds.push(BuildInfo {
                target,
                kind: animation.string(animation::TYPE),
                effect: animation.string(animation::EFFECT),
                delivery: message.string(build::DELIVERY),
                duration: animation.double(animation::DURATION),
            });
        }

        let mut order = Vec::new();
        for chunk in references(slide_message, slide::BUILD_CHUNKS)? {
            if !self.package.contains(chunk) {
                continue;
            }
            let message = self.package.message(chunk)?;
            let Some(index) =
                reference(&message, build_chunk::BUILD)?.and_then(|e| build_ids.iter().position(|b| *b == e))
            else {
                continue;
            };

            order.push(BuildStep {
                build: index + 1,
                automatic: message.bool(build_chunk::AUTOMATIC).unwrap_or(false),
                delay: message.double(build_chunk::DELAY),
                duration: message.double(build_chunk::DURATION),
            });
        }
        Ok((builds, order))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn build_target(&self, slide_message: &Message, drawable: u64) -> Result<BuildTarget> {
        if !self.package.contains(drawable) {
            return Ok(BuildTarget {
                id: Some(drawable),
                kind: "missing".to_string(),
                text: None,
            });
        }

        let kind = self.object_kind(slide_message, drawable)?;
        let storage = self.drawable_storage(drawable)?;
        let text = storage.map(|e| self.text(e)).transpose()?.filter(|e| !e.is_empty());
        Ok(BuildTarget {
            id: Some(drawable),
            kind,
            text,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// 1-based number of the first slide using a layout
    #[cfg(test)]
    pub fn find_layout_slide(&self, layout: &str) -> Result<usize> {
        for (index, node) in self.slide_nodes()?.into_iter().enumerate() {
            let slide = self.package.message(self.slide_of(node)?)?;
            if self.layout_name(&slide)?.as_deref() == Some(layout) {
                return Ok(index + 1);
            }
        }

        let mut available = Vec::new();
        for node in self.slide_nodes()? {
            let slide = self.package.message(self.slide_of(node)?)?;
            if let Some(name) = self.layout_name(&slide)?
                && !available.contains(&name)
            {
                available.push(name);
            }
        }
        bail!(
            "no slide uses layout {layout:?} to copy from; layouts in use: {}",
            available.join(", ")
        )
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Names of the theme's layouts
    pub fn layout_names(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for slide in self.layout_slide_list()? {
            names.extend(self.package.message(slide)?.string(slide::NAME));
        }
        Ok(names)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn layout_infos(&self) -> Result<Vec<LayoutInfo>> {
        let mut layouts = Vec::new();
        for slide in self.layout_slide_list()? {
            if let Some(name) = self.package.message(slide)?.string(slide::NAME) {
                layouts.push(LayoutInfo {
                    name,
                    placeholders: self.layout_placeholders(slide)?,
                });
            }
        }
        Ok(layouts)
    }


    ////////////////////////////////////////////////////////////////////////////////
    fn layout_slides(&self) -> Result<HashSet<u64>> {
        Ok(self.layout_slide_list()?.into_iter().collect())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Slide archives of the theme's layouts, in theme order
    pub(super) fn layout_slide_list(&self) -> Result<Vec<u64>> {
        let (_, show) = self.show()?;
        let mut slides = Vec::new();

        if let Some(theme) = reference(&show, show::THEME)?
            && self.package.contains(theme)
        {
            for node in references(&self.package.message(theme)?, theme::TEMPLATES)? {
                if let Ok(slide) = self.slide_of(node)
                    && self.package.contains(slide)
                {
                    slides.push(slide);
                }
            }
        }
        Ok(slides)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn layout_name(&self, slide_message: &Message) -> Result<Option<String>> {
        match reference(slide_message, slide::TEMPLATE_SLIDE)? {
            Some(template) if self.package.contains(template) => {
                Ok(self.package.message(template)?.string(slide::NAME))
            }
            _ => Ok(None),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn text(&self, storage: u64) -> Result<String> {
        Ok(storage_text(&self.package.message(storage)?))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn placeholder_storage(&self, slide_message: &Message, field: u32) -> Result<Option<u64>> {
        let Some(placeholder) = reference(slide_message, field)?.filter(|e| self.package.contains(*e)) else {
            return Ok(None);
        };

        let placeholder = self.package.message(placeholder)?;
        let Some(shape) = placeholder.message(placeholder::SUPER)? else {
            return Ok(None);
        };
        Ok(reference(&shape, shape_info::OWNED_STORAGE)?.filter(|e| self.package.contains(*e)))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn note_storage(&self, slide_message: &Message) -> Result<Option<u64>> {
        let Some(note) = reference(slide_message, slide::NOTE)?.filter(|e| self.package.contains(*e)) else {
            return Ok(None);
        };
        let note = self.package.message(note)?;
        Ok(reference(&note, note::CONTAINED_STORAGE)?.filter(|e| self.package.contains(*e)))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn show(&self) -> Result<(u64, Message)> {
        let identifier = self.package.find_single(TYPE_SHOW)?;
        Ok((identifier, self.package.message(identifier)?))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn slide_nodes(&self) -> Result<Vec<u64>> {
        let (_, show) = self.show()?;
        let tree = show
            .message(show::SLIDE_TREE)?
            .ok_or_else(|| anyhow!("the slide tree is missing"))?;
        references(&tree, slide_tree::SLIDES)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn slide_node(&self, number: usize) -> Result<u64> {
        let nodes = self.slide_nodes()?;
        match number.checked_sub(1).and_then(|e| nodes.get(e)) {
            Some(node) => Ok(*node),
            None => bail!(
                "slide {number} is out of range, the presentation has {} slides",
                nodes.len()
            ),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn slide_of(&self, node: u64) -> Result<u64> {
        let slide = reference(&self.package.message(node)?, slide_node::SLIDE)?;
        slide.ok_or_else(|| anyhow!("slide node {node} has no slide"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Editing
    ////////////////////////////////////////////////////////////////////////////////

    pub fn edit_slide(&mut self, number: usize, content: &SlideContent) -> Result<()> {
        let node = self.slide_node(number)?;
        let slide = self.slide_of(node)?;

        if let Some(background) = &content.background {
            self.set_background(number, background)?;
        }
        for (visible, kind) in [
            (content.show_title, PlaceholderKind::Title),
            (content.show_body, PlaceholderKind::Body),
            (content.show_slide_number, PlaceholderKind::SlideNumber),
        ] {
            if let Some(visible) = visible {
                self.set_placeholder_visible(number, kind, visible)?;
            }
        }
        let slide_message = self.package.message(slide)?;

        let placeholders = [
            (
                &content.title,
                &content.title_rich,
                slide::TITLE_PLACEHOLDER,
                slide::THUMBNAIL_TEXT_FOR_TITLE,
                "title",
            ),
            (
                &content.body,
                &content.body_rich,
                slide::BODY_PLACEHOLDER,
                slide::THUMBNAIL_TEXT_FOR_BODY,
                "body",
            ),
        ];
        for (text, rich, field, thumbnail_field, kind) in placeholders {
            if text.is_none() && rich.is_none() {
                continue;
            }
            let storage = self.placeholder_storage(&slide_message, field)?;
            let storage = storage.ok_or_else(|| {
                let layout = self.layout_name(&slide_message).ok().flatten().unwrap_or_default();
                anyhow!("slide {number} (layout {layout:?}) has no {kind} placeholder")
            })?;
            match (rich, text) {
                (Some(paragraphs), _) => self.set_rich_text(storage, paragraphs)?,
                (None, Some(text)) => self.set_text(storage, text)?,
                (None, None) => unreachable!("checked above"),
            }
            let thumbnail = self.text(storage)?;

            self.package.update(slide, &[], |e| {
                if e.has(thumbnail_field) {
                    e.set_bytes(thumbnail_field, thumbnail.into_bytes());
                }
                Ok(())
            })?;
        }

        if let Some(notes) = &content.notes {
            let storage = self.note_storage(&slide_message)?;
            let storage = storage.ok_or_else(|| anyhow!("slide {number} has no presenter notes"))?;
            self.set_text(storage, notes)?;
        }

        if let Some(transition) = &content.transition {
            self.set_transition(slide, transition)?;
        }

        // after the text, a bullet by bullet build takes a step per paragraph
        let build_events = match &content.builds {
            Some(builds) => Some(self.set_builds(number, slide, builds)?),
            None => None,
        };

        self.package.update(node, &[], |e| {
            if let Some(notes) = &content.notes {
                e.set_bool(slide_node::HAS_NOTE, !notes.trim().is_empty());
            }
            if let Some(skipped) = content.skipped {
                e.set_bool(slide_node::IS_SKIPPED, skipped);
            }
            if let Some(transition) = &content.transition {
                e.set_bool(slide_node::HAS_TRANSITION, transition.effect != NO_TRANSITION);
            }
            if let Some(events) = build_events {
                e.set_varint(slide_node::BUILD_EVENT_COUNT, events as u64);
                e.set_bool(
                    slide_node::HAS_EXPLICIT_BUILDS,
                    content.builds.as_ref().is_some_and(|e| !e.is_empty()),
                );
            }
            // Keynote redraws the slide navigator thumbnails on open
            e.set_bool(slide_node::THUMBNAILS_ARE_DIRTY, true);
            Ok(())
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Empties the title, body and notes the slide has
    pub fn clear_slide(&mut self, number: usize) -> Result<()> {
        let slide = self.slide_of(self.slide_node(number)?)?;
        let slide_message = self.package.message(slide)?;

        let has_title = self
            .placeholder_storage(&slide_message, slide::TITLE_PLACEHOLDER)?
            .is_some();
        let has_body = self
            .placeholder_storage(&slide_message, slide::BODY_PLACEHOLDER)?
            .is_some();
        let has_notes = self.note_storage(&slide_message)?.is_some();

        let content = SlideContent {
            title: has_title.then(String::new),
            body: has_body.then(String::new),
            notes: has_notes.then(String::new),
            skipped: Some(false),
            ..Default::default()
        };
        self.edit_slide(number, &content)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn set_text(&mut self, storage: u64, text: &str) -> Result<()> {
        self.package.update(storage, &[], |e| set_storage_text(e, text))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn set_transition(&mut self, slide: u64, spec: &TransitionSpec) -> Result<()> {
        self.package.update(slide, &[], |e| {
            let mut transition = e.message(slide::TRANSITION)?.unwrap_or_default();
            let mut attributes = transition.message(transition::ATTRIBUTES)?.unwrap_or_default();
            let mut animation = attributes
                .message(transition_attributes::ANIMATION)?
                .unwrap_or_default();

            animation.set_bytes(animation::TYPE, b"Transition".to_vec());
            animation.set_bytes(animation::EFFECT, spec.effect.clone().into_bytes());
            animation.set(animation::DURATION, Value::Fixed64(spec.duration.to_bits()));
            animation.set(animation::DELAY, Value::Fixed64(spec.delay.to_bits()));
            animation.set_bool(animation::IS_AUTOMATIC, spec.automatic);
            if !animation.has(animation::RANDOM_SEED) {
                animation.set_varint(animation::RANDOM_SEED, u64::from(rand::random::<u32>() >> 1));
            }

            attributes.set_message(transition_attributes::ANIMATION, &animation);
            transition.set_message(transition::ATTRIBUTES, &attributes);
            e.set_message(slide::TRANSITION, &transition);
            Ok(())
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Replaces the slide's builds, returns how many clicks they take
    fn set_builds(&mut self, number: usize, slide: u64, specs: &[BuildSpec]) -> Result<usize> {
        let slide_message = self.package.message(slide)?;
        let component = self.package.component_of(slide)?.to_string();
        let version = self.package.object(slide)?.version();

        // resolve every target first, a bad one leaves the slide untouched
        let mut targets = Vec::new();
        for (index, spec) in specs.iter().enumerate() {
            let drawable = self
                .resolve_target(&slide_message, &spec.target)
                .with_context(|| format!("slide {number}, build {}", index + 1))?;
            let steps = match spec.delivery.to_lowercase().contains("bullet") {
                true => self.paragraph_count(drawable)?,
                false => 1,
            };
            targets.push((drawable, steps));
        }

        let old = references(&slide_message, slide::BUILDS)?;
        let old = old.into_iter().chain(references(&slide_message, slide::BUILD_CHUNKS)?);
        for identifier in old.filter(|e| self.package.contains(*e)).collect::<Vec<_>>() {
            self.package.remove(identifier)?;
        }

        let mut next = self.package.last_object_identifier()?;
        let mut builds = Vec::new();
        let mut chunks = Vec::new();
        let mut events = 0;
        for (spec, (drawable, steps)) in specs.iter().zip(&targets) {
            next += 1;
            let build = next;
            let message = build_message(spec, *drawable, *steps);
            // like Keynote, builds and chunks leave their references out of the header
            self.package
                .insert(&component, IwaObject::new(build, TYPE_BUILD, &version, &message, &[]))?;
            builds.push(build);

            let uuid = random_uuid();
            for step in 0..*steps {
                next += 1;
                let automatic = step == 0 && spec.automatic;
                events += usize::from(!automatic);
                let message = chunk_message(spec, build, step, &uuid, automatic);
                self.package.insert(
                    &component,
                    IwaObject::new(next, TYPE_BUILD_CHUNK, &version, &message, &[]),
                )?;
                chunks.push(next);
            }
        }
        self.package.set_last_object_identifier(next)?;

        let added: Vec<u64> = builds.iter().chain(&chunks).copied().collect();
        self.package.update(slide, &added, |e| {
            let builds: Vec<_> = builds.iter().map(|e| make_reference(*e)).collect();
            let chunks: Vec<_> = chunks.iter().map(|e| make_reference(*e)).collect();
            e.set_messages(slide::BUILDS, &builds);
            e.set_messages(slide::BUILD_CHUNKS, &chunks);
            Ok(())
        })?;
        Ok(events)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Drawable a build target names: "title", "body", "id:N" or "object N" (1-based, see
    /// SlideInfo.objects). Only objects shown on the slide qualify, layouts like Blank keep
    /// hidden placeholders.
    fn resolve_target(&self, slide_message: &Message, target: &str) -> Result<u64> {
        let target = target.trim().to_lowercase();
        let objects = references(slide_message, slide::DRAWABLES_Z_ORDER)?;
        let drawable = match target.as_str() {
            "title" => reference(slide_message, slide::TITLE_PLACEHOLDER)?,
            "body" => reference(slide_message, slide::BODY_PLACEHOLDER)?,
            other if other.starts_with("id:") => Some(super::objects::parse_object_id(other)?),
            other => {
                let number = other
                    .strip_prefix("object")
                    .map(str::trim)
                    .and_then(|e| e.parse::<usize>().ok());
                let number = number
                    .ok_or_else(|| anyhow!("unknown build target {other:?}, use title, body, id:N or object N"))?;
                let object = number.checked_sub(1).and_then(|e| objects.get(e)).copied();
                Some(object.ok_or_else(|| {
                    anyhow!(
                        "object {number} is out of range, the slide has {} objects",
                        objects.len()
                    )
                })?)
            }
        };
        drawable
            .filter(|e| self.package.contains(*e) && objects.contains(e))
            .ok_or_else(|| anyhow!("the slide shows no {target}"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Non-empty paragraphs of a drawable's text, at least one
    fn paragraph_count(&self, drawable: u64) -> Result<usize> {
        let Some(storage) = self.drawable_storage(drawable)? else {
            return Ok(1);
        };
        let text = self.text(storage)?;
        Ok(text.split('\n').filter(|e| !e.trim().is_empty()).count().max(1))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn drawable_storage(&self, drawable: u64) -> Result<Option<u64>> {
        let message = self.package.message(drawable)?;
        let storage = match self.package.object(drawable)?.message_type() {
            TYPE_PLACEHOLDER => {
                let shape = message.message(placeholder::SUPER)?.unwrap_or_default();
                reference(&shape, shape_info::OWNED_STORAGE)?
            }
            TYPE_SHAPE_INFO => reference(&message, shape_info::OWNED_STORAGE)?,
            _ => None,
        };
        Ok(storage.filter(|e| self.package.contains(*e)))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn set_slide_nodes(&mut self, nodes: &[u64], added: &[u64]) -> Result<()> {
        let (show, _) = self.show()?;
        self.package.update(show, added, |show| {
            let tree = show.message(show::SLIDE_TREE)?;
            let mut tree = tree.ok_or_else(|| anyhow!("the slide tree is missing"))?;
            let references: Vec<_> = nodes.iter().map(|e| make_reference(*e)).collect();
            tree.set_messages(slide_tree::SLIDES, &references);
            show.set_message(show::SLIDE_TREE, &tree);
            Ok(())
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn move_slide(&mut self, from: usize, to: usize) -> Result<()> {
        let node = self.slide_node(from)?;
        let mut nodes = self.slide_nodes()?;
        nodes.remove(from - 1);
        nodes.insert(to.clamp(1, nodes.len() + 1) - 1, node);
        self.set_slide_nodes(&nodes, &[])
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Inserts a copy of slide `number` so that it ends up at `position` (1-based)
    pub fn duplicate_slide(&mut self, number: usize, position: usize) -> Result<()> {
        let source = self.slide_node(number)?;
        let copy = self.copy_graph(source, None)?[&source];

        let mut nodes = self.slide_nodes()?;
        nodes.insert(position.clamp(1, nodes.len() + 1) - 1, copy);
        self.set_slide_nodes(&nodes, &[copy])?;

        self.package.update(copy, &[], |e| {
            e.remove(slide_node::UNIQUE_IDENTIFIER);
            e.remove(slide_node::COPY_FROM_SLIDE_IDENTIFIER);
            e.set_bool(slide_node::THUMBNAILS_ARE_DIRTY, true);
            Ok(())
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Inserts a new empty slide made from a layout so that it ends up at `position` (1-based).
    ///
    /// Keynote builds a slide from its layout's own objects: copies of the layout's placeholders
    /// with their text, pointing at the layout through `template_slide`. The slide node and
    /// presenter notes are modelled on the first slide of the presentation.
    pub fn new_slide_from_layout(&mut self, layout: &str, position: usize) -> Result<()> {
        let mut names = Vec::new();
        let mut layout_slide = None;
        for identifier in self.layout_slide_list()? {
            let name = self
                .package
                .message(identifier)?
                .string(slide::NAME)
                .unwrap_or_default();
            if name == layout && layout_slide.is_none() {
                layout_slide = Some(identifier);
            }
            names.push(name);
        }
        let layout_slide =
            layout_slide.ok_or_else(|| anyhow!("unknown layout {layout:?}, available: {}", names.join(", ")))?;
        let model_node = *self
            .slide_nodes()?
            .first()
            .ok_or_else(|| anyhow!("the presentation has no slides"))?;

        let map = self.copy_graph(layout_slide, Some("Slide"))?;
        let slide = map[&layout_slide];

        let drawables = references(&self.package.message(slide)?, slide::OWNED_DRAWABLES)?;
        self.package.update(slide, &[layout_slide], |e| {
            for field in LAYOUT_ONLY_FIELDS {
                e.remove(field);
            }
            // a layout's object placeholder only carries over when it is one of its drawables
            if reference(e, slide::OBJECT_PLACEHOLDER)?.is_some_and(|e| !drawables.contains(&e)) {
                e.remove(slide::OBJECT_PLACEHOLDER);
            }
            e.set_message(slide::TEMPLATE_SLIDE, &make_reference(layout_slide));
            e.set_bytes(slide::INSTRUCTIONAL_TEXT_MAP, Vec::new());
            Ok(())
        })?;

        // the layout's guides stay with the layout
        if let Some(guides) = reference(&self.package.message(slide)?, slide::USER_GUIDES)?
            && map.values().any(|e| *e == guides)
        {
            self.package.update(guides, &[], |e| {
                e.fields.clear();
                Ok(())
            })?;
        }

        // copies only the dropped layout fields pointed at
        let copies: HashSet<u64> = map.values().copied().collect();
        let mut reachable = HashSet::from([slide]);
        let mut queue = vec![slide];
        while let Some(identifier) = queue.pop() {
            for reference in self.package.object(identifier)?.object_references() {
                if copies.contains(&reference) && reachable.insert(reference) {
                    queue.push(reference);
                }
            }
        }
        let orphans: HashSet<u64> = copies.difference(&reachable).copied().collect();
        self.remove_objects(&orphans)?;

        let note = self.add_note(slide, model_node)?;
        let node = self.add_slide_node(slide, layout_slide, model_node)?;

        let mut nodes = self.slide_nodes()?;
        nodes.insert(position.clamp(1, nodes.len() + 1) - 1, node);
        self.set_slide_nodes(&nodes, &[node])?;

        let (show, _) = self.show()?;
        let mut infos = self.package.component_infos()?;
        let mut referring = vec![slide, node, show];
        referring.extend(note);
        self.add_external_references(&mut infos, &referring)?;
        self.package.set_component_infos(&infos)?;

        // the layout's sample text goes, its paragraph styles stay
        self.clear_slide(position.clamp(1, nodes.len()))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Gives a new slide empty presenter notes modelled on the model slide's, returns the note
    /// and its storage
    fn add_note(&mut self, slide: u64, model_node: u64) -> Result<Vec<u64>> {
        let model_slide = self.package.message(self.slide_of(model_node)?)?;
        let Some(model_note) = reference(&model_slide, slide::NOTE)?.filter(|e| self.package.contains(*e)) else {
            return Ok(Vec::new());
        };
        let model_storage = reference(&self.package.message(model_note)?, note::CONTAINED_STORAGE)?;
        let Some(model_storage) = model_storage.filter(|e| self.package.contains(*e)) else {
            return Ok(Vec::new());
        };

        let component = self.package.component_of(slide)?.to_string();
        let next = self.package.last_object_identifier()?;
        let (note, storage) = (next + 1, next + 2);

        let storage_object = self.package.object(model_storage)?.remapped(storage, &|_| None)?;
        let note_object = self
            .package
            .object(model_note)?
            .remapped(note, &|e| (e == model_storage).then_some(storage))?;
        self.package.insert(&component, storage_object)?;
        self.package.insert(&component, note_object)?;
        self.package.set_last_object_identifier(storage)?;
        self.set_text(storage, "")?;

        self.package.update(slide, &[note], |e| {
            e.set_message(slide::NOTE, &make_reference(note));
            Ok(())
        })?;
        Ok(vec![note, storage])
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// A slide node for a new slide, modelled on an existing one
    fn add_slide_node(&mut self, slide: u64, layout_slide: u64, model_node: u64) -> Result<u64> {
        let model_slide = self.slide_of(model_node)?;
        let node = self.package.last_object_identifier()? + 1;
        let component = self.package.component_of(model_node)?.to_string();

        let object = self
            .package
            .object(model_node)?
            .remapped(node, &|e| (e == model_slide).then_some(slide))?;
        self.package.insert(&component, object)?;
        self.package.set_last_object_identifier(node)?;

        // it shares the model's (dirty) thumbnail until Keynote redraws it
        let mut infos = self.package.component_infos()?;
        if let Some(index) = infos
            .iter()
            .position(|e| component_path(e).as_deref() == Some(component.as_str()))
        {
            let source = infos[index].clone();
            copy_object_bookkeeping(&source, &mut infos[index], model_node, node)?;
            self.package.set_component_infos(&infos)?;
        }

        let layout_uuid = self.object_uuid(layout_slide)?;
        self.package.update(node, &[], |e| {
            for field in [
                slide_node::UNIQUE_IDENTIFIER,
                slide_node::COPY_FROM_SLIDE_IDENTIFIER,
                slide_node::HYPERLINK_MAP,
                slide_node::DIGESTS_FOR_THUMBNAIL,
                slide_node::LIVE_VIDEO_SOURCE_IDS,
                slide_node::LIVE_VIDEO_SOURCE_USAGE,
            ] {
                e.remove(field);
            }
            e.set_bool(slide_node::IS_SKIPPED, false);
            e.set_bool(slide_node::HAS_TRANSITION, false);
            e.set_bool(slide_node::HAS_NOTE, false);
            e.set_bool(slide_node::THUMBNAILS_ARE_DIRTY, true);
            e.set_varint(slide_node::BUILD_EVENT_COUNT, 0);
            e.set_bool(slide_node::HAS_EXPLICIT_BUILDS, false);
            match &layout_uuid {
                Some(uuid) => e.set_message(slide_node::TEMPLATE_SLIDE_ID, uuid),
                None => e.remove(slide_node::TEMPLATE_SLIDE_ID),
            }
            Ok(())
        })?;
        Ok(node)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// TSP.UUID a component info assigns to an object
    fn object_uuid(&self, identifier: u64) -> Result<Option<Message>> {
        for info in self.package.component_infos()? {
            for entry in info.messages(component_info::OBJECT_UUID_MAP_ENTRIES)? {
                if entry.varint(uuid_entry::IDENTIFIER) == Some(identifier) {
                    return entry.message(uuid_entry::UUID);
                }
            }
        }
        Ok(None)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn delete_slide(&mut self, number: usize) -> Result<()> {
        let node = self.slide_node(number)?;
        let mut nodes = self.slide_nodes()?;
        nodes.retain(|e| *e != node);
        if nodes.is_empty() {
            bail!("refusing to delete the last slide of the presentation");
        }
        self.set_slide_nodes(&nodes, &[])?;

        let owned: HashSet<u64> = self.owned_graph(node)?.into_iter().collect();
        self.remove_objects(&owned)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Removes objects along with every mention of them, and components left empty
    pub(super) fn remove_objects(&mut self, removed: &HashSet<u64>) -> Result<()> {
        let mut components = HashSet::new();
        for identifier in removed {
            components.insert(self.package.component_of(*identifier)?.to_string());
            self.package.remove(*identifier)?;
        }

        // whatever still points at the removed objects (view state mostly) lets go of them
        let referrers = self.package.referrers();
        let mut dangling: Vec<u64> = removed
            .iter()
            .filter_map(|e| referrers.get(e))
            .flatten()
            .copied()
            .collect();
        dangling.sort_unstable();
        dangling.dedup();
        for identifier in dangling {
            self.package
                .object_mut(identifier)?
                .strip_references(&|e| removed.contains(&e))?;
        }

        let mut infos = self.package.component_infos()?;
        let mut removed_components = HashSet::new();
        for component in components {
            if self.package.component_objects(&component).next().is_some() {
                continue;
            }
            self.package.remove_component(&component)?;
            infos.retain(|info| {
                let matches = component_path(info).as_deref() == Some(component.as_str());
                if matches {
                    removed_components.extend(info.varint(component_info::IDENTIFIER));
                }
                !matches
            });
        }

        for info in &mut infos {
            forget_objects(info, &|e| removed.contains(&e), &|e| removed_components.contains(&e))?;
        }
        self.package.set_component_infos(&infos)
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Slide copies
    ////////////////////////////////////////////////////////////////////////////////

    /// Objects only `root` can reach, `root` first
    pub(super) fn owned_graph(&self, root: u64) -> Result<Vec<u64>> {
        let view_state: HashSet<u64> = VIEW_STATE_TYPES
            .iter()
            .flat_map(|e| self.package.find_type(*e))
            .collect();
        // layouts share the slide archive type, they are told apart by the theme listing them
        let layouts = self.layout_slides()?;
        // styles belong to the document even when one slide alone uses them (Keynote leaves
        // style variations out of the stylesheet's reference list)
        let stylesheet_component = self.stylesheet_component();

        let mut order = vec![root];
        let mut owned = HashSet::from([root]);
        let mut queue = VecDeque::from([root]);
        while let Some(identifier) = queue.pop_front() {
            for reference in self.package.object(identifier)?.object_references() {
                if owned.contains(&reference) || !self.package.contains(reference) {
                    continue;
                }
                let message_type = self.package.object(reference)?.message_type();
                if BOUNDARY_TYPES.contains(&message_type)
                    || view_state.contains(&reference)
                    || layouts.contains(&reference)
                    || stylesheet_component.as_deref() == self.package.component_of(reference).ok()
                {
                    continue;
                }
                owned.insert(reference);
                order.push(reference);
                queue.push_back(reference);
            }
        }

        // anything referenced from outside is shared, and so is whatever hangs off it
        let referrers = self.package.referrers();
        loop {
            let shared: Vec<u64> = owned
                .iter()
                .copied()
                .filter(|e| *e != root)
                .filter(|e| {
                    let mut referrers = referrers.get(e).into_iter().flatten();
                    referrers.any(|e| !owned.contains(e) && !view_state.contains(e))
                })
                .collect();

            if shared.is_empty() {
                break;
            }
            for identifier in shared {
                owned.remove(&identifier);
            }
        }

        order.retain(|e| owned.contains(e));
        Ok(order)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Copies the objects `root` owns under fresh ids, returns the id map.
    ///
    /// Objects go to a copy of their component when its root object is copied too (a slide's
    /// own Slide-N.iwa), otherwise next to their source (eg Document.iwa). With
    /// `new_component`, objects of the root's component always go to a new component with
    /// that locator prefix, used for slides made from a layout's TemplateSlide-N.iwa.
    fn copy_graph(&mut self, root: u64, new_component: Option<&str>) -> Result<HashMap<u64, u64>> {
        let owned = self.owned_graph(root)?;
        let mut next = self.package.last_object_identifier()?;
        let mut map = HashMap::new();
        for identifier in &owned {
            next += 1;
            map.insert(*identifier, next);
        }

        let mut infos = self.package.component_infos()?;
        let source_infos = infos.clone();
        let info_index: HashMap<String, usize> = source_infos
            .iter()
            .enumerate()
            .filter_map(|(index, info)| Some((component_path(info)?, index)))
            .collect();
        let root_path = self.package.component_of(root)?.to_string();

        let mut targets = HashMap::new();
        let mut created: HashMap<String, String> = HashMap::new();
        for identifier in &owned {
            let source_path = self.package.component_of(*identifier)?.to_string();
            let source_index = info_index.get(&source_path).copied();
            let component = source_index.and_then(|e| source_infos[e].varint(component_info::IDENTIFIER));

            let new_component_id = match new_component {
                Some(_) if source_path == root_path => Some(map[&root]),
                _ => component.and_then(|e| map.get(&e)).copied(),
            };
            let target = match (new_component_id, source_index) {
                (Some(new_component_id), Some(source_index)) => match created.get(&source_path) {
                    Some(path) => path.clone(),
                    None => {
                        let source_info = &source_infos[source_index];
                        let prefix = match new_component {
                            Some(prefix) => prefix.to_string(),
                            None => locator_prefix(source_info),
                        };
                        let locator = self.new_locator(&prefix, new_component_id);
                        let path = format!("Index/{locator}.iwa");

                        let mut info = source_info.clone();
                        info.set_varint(component_info::IDENTIFIER, new_component_id);
                        info.set_bytes(component_info::PREFERRED_LOCATOR, locator.into_bytes());
                        info.remove(component_info::LOCATOR);
                        info.remove(component_info::DATA_REFERENCES);
                        info.remove(component_info::OBJECT_UUID_MAP_ENTRIES);
                        info.remove(COMPONENT_AMBIGUOUS_OBJECT_IDENTIFIERS);

                        let references = remap_external_references(source_info, &map)?;
                        info.set_messages(component_info::EXTERNAL_REFERENCES, &references);

                        infos.push(info);
                        created.insert(source_path.clone(), path.clone());
                        path
                    }
                },
                _ => source_path.clone(),
            };
            targets.insert(*identifier, (source_path, target));
        }

        for identifier in &owned {
            let copy = self
                .package
                .object(*identifier)?
                .remapped(map[identifier], &|e| map.get(&e).copied())?;
            self.package.insert(&targets[identifier].1, copy)?;
        }

        // per-object component bookkeeping: media usage and UUIDs
        for identifier in &owned {
            let (source_path, target_path) = &targets[identifier];
            let (Some(source_index), Some(target_index)) = (
                info_index.get(source_path).copied(),
                infos
                    .iter()
                    .position(|e| component_path(e).as_deref() == Some(target_path.as_str())),
            ) else {
                continue;
            };
            copy_object_bookkeeping(
                &source_infos[source_index],
                &mut infos[target_index],
                *identifier,
                map[identifier],
            )?;
        }

        // cross-component references of the copies and of the slide tree pointing at them
        let (show, _) = self.show()?;
        let mut referring: Vec<u64> = map.values().copied().collect();
        referring.push(show);
        self.add_external_references(&mut infos, &referring)?;

        self.package.set_component_infos(&infos)?;
        self.package.set_last_object_identifier(next)?;

        // copied builds get their own identity, their chunks share it
        let mut build_uuids: HashMap<u64, Message> = HashMap::new();
        for copy in map.values().copied().collect::<Vec<_>>() {
            if self.package.object(copy)?.message_type() != TYPE_BUILD_CHUNK {
                continue;
            }
            let Some(build) = reference(&self.package.message(copy)?, build_chunk::BUILD)? else {
                continue;
            };
            let uuid = build_uuids.entry(build).or_insert_with(random_uuid).clone();
            self.package.update(copy, &[], |e| {
                if e.has(build_chunk::BUILD_ID) {
                    e.set_message(build_chunk::BUILD_ID, &uuid);
                }
                if let Some(mut identifier) = e.message(build_chunk::IDENTIFIER)? {
                    identifier.set_message(chunk_identifier::BUILD_ID, &uuid);
                    e.set_message(build_chunk::IDENTIFIER, &identifier);
                }
                Ok(())
            })?;
        }
        Ok(map)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Locator for a new component, eg "Slide-<id>"
    fn new_locator(&self, prefix: &str, identifier: u64) -> String {
        let mut locator = format!("{prefix}-{identifier}");
        let mut suffix = 1;
        while self.package.has_component(&format!("Index/{locator}.iwa")) {
            suffix += 1;
            locator = format!("{prefix}-{identifier}-{suffix}");
        }
        locator
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Lists references of `objects` into other components in their component's info
    pub(super) fn add_external_references(&self, infos: &mut [Message], objects: &[u64]) -> Result<()> {
        let component_ids: HashMap<String, u64> = infos
            .iter()
            .filter_map(|e| Some((component_path(e)?, e.varint(component_info::IDENTIFIER)?)))
            .collect();

        for object in objects {
            let path = self.package.component_of(*object)?;
            let Some(info) = infos.iter_mut().find(|e| component_path(e).as_deref() == Some(path)) else {
                continue;
            };

            let mut references = info.messages(component_info::EXTERNAL_REFERENCES)?;
            let mut known: HashSet<(u64, u64)> = references
                .iter()
                .filter_map(|e| {
                    Some((
                        e.varint(external_reference::COMPONENT)?,
                        e.varint(external_reference::OBJECT)?,
                    ))
                })
                .collect();

            let before = references.len();
            for target in self.package.object(*object)?.object_references() {
                let Ok(target_path) = self.package.component_of(target) else {
                    continue;
                };
                if target_path == path {
                    continue;
                }
                let Some(component) = component_ids.get(target_path) else {
                    continue;
                };
                if known.insert((*component, target)) {
                    let mut reference = Message::default();
                    reference.set_varint(external_reference::COMPONENT, *component);
                    reference.set_varint(external_reference::OBJECT, target);
                    references.push(reference);
                }
            }

            if references.len() != before {
                info.set_messages(component_info::EXTERNAL_REFERENCES, &references);
            }
        }
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
/// KN.BuildArchive the way Keynote writes one
fn build_message(spec: &BuildSpec, drawable: u64, steps: usize) -> Message {
    let mut animation = Message::default();
    animation.set_bytes(animation::TYPE, spec.kind.clone().into_bytes());
    animation.set_bytes(animation::EFFECT, spec.effect.clone().into_bytes());
    animation.set(animation::DURATION, Value::Fixed64(spec.duration.to_bits()));
    animation.set(animation::DELAY, Value::Fixed64(0f64.to_bits()));
    animation.set_varint(animation::RANDOM_SEED, u64::from(rand::random::<u32>() >> 1));
    animation.set_bool(animation::WRITING_DIRECTION_IS_RTL, false);

    let mut attributes = Message::default();
    attributes.set_varint(build_attributes::EVENT_TRIGGER, 1);
    attributes.set(build_attributes::CHART_ROTATION_3D, Value::Fixed64(60f64.to_bits()));
    attributes.set_message(build_attributes::ANIMATION, &animation);
    // by object, forward
    attributes.set_varint(build_attributes::TEXT_DELIVERY, 1);
    attributes.set_varint(build_attributes::DELIVERY_OPTION, 1);

    let mut build = Message::default();
    build.set_message(build::DRAWABLE, &make_reference(drawable));
    build.set_bytes(build::DELIVERY, spec.delivery.clone().into_bytes());
    build.set(build::DURATION, Value::Fixed64(0f64.to_bits()));
    build.set_message(build::ATTRIBUTES, &attributes);
    build.set_varint(build::CHUNK_ID_SEED, steps as u64 + 1);
    build
}

////////////////////////////////////////////////////////////////////////////////
/// KN.BuildChunkArchive for step `step` (0-based) of a build
fn chunk_message(spec: &BuildSpec, build: u64, step: usize, uuid: &Message, automatic: bool) -> Message {
    let delay = if step == 0 { spec.delay } else { 0.0 };

    let mut identifier = Message::default();
    identifier.set_message(chunk_identifier::BUILD_ID, uuid);
    identifier.set_varint(chunk_identifier::CHUNK_ID, step as u64 + 1);

    let mut chunk = Message::default();
    chunk.set_message(build_chunk::BUILD, &make_reference(build));
    chunk.set(build_chunk::DELAY, Value::Fixed64(delay.to_bits()));
    chunk.set(build_chunk::DURATION, Value::Fixed64(spec.duration.to_bits()));
    chunk.set_bool(build_chunk::AUTOMATIC, automatic);
    chunk.set_bool(build_chunk::REFERENT, true);
    chunk.set_message(build_chunk::IDENTIFIER, &identifier);
    chunk.set_message(build_chunk::BUILD_ID, uuid);
    chunk
}

////////////////////////////////////////////////////////////////////////////////
/// TSP.UUID
fn random_uuid() -> Message {
    let mut uuid = Message::default();
    uuid.set_varint(uuid::LOWER, rand::random());
    uuid.set_varint(uuid::UPPER, rand::random());
    uuid
}

////////////////////////////////////////////////////////////////////////////////
/// "Slide-123" gives "Slide"
fn locator_prefix(info: &Message) -> String {
    let locator = info.string(component_info::PREFERRED_LOCATOR).unwrap_or_default();
    let prefix = locator.split('-').next().filter(|e| !e.is_empty());
    prefix.unwrap_or("Slide").to_string()
}

////////////////////////////////////////////////////////////////////////////////
/// The slide's transition, None when it has no effect set
fn transition(slide_message: &Message) -> Result<Option<TransitionInfo>> {
    let transition = slide_message.message(slide::TRANSITION)?.unwrap_or_default();
    let attributes = transition.message(transition::ATTRIBUTES)?.unwrap_or_default();
    let animation = attributes
        .message(transition_attributes::ANIMATION)?
        .unwrap_or_default();

    // Keynote stores "none" when the transition was removed
    let effect = animation.string(animation::EFFECT);
    let Some(effect) = effect.filter(|e| !e.is_empty() && e != "none") else {
        return Ok(None);
    };
    Ok(Some(TransitionInfo {
        effect,
        duration: animation.double(animation::DURATION),
        delay: animation.double(animation::DELAY),
        automatic: animation.bool(animation::IS_AUTOMATIC).unwrap_or(false),
    }))
}

////////////////////////////////////////////////////////////////////////////////
/// External references of a copied component: targets that were copied along point at the copies
fn remap_external_references(source_info: &Message, map: &HashMap<u64, u64>) -> Result<Vec<Message>> {
    let references = source_info.messages(component_info::EXTERNAL_REFERENCES)?;
    // copied targets are re-added with their new component by add_external_references
    let references = references.into_iter().filter(|e| {
        let object = e.varint(external_reference::OBJECT);
        !object.is_some_and(|e| map.contains_key(&e))
    });
    Ok(references.collect())
}

////////////////////////////////////////////////////////////////////////////////
/// Carries media usage counts and a fresh UUID (when the source had one) over to a copied object
fn copy_object_bookkeeping(source_info: &Message, target_info: &mut Message, source: u64, copy: u64) -> Result<()> {
    let mut target_data = target_info.messages(component_info::DATA_REFERENCES)?;
    let mut data_changed = false;
    for data in source_info.messages(component_info::DATA_REFERENCES)? {
        let objects = data.messages(data_reference::OBJECTS)?;
        let Some(mut usage) = objects
            .into_iter()
            .find(|e| e.varint(data_object::OBJECT) == Some(source))
        else {
            continue;
        };
        usage.set_varint(data_object::OBJECT, copy);

        let data_id = data.varint(data_reference::DATA);
        match target_data
            .iter_mut()
            .find(|e| e.varint(data_reference::DATA) == data_id)
        {
            Some(entry) => entry.push_message(data_reference::OBJECTS, &usage),
            None => {
                let mut entry = Message::default();
                if let Some(data_id) = data_id {
                    entry.set_varint(data_reference::DATA, data_id);
                }
                entry.push_message(data_reference::OBJECTS, &usage);
                target_data.push(entry);
            }
        }
        data_changed = true;
    }
    if data_changed {
        target_info.set_messages(component_info::DATA_REFERENCES, &target_data);
    }

    let entries = source_info.messages(component_info::OBJECT_UUID_MAP_ENTRIES)?;
    if entries.iter().any(|e| e.varint(uuid_entry::IDENTIFIER) == Some(source)) {
        let mut uuid = Message::default();
        uuid.set_varint(uuid::LOWER, rand::random());
        uuid.set_varint(uuid::UPPER, rand::random());

        let mut entry = Message::default();
        entry.set_varint(uuid_entry::IDENTIFIER, copy);
        entry.set_message(uuid_entry::UUID, &uuid);
        target_info.push_message(component_info::OBJECT_UUID_MAP_ENTRIES, &entry);
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
/// Drops every mention of removed objects and components from a component info
fn forget_objects(
    info: &mut Message,
    removed: &dyn Fn(u64) -> bool,
    removed_component: &dyn Fn(u64) -> bool,
) -> Result<()> {
    let references = info.messages(component_info::EXTERNAL_REFERENCES)?;
    let kept: Vec<_> = references
        .iter()
        .filter(|e| !e.varint(external_reference::OBJECT).is_some_and(removed))
        .filter(|e| !e.varint(external_reference::COMPONENT).is_some_and(removed_component))
        .cloned()
        .collect();
    if kept.len() != references.len() {
        info.set_messages(component_info::EXTERNAL_REFERENCES, &kept);
    }

    let datas = info.messages(component_info::DATA_REFERENCES)?;
    let mut kept_datas = Vec::new();
    let mut datas_changed = false;
    for mut data in datas {
        let objects = data.messages(data_reference::OBJECTS)?;
        let kept: Vec<_> = objects
            .iter()
            .filter(|e| !e.varint(data_object::OBJECT).is_some_and(removed))
            .cloned()
            .collect();
        if kept.len() != objects.len() {
            datas_changed = true;
            data.set_messages(data_reference::OBJECTS, &kept);
        }
        if !kept.is_empty() {
            kept_datas.push(data);
        }
    }
    if datas_changed {
        info.set_messages(component_info::DATA_REFERENCES, &kept_datas);
    }

    let entries = info.messages(component_info::OBJECT_UUID_MAP_ENTRIES)?;
    let kept: Vec<_> = entries
        .iter()
        .filter(|e| !e.varint(uuid_entry::IDENTIFIER).is_some_and(removed))
        .cloned()
        .collect();
    if kept.len() != entries.len() {
        info.set_messages(component_info::OBJECT_UUID_MAP_ENTRIES, &kept);
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
pub(super) fn format_uuid(upper: u64, lower: u64) -> String {
    // random (version 4, RFC 4122 variant) UUID in the uppercase form Keynote writes
    let upper = (upper & !0xf000) | 0x4000;
    let lower = (lower & !(0xc000 << 48)) | (0x8000 << 48);
    format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        upper >> 32,
        (upper >> 16) & 0xffff,
        upper & 0xffff,
        lower >> 48,
        lower & 0xffff_ffff_ffff
    )
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::tools::keynote::format::iwa::{IwaObject, encode_iwa, test_object};
    use crate::tools::keynote::format::proto::Value;

    const TYPE_TEMPLATE_SLIDE: u32 = 6;
    const TYPE_UI_STATE: u32 = 3;
    const TYPE_SLIDE: u32 = 5;
    const TYPE_PLACEHOLDER: u32 = 7;
    const TYPE_NOTE: u32 = 15;
    const TYPE_STYLESHEET: u32 = 401;
    const TYPE_PARAGRAPH_STYLE: u32 = 2021;
    const TYPE_STORAGE: u32 = 2001;

    const DOCUMENT_COMPONENT: u64 = 1;
    const SLIDE: u64 = 200;
    const TEMPLATE_SLIDE: u64 = 31;
    const PARAGRAPH_STYLE: u64 = 41;
    const TITLE_PLACEHOLDER: u64 = 201;

    ////////////////////////////////////////////////////////////////////////////////
    fn object(identifier: u64, message_type: u32, message: Message, references: &[u64]) -> IwaObject {
        test_object(identifier, message_type, &message, references)
    }

    fn with_reference(number: u32, identifier: u64) -> Message {
        let mut message = Message::default();
        message.set_message(number, &make_reference(identifier));
        message
    }

    fn storage(text: &str, paragraph_starts: &[u64]) -> Message {
        let mut table = Message::default();
        for start in paragraph_starts {
            let mut entry = Message::default();
            entry.set_varint(1, *start);
            entry.set_message(2, &make_reference(PARAGRAPH_STYLE));
            table.push_message(1, &entry);
        }

        let mut storage = Message::default();
        storage.set_bytes(3, text.as_bytes().to_vec());
        storage.set_message(5, &table);
        storage
    }

    fn placeholder(storage: u64) -> Message {
        let mut placeholder = Message::default();
        placeholder.set_message(placeholder::SUPER, &with_reference(shape_info::OWNED_STORAGE, storage));
        placeholder
    }

    fn component(identifier: u64, locator: &str, external: &[(u64, u64)]) -> Message {
        let mut info = Message::default();
        info.set_varint(component_info::IDENTIFIER, identifier);
        info.set_bytes(component_info::PREFERRED_LOCATOR, locator.as_bytes().to_vec());
        for (component, object) in external {
            let mut reference = Message::default();
            reference.set_varint(external_reference::COMPONENT, *component);
            reference.set_varint(external_reference::OBJECT, *object);
            info.push_message(component_info::EXTERNAL_REFERENCES, &reference);
        }
        info
    }

    /// Minimal presentation: one "Title & Bullets" slide in its own component, the document,
    /// show, theme, layout, a shared paragraph style and UI state (selecting the title) in
    /// Document.iwa
    fn write_fixture(path: &Path) {
        let mut size = Message::default();
        size.set(size::WIDTH, Value::Fixed32(1920f32.to_bits()));
        size.set(size::HEIGHT, Value::Fixed32(1080f32.to_bits()));

        let mut show = with_reference(show::THEME, 20);
        show.set_message(show::SLIDE_TREE, &with_reference(slide_tree::SLIDES, 100));
        show.set_message(show::SIZE, &size);

        let mut template = Message::default();
        template.set_bytes(slide::NAME, b"Title & Bullets".to_vec());

        let mut node = with_reference(slide_node::SLIDE, SLIDE);
        node.set_bool(slide_node::IS_SKIPPED, false);

        let document = vec![
            object(1, TYPE_DOCUMENT, with_reference(2, 10), &[10]),
            object(10, TYPE_SHOW, show, &[20, 100]),
            object(20, TYPE_THEME, with_reference(theme::TEMPLATES, 30), &[30]),
            object(
                30,
                TYPE_SLIDE_NODE,
                with_reference(slide_node::SLIDE, TEMPLATE_SLIDE),
                &[TEMPLATE_SLIDE],
            ),
            object(TEMPLATE_SLIDE, TYPE_TEMPLATE_SLIDE, template, &[]),
            object(
                40,
                TYPE_STYLESHEET,
                with_reference(1, PARAGRAPH_STYLE),
                &[PARAGRAPH_STYLE],
            ),
            object(PARAGRAPH_STYLE, TYPE_PARAGRAPH_STYLE, Message::default(), &[]),
            object(
                50,
                TYPE_UI_STATE,
                with_reference(1, TITLE_PLACEHOLDER),
                &[TITLE_PLACEHOLDER],
            ),
            object(100, TYPE_SLIDE_NODE, node, &[SLIDE]),
        ];

        let mut slide = with_reference(slide::TITLE_PLACEHOLDER, TITLE_PLACEHOLDER);
        slide.set_message(slide::BODY_PLACEHOLDER, &make_reference(203));
        slide.set_message(slide::TEMPLATE_SLIDE, &make_reference(TEMPLATE_SLIDE));
        slide.set_message(slide::NOTE, &make_reference(205));

        let slide = vec![
            object(SLIDE, TYPE_SLIDE, slide, &[TITLE_PLACEHOLDER, 203, TEMPLATE_SLIDE, 205]),
            object(TITLE_PLACEHOLDER, TYPE_PLACEHOLDER, placeholder(202), &[202]),
            object(202, TYPE_STORAGE, storage("Hello", &[0]), &[PARAGRAPH_STYLE]),
            object(203, TYPE_PLACEHOLDER, placeholder(204), &[204]),
            object(204, TYPE_STORAGE, storage("One\nTwo", &[0, 4]), &[PARAGRAPH_STYLE]),
            object(205, TYPE_NOTE, with_reference(note::CONTAINED_STORAGE, 206), &[206]),
            object(206, TYPE_STORAGE, storage("Speak slowly", &[0]), &[PARAGRAPH_STYLE]),
        ];

        let mut metadata = Message::default();
        metadata.set_varint(1, 206);
        let document_component = component(
            DOCUMENT_COMPONENT,
            "Document",
            &[(SLIDE, SLIDE), (SLIDE, TITLE_PLACEHOLDER)],
        );
        let slide_component = component(
            SLIDE,
            "Slide-200",
            &[
                (DOCUMENT_COMPONENT, TEMPLATE_SLIDE),
                (DOCUMENT_COMPONENT, PARAGRAPH_STYLE),
            ],
        );
        metadata.push_message(3, &document_component);
        metadata.push_message(3, &slide_component);
        let metadata = vec![object(2, TYPE_PACKAGE_METADATA, metadata, &[])];

        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, objects) in [
            ("Index/Document.iwa", document),
            ("Index/Slide-200.iwa", slide),
            ("Index/Metadata.iwa", metadata),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(&encode_iwa(&objects).unwrap()).unwrap();
        }
        zip.start_file(DOCUMENT_IDENTIFIER_FILE, options).unwrap();
        zip.write_all(b"00000000-0000-4000-8000-000000000000").unwrap();
        zip.start_file("Data/image.png", options).unwrap();
        zip.write_all(b"not really a png").unwrap();
        zip.finish().unwrap();
    }

    fn temporary(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("mdev-keynote-test-{:016x}", rand::random::<u64>()));
        std::fs::create_dir_all(&folder).unwrap();
        folder.join(name)
    }

    fn titles(document: &KeynoteDocument) -> Vec<Option<String>> {
        document.info().unwrap().slides.into_iter().map(|e| e.title).collect()
    }

    fn external_references(info: &Message) -> Vec<(u64, u64)> {
        let references = info.messages(component_info::EXTERNAL_REFERENCES).unwrap();
        let references = references.iter().map(|e| {
            let component = e.varint(external_reference::COMPONENT).unwrap();
            (component, e.varint(external_reference::OBJECT).unwrap())
        });
        references.collect()
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn describes_the_presentation() {
        let path = temporary("fixture.key");
        write_fixture(&path);

        let info = KeynoteDocument::open(&path).unwrap().info().unwrap();
        assert_eq!(info.width, Some(1920.0));
        assert_eq!(info.layouts.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Title & Bullets"]);
        assert_eq!(info.slides.len(), 1);

        let slide = &info.slides[0];
        assert_eq!(slide.layout.as_deref(), Some("Title & Bullets"));
        assert_eq!(slide.title.as_deref(), Some("Hello"));
        assert_eq!(slide.body.as_deref(), Some("One\nTwo"));
        assert_eq!(slide.notes.as_deref(), Some("Speak slowly"));
        assert!(!slide.skipped);
    }

    #[test]
    fn copies_edits_moves_and_deletes_slides() {
        let path = temporary("fixture.key");
        write_fixture(&path);

        // copy the slide and give the copy its own text
        let mut document = KeynoteDocument::open(&path).unwrap();
        assert_eq!(document.find_layout_slide("Title & Bullets").unwrap(), 1);
        document.duplicate_slide(1, 2).unwrap();
        document.clear_slide(2).unwrap();
        let content = SlideContent {
            title: Some("Copy".to_string()),
            body: Some("A\nB\nC".to_string()),
            notes: None,
            skipped: Some(true),
            ..Default::default()
        };
        document.edit_slide(2, &content).unwrap();

        let copied = path.with_file_name("copied.key");
        document.save(&copied).unwrap();

        let document = KeynoteDocument::open(&copied).unwrap();
        let info = document.info().unwrap();
        assert_eq!(
            titles(&document),
            vec![Some("Hello".to_string()), Some("Copy".to_string())]
        );
        assert_eq!(info.slides[0].body.as_deref(), Some("One\nTwo"));
        assert_eq!(info.slides[1].body.as_deref(), Some("A\nB\nC"));
        assert_eq!(info.slides[1].notes, None);
        assert!(info.slides[1].skipped);

        // the copy lives in its own component, pointing at the shared layout and style
        let infos = document.package.component_infos().unwrap();
        assert_eq!(infos.len(), 3);
        let copy_info = &infos[2];
        let copy_component = copy_info.varint(component_info::IDENTIFIER).unwrap();
        let copy_path = component_path(copy_info).unwrap();
        assert_eq!(copy_path, format!("Index/Slide-{copy_component}.iwa"));
        assert!(document.package.has_component(&copy_path));
        let copy_references = external_references(copy_info);
        assert!(copy_references.contains(&(DOCUMENT_COMPONENT, TEMPLATE_SLIDE)));
        assert!(copy_references.contains(&(DOCUMENT_COMPONENT, PARAGRAPH_STYLE)));

        // the new slide node sits next to the old one and points into the new component
        let new_node = document.slide_nodes().unwrap()[1];
        assert_eq!(document.package.component_of(new_node).unwrap(), "Index/Document.iwa");
        let new_slide = document.slide_of(new_node).unwrap();
        assert_eq!(new_slide, copy_component);
        assert!(external_references(&infos[0]).contains(&(copy_component, new_slide)));
        assert!(document.package.last_object_identifier().unwrap() > 206);

        // reorder, then delete the original slide
        let mut document = document;
        document.move_slide(2, 1).unwrap();
        assert_eq!(
            titles(&document),
            vec![Some("Copy".to_string()), Some("Hello".to_string())]
        );
        document.delete_slide(2).unwrap();

        let deleted = path.with_file_name("deleted.key");
        document.save(&deleted).unwrap();

        let document = KeynoteDocument::open(&deleted).unwrap();
        assert_eq!(titles(&document), vec![Some("Copy".to_string())]);
        assert!(!document.package.has_component("Index/Slide-200.iwa"));
        assert!(!document.package.contains(TITLE_PLACEHOLDER));
        assert!(document.package.contains(PARAGRAPH_STYLE));
        assert!(document.package.contains(TEMPLATE_SLIDE));

        let infos = document.package.component_infos().unwrap();
        assert_eq!(infos.len(), 2);
        assert!(
            external_references(&infos[0])
                .iter()
                .all(|(component, _)| *component != SLIDE)
        );

        // the UI state let go of the deleted title placeholder
        assert!(document.package.object(50).unwrap().object_references().is_empty());
        assert!(document.package.message(50).unwrap().fields.is_empty());

        // untouched entries are carried over
        assert!(document.package.has_file("Data/image.png"));
    }

    #[test]
    fn refuses_to_delete_the_last_slide() {
        let path = temporary("fixture.key");
        write_fixture(&path);

        let mut document = KeynoteDocument::open(&path).unwrap();
        assert!(document.delete_slide(1).is_err());
        assert!(document.find_layout_slide("Blank").is_err());
    }

    #[test]
    fn formats_random_uuids() {
        let uuid = format_uuid(u64::MAX, u64::MAX);
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert_eq!(&uuid[19..20], "B");
    }
}
