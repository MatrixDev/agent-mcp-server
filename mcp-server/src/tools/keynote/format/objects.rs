//! Slide objects (drawables): text boxes, shapes, lines and images, written the way Keynote
//! writes them, plus editing, deleting and describing any object of a slide.
//!
//! Every object lives in its slide's component and is listed twice by the slide: in
//! `owned_drawables` and in `drawables_z_order` (back to front). Its style is a variation of
//! one of the theme's presets ("shape-0-shapestyle", "textbox-0-shapestyle",
//! "line-0-shapestyle", "image-0-imageStyle"). Shapes and text boxes are TSWP.ShapeInfoArchive
//! with a bezier outline and their own text storage; lines are the same without storage;
//! images are TSD.ImageArchive, cropped by a TSD.MaskArchive when needed.

use std::collections::HashSet;

use anyhow::{Result, anyhow, bail};
use serde::Serialize;

use super::document::{KeynoteDocument, build, build_chunk, show, size, slide, slide_node};
use super::drawing::{
    Fill, Frame, LineEnd, Outline, Shadow, Stroke, bezier_path_source, float, outline_path, pair_message,
    path_source_outline,
};
use super::iwa::IwaObject;
use super::media::ImageFile;
use super::proto::{Message, make_reference, reference, references};
use super::rich_text::{Paragraph, TYPE_STORAGE, new_storage_message};
use super::style::{
    StyleProps, TYPE_LIST_STYLE, TYPE_MEDIA_STYLE, TYPE_PARAGRAPH_STYLE, TYPE_SHAPE_STYLE, TYPE_SLIDE_STYLE,
    media_props, message_at, para_props, set_message_at, shape_props, slide_props, text_layout_props,
};

////////////////////////////////////////////////////////////////////////////////
pub const TYPE_PLACEHOLDER: u32 = 7;
pub const TYPE_SHAPE_INFO: u32 = 2011;
pub const TYPE_SHAPE: u32 = 3004;
pub const TYPE_IMAGE: u32 = 3005;
pub const TYPE_MASK: u32 = 3006;
pub const TYPE_MOVIE: u32 = 3007;
pub const TYPE_GROUP: u32 = 3008;
pub const TYPE_CONNECTION_LINE: u32 = 3009;
/// empty title and caption every drawable points at
pub const TYPE_DRAWABLE_TITLE: u32 = 3097;
pub const TYPE_CHART: u32 = 5021;
pub const TYPE_TABLE: u32 = 6000;

/// TSD.DrawableArchive
pub mod drawable {
    pub const GEOMETRY: u32 = 1;
    pub const PARENT: u32 = 2;
    pub const TEXT_WRAP: u32 = 3;
    pub const HYPERLINK: u32 = 4;
    pub const LOCKED: u32 = 5;
    pub const ASPECT_LOCKED: u32 = 7;
    pub const DESCRIPTION: u32 = 8;
    pub const TITLE: u32 = 10;
    pub const CAPTION: u32 = 11;
    pub const TITLE_HIDDEN: u32 = 12;
    pub const CAPTION_HIDDEN: u32 = 13;
}

/// TSD.ShapeArchive
mod shape {
    pub const DRAWABLE: u32 = 1;
    pub const STYLE: u32 = 2;
    pub const PATH_SOURCE: u32 = 3;
    pub const STROKE_OFFSET: u32 = 6;
}

/// TSWP.ShapeInfoArchive
mod shape_info {
    pub const SHAPE: u32 = 1;
    pub const DEPRECATED_STORAGE: u32 = 2;
    pub const OWNED_STORAGE: u32 = 4;
    pub const IS_TEXT_BOX: u32 = 6;
}

/// KN.PlaceholderArchive
mod placeholder {
    pub const KIND: u32 = 2;
    pub const SLIDE_NUMBER: u64 = 1;
    pub const TITLE: u64 = 2;
    pub const BODY: u64 = 3;
    pub const OBJECT: u64 = 4;
}

/// TSD.ImageArchive
mod image {
    pub const DRAWABLE: u32 = 1;
    pub const STYLE: u32 = 3;
    pub const ORIGINAL_SIZE: u32 = 4;
    pub const MASK: u32 = 5;
    pub const FLAGS: u32 = 7;
    pub const NATURAL_SIZE: u32 = 9;
    pub const DATA: u32 = 11;
    pub const INTERPRETS_UNTAGGED: u32 = 18;
    pub const ORIGINAL_SVG: u32 = 23;
}

/// TSD.MaskArchive
mod mask {
    pub const DRAWABLE: u32 = 1;
    pub const PATH_SOURCE: u32 = 2;
}

/// KN.SlideArchive.SageTagMapEntry: layouts tag their sample photo "Media"
mod sage_tag {
    pub const TAG: u32 = 1;
    pub const INFO: u32 = 2;
}

/// TSD.GroupArchive
pub mod group {
    pub const CHILDREN: u32 = 2;
}

////////////////////////////////////////////////////////////////////////////////
/// Where an object type keeps its drawable archive, style, text storage and outline
pub struct ObjectLayout {
    pub drawable: &'static [u32],
    pub style: Option<&'static [u32]>,
    pub storage: Option<&'static [u32]>,
    pub path_source: Option<&'static [u32]>,
}

pub fn object_layout(message_type: u32) -> Option<ObjectLayout> {
    let layout = |drawable, style, storage, path_source| {
        Some(ObjectLayout {
            drawable,
            style,
            storage,
            path_source,
        })
    };
    match message_type {
        TYPE_PLACEHOLDER => layout(&[1, 1, 1], Some(&[1, 1, 2]), Some(&[1, 4]), Some(&[1, 1, 3])),
        TYPE_SHAPE_INFO => layout(&[1, 1], Some(&[1, 2]), Some(&[4]), Some(&[1, 3])),
        TYPE_SHAPE => layout(&[1], Some(&[2]), None, Some(&[3])),
        TYPE_CONNECTION_LINE => layout(&[1, 1], Some(&[1, 2]), None, Some(&[1, 3])),
        TYPE_IMAGE => layout(&[1], Some(&[3]), None, None),
        TYPE_MASK => layout(&[1], None, None, Some(&[2])),
        TYPE_MOVIE | TYPE_GROUP | TYPE_TABLE | TYPE_CHART => layout(&[1], None, None, None),
        _ => None,
    }
}

////////////////////////////////////////////////////////////////////////////////
// Specs
////////////////////////////////////////////////////////////////////////////////

/// Stacking position of an object
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZOrder {
    Front,
    Back,
    Above(u64),
    Below(u64),
}

impl ZOrder {
    ////////////////////////////////////////////////////////////////////////////////
    /// "front", "back", "above:<id>" or "below:<id>"
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim().to_lowercase();
        match text.as_str() {
            "front" | "top" => return Ok(ZOrder::Front),
            "back" | "bottom" => return Ok(ZOrder::Back),
            _ => {}
        }
        if let Some(id) = text.strip_prefix("above:") {
            return Ok(ZOrder::Above(parse_object_id(id)?));
        }
        if let Some(id) = text.strip_prefix("below:") {
            return Ok(ZOrder::Below(parse_object_id(id)?));
        }
        bail!("unknown z order {text:?}, use front, back, above:<id> or below:<id>")
    }
}

////////////////////////////////////////////////////////////////////////////////
/// "123" or "id:123"
pub fn parse_object_id(text: &str) -> Result<u64> {
    let text = text.trim();
    let digits = text.strip_prefix("id:").unwrap_or(text).trim();
    digits
        .parse()
        .map_err(|_| anyhow!("invalid object id {text:?}, use the id keynote_info lists, eg \"id:123\""))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Top,
    Middle,
    Bottom,
}

