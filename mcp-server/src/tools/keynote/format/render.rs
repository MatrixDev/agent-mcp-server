//! Previews of slides drawn without Keynote: the object model turned into SVG, rasterised to
//! PNG with resvg. Close to Keynote, not identical: backgrounds, shapes, lines, images (PNG,
//! JPEG, GIF, WebP), masks, fills, strokes, shadows, opacity and styled text with wrapping,
//! alignment, spacing and bullets are drawn; tables, charts, movies and effects show as
//! labelled boxes. Text is measured with the system's fonts (the closest match of the font
//! Keynote names), which also tells whether text overflows its frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::{Arc, OnceLock};

use anyhow::{Result, anyhow};
use base64::Engine;
use resvg::usvg::fontdb;

use super::document::{KeynoteDocument, slide};
use super::drawing::{Color, Fill, Frame, LineEnd, Shadow, Stroke, path_source_outline};
use super::objects::{
    TYPE_CHART, TYPE_CONNECTION_LINE, TYPE_GROUP, TYPE_IMAGE, TYPE_MOVIE, TYPE_PLACEHOLDER, TYPE_SHAPE,
    TYPE_SHAPE_INFO, TYPE_TABLE, group, object_layout,
};
use super::proto::{Message, reference, references};
use super::rich_text::{StoredParagraph, stored_paragraphs};
use super::style::{
    TYPE_MEDIA_STYLE, char_props, media_props, message_at, para_props, shape_props, slide_props, text_layout_props,
};

////////////////////////////////////////////////////////////////////////////////
/// A slide drawn as SVG, with what looked wrong while drawing it
#[derive(Debug, Clone)]
pub struct RenderedSlide {
    pub svg: String,
    pub width: f32,
    pub warnings: Vec<String>,
}

////////////////////////////////////////////////////////////////////////////////
/// The system fonts, loaded once
pub fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut database = fontdb::Database::new();
            database.load_system_fonts();
            database.set_sans_serif_family("Helvetica Neue");
            Arc::new(database)
        })
        .clone()
}

////////////////////////////////////////////////////////////////////////////////
/// Rasterises an SVG to PNG, `width` pixels wide
pub fn svg_to_png(svg: &str, width: u32) -> Result<Vec<u8>> {
    let options = resvg::usvg::Options {
        fontdb: fonts(),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(svg, &options).map_err(|e| anyhow!("invalid slide SVG: {e}"))?;
    let size = tree.size();
    let scale = width as f32 / size.width();
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(width.max(1), height).ok_or_else(|| anyhow!("the image is too large"))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| anyhow!("failed to encode the PNG: {e}"))
}

////////////////////////////////////////////////////////////////////////////////
// Fonts and measuring
////////////////////////////////////////////////////////////////////////////////

/// A face picked for a font name, with cached advances
struct FaceMetrics {
    family: String,
    units_per_em: f32,
    ascender: f32,
    advances: HashMap<char, f32>,
    face: Option<fontdb::ID>,
}

#[derive(Default)]
struct Measurer {
    faces: RefCell<HashMap<(String, bool, bool), Arc<RefCell<FaceMetrics>>>>,
}