impl VerticalAlign {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_lowercase().as_str() {
            "top" => Ok(VerticalAlign::Top),
            "middle" | "center" | "centre" => Ok(VerticalAlign::Middle),
            "bottom" => Ok(VerticalAlign::Bottom),
            other => bail!("unknown vertical alignment {other:?}, use top, middle or bottom"),
        }
    }
}

/// Fill, line, shadow and opacity of a shape; None leaves the preset's
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapeStyleSpec {
    pub fill: Option<Fill>,
    /// Some(None) removes the line
    pub stroke: Option<Option<Stroke>>,
    /// Some(None) removes the shadow
    pub shadow: Option<Option<Shadow>>,
    pub opacity: Option<f32>,
}

impl ShapeStyleSpec {
    fn is_empty(&self) -> bool {
        self == &ShapeStyleSpec::default()
    }
}

/// Layout of the text inside a shape; None leaves the preset's
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextLayoutSpec {
    /// padding on every side
    pub inset: Option<f32>,
    pub vertical_align: Option<VerticalAlign>,
    pub shrink_to_fit: Option<bool>,
    /// column count and gap
    pub columns: Option<(u32, f32)>,
}

impl TextLayoutSpec {
    fn is_empty(&self) -> bool {
        self == &TextLayoutSpec::default()
    }
}

/// Settings every new object has
#[derive(Debug, Clone, PartialEq)]
pub struct CommonSpec {
    pub z: ZOrder,
    pub locked: bool,
    /// accessibility description
    pub description: Option<String>,
    /// URL opened when the object is clicked while presenting
    pub link: Option<String>,
}