impl Measurer {
    ////////////////////////////////////////////////////////////////////////////////
    fn face(&self, name: &str, bold: bool, italic: bool) -> Arc<RefCell<FaceMetrics>> {
        let key = (name.to_string(), bold, italic);
        if let Some(face) = self.faces.borrow().get(&key) {
            return face.clone();
        }
        let face = Arc::new(RefCell::new(load_face(name, bold, italic)));
        self.faces.borrow_mut().insert(key, face.clone());
        face
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Width of `text` in points
    fn width(&self, text: &str, style: &TextStyle) -> f32 {
        let face = self.face(&style.font, style.bold, style.italic);
        let mut face = face.borrow_mut();
        let size = style.size * if style.baseline != 0 { 0.65 } else { 1.0 };
        let mut width = 0.0;
        for character in text.chars() {
            width += advance(&mut face, character) * size;
        }
        width
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn family(&self, style: &TextStyle) -> String {
        self.face(&style.font, style.bold, style.italic).borrow().family.clone()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Ascender as a fraction of the font size
    fn ascender(&self, style: &TextStyle) -> f32 {
        self.face(&style.font, style.bold, style.italic).borrow().ascender
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Advance of a character as a fraction of the font size
fn advance(face: &mut FaceMetrics, character: char) -> f32 {
    if let Some(advance) = face.advances.get(&character) {
        return *advance;
    }
    let fallback = if character.is_whitespace() { 0.28 } else { 0.55 };
    let advance = face
        .face
        .and_then(|id| {
            fonts().with_face_data(id, |data, index| {
                let parsed = ttf_parser::Face::parse(data, index).ok()?;
                let glyph = parsed.glyph_index(character)?;
                Some(f32::from(parsed.glyph_hor_advance(glyph)?) / face.units_per_em)
            })?
        })
        .unwrap_or(fallback);
    face.advances.insert(character, advance);
    advance
}

////////////////////////////////////////////////////////////////////////////////
/// The installed face closest to a Keynote font name ("HelveticaNeue-Bold", "Menlo", "Inter")
fn load_face(name: &str, bold: bool, italic: bool) -> FaceMetrics {
    let database = fonts();
    let normalized = |e: &str| e.to_lowercase().replace([' ', '-', '_'], "");
    let wanted = normalized(name);
    let family_part = normalized(name.split('-').next().unwrap_or(name));
    let name_bold = wanted.contains("bold") || wanted.contains("black") || wanted.contains("heavy");
    let name_italic = wanted.contains("italic") || wanted.contains("oblique");
    let bold = bold || name_bold;
    let italic = italic || name_italic;

    // the family the name means: a PostScript name's family, or the family itself
    let mut family = None;
    for face in database.faces() {
        let postscript = normalized(&face.post_script_name);
        let family_name = face.families.first().map(|e| e.0.clone()).unwrap_or_default();
        if postscript == wanted {
            family = Some(family_name);
            break;
        }
        let family_match = face.families.iter().any(|(e, _)| {
            let e = normalized(e);
            e == wanted || e == family_part
        });
        if family_match && family.is_none() {
            family = Some(family_name);
        }
    }

    // then the face SVG text in that family gets, so measuring matches drawing
    let monospace_name = ["mono", "menlo", "courier", "consolas", "code"].iter().any(|e| wanted.contains(e));
    let query = |families: &[fontdb::Family]| {
        database.query(&fontdb::Query {
            families,
            weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
            stretch: fontdb::Stretch::Normal,
            style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
        })
    };
    let best = family.as_deref().and_then(|e| query(&[fontdb::Family::Name(e)]));

    let monospace = monospace_name;
    let face = best.or_else(|| {
        let family = if monospace { fontdb::Family::Monospace } else { fontdb::Family::SansSerif };
        query(&[family])
    });

    let (family, units_per_em, ascender) = face
        .and_then(|id| {
            let info = database.face(id)?;
            let family = info.families.first().map(|e| e.0.clone()).unwrap_or_default();
            database
                .with_face_data(id, |data, index| {
                    let parsed = ttf_parser::Face::parse(data, index).ok()?;
                    let units = f32::from(parsed.units_per_em());
                    Some((family.clone(), units, f32::from(parsed.ascender()) / units))
                })
                .flatten()
        })
        .unwrap_or_else(|| {
            let family = if monospace { "monospace" } else { "sans-serif" };
            (family.to_string(), 1000.0, 0.8)
        });
    FaceMetrics {
        family,
        units_per_em,
        ascender,
        advances: HashMap::new(),
        face,
    }
}

////////////////////////////////////////////////////////////////////////////////
// Text layout
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, PartialEq)]
struct TextStyle {
    font: String,
    size: f32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    color: Color,
    highlight: Option<Color>,
    /// 0 normal, 1 superscript, 2 subscript
    baseline: u64,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font: "Helvetica Neue".to_string(),
            size: 24.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            color: Color::BLACK,
            highlight: None,
            baseline: 0,
        }
    }
}

impl TextStyle {
    ////////////////////////////////////////////////////////////////////////////////
    /// `self` with the character properties a message sets
    fn apply(&mut self, properties: &Message) -> Result<()> {
        if let Some(font) = properties.string(char_props::FONT).filter(|e| !e.is_empty()) {
            self.font = font;
        }
        if let Some(size) = properties.float(char_props::SIZE).filter(|e| *e > 0.0) {
            self.size = size;
        }
        if let Some(bold) = properties.bool(char_props::BOLD) {
            self.bold = bold;
        }
        if let Some(italic) = properties.bool(char_props::ITALIC) {
            self.italic = italic;
        }
        if let Some(underline) = properties.varint(char_props::UNDERLINE) {
            self.underline = underline != 0;
        }
        if let Some(strike) = properties.varint(char_props::STRIKE) {
            self.strike = strike != 0;
        }
        if let Some(baseline) = properties.varint(char_props::SUPERSCRIPT) {
            self.baseline = baseline;
        }
        let fill_color = properties
            .message(char_props::FILL)?
            .and_then(|e| e.message(1).ok().flatten())
            .and_then(|e| Color::from_message(&e));
        let color = properties.message(char_props::COLOR)?.and_then(|e| Color::from_message(&e));
        if let Some(color) = fill_color.or(color) {
            self.color = color;
        }
        if let Some(highlight) = properties.message(char_props::BACKGROUND_COLOR)? {
            self.highlight = Color::from_message(&highlight).filter(|e| e.a > 0.0);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Spacing {
    Relative(f32),
    Exact(f32),
}

#[derive(Debug, Clone)]
struct ParagraphLayout {
    align: u64,
    spacing: Spacing,
    space_before: f32,
    space_after: f32,
    left_indent: f32,
    first_indent: f32,
    label: Option<String>,
    level: u32,
    runs: Vec<(String, TextStyle)>,
}

#[derive(Debug, Clone)]
struct Span {
    x: f32,
    text: String,
    style: TextStyle,
    width: f32,
}

#[derive(Debug, Clone)]
struct Line {
    /// baseline offset from the top of the text
    baseline: f32,
    spans: Vec<Span>,
}

/// Lays paragraphs out in `width`, returns the lines and the total height
fn layout_text(measurer: &Measurer, paragraphs: &[ParagraphLayout], width: f32, scale: f32) -> (Vec<Line>, f32) {
    let mut lines = Vec::new();
    let mut y = 0.0f32;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if index > 0 {
            y += paragraph.space_before * scale;
        }
        let max_size = paragraph
            .runs
            .iter()
            .map(|e| e.1.size)
            .fold(0.0f32, f32::max)
            .max(1.0)
            * scale;
        let line_height = match paragraph.spacing {
            Spacing::Relative(e) => max_size * 1.2 * e.max(0.1),
            Spacing::Exact(e) => e * scale,
        };
        let indent = paragraph.left_indent * scale + paragraph.level as f32 * 36.0 * scale;
        let label_width = paragraph.label.as_ref().map(|_| max_size * 1.1).unwrap_or(0.0);
        let start = indent + label_width;
        let available = (width - start).max(max_size);

        // words with their styles; a run may break inside, a word may span runs
        let mut words: Vec<Vec<(String, TextStyle)>> = vec![Vec::new()];
        for (text, style) in &paragraph.runs {
            let mut style = style.clone();
            style.size *= scale;
            for (index, piece) in text.split('\u{2028}').enumerate() {
                if index > 0 {
                    words.push(vec![("\n".to_string(), style.clone())]);
                    words.push(Vec::new());
                }
                let mut current = String::new();
                for character in piece.chars() {
                    current.push(character);
                    if character == ' ' {
                        words.last_mut().unwrap().push((std::mem::take(&mut current), style.clone()));
                        words.push(Vec::new());
                    }
                }
                if !current.is_empty() {
                    words.last_mut().unwrap().push((current, style.clone()));
                }
            }
        }

        let mut current: Vec<Span> = Vec::new();
        let mut x = paragraph.first_indent * scale;
        let mut first_line = true;
        let flush = |lines: &mut Vec<Line>, spans: &mut Vec<Span>, y: &mut f32, first: &mut bool| {
            let ascent = spans
                .iter()
                .map(|e| e.style.size * measurer.ascender(&e.style))
                .fold(max_size * 0.8, f32::max);
            let mut spans = std::mem::take(spans);
            // trailing spaces don't count for alignment
            if let Some(last) = spans.last_mut() {
                let trimmed = last.text.trim_end().to_string();
                if trimmed.len() != last.text.len() {
                    last.width = measurer.width(&trimmed, &last.style);
                    last.text = trimmed;
                }
            }
            let used: f32 = spans.last().map(|e| e.x + e.width).unwrap_or(0.0);
            let shift = match paragraph.align {
                1 => available - used,
                2 => (available - used) / 2.0,
                _ => 0.0,
            };
            for span in &mut spans {
                span.x += start + shift.max(0.0);
            }
            if *first && let Some(label) = &paragraph.label {
                let style = spans.first().map(|e| e.style.clone()).unwrap_or_default();
                spans.insert(
                    0,
                    Span {
                        x: indent,
                        text: label.clone(),
                        width: 0.0,
                        style,
                    },
                );
            }
            *first = false;
            lines.push(Line {
                baseline: *y + ascent.min(line_height),
                spans,
            });
            *y += line_height;
        };

        for word in words {
            if word.len() == 1 && word[0].0 == "\n" {
                flush(&mut lines, &mut current, &mut y, &mut first_line);
                x = 0.0;
                continue;
            }
            let word_width: f32 = word.iter().map(|(text, style)| measurer.width(text.trim_end(), style)).sum();
            if x + word_width > available && !current.is_empty() {
                flush(&mut lines, &mut current, &mut y, &mut first_line);
                x = 0.0;
            }
            for (text, style) in word {
                let width = measurer.width(&text, &style);
                match current.last_mut() {
                    Some(last) if last.style == style && (last.x + last.width - x).abs() < 0.01 => {
                        last.text.push_str(&text);
                        last.width += width;
                    }
                    _ => current.push(Span {
                        x,
                        text,
                        style,
                        width,
                    }),
                }
                x += width;
            }
        }
        flush(&mut lines, &mut current, &mut y, &mut first_line);
        y += paragraph.space_after * scale;
    }
    (lines, y)
}

////////////////////////////////////////////////////////////////////////////////
// SVG helpers
////////////////////////////////////////////////////////////////////////////////

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// ` fill="rgb(…)" fill-opacity="…"`, likewise for stroke and stop-color
fn color_attribute(kind: &str, color: &Color) -> String {
    let opacity = match kind {
        "stop-color" => "stop-opacity".to_string(),
        other => format!("{other}-opacity"),
    };
    format!(
        " {kind}=\"rgb({},{},{})\" {opacity}=\"{:.3}\"",
        (color.r.clamp(0.0, 1.0) * 255.0).round(),
        (color.g.clamp(0.0, 1.0) * 255.0).round(),
        (color.b.clamp(0.0, 1.0) * 255.0).round(),
        color.a.clamp(0.0, 1.0)
    )
}

fn svg_path(path: &kurbo::BezPath) -> String {
    path.to_svg()
}

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

////////////////////////////////////////////////////////////////////////////////
struct Canvas {
    body: String,
    defs: String,
    next_id: usize,
    warnings: Vec<String>,
    measurer: Measurer,
    number: usize,
}

impl Canvas {
    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// A fill as an SVG paint attribute, gradients go to the defs
    fn fill_attribute(&mut self, fill: &Fill, w: f32, h: f32, document: &KeynoteDocument) -> Result<Option<String>> {
        match fill {
            Fill::None => Ok(None),
            Fill::Color(color) => Ok(Some(color_attribute("fill", color))),
            Fill::Gradient { angle, stops } => {
                let id = self.id("g");
                let radians = angle.to_radians();
                let (dx, dy) = (radians.cos() / 2.0, radians.sin() / 2.0);
                let _ = write!(
                    self.defs,
                    "<linearGradient id=\"{id}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">",
                    0.5 - dx,
                    0.5 - dy,
                    0.5 + dx,
                    0.5 + dy
                );
                for (color, offset) in stops {
                    let _ = write!(
                        self.defs,
                        "<stop offset=\"{offset}\"{}/>",
                        color_attribute("stop-color", color)
                    );
                }
                self.defs.push_str("</linearGradient>");
                Ok(Some(format!(" fill=\"url(#{id})\"")))
            }
            Fill::Image { data, technique, .. } => {
                let Some((_, bytes)) = document.data_bytes(*data)? else {
                    return Ok(None);
                };
                let Some(mime) = image_mime(&bytes) else {
                    return Ok(Some(" fill=\"#808080\"".to_string()));
                };
                let id = self.id("p");
                let aspect = match *technique {
                    super::drawing::image_fill::STRETCH => "none",
                    super::drawing::image_fill::SCALE_TO_FIT => "xMidYMid meet",
                    _ => "xMidYMid slice",
                };
                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let _ = write!(
                    self.defs,
                    "<pattern id=\"{id}\" patternUnits=\"userSpaceOnUse\" width=\"{w}\" height=\"{h}\"><image width=\"{w}\" height=\"{h}\" preserveAspectRatio=\"{aspect}\" href=\"data:{mime};base64,{encoded}\"/></pattern>"
                );
                Ok(Some(format!(" fill=\"url(#{id})\"")))
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn shadow_filter(&mut self, shadow: &Shadow) -> String {
        let id = self.id("s");
        let _ = write!(
            self.defs,
            "<filter id=\"{id}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\"><feDropShadow dx=\"{}\" dy=\"{}\" stdDeviation=\"{}\" flood-color=\"rgb({},{},{})\" flood-opacity=\"{}\"/></filter>",
            shadow.dx,
            shadow.dy,
            shadow.blur / 2.0,
            (shadow.color.r * 255.0).round(),
            (shadow.color.g * 255.0).round(),
            (shadow.color.b * 255.0).round(),
            shadow.color.a
        );
        format!(" filter=\"url(#{id})\"")
    }
}

////////////////////////////////////////////////////////////////////////////////
// Slides
////////////////////////////////////////////////////////////////////////////////

impl KeynoteDocument {
    ////////////////////////////////////////////////////////////////////////////////
    /// Draws slide `number` (1-based) as SVG
    pub fn render_slide(&self, number: usize) -> Result<RenderedSlide> {
        let (width, height) = self.slide_size()?;
        let slide_id = self.slide_of(self.slide_node(number)?)?;
        let slide_message = self.package.message(slide_id)?;
        let mut canvas = Canvas {
            body: String::new(),
            defs: String::new(),
            next_id: 0,
            warnings: Vec::new(),
            measurer: Measurer::default(),
            number,
        };

        // background: the slide style's fill, white without one
        let mut background = Fill::Color(Color::WHITE);
        if let Some(style) = reference(&slide_message, slide::STYLE)?.filter(|e| self.package.contains(*e)) {
            let props = self.resolved_properties(style)?;
            if let Some(fill) = props.group(0).message(slide_props::FILL)? {
                background = Fill::from_message(&fill)?;
            }
        }
        if let Some(fill) = canvas.fill_attribute(&background, width, height, self)? {
            let _ = write!(canvas.body, "<rect width=\"{width}\" height=\"{height}\"{fill}/>");
        }

        // the layout's own decorations, then the slide's objects
        if let Some(layout) = reference(&slide_message, slide::TEMPLATE_SLIDE)?.filter(|e| self.package.contains(*e)) {
            let layout_message = self.package.message(layout)?;
            for drawable in references(&layout_message, slide::DRAWABLES_Z_ORDER)? {
                let placeholder = self.package.object(drawable).is_ok_and(|e| e.message_type() == TYPE_PLACEHOLDER);
                if self.package.contains(drawable) && !placeholder {
                    self.render_object(&mut canvas, &layout_message, drawable, false)?;
                }
            }
        }
        for drawable in references(&slide_message, slide::DRAWABLES_Z_ORDER)? {
            if self.package.contains(drawable) {
                self.render_object(&mut canvas, &slide_message, drawable, true)?;
            }
        }

        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\"><defs>{}</defs>{}</svg>",
            canvas.defs, canvas.body
        );
        Ok(RenderedSlide {
            svg,
            width,
            warnings: canvas.warnings,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Text that doesn't fit its frame on a slide
    pub fn text_warnings(&self, number: usize) -> Result<Vec<String>> {
        Ok(self.render_slide(number)?.warnings)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn render_object(&self, canvas: &mut Canvas, slide_message: &Message, identifier: u64, warn: bool) -> Result<()> {
        let message_type = self.package.object(identifier)?.message_type();
        let Some(layout) = object_layout(message_type) else {
            return Ok(());
        };
        let message = self.package.message(identifier)?;
        let (frame, rotation) = self.object_frame(identifier)?;
        let transform = format!(
            " transform=\"translate({} {}) rotate({} {} {})\"",
            frame.x,
            frame.y,
            rotation,
            frame.w / 2.0,
            frame.h / 2.0
        );

        match message_type {
            TYPE_PLACEHOLDER | TYPE_SHAPE_INFO | TYPE_SHAPE | TYPE_CONNECTION_LINE => {
                let _ = write!(canvas.body, "<g{transform}>");
                self.render_shape(canvas, slide_message, identifier, &message, &layout, &frame, warn)?;
                canvas.body.push_str("</g>");
            }
            TYPE_IMAGE => {
                let _ = write!(canvas.body, "<g{transform}>");
                self.render_image(canvas, &message, &frame)?;
                canvas.body.push_str("</g>");
            }
            TYPE_GROUP => {
                // children are placed in the group's coordinates
                let _ = write!(canvas.body, "<g{transform}>");
                for child in references(&message, group::CHILDREN)? {
                    if self.package.contains(child) {
                        self.render_object(canvas, slide_message, child, warn)?;
                    }
                }
                canvas.body.push_str("</g>");
            }
            TYPE_TABLE | TYPE_CHART | TYPE_MOVIE | _ => {
                let label = match message_type {
                    TYPE_TABLE => "table",
                    TYPE_CHART => "chart",
                    TYPE_MOVIE => "movie",
                    _ => "object",
                };
                let size = (frame.h / 6.0).clamp(10.0, 28.0);
                let _ = write!(
                    canvas.body,
                    "<g{transform}><rect width=\"{}\" height=\"{}\" fill=\"#888888\" fill-opacity=\"0.15\" stroke=\"#888888\" stroke-dasharray=\"8 6\"/><text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"{size}\" fill=\"#888888\" text-anchor=\"middle\">{label} id:{identifier}</text></g>",
                    frame.w,
                    frame.h,
                    frame.w / 2.0,
                    frame.h / 2.0 + size / 3.0
                );
            }
        }
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[allow(clippy::too_many_arguments)]
    fn render_shape(
        &self,
        canvas: &mut Canvas,
        slide_message: &Message,
        identifier: u64,
        message: &Message,
        layout: &super::objects::ObjectLayout,
        frame: &Frame,
        warn: bool,
    ) -> Result<()> {
        let style = layout
            .style
            .map(|path| message_at(message, path))
            .transpose()?
            .flatten()
            .and_then(|e| e.varint(1))
            .filter(|e| self.package.contains(*e));
        let props = match style {
            Some(style) => self.resolved_properties(style)?,
            None => Default::default(),
        };
        let shape = props.group(0);
        let text_layout = props.group(1);

        let opacity = shape.float(shape_props::OPACITY).unwrap_or(1.0);
        let shadow = match shape.message(shape_props::SHADOW)? {
            Some(e) => Shadow::from_message(&e)?,
            None => None,
        };
        let mut group = format!("<g opacity=\"{opacity}\"");
        if let Some(shadow) = &shadow {
            group.push_str(&canvas.shadow_filter(shadow));
        }
        group.push('>');
        canvas.body.push_str(&group);

        // outline
        let source = layout.path_source.map(|e| message_at(message, e)).transpose()?.flatten();
        let outline = match source {
            Some(source) => path_source_outline(&source, f64::from(frame.w), f64::from(frame.h))?,
            None => None,
        };
        if let Some(outline) = &outline {
            let fill = match shape.message(shape_props::FILL)? {
                Some(e) => Fill::from_message(&e)?,
                None => Fill::None,
            };
            let fill = canvas
                .fill_attribute(&fill, frame.w, frame.h, self)?
                .unwrap_or_else(|| " fill=\"none\"".to_string());
            let stroke = match shape.message(shape_props::STROKE)? {
                Some(e) => Stroke::from_message(&e)?,
                None => None,
            };
            let mut stroke_attributes = String::new();
            if let Some(stroke) = &stroke {
                stroke_attributes.push_str(&color_attribute("stroke", &stroke.color));
                let _ = write!(stroke_attributes, " stroke-width=\"{}\"", stroke.width);
                let dash = match stroke.dash {
                    super::drawing::Dash::Solid => None,
                    super::drawing::Dash::Dash => Some(format!("{} {}", stroke.width * 3.0, stroke.width * 2.0)),
                    super::drawing::Dash::Dot => Some(format!("0.01 {}", stroke.width * 2.0)),
                    super::drawing::Dash::DashDot => Some(format!(
                        "{} {} 0.01 {}",
                        stroke.width * 3.0,
                        stroke.width * 2.0,
                        stroke.width * 2.0
                    )),
                };
                if let Some(dash) = dash {
                    let _ = write!(stroke_attributes, " stroke-dasharray=\"{dash}\" stroke-linecap=\"round\"");
                }
            }
            let _ = write!(canvas.body, "<path d=\"{}\"{fill}{stroke_attributes}/>", svg_path(outline));

            // arrow heads of open lines
            if let Some(stroke) = &stroke {
                let elements = outline.elements();
                if let (Some(kurbo::PathEl::MoveTo(start)), Some(end)) = (elements.first(), elements.last().and_then(|e| e.end_point())) {
                    for (field, tip, from) in [(shape_props::HEAD_LINE_END, *start, end), (shape_props::TAIL_LINE_END, end, *start)] {
                        let kind = shape.message(field)?.map(|e| LineEnd::from_message(&e)).unwrap_or(LineEnd::None);
                        if kind != LineEnd::None {
                            canvas.body.push_str(&line_end_svg(kind, tip, from, stroke));
                        }
                    }
                }
            }
        }

        // text
        if let Some(storage) = self.drawable_storage(identifier)? {
            let storage_message = self.package.message(storage)?;
            let paragraphs = stored_paragraphs(&storage_message)?;
            if paragraphs.iter().any(|e| !e.text.trim().is_empty()) {
                let padding = text_layout.message(text_layout_props::PADDING)?.unwrap_or_default();
                let pad = |field| padding.float(field).unwrap_or(4.0);
                let (left, top, right, bottom) = (
                    pad(text_layout_props::PADDING_LEFT),
                    pad(text_layout_props::PADDING_TOP),
                    pad(text_layout_props::PADDING_RIGHT),
                    pad(text_layout_props::PADDING_BOTTOM),
                );
                let width = (frame.w - left - right).max(1.0);
                let height = (frame.h - top - bottom).max(1.0);
                let layouts = self.paragraph_layouts(&paragraphs, &storage_message)?;
                let shrink = text_layout.bool(text_layout_props::SHRINK_TO_FIT).unwrap_or(false);

                let (mut lines, mut used) = layout_text(&canvas.measurer, &layouts, width, 1.0);
                if shrink && used > height {
                    let mut scale = 1.0;
                    while used > height && scale > 0.3 {
                        scale *= 0.92;
                        (lines, used) = layout_text(&canvas.measurer, &layouts, width, scale);
                    }
                } else if warn && used > height + 1.0 {
                    let kind = self.object_kind(slide_message, identifier)?;
                    let grows = kind == "text box";
                    canvas.warnings.push(format!(
                        "slide {}: {kind} id:{identifier} text is {:.0}pt taller than its frame ({:.0}pt){}",
                        canvas.number,
                        used - height,
                        frame.h,
                        if grows { ", Keynote grows the box" } else { "" }
                    ));
                }

                let valign = text_layout.varint(text_layout_props::VERTICAL_ALIGNMENT).unwrap_or(0);
                let offset = match valign {
                    1 => (height - used) / 2.0,
                    2 => height - used,
                    _ => 0.0,
                };
                for line in &lines {
                    for span in &line.spans {
                        // SVG renderers drop leading spaces, move the text past them instead
                        let text = span.text.trim_start();
                        if text.trim().is_empty() {
                            continue;
                        }
                        let style = &span.style;
                        let leading = &span.text[..span.text.len() - text.len()];
                        let x = left + span.x + canvas.measurer.width(leading, style);
                        let shift = match style.baseline {
                            1 => -style.size * 0.35,
                            2 => style.size * 0.15,
                            _ => 0.0,
                        };
                        let y = top + offset + line.baseline + shift;
                        let size = if style.baseline != 0 { style.size * 0.65 } else { style.size };
                        if let Some(highlight) = &style.highlight {
                            let _ = write!(
                                canvas.body,
                                "<rect x=\"{x}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                                y - size * 0.85,
                                span.width,
                                size * 1.1,
                                color_attribute("fill", highlight)
                            );
                        }
                        let mut decoration = Vec::new();
                        if style.underline {
                            decoration.push("underline");
                        }
                        if style.strike {
                            decoration.push("line-through");
                        }
                        let _ = write!(
                            canvas.body,
                            "<text x=\"{x}\" y=\"{y}\" font-family=\"{}\" font-size=\"{size}\" font-weight=\"{}\" font-style=\"{}\"{}{} xml:space=\"preserve\">{}</text>",
                            escape(&canvas.measurer.family(style)),
                            if style.bold { "bold" } else { "normal" },
                            if style.italic { "italic" } else { "normal" },
                            color_attribute("fill", &style.color),
                            if decoration.is_empty() {
                                String::new()
                            } else {
                                format!(" text-decoration=\"{}\"", decoration.join(" "))
                            },
                            escape(text)
                        );
                    }
                }
            }
        }
        canvas.body.push_str("</g>");
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Effective styles of stored paragraphs
    fn paragraph_layouts(&self, paragraphs: &[StoredParagraph], storage_message: &Message) -> Result<Vec<ParagraphLayout>> {
        let text = storage_message.strings(super::rich_text::storage::TEXT).concat();
        let _ = text;
        let mut counters: HashMap<u32, usize> = HashMap::new();
        let mut layouts = Vec::new();
        for paragraph in paragraphs {
            let mut base = TextStyle::default();
            let mut para = Message::default();
            if let Some(style) = paragraph.paragraph_style.filter(|e| self.package.contains(*e)) {
                let props = self.resolved_properties(style)?;
                base.apply(&props.group(0))?;
                para = props.group(1);
            }

            let spacing = para
                .message(para_props::LINE_SPACING)?
                .map(|e| {
                    let amount = e.float(para_props::LINE_SPACING_AMOUNT).unwrap_or(1.0);
                    match e.varint(para_props::LINE_SPACING_MODE).unwrap_or(para_props::MODE_RELATIVE) {
                        para_props::MODE_EXACT | 1 => Spacing::Exact(amount.max(1.0)),
                        _ => Spacing::Relative(amount),
                    }
                })
                .unwrap_or(Spacing::Relative(1.0));

            // bullets by the list style's name
            let list_name = match paragraph.list_style.filter(|e| self.package.contains(*e)) {
                Some(list) => self.style_name(list)?.unwrap_or_default().to_lowercase(),
                None => String::new(),
            };
            let counter = counters.entry(paragraph.level).or_insert(0);
            let label = if list_name.contains("bullet") || list_name.contains("dash") {
                *counter = 0;
                Some("•".to_string())
            } else if list_name.contains("number") {
                *counter += 1;
                Some(format!("{}.", counter))
            } else if list_name.contains("letter") {
                *counter += 1;
                Some(format!("{}.", (b'a' + ((*counter - 1) % 26) as u8) as char))
            } else {
                *counter = 0;
                None
            };
            counters.retain(|level, _| *level <= paragraph.level);

            let mut runs = Vec::new();
            let units: Vec<u16> = paragraph.text.encode_utf16().collect();
            for (start, end, style) in &paragraph.runs {
                let mut run_style = base.clone();
                if let Some(style) = style.filter(|e| self.package.contains(*e)) {
                    run_style.apply(&self.resolved_properties(style)?.group(0))?;
                }
                let end = (*end).min(units.len());
                let start = (*start).min(end);
                runs.push((String::from_utf16_lossy(&units[start..end]), run_style));
            }
            if runs.is_empty() {
                runs.push((String::new(), base.clone()));
            }

            layouts.push(ParagraphLayout {
                align: para.varint(para_props::ALIGNMENT).unwrap_or(0),
                spacing,
                space_before: para.float(para_props::SPACE_BEFORE).unwrap_or(0.0),
                space_after: para.float(para_props::SPACE_AFTER).unwrap_or(0.0),
                left_indent: para.float(para_props::LEFT_INDENT).unwrap_or(0.0),
                first_indent: para.float(para_props::FIRST_LINE_INDENT).unwrap_or(0.0),
                label,
                level: paragraph.level,
                runs,
            });
        }
        Ok(layouts)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn render_image(&self, canvas: &mut Canvas, message: &Message, frame: &Frame) -> Result<()> {
        let style = reference(message, 3)?.filter(|e| self.package.contains(*e));
        let props = match style {
            Some(style) if self.package.object(style)?.message_type() == TYPE_MEDIA_STYLE => {
                self.resolved_properties(style)?.group(0)
            }
            _ => Message::default(),
        };
        let opacity = props.float(media_props::OPACITY).unwrap_or(1.0);
        let shadow = match props.message(media_props::SHADOW)? {
            Some(e) => Shadow::from_message(&e)?,
            None => None,
        };

        // the mask crops, in the image's own coordinates
        let mut clip = String::new();
        if let Some(mask) = reference(message, 5)?.filter(|e| self.package.contains(*e)) {
            let mask_message = self.package.message(mask)?;
            let (mask_frame, _) = self.object_frame(mask)?;
            if let Some(source) = mask_message.message(2)?
                && let Some(path) = path_source_outline(&source, f64::from(mask_frame.w), f64::from(mask_frame.h))?
            {
                let id = canvas.id("c");
                let path = kurbo::Affine::translate((f64::from(mask_frame.x), f64::from(mask_frame.y))) * path;
                let _ = write!(canvas.defs, "<clipPath id=\"{id}\"><path d=\"{}\"/></clipPath>", svg_path(&path));
                clip = format!(" clip-path=\"url(#{id})\"");
            }
        }

        let mut group = format!("<g opacity=\"{opacity}\"{clip}");
        if let Some(shadow) = &shadow {
            group.push_str(&canvas.shadow_filter(shadow));
        }
        group.push('>');
        canvas.body.push_str(&group);

        let data = message.message(11)?.and_then(|e| e.varint(1));
        let bytes = match data {
            Some(data) => self.data_bytes(data)?.map(|e| e.1),
            None => None,
        };
        match bytes.as_deref().and_then(|e| image_mime(e).map(|mime| (mime, e))) {
            Some((mime, bytes)) => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                let _ = write!(
                    canvas.body,
                    "<image width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" href=\"data:{mime};base64,{encoded}\"/>",
                    frame.w, frame.h
                );
            }
            None => {
                let _ = write!(
                    canvas.body,
                    "<rect width=\"{}\" height=\"{}\" fill=\"#888888\" fill-opacity=\"0.3\"/>",
                    frame.w, frame.h
                );
            }
        }

        if let Some(stroke) = props.message(media_props::STROKE)?
            && let Some(stroke) = Stroke::from_message(&stroke)?
        {
            let _ = write!(
                canvas.body,
                "<rect width=\"{}\" height=\"{}\" fill=\"none\"{} stroke-width=\"{}\"/>",
                frame.w,
                frame.h,
                color_attribute("stroke", &stroke.color),
                stroke.width
            );
        }
        canvas.body.push_str("</g>");
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
/// An arrow head at `tip` pointing away from `from`
fn line_end_svg(kind: LineEnd, tip: kurbo::Point, from: kurbo::Point, stroke: &Stroke) -> String {
    let size = f64::from(stroke.width.max(1.0)) * 4.0;
    let direction = (tip - from).normalize();
    let normal = kurbo::Vec2::new(-direction.y, direction.x);
    let back = tip - direction * size;
    let color = color_attribute("fill", &stroke.color);
    let outline = color_attribute("stroke", &stroke.color);
    match kind {
        LineEnd::None => String::new(),
        LineEnd::Circle => format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\"{color}/>", tip.x, tip.y, size / 2.5),
        LineEnd::Bar => {
            let (a, b) = (tip + normal * size / 2.0, tip - normal * size / 2.0);
            format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{outline} stroke-width=\"{}\"/>",
                a.x, a.y, b.x, b.y, stroke.width
            )
        }
        LineEnd::Arrow => {
            let (a, b) = (back + normal * size / 2.0, back - normal * size / 2.0);
            format!(
                "<polyline points=\"{},{} {},{} {},{}\" fill=\"none\"{outline} stroke-width=\"{}\"/>",
                a.x, a.y, tip.x, tip.y, b.x, b.y, stroke.width
            )
        }
        LineEnd::FilledArrow | LineEnd::Square | LineEnd::Diamond => {
            let points = match kind {
                LineEnd::FilledArrow => vec![back + normal * size / 2.0, tip, back - normal * size / 2.0],
                LineEnd::Square => {
                    let half = normal * size / 3.0;
                    let behind = tip - direction * size / 1.5;
                    vec![tip + half, tip - half, behind - half, behind + half]
                }
                _ => {
                    let middle = tip - direction * size / 2.0;
                    vec![tip, middle + normal * size / 3.0, tip - direction * size, middle - normal * size / 3.0]
                }
            };
            let points: Vec<String> = points.iter().map(|e| format!("{},{}", e.x, e.y)).collect();
            format!("<polygon points=\"{}\"{color}/>", points.join(" "))
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_and_wraps_text() {
        let measurer = Measurer::default();
        let style = TextStyle {
            size: 20.0,
            ..Default::default()
        };
        let paragraph = ParagraphLayout {
            align: 0,
            spacing: Spacing::Relative(1.0),
            space_before: 0.0,
            space_after: 0.0,
            left_indent: 0.0,
            first_indent: 0.0,
            label: None,
            level: 0,
            runs: vec![("one two three four five six seven eight nine ten".to_string(), style)],
        };
        let (wide, wide_height) = layout_text(&measurer, std::slice::from_ref(&paragraph), 10_000.0, 1.0);
        let (narrow, narrow_height) = layout_text(&measurer, &[paragraph], 120.0, 1.0);
        assert_eq!(wide.len(), 1);
        assert!(narrow.len() > 2, "{} lines", narrow.len());
        assert!(narrow_height > wide_height);
    }

    #[test]
    fn keeps_spaces_between_styled_runs() {
        let measurer = Measurer::default();
        let normal = TextStyle {
            size: 30.0,
            ..Default::default()
        };
        let bold = TextStyle {
            bold: true,
            ..normal.clone()
        };
        let paragraph = ParagraphLayout {
            align: 0,
            spacing: Spacing::Relative(1.0),
            space_before: 0.0,
            space_after: 0.0,
            left_indent: 0.0,
            first_indent: 0.0,
            label: None,
            level: 0,
            runs: vec![
                ("are ".to_string(), normal.clone()),
                ("memory".to_string(), bold.clone()),
                (" bugs".to_string(), normal.clone()),
            ],
        };
        let (lines, _) = layout_text(&measurer, &[paragraph], 10_000.0, 1.0);
        let spans: Vec<(String, f32, f32)> = lines[0].spans.iter().map(|e| (e.text.clone(), e.x, e.width)).collect();
        let memory = spans.iter().find(|e| e.0 == "memory").unwrap();
        let bugs = spans.iter().find(|e| e.0.contains("bugs")).unwrap();
        let space = measurer.width(" ", &normal);
        assert!(space > 1.0, "space width {space}");
        assert!(
            bugs.1 + measurer.width(&bugs.0[..bugs.0.len() - bugs.0.trim_start().len()], &normal) >= memory.1 + memory.2 + space - 0.01,
            "{spans:?}"
        );
    }

    #[test]
    fn rasterises_svg() {
        let png = svg_to_png(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>"#,
            200,
        )
        .unwrap();
        let size = imagesize::blob_size(&png).unwrap();
        assert_eq!((size.width, size.height), (200, 100));
    }
}