impl Default for CommonSpec {
    fn default() -> Self {
        Self {
            z: ZOrder::Front,
            locked: false,
            description: None,
            link: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapeSpec {
    pub outline: Outline,
    pub frame: Frame,
    /// degrees clockwise
    pub rotation: f32,
    /// a text box (no fill or line by default, grows with its text) rather than a shape
    pub text_box: bool,
    /// index of the theme's shape preset (shape-N-shapestyle), defaults to 0
    pub preset: Option<u32>,
    pub style: ShapeStyleSpec,
    pub layout: TextLayoutSpec,
    pub text: Option<Vec<Paragraph>>,
    pub common: CommonSpec,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineSpec {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub stroke: Stroke,
    pub head: LineEnd,
    pub tail: LineEnd,
    pub opacity: Option<f32>,
    pub shadow: Option<Option<Shadow>>,
    pub common: CommonSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// all of the image inside the frame, keeping its proportions
    Contain,
    /// the frame filled, keeping proportions, cropped by a mask
    Cover,
    /// the frame filled, distorting the image
    Stretch,
    /// the image's own size (1 pixel = 1 point) centred in the frame, cropped by a mask
    Original,
}

impl Fit {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_lowercase().as_str() {
            "contain" | "fit" => Ok(Fit::Contain),
            "cover" | "fill" => Ok(Fit::Cover),
            "stretch" => Ok(Fit::Stretch),
            "original" | "none" => Ok(Fit::Original),
            other => bail!("unknown fit {other:?}, use contain, cover, stretch or original"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageSpec {
    pub file: ImageFile,
    /// None fills the slide (or the media placeholder)
    pub frame: Option<Frame>,
    pub fit: Fit,
    pub rotation: f32,
    pub opacity: Option<f32>,
    pub corner_radius: Option<f32>,
    pub stroke: Option<Option<Stroke>>,
    pub shadow: Option<Option<Shadow>>,
    /// take the place (and frame) of the slide's media placeholder
    pub fill_placeholder: bool,
    pub common: CommonSpec,
}

/// Changes to a frame, None keeps that part
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FramePatch {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub w: Option<f32>,
    pub h: Option<f32>,
}

impl FramePatch {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn apply(&self, frame: Frame) -> Frame {
        Frame {
            x: self.x.unwrap_or(frame.x),
            y: self.y.unwrap_or(frame.y),
            w: self.w.unwrap_or(frame.w),
            h: self.h.unwrap_or(frame.h),
        }
    }

    fn is_empty(&self) -> bool {
        self == &FramePatch::default()
    }
}

/// Changes to an existing object, None leaves that part as it is
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectEdit {
    pub frame: FramePatch,
    pub rotation: Option<f32>,
    pub z: Option<ZOrder>,
    pub style: ShapeStyleSpec,
    pub layout: TextLayoutSpec,
    pub text: Option<Vec<Paragraph>>,
    pub locked: Option<bool>,
    /// empty removes it
    pub description: Option<String>,
    /// empty removes it
    pub link: Option<String>,
}

/// Slide background
#[derive(Debug, Clone)]
pub enum BackgroundSpec {
    /// back to the layout's background
    Layout,
    Fill(Fill),
    Image { file: ImageFile, fit: Fit },
}

/// Placeholders a layout can show
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderKind {
    Title,
    Body,
    SlideNumber,
}

impl PlaceholderKind {
    fn field(&self) -> u32 {
        match self {
            PlaceholderKind::Title => slide::TITLE_PLACEHOLDER,
            PlaceholderKind::Body => slide::BODY_PLACEHOLDER,
            PlaceholderKind::SlideNumber => slide::SLIDE_NUMBER_PLACEHOLDER,
        }
    }

    fn visibility_property(&self) -> u32 {
        match self {
            PlaceholderKind::Title => slide_props::TITLE_VISIBLE,
            PlaceholderKind::Body => slide_props::BODY_VISIBLE,
            PlaceholderKind::SlideNumber => slide_props::SLIDE_NUMBER_VISIBLE,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            PlaceholderKind::Title => "title",
            PlaceholderKind::Body => "body",
            PlaceholderKind::SlideNumber => "slide number",
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// Description
////////////////////////////////////////////////////////////////////////////////

fn is_zero(value: &f32) -> bool {
    value.abs() < 1e-4
}

fn is_false(value: &bool) -> bool {
    !value
}

/// An object of a slide as keynote_info lists it
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ObjectInfo {
    /// stable id, use it as "id:N" in builds and object tools
    pub id: u64,
    /// "title", "body", "slide number", "media placeholder", "placeholder", "text box", "shape",
    /// "line", "image", "movie", "group", "table", "chart", "connection line" or "type N"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<Frame>,
    /// degrees clockwise
    #[serde(skip_serializing_if = "is_zero")]
    pub rotation: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// file name of an image
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// "#RRGGBB[AA]", "gradient", "image" or "none"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    /// eg "#FFFFFF 2pt dash"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(skip_serializing_if = "is_false")]
    pub locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

////////////////////////////////////////////////////////////////////////////////
pub fn describe_fill(fill: &Fill) -> String {
    match fill {
        Fill::None => "none".to_string(),
        Fill::Color(color) => color.hex(),
        Fill::Gradient { angle, stops } => {
            let stops: Vec<String> = stops.iter().map(|(color, at)| format!("{} {at}", color.hex())).collect();
            format!("gradient {angle}° {}", stops.join(", "))
        }
        Fill::Image { .. } => "image".to_string(),
    }
}

////////////////////////////////////////////////////////////////////////////////
fn describe_stroke(stroke: &Stroke) -> String {
    let dash = match stroke.dash {
        super::drawing::Dash::Solid => "",
        super::drawing::Dash::Dash => " dash",
        super::drawing::Dash::Dot => " dot",
        super::drawing::Dash::DashDot => " dash_dot",
    };
    format!("{} {}pt{dash}", stroke.color.hex(), stroke.width)
}

////////////////////////////////////////////////////////////////////////////////
/// Keynote's text wrap settings for slide objects
fn text_wrap() -> Message {
    let mut wrap = Message::default();
    wrap.set_varint(1, 4);
    wrap.set_varint(2, 2);
    wrap.set_varint(3, 1);
    wrap.set(4, float(12.0));
    wrap.set(5, float(0.5));
    wrap.set_bool(6, false);
    wrap
}

////////////////////////////////////////////////////////////////////////////////
/// TSD.DrawableArchive of a new object
fn drawable_archive(frame: &Frame, rotation: f32, parent: u64, titles: Option<(u64, u64)>, common: &CommonSpec) -> Message {
    let mut message = Message::default();
    message.set_message(drawable::GEOMETRY, &frame.geometry_message(rotation));
    message.set_message(drawable::PARENT, &make_reference(parent));
    message.set_message(drawable::TEXT_WRAP, &text_wrap());
    if let Some(link) = common.link.as_ref().filter(|e| !e.is_empty()) {
        message.set_bytes(drawable::HYPERLINK, link.clone().into_bytes());
    }
    message.set_bool(drawable::LOCKED, common.locked);
    message.set_bool(drawable::ASPECT_LOCKED, false);
    if let Some(description) = common.description.as_ref().filter(|e| !e.is_empty()) {
        message.set_bytes(drawable::DESCRIPTION, description.clone().into_bytes());
    }
    if let Some((title, caption)) = titles {
        message.set_message(drawable::TITLE, &make_reference(title));
        message.set_message(drawable::CAPTION, &make_reference(caption));
    }
    message.set_bool(drawable::TITLE_HIDDEN, false);
    message.set_bool(drawable::CAPTION_HIDDEN, false);
    message
}

////////////////////////////////////////////////////////////////////////////////
/// Style overrides of a TSWP shape style: [TSD shape properties, text layout properties]
fn shape_style_props(style: &ShapeStyleSpec, layout: &TextLayoutSpec, ends: Option<(LineEnd, LineEnd)>) -> StyleProps {
    let mut props = StyleProps::with_groups(2);
    let tsd = props.group_mut(0);
    if let Some(fill) = &style.fill {
        tsd.set_message(shape_props::FILL, &fill.message());
    }
    if let Some(stroke) = &style.stroke {
        tsd.set_message(shape_props::STROKE, &Stroke::message(stroke.as_ref()));
    }
    if let Some(opacity) = style.opacity {
        tsd.set(shape_props::OPACITY, float(opacity.clamp(0.0, 1.0)));
    }
    if let Some(shadow) = &style.shadow {
        tsd.set_message(shape_props::SHADOW, &Shadow::message(shadow.as_ref()));
    }
    if let Some((head, tail)) = ends {
        tsd.set_message(shape_props::HEAD_LINE_END, &head.message());
        tsd.set_message(shape_props::TAIL_LINE_END, &tail.message());
    }

    let text = props.group_mut(1);
    if let Some(shrink) = layout.shrink_to_fit {
        text.set_bool(text_layout_props::SHRINK_TO_FIT, shrink);
    }
    if let Some(align) = layout.vertical_align {
        let value = match align {
            VerticalAlign::Top => 0,
            VerticalAlign::Middle => 1,
            VerticalAlign::Bottom => 2,
        };
        text.set_varint(text_layout_props::VERTICAL_ALIGNMENT, value);
    }
    if let Some((count, gap)) = layout.columns {
        let mut equal = Message::default();
        equal.set_varint(text_layout_props::COLUMNS_COUNT, u64::from(count.max(1)));
        equal.set(text_layout_props::COLUMNS_GAP, float(gap));
        let mut columns = Message::default();
        columns.set_message(text_layout_props::COLUMNS_EQUAL, &equal);
        text.set_message(text_layout_props::COLUMNS, &columns);
    }
    if let Some(inset) = layout.inset {
        let mut padding = Message::default();
        for field in [
            text_layout_props::PADDING_LEFT,
            text_layout_props::PADDING_TOP,
            text_layout_props::PADDING_RIGHT,
            text_layout_props::PADDING_BOTTOM,
        ] {
            padding.set(field, float(inset));
        }
        text.set_message(text_layout_props::PADDING, &padding);
    }
    props
}

////////////////////////////////////////////////////////////////////////////////
/// Style overrides of an image's media style
fn media_style_props(
    stroke: &Option<Option<Stroke>>,
    shadow: &Option<Option<Shadow>>,
    opacity: Option<f32>,
) -> StyleProps {
    let mut props = StyleProps::with_groups(1);
    let group = props.group_mut(0);
    if let Some(stroke) = stroke {
        group.set_message(media_props::STROKE, &Stroke::message(stroke.as_ref()));
    }
    if let Some(opacity) = opacity {
        group.set(media_props::OPACITY, float(opacity.clamp(0.0, 1.0)));
    }
    if let Some(shadow) = shadow {
        group.set_message(media_props::SHADOW, &Shadow::message(shadow.as_ref()));
    }
    props
}

////////////////////////////////////////////////////////////////////////////////
/// Where an image of `width` × `height` goes to fit `frame`
fn image_rect(frame: &Frame, width: f32, height: f32, fit: Fit) -> Frame {
    let (width, height) = (width.max(1.0), height.max(1.0));
    let centered = |w: f32, h: f32| Frame {
        x: frame.x + (frame.w - w) / 2.0,
        y: frame.y + (frame.h - h) / 2.0,
        w,
        h,
    };
    match fit {
        Fit::Stretch => *frame,
        Fit::Original => centered(width, height),
        Fit::Contain => {
            let scale = (frame.w / width).min(frame.h / height);
            centered(width * scale, height * scale)
        }
        Fit::Cover => {
            let scale = (frame.w / width).max(frame.h / height);
            centered(width * scale, height * scale)
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
fn intersect(a: &Frame, b: &Frame) -> Frame {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    Frame {
        x,
        y,
        w: (a.right().min(b.right()) - x).max(0.0),
        h: (a.bottom().min(b.bottom()) - y).max(0.0),
    }
}

impl KeynoteDocument {
    ////////////////////////////////////////////////////////////////////////////////
    // Helpers
    ////////////////////////////////////////////////////////////////////////////////

    /// Slide size in points
    pub fn slide_size(&self) -> Result<(f32, f32)> {
        let (_, show) = self.show()?;
        let size = show.message(show::SIZE)?.unwrap_or_default();
        Ok((
            size.float(size::WIDTH).unwrap_or(1920.0),
            size.float(size::HEIGHT).unwrap_or(1080.0),
        ))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn slide_ids(&self, number: usize) -> Result<(u64, u64)> {
        let node = self.slide_node(number)?;
        Ok((node, self.slide_of(node)?))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// A fresh object id
    fn allocate(&mut self) -> Result<u64> {
        let identifier = self.package.last_object_identifier()? + 1;
        self.package.set_last_object_identifier(identifier)?;
        Ok(identifier)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn version_for(&self, message_type: u32, fallback: u64) -> Vec<u64> {
        self.object_version(message_type)
            .or_else(|| self.package.object(fallback).ok().map(IwaObject::version))
            .unwrap_or_default()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// The empty title and caption objects Keynote gives every drawable
    fn new_titles(&mut self, component: &str, slide: u64) -> Result<(u64, u64)> {
        let version = self.version_for(TYPE_DRAWABLE_TITLE, slide);
        let title = self.allocate()?;
        let caption = self.allocate()?;
        for identifier in [title, caption] {
            let object = IwaObject::new(identifier, TYPE_DRAWABLE_TITLE, &version, &Message::default(), &[]);
            self.package.insert(component, object)?;
        }
        Ok((title, caption))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Lists a new object on its slide at a stacking position
    fn place_drawable(&mut self, node: u64, slide: u64, identifier: u64, z: &ZOrder) -> Result<()> {
        let slide_message = self.package.message(slide)?;
        let mut order = references(&slide_message, slide::DRAWABLES_Z_ORDER)?;
        order.retain(|e| *e != identifier);
        let index = match z {
            ZOrder::Front => order.len(),
            ZOrder::Back => 0,
            ZOrder::Above(other) | ZOrder::Below(other) => {
                let position = order
                    .iter()
                    .position(|e| e == other)
                    .ok_or_else(|| anyhow!("object id:{other} is not on this slide"))?;
                if matches!(z, ZOrder::Above(_)) { position + 1 } else { position }
            }
        };
        order.insert(index, identifier);

        let mut owned = references(&slide_message, slide::OWNED_DRAWABLES)?;
        if !owned.contains(&identifier) {
            owned.push(identifier);
        }
        self.package.update(slide, &[identifier], |e| {
            let owned: Vec<_> = owned.iter().map(|e| make_reference(*e)).collect();
            let order: Vec<_> = order.iter().map(|e| make_reference(*e)).collect();
            e.set_messages(slide::OWNED_DRAWABLES, &owned);
            e.set_messages(slide::DRAWABLES_Z_ORDER, &order);
            Ok(())
        })?;
        self.mark_dirty(node)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn mark_dirty(&mut self, node: u64) -> Result<()> {
        self.package.update(node, &[], |e| {
            e.set_bool(slide_node::THUMBNAILS_ARE_DIRTY, true);
            Ok(())
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Lists cross-component references of new or changed objects
    fn link_components(&mut self, objects: &[u64]) -> Result<()> {
        let mut infos = self.package.component_infos()?;
        self.add_external_references(&mut infos, objects)?;
        self.package.set_component_infos(&infos)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// A theme preset by identifier, or the first root style of the type
    fn preset_style(&self, identifiers: &[String], message_type: u32) -> Result<u64> {
        for identifier in identifiers {
            if let Some(style) = self.named_style(identifier)? {
                return Ok(style);
            }
        }
        for style in self.package.find_type(message_type) {
            if self.style_parent(style)?.is_none() {
                return Ok(style);
            }
        }
        bail!("the theme has no style for this object ({})", identifiers.join(", "))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// A new empty text storage in the paragraph style a shape style uses
    fn new_storage(&mut self, component: &str, shape_style: u64, slide: u64) -> Result<u64> {
        let props = self.resolved_properties(shape_style)?;
        let paragraph = reference(&props.group(1), text_layout_props::PARAGRAPH_STYLE)?
            .filter(|e| self.package.contains(*e));
        let paragraph = match paragraph {
            Some(e) => e,
            None => self
                .style_named(TYPE_PARAGRAPH_STYLE, "Body")?
                .ok_or_else(|| anyhow!("the theme has no paragraph style for text"))?,
        };
        let list = reference(&self.resolved_properties(paragraph)?.group(1), para_props::LIST_STYLE)?
            .filter(|e| self.package.contains(*e));
        let list = match list {
            Some(e) => Some(e),
            None => self.style_named(TYPE_LIST_STYLE, "None")?,
        };

        let message = new_storage_message(self.stylesheet()?, paragraph, list);
        let version = self.version_for(TYPE_STORAGE, slide);
        let identifier = self.allocate()?;
        let mut header = vec![paragraph];
        header.extend(list);
        self.package
            .insert(component, IwaObject::new(identifier, TYPE_STORAGE, &version, &message, &header))?;
        Ok(identifier)
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Adding
    ////////////////////////////////////////////////////////////////////////////////

    /// Adds a shape or text box, returns its id
    pub fn add_shape(&mut self, number: usize, spec: &ShapeSpec) -> Result<u64> {
        let (node, slide) = self.slide_ids(number)?;
        let component = self.package.component_of(slide)?.to_string();
        let frame = spec.frame;
        if frame.w <= 0.0 || frame.h <= 0.0 {
            bail!("the frame needs a positive width and height");
        }
        let path = outline_path(&spec.outline, f64::from(frame.w), f64::from(frame.h))?;

        let preset = spec.preset.unwrap_or(0);
        let identifiers = match spec.text_box {
            true => vec!["textbox-0-shapestyle".to_string()],
            false => vec![format!("shape-{preset}-shapestyle"), "shape-0-shapestyle".to_string()],
        };
        let base = self.preset_style(&identifiers, TYPE_SHAPE_STYLE)?;
        let style = self.style_variation(base, &shape_style_props(&spec.style, &spec.layout, None))?;

        let storage = self.new_storage(&component, style, slide)?;
        let titles = self.new_titles(&component, slide)?;
        let identifier = self.allocate()?;

        let mut shape_archive = Message::default();
        shape_archive.set_message(shape::DRAWABLE, &drawable_archive(&frame, spec.rotation, slide, Some(titles), &spec.common));
        shape_archive.set_message(shape::STYLE, &make_reference(style));
        shape_archive.set_message(shape::PATH_SOURCE, &bezier_path_source(&path, frame.w, frame.h));
        shape_archive.set(shape::STROKE_OFFSET, float(0.0));

        let mut message = Message::default();
        message.set_message(shape_info::SHAPE, &shape_archive);
        message.set_message(shape_info::DEPRECATED_STORAGE, &make_reference(storage));
        message.set_message(shape_info::OWNED_STORAGE, &make_reference(storage));
        if spec.text_box {
            message.set_bool(shape_info::IS_TEXT_BOX, true);
        }

        let version = self.version_for(TYPE_SHAPE_INFO, slide);
        let header = [titles.1, titles.0, style, storage];
        self.package
            .insert(&component, IwaObject::new(identifier, TYPE_SHAPE_INFO, &version, &message, &header))?;

        self.place_drawable(node, slide, identifier, &spec.common.z)?;
        if let Some(text) = &spec.text {
            self.set_rich_text(storage, text)?;
        }
        self.link_components(&[identifier, storage, slide])?;
        Ok(identifier)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Adds a straight line, returns its id
    pub fn add_line(&mut self, number: usize, spec: &LineSpec) -> Result<u64> {
        let (node, slide) = self.slide_ids(number)?;
        let component = self.package.component_of(slide)?.to_string();
        let (x1, y1) = spec.from;
        let (x2, y2) = spec.to;
        if (x1 - x2).abs() < 1e-3 && (y1 - y2).abs() < 1e-3 {
            bail!("a line needs two different points");
        }
        let frame = Frame {
            x: x1.min(x2),
            y: y1.min(y2),
            w: (x2 - x1).abs(),
            h: (y2 - y1).abs(),
        };
        let mut path = kurbo::BezPath::new();
        path.move_to((f64::from(x1 - frame.x), f64::from(y1 - frame.y)));
        path.line_to((f64::from(x2 - frame.x), f64::from(y2 - frame.y)));

        let identifiers = ["line-0-shapestyle".to_string(), "shape-0-shapestyle".to_string()];
        let base = self.preset_style(&identifiers, TYPE_SHAPE_STYLE)?;
        let style_spec = ShapeStyleSpec {
            fill: None,
            stroke: Some(Some(spec.stroke.clone())),
            shadow: spec.shadow.clone(),
            opacity: spec.opacity,
        };
        let props = shape_style_props(&style_spec, &TextLayoutSpec::default(), Some((spec.head, spec.tail)));
        let style = self.style_variation(base, &props)?;

        let titles = self.new_titles(&component, slide)?;
        let identifier = self.allocate()?;
        let mut shape_archive = Message::default();
        shape_archive.set_message(shape::DRAWABLE, &drawable_archive(&frame, 0.0, slide, Some(titles), &spec.common));
        shape_archive.set_message(shape::STYLE, &make_reference(style));
        shape_archive.set_message(shape::PATH_SOURCE, &bezier_path_source(&path, frame.w, frame.h));
        shape_archive.set(shape::STROKE_OFFSET, float(0.0));
        let mut message = Message::default();
        message.set_message(shape_info::SHAPE, &shape_archive);

        let version = self.version_for(TYPE_SHAPE_INFO, slide);
        let header = [titles.1, titles.0, style];
        self.package
            .insert(&component, IwaObject::new(identifier, TYPE_SHAPE_INFO, &version, &message, &header))?;
        self.place_drawable(node, slide, identifier, &spec.common.z)?;
        self.link_components(&[identifier, slide])?;
        Ok(identifier)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Adds an image, returns its id
    pub fn add_image(&mut self, number: usize, spec: &ImageSpec) -> Result<u64> {
        let (node, slide) = self.slide_ids(number)?;
        let component = self.package.component_of(slide)?.to_string();
        let slide_message = self.package.message(slide)?;

        let (width, height) = self.slide_size()?;
        let mut replaced = None;
        let frame = if spec.fill_placeholder {
            let placeholder = self
                .media_placeholder(&slide_message)?
                .ok_or_else(|| anyhow!("slide {number} shows no media placeholder, pick a layout with a photo"))?;
            replaced = Some(placeholder);
            match spec.frame {
                Some(frame) => frame,
                None => self.visible_frame(placeholder)?,
            }
        } else {
            spec.frame.unwrap_or(Frame {
                x: 0.0,
                y: 0.0,
                w: width,
                h: height,
            })
        };
        if frame.w <= 0.0 || frame.h <= 0.0 {
            bail!("the frame needs a positive width and height");
        }

        let file = &spec.file;
        let rect = image_rect(&frame, file.width, file.height, spec.fit);
        let crop = rect.x < frame.x - 0.01
            || rect.y < frame.y - 0.01
            || rect.right() > frame.right() + 0.01
            || rect.bottom() > frame.bottom() + 0.01;
        let visible = if crop { intersect(&rect, &frame) } else { rect };
        let needs_mask = crop || spec.corner_radius.is_some_and(|e| e > 0.0);

        let data = self.add_data(file.bytes.clone(), &file.file_name)?;
        let svg = match &file.svg {
            Some(svg) => {
                let stem = file.file_name.rsplit_once('.').map(|e| e.0).unwrap_or(&file.file_name);
                Some(self.add_data(svg.clone(), &format!("{stem}.svg"))?)
            }
            None => None,
        };

        let base = self.preset_style(&["image-0-imageStyle".to_string()], TYPE_MEDIA_STYLE)?;
        let props = media_style_props(&spec.stroke, &spec.shadow, spec.opacity);
        let style = self.style_variation(base, &props)?;

        let titles = self.new_titles(&component, slide)?;
        let identifier = self.allocate()?;
        let mask_identifier = if needs_mask { Some(self.allocate()?) } else { None };

        let mut drawable = drawable_archive(&rect, spec.rotation, slide, Some(titles), &spec.common);
        drawable.set_bool(drawable::ASPECT_LOCKED, spec.fit != Fit::Stretch);
        let mut data_reference = Message::default();
        data_reference.set_varint(1, data);

        let mut message = Message::default();
        message.set_message(image::DRAWABLE, &drawable);
        message.set_message(image::STYLE, &make_reference(style));
        message.set_message(image::ORIGINAL_SIZE, &pair_message(rect.w, rect.h));
        if let Some(mask) = mask_identifier {
            message.set_message(image::MASK, &make_reference(mask));
        }
        message.set_varint(image::FLAGS, 0);
        message.set_message(image::NATURAL_SIZE, &pair_message(file.width, file.height));
        message.set_message(image::DATA, &data_reference);
        message.set_bool(image::INTERPRETS_UNTAGGED, false);
        if let Some(svg) = svg {
            let mut svg_reference = Message::default();
            svg_reference.set_varint(1, svg);
            message.set_message(image::ORIGINAL_SVG, &svg_reference);
        }

        let version = self.version_for(TYPE_IMAGE, slide);
        let mut header = vec![titles.1, titles.0, style];
        header.extend(mask_identifier);
        let mut object = IwaObject::new(identifier, TYPE_IMAGE, &version, &message, &header);
        let mut datas = vec![data];
        datas.extend(svg);
        object.add_data_references(&datas)?;
        self.package.insert(&component, object)?;

        if let Some(mask_identifier) = mask_identifier {
            let local = Frame {
                x: visible.x - rect.x,
                y: visible.y - rect.y,
                w: visible.w,
                h: visible.h,
            };
            let outline = match spec.corner_radius {
                Some(radius) if radius > 0.0 => Outline::RoundedRect { radius },
                _ => Outline::Rect,
            };
            let path = outline_path(&outline, f64::from(local.w), f64::from(local.h))?;
            let mut mask_drawable = drawable_archive(&local, 0.0, identifier, None, &CommonSpec::default());
            mask_drawable.remove(drawable::TITLE_HIDDEN);
            mask_drawable.remove(drawable::CAPTION_HIDDEN);
            let mut mask_message = Message::default();
            mask_message.set_message(mask::DRAWABLE, &mask_drawable);
            mask_message.set_message(mask::PATH_SOURCE, &bezier_path_source(&path, local.w, local.h));
            let version = self.version_for(TYPE_MASK, slide);
            self.package
                .insert(&component, IwaObject::new(mask_identifier, TYPE_MASK, &version, &mask_message, &[]))?;
        }

        for e in &datas {
            self.note_data_use(identifier, *e)?;
        }

        match replaced {
            Some(placeholder) => {
                // the new image takes the sample photo's place and its "Media" tag
                self.place_drawable(node, slide, identifier, &ZOrder::Above(placeholder))?;
                self.package.update(slide, &[identifier], |e| {
                    let mut entries = e.messages(slide::SAGE_TAG_MAP)?;
                    for entry in &mut entries {
                        if reference(entry, sage_tag::INFO)? == Some(placeholder) {
                            entry.set_message(sage_tag::INFO, &make_reference(identifier));
                        }
                    }
                    e.set_messages(slide::SAGE_TAG_MAP, &entries);
                    Ok(())
                })?;
                self.delete_objects(&[placeholder])?;
            }
            None => self.place_drawable(node, slide, identifier, &spec.common.z)?,
        }
        self.link_components(&[identifier, slide])?;
        Ok(identifier)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// The sample photo a layout tags "Media", when the slide shows it
    pub(super) fn media_placeholder(&self, slide_message: &Message) -> Result<Option<u64>> {
        let order = references(slide_message, slide::DRAWABLES_Z_ORDER)?;
        for entry in slide_message.messages(slide::SAGE_TAG_MAP)? {
            if entry.string(sage_tag::TAG).as_deref() == Some("Media")
                && let Some(info) = reference(&entry, sage_tag::INFO)?
                && order.contains(&info)
                && self.package.contains(info)
            {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// The part of an object that shows: an image's mask, or its frame
    fn visible_frame(&self, identifier: u64) -> Result<Frame> {
        let (frame, _) = self.object_frame(identifier)?;
        if self.package.object(identifier)?.message_type() == TYPE_IMAGE
            && let Some(mask) = reference(&self.package.message(identifier)?, image::MASK)?
            && self.package.contains(mask)
        {
            let (local, _) = self.object_frame(mask)?;
            return Ok(Frame {
                x: frame.x + local.x,
                y: frame.y + local.y,
                w: local.w,
                h: local.h,
            });
        }
        Ok(frame)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn remove_from_z_order(&mut self, slide: u64, identifier: u64) -> Result<()> {
        let order = references(&self.package.message(slide)?, slide::DRAWABLES_Z_ORDER)?;
        let order: Vec<_> = order.into_iter().filter(|e| *e != identifier).map(make_reference).collect();
        self.package.update(slide, &[], |e| {
            e.set_messages(slide::DRAWABLES_Z_ORDER, &order);
            Ok(())
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Editing
    ////////////////////////////////////////////////////////////////////////////////

    /// 1-based number of the slide showing or owning an object
    pub fn slide_with_object(&self, identifier: u64) -> Result<usize> {
        for (index, node) in self.slide_nodes()?.into_iter().enumerate() {
            let slide_message = self.package.message(self.slide_of(node)?)?;
            let order = references(&slide_message, slide::DRAWABLES_Z_ORDER)?;
            let owned = references(&slide_message, slide::OWNED_DRAWABLES)?;
            if order.contains(&identifier) || owned.contains(&identifier) {
                return Ok(index + 1);
            }
        }
        bail!("no slide has an object id:{identifier}, see keynote_info for the ids")
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Frame and clockwise rotation of an object
    pub(super) fn object_frame(&self, identifier: u64) -> Result<(Frame, f32)> {
        let message_type = self.package.object(identifier)?.message_type();
        let layout = object_layout(message_type).ok_or_else(|| anyhow!("object id:{identifier} has no frame"))?;
        let drawable = message_at(&self.package.message(identifier)?, layout.drawable)?.unwrap_or_default();
        let geometry = drawable.message(drawable::GEOMETRY)?.unwrap_or_default();
        Frame::from_geometry(&geometry)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Changes an object anywhere in the presentation
    pub fn edit_object(&mut self, identifier: u64, edit: &ObjectEdit) -> Result<()> {
        let number = self.slide_with_object(identifier)?;
        let (node, slide) = self.slide_ids(number)?;
        let message_type = self.package.object(identifier)?.message_type();
        let layout = object_layout(message_type)
            .ok_or_else(|| anyhow!("object id:{identifier} (type {message_type}) can't be edited"))?;
        let mut message = self.package.message(identifier)?;

        // geometry and drawable settings
        let mut drawable = message_at(&message, layout.drawable)?.unwrap_or_default();
        let geometry = drawable.message(drawable::GEOMETRY)?.unwrap_or_default();
        let (frame, rotation) = Frame::from_geometry(&geometry)?;
        let new_frame = edit.frame.apply(frame);
        if new_frame.w < 0.0 || new_frame.h < 0.0 {
            bail!("the frame can't have a negative size");
        }
        if !edit.frame.is_empty() || edit.rotation.is_some() {
            let mut new_geometry = new_frame.geometry_message(edit.rotation.unwrap_or(rotation));
            if let Some(flags) = geometry.varint(super::drawing::geometry::FLAGS) {
                new_geometry.set_varint(super::drawing::geometry::FLAGS, flags);
            }
            drawable.set_message(drawable::GEOMETRY, &new_geometry);
        }
        if let Some(locked) = edit.locked {
            drawable.set_bool(drawable::LOCKED, locked);
        }
        if let Some(description) = &edit.description {
            match description.is_empty() {
                true => drawable.remove(drawable::DESCRIPTION),
                false => drawable.set_bytes(drawable::DESCRIPTION, description.clone().into_bytes()),
            }
        }
        if let Some(link) = &edit.link {
            match link.is_empty() {
                true => drawable.remove(drawable::HYPERLINK),
                false => drawable.set_bytes(drawable::HYPERLINK, link.clone().into_bytes()),
            }
        }
        set_message_at(&mut message, layout.drawable, &drawable)?;

        // an image keeps showing all of its new frame
        let mut mask_update = None;
        if message_type == TYPE_IMAGE && (new_frame.w != frame.w || new_frame.h != frame.h) {
            message.set_message(image::ORIGINAL_SIZE, &pair_message(new_frame.w, new_frame.h));
            mask_update = reference(&message, image::MASK)?.filter(|e| self.package.contains(*e));
        }

        // style
        let mut added = Vec::new();
        if !edit.style.is_empty() || !edit.layout.is_empty() {
            let style_path = layout
                .style
                .ok_or_else(|| anyhow!("object id:{identifier} has no style to change"))?;
            let current = message_at(&message, style_path)?
                .and_then(|e| e.varint(1))
                .filter(|e| self.package.contains(*e))
                .ok_or_else(|| anyhow!("object id:{identifier} has no style to change"))?;
            let props = if message_type == TYPE_IMAGE {
                if edit.style.fill.is_some() || !edit.layout.is_empty() {
                    bail!("images have no fill or text layout");
                }
                media_style_props(&edit.style.stroke, &edit.style.shadow, edit.style.opacity)
            } else {
                shape_style_props(&edit.style, &edit.layout, None)
            };
            let style = self.style_variation(current, &props)?;
            set_message_at(&mut message, style_path, &make_reference(style))?;
            added.push(style);
        }

        self.package.update(identifier, &added, |e| {
            *e = message;
            Ok(())
        })?;

        if let Some(mask) = mask_update {
            self.package.update(mask, &[], |e| {
                let mut drawable = e.message(mask::DRAWABLE)?.unwrap_or_default();
                let local = Frame {
                    x: 0.0,
                    y: 0.0,
                    w: new_frame.w,
                    h: new_frame.h,
                };
                drawable.set_message(drawable::GEOMETRY, &local.geometry_message(0.0));
                e.set_message(mask::DRAWABLE, &drawable);
                let path = outline_path(&Outline::Rect, f64::from(local.w), f64::from(local.h))?;
                e.set_message(mask::PATH_SOURCE, &bezier_path_source(&path, local.w, local.h));
                Ok(())
            })?;
        }

        if let Some(text) = &edit.text {
            let storage_path = layout
                .storage
                .ok_or_else(|| anyhow!("object id:{identifier} holds no text"))?;
            let storage = message_at(&self.package.message(identifier)?, storage_path)?
                .and_then(|e| e.varint(1))
                .filter(|e| self.package.contains(*e))
                .ok_or_else(|| anyhow!("object id:{identifier} holds no text"))?;
            self.set_rich_text(storage, text)?;
        }

        if let Some(z) = &edit.z {
            self.place_drawable(node, slide, identifier, z)?;
        }
        self.mark_dirty(node)?;
        self.link_components(&[identifier])
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Deletes objects of a slide with their builds; title and body placeholders can only be
    /// hidden
    pub fn delete_objects(&mut self, identifiers: &[u64]) -> Result<()> {
        let mut by_slide: Vec<(usize, u64)> = Vec::new();
        for identifier in identifiers {
            by_slide.push((self.slide_with_object(*identifier)?, *identifier));
        }

        for (number, identifier) in &by_slide {
            let (_, slide) = self.slide_ids(*number)?;
            let slide_message = self.package.message(slide)?;
            for (field, kind) in [
                (slide::TITLE_PLACEHOLDER, "title"),
                (slide::BODY_PLACEHOLDER, "body"),
                (slide::SLIDE_NUMBER_PLACEHOLDER, "slide number"),
            ] {
                if reference(&slide_message, field)? == Some(*identifier) {
                    bail!("id:{identifier} is the {kind} placeholder, hide it with show_{} instead", kind.replace(' ', "_"));
                }
            }
        }

        for (number, identifier) in by_slide {
            let (node, slide) = self.slide_ids(number)?;
            let slide_message = self.package.message(slide)?;

            // builds of the object and their steps go first
            let builds = references(&slide_message, slide::BUILDS)?;
            let chunks = references(&slide_message, slide::BUILD_CHUNKS)?;
            let mut removed_builds = HashSet::new();
            for build_id in builds.iter().filter(|e| self.package.contains(**e)) {
                if reference(&self.package.message(*build_id)?, build::DRAWABLE)? == Some(identifier) {
                    removed_builds.insert(*build_id);
                }
            }
            let mut removed_chunks = HashSet::new();
            let mut events = 0;
            for chunk in chunks.iter().filter(|e| self.package.contains(**e)) {
                let chunk_message = self.package.message(*chunk)?;
                let build_id = reference(&chunk_message, build_chunk::BUILD)?;
                if build_id.is_some_and(|e| removed_builds.contains(&e)) {
                    removed_chunks.insert(*chunk);
                } else if !chunk_message.bool(build_chunk::AUTOMATIC).unwrap_or(false) {
                    events += 1;
                }
            }

            let keep = |list: &[u64], removed: &HashSet<u64>| -> Vec<Message> {
                list.iter()
                    .filter(|e| !removed.contains(e) && **e != identifier)
                    .map(|e| make_reference(*e))
                    .collect()
            };
            let builds_left = keep(&builds, &removed_builds);
            let chunks_left = keep(&chunks, &removed_chunks);
            let no_removal = HashSet::new();
            let owned = keep(&references(&slide_message, slide::OWNED_DRAWABLES)?, &no_removal);
            let order = keep(&references(&slide_message, slide::DRAWABLES_Z_ORDER)?, &no_removal);
            let using = keep(&references(&slide_message, slide::INFOS_USING_OBJECT_PLACEHOLDER)?, &no_removal);
            let had_builds = !builds_left.is_empty();
            self.package.update(slide, &[], |e| {
                e.set_messages(slide::BUILDS, &builds_left);
                e.set_messages(slide::BUILD_CHUNKS, &chunks_left);
                e.set_messages(slide::OWNED_DRAWABLES, &owned);
                e.set_messages(slide::DRAWABLES_Z_ORDER, &order);
                e.set_messages(slide::INFOS_USING_OBJECT_PLACEHOLDER, &using);
                Ok(())
            })?;
            if !removed_builds.is_empty() {
                self.package.update(node, &[], |e| {
                    e.set_varint(slide_node::BUILD_EVENT_COUNT, events);
                    e.set_bool(slide_node::HAS_EXPLICIT_BUILDS, had_builds);
                    Ok(())
                })?;
            }

            let mut removed: HashSet<u64> = self.owned_graph(identifier)?.into_iter().collect();
            removed.extend(removed_builds);
            removed.extend(removed_chunks);
            self.remove_objects(&removed)?;
            self.mark_dirty(node)?;
        }
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Slide background and placeholders
    ////////////////////////////////////////////////////////////////////////////////

    /// Sets the background of a slide
    pub fn set_background(&mut self, number: usize, background: &BackgroundSpec) -> Result<()> {
        let (node, slide) = self.slide_ids(number)?;
        let slide_message = self.package.message(slide)?;
        let current = reference(&slide_message, slide::STYLE)?
            .filter(|e| self.package.contains(*e))
            .ok_or_else(|| anyhow!("slide {number} has no style"))?;

        let style = match background {
            BackgroundSpec::Layout => {
                let layout = reference(&slide_message, slide::TEMPLATE_SLIDE)?
                    .filter(|e| self.package.contains(*e))
                    .ok_or_else(|| anyhow!("slide {number} has no layout"))?;
                let style = reference(&self.package.message(layout)?, slide::STYLE)?
                    .filter(|e| self.package.contains(*e))
                    .ok_or_else(|| anyhow!("the layout of slide {number} has no style"))?;
                // the layout's own visibility settings come back too
                style
            }
            BackgroundSpec::Fill(fill) => self.slide_style_with(current, |e| {
                e.set_message(slide_props::FILL, &fill.message());
            })?,
            BackgroundSpec::Image { file, fit } => {
                let data = self.add_data(file.bytes.clone(), &file.file_name)?;
                let technique = match fit {
                    Fit::Contain => super::drawing::image_fill::SCALE_TO_FIT,
                    Fit::Cover => super::drawing::image_fill::SCALE_TO_FILL,
                    Fit::Stretch => super::drawing::image_fill::STRETCH,
                    Fit::Original => super::drawing::image_fill::NATURAL_SIZE,
                };
                let fill = Fill::Image {
                    data,
                    technique,
                    size: (file.width, file.height),
                };
                let style = self.slide_style_with(current, |e| {
                    e.set_message(slide_props::FILL, &fill.message());
                })?;
                self.package.object_mut(style)?.add_data_references(&[data])?;
                self.note_data_use(style, data)?;
                style
            }
        };

        self.package.update(slide, &[style], |e| {
            e.set_message(slide::STYLE, &make_reference(style));
            Ok(())
        })?;
        self.mark_dirty(node)?;
        self.link_components(&[slide])
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn slide_style_with(&mut self, current: u64, edit: impl FnOnce(&mut Message)) -> Result<u64> {
        if self.package.object(current)?.message_type() != TYPE_SLIDE_STYLE {
            bail!("object {current} is not a slide style");
        }
        let mut props = StyleProps::with_groups(1);
        edit(props.group_mut(0));
        self.style_variation(current, &props)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Shows or hides the title, body or slide number placeholder of a slide
    pub fn set_placeholder_visible(&mut self, number: usize, kind: PlaceholderKind, visible: bool) -> Result<()> {
        let (node, slide) = self.slide_ids(number)?;
        let slide_message = self.package.message(slide)?;
        let placeholder = reference(&slide_message, kind.field())?
            .filter(|e| self.package.contains(*e))
            .ok_or_else(|| {
                let layout = self.layout_name(&slide_message).ok().flatten().unwrap_or_default();
                anyhow!("slide {number} (layout {layout:?}) has no {} placeholder", kind.name())
            })?;

        let order = references(&slide_message, slide::DRAWABLES_Z_ORDER)?;
        let shown = order.contains(&placeholder);
        if visible && !shown {
            self.place_drawable(node, slide, placeholder, &ZOrder::Back)?;
        } else if !visible && shown {
            self.remove_from_z_order(slide, placeholder)?;
        }

        if let Some(current) = reference(&slide_message, slide::STYLE)?.filter(|e| self.package.contains(*e)) {
            let style = self.slide_style_with(current, |e| e.set_bool(kind.visibility_property(), visible))?;
            self.package.update(slide, &[style], |e| {
                e.set_message(slide::STYLE, &make_reference(style));
                Ok(())
            })?;
        }
        if kind == PlaceholderKind::SlideNumber {
            self.package.update(node, &[], |e| {
                e.set_bool(18, visible);
                Ok(())
            })?;
        }
        self.mark_dirty(node)?;
        self.link_components(&[slide])
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// The slide background as text: "#17161B", "gradient …", "image <file>" or "none"
    pub(super) fn background_info(&self, slide_message: &Message) -> Result<Option<String>> {
        let Some(style) = reference(slide_message, slide::STYLE)?.filter(|e| self.package.contains(*e)) else {
            return Ok(None);
        };
        let props = self.resolved_properties(style)?;
        let Some(fill) = props.group(0).message(slide_props::FILL)? else {
            return Ok(None);
        };
        let fill = Fill::from_message(&fill)?;
        Ok(Some(match &fill {
            Fill::Image { data, .. } => {
                let name = self.data_entry(*data)?.map(|e| e.preferred_name).unwrap_or_default();
                format!("image {name}")
            }
            other => describe_fill(other),
        }))
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Description
    ////////////////////////////////////////////////////////////////////////////////

    /// Every object a slide shows, back to front
    pub(super) fn object_infos(&self, slide_message: &Message) -> Result<Vec<ObjectInfo>> {
        let mut objects = Vec::new();
        for identifier in references(slide_message, slide::DRAWABLES_Z_ORDER)? {
            objects.push(self.object_info(slide_message, identifier)?);
        }
        Ok(objects)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// What kind of object `identifier` is on a slide
    pub(super) fn object_kind(&self, slide_message: &Message, identifier: u64) -> Result<String> {
        if !self.package.contains(identifier) {
            return Ok("missing".to_string());
        }
        let message_type = self.package.object(identifier)?.message_type();
        let message = self.package.message(identifier)?;
        Ok(match message_type {
            TYPE_PLACEHOLDER => {
                if reference(slide_message, slide::TITLE_PLACEHOLDER)? == Some(identifier) {
                    "title"
                } else if reference(slide_message, slide::BODY_PLACEHOLDER)? == Some(identifier) {
                    "body"
                } else if reference(slide_message, slide::SLIDE_NUMBER_PLACEHOLDER)? == Some(identifier) {
                    "slide number"
                } else if reference(slide_message, slide::OBJECT_PLACEHOLDER)? == Some(identifier) {
                    "object placeholder"
                } else {
                    match message.varint(placeholder::KIND) {
                        Some(placeholder::TITLE) => "title",
                        Some(placeholder::BODY) => "body",
                        Some(placeholder::SLIDE_NUMBER) => "slide number",
                        Some(placeholder::OBJECT) => "object placeholder",
                        _ => "placeholder",
                    }
                }
                .to_string()
            }
            TYPE_SHAPE_INFO => {
                if message.bool(shape_info::IS_TEXT_BOX).unwrap_or(false) {
                    "text box".to_string()
                } else if self.is_line(&message, &[1, 3])? {
                    "line".to_string()
                } else {
                    "shape".to_string()
                }
            }
            TYPE_SHAPE => match self.is_line(&message, &[3])? {
                true => "line".to_string(),
                false => "shape".to_string(),
            },
            TYPE_CONNECTION_LINE => "connection line".to_string(),
            TYPE_IMAGE => match self.media_placeholder(slide_message)? == Some(identifier) {
                true => "media placeholder".to_string(),
                false => "image".to_string(),
            },
            TYPE_MOVIE => "movie".to_string(),
            TYPE_GROUP => "group".to_string(),
            TYPE_TABLE => "table".to_string(),
            TYPE_CHART => "chart".to_string(),
            other => format!("type {other}"),
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// An open outline of two points
    fn is_line(&self, message: &Message, path_source: &[u32]) -> Result<bool> {
        let Some(source) = message_at(message, path_source)? else {
            return Ok(false);
        };
        let Some(path) = path_source_outline(&source, 1.0, 1.0)? else {
            return Ok(false);
        };
        let elements = path.elements();
        Ok(elements.len() == 2 && !elements.iter().any(|e| matches!(e, kurbo::PathEl::ClosePath)))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub(super) fn object_info(&self, slide_message: &Message, identifier: u64) -> Result<ObjectInfo> {
        let kind = self.object_kind(slide_message, identifier)?;
        let mut info = ObjectInfo {
            id: identifier,
            kind,
            frame: None,
            rotation: 0.0,
            text: None,
            image: None,
            fill: None,
            stroke: None,
            opacity: None,
            locked: false,
            description: None,
            link: None,
        };
        if !self.package.contains(identifier) {
            return Ok(info);
        }
        let message_type = self.package.object(identifier)?.message_type();
        let Some(layout) = object_layout(message_type) else {
            return Ok(info);
        };
        let message = self.package.message(identifier)?;

        let drawable = message_at(&message, layout.drawable)?.unwrap_or_default();
        if let Some(geometry) = drawable.message(drawable::GEOMETRY)? {
            let (frame, rotation) = Frame::from_geometry(&geometry)?;
            info.frame = Some(frame);
            info.rotation = rotation;
        }
        info.locked = drawable.bool(drawable::LOCKED).unwrap_or(false);
        info.description = drawable.string(drawable::DESCRIPTION).filter(|e| !e.is_empty());
        info.link = drawable.string(drawable::HYPERLINK).filter(|e| !e.is_empty());

        if let Some(storage) = self.drawable_storage(identifier)? {
            info.text = Some(self.text(storage)?).filter(|e| !e.is_empty());
        }

        if message_type == TYPE_IMAGE
            && let Some(data) = message.message(image::DATA)?.and_then(|e| e.varint(1))
        {
            info.image = self.data_entry(data)?.map(|e| e.preferred_name);
        }

        let style = match layout.style {
            Some(path) => message_at(&message, path)?
                .and_then(|e| e.varint(1))
                .filter(|e| self.package.contains(*e)),
            None => None,
        };
        if let Some(style) = style {
            let props = self.resolved_properties(style)?;
            let group = props.group(0);
            let (fill, stroke, opacity) = match self.package.object(style)?.message_type() {
                TYPE_MEDIA_STYLE => (None, media_props::STROKE, media_props::OPACITY),
                _ => (Some(shape_props::FILL), shape_props::STROKE, shape_props::OPACITY),
            };
            if let Some(fill) = fill.map(|e| group.message(e)).transpose()?.flatten() {
                let fill = Fill::from_message(&fill)?;
                if fill != Fill::None {
                    info.fill = Some(describe_fill(&fill));
                }
            }
            if let Some(stroke) = group.message(stroke)?
                && let Some(stroke) = Stroke::from_message(&stroke)?
            {
                info.stroke = Some(describe_stroke(&stroke));
            }
            info.opacity = group.float(opacity).filter(|e| *e < 0.999);
        }
        Ok(info)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Placeholders a layout shows: "title", "body", "slide number", "media"
    pub(super) fn layout_placeholders(&self, layout: u64) -> Result<Vec<String>> {
        let message = self.package.message(layout)?;
        let order = references(&message, slide::DRAWABLES_Z_ORDER)?;
        let mut shown = Vec::new();
        for (field, name) in [
            (slide::TITLE_PLACEHOLDER, "title"),
            (slide::BODY_PLACEHOLDER, "body"),
            (slide::OBJECT_PLACEHOLDER, "object"),
            (slide::SLIDE_NUMBER_PLACEHOLDER, "slide number"),
        ] {
            if reference(&message, field)?.is_some_and(|e| order.contains(&e)) {
                shown.push(name.to_string());
            }
        }
        if self.media_placeholder(&message)?.is_some() {
            shown.push("media".to_string());
        }
        Ok(shown)
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_images() {
        let frame = Frame {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let contain = image_rect(&frame, 100.0, 100.0, Fit::Contain);
        assert_eq!((contain.x, contain.w, contain.h), (50.0, 100.0, 100.0));
        let cover = image_rect(&frame, 100.0, 100.0, Fit::Cover);
        assert_eq!((cover.y, cover.w, cover.h), (-50.0, 200.0, 200.0));
        assert_eq!(intersect(&cover, &frame), frame);
        assert_eq!(image_rect(&frame, 10.0, 10.0, Fit::Stretch), frame);
    }

    #[test]
    fn parses_ids_and_z_orders() {
        assert_eq!(parse_object_id("id:42").unwrap(), 42);
        assert_eq!(parse_object_id(" 7 ").unwrap(), 7);
        assert!(parse_object_id("title").is_err());
        assert_eq!(ZOrder::parse("above:id:3").unwrap(), ZOrder::Above(3));
        assert_eq!(ZOrder::parse("back").unwrap(), ZOrder::Back);
    }
}
