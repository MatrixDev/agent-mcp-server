//! Object operations shared by the single-purpose tools and keynote_batch: each takes its
//! parameters (without the presentation path) and applies them to an open document.

use anyhow::{Context, Result, anyhow, bail};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::tools::keynote::code::{CodeOptions, CodeTheme, code_paragraphs};
use crate::tools::keynote::format::document::KeynoteDocument;
use crate::tools::keynote::format::drawing::{Color, Dash, Fill, Outline, Stroke, svg_path_bounds};
use crate::tools::keynote::format::objects::{
    Fit, ImageSpec, LineSpec, ObjectEdit, ShapeSpec, ShapeStyleSpec, TextLayoutSpec, ZOrder, parse_object_id,
};
use crate::tools::keynote::params::{
    CommonParams, FrameParams, FramePatchParams, Images, ShadowParams, StrokeParams, StyleParams, TextLayoutParams,
    TextParams, parse_fit, parse_line_end,
};

////////////////////////////////////////////////////////////////////////////////
/// Result of adding or editing an object
#[derive(Debug, Serialize)]
pub struct ObjectResult {
    /// "id:N", use it in builds, keynote_edit_object and z orders
    pub id: String,
    #[serde(flatten)]
    pub slide: crate::tools::keynote::CheckedSlide,
}

////////////////////////////////////////////////////////////////////////////////
fn slide_number(slide: u32) -> Result<usize> {
    if slide == 0 {
        bail!("slides are numbered from 1");
    }
    Ok(slide as usize)
}

////////////////////////////////////////////////////////////////////////////////
// Images
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddImageParams {
    /// 1-based slide number
    pub slide: u32,
    /// image file: png, jpg, gif, heic, tiff, pdf or svg (an SVG is stored as a sharp PNG
    /// rendition next to its source)
    pub file: String,
    /// where the image goes; omitted, it fills the slide (or the media placeholder)
    pub frame: Option<FrameParams>,
    /// "contain" (default, or "cover" for a placeholder), "cover", "stretch" or "original"
    pub fit: Option<String>,
    /// 0..1
    pub opacity: Option<f32>,
    /// rounds the corners, points
    pub corner_radius: Option<f32>,
    /// degrees clockwise
    pub rotation: Option<f32>,
    /// border: "none" or {color, width, dash}
    pub stroke: Option<StrokeParams>,
    /// "none" or {color, blur, offset}
    pub shadow: Option<ShadowParams>,
    /// true: replace the layout's media placeholder (layouts like "Title & Photo")
    pub placeholder: Option<bool>,
    #[serde(flatten)]
    pub common: CommonParams,
}

impl AddImageParams {
    pub fn image_paths(&self) -> Vec<String> {
        vec![self.file.clone()]
    }

    pub fn apply(&self, document: &mut KeynoteDocument, images: &Images) -> Result<u64> {
        let fill_placeholder = self.placeholder.unwrap_or(false);
        let spec = ImageSpec {
            file: images.get(&self.file)?,
            frame: self.frame.map(|e| e.frame()),
            fit: parse_fit(&self.fit, if fill_placeholder { Fit::Cover } else { Fit::Contain })?,
            rotation: self.rotation.unwrap_or(0.0),
            opacity: self.opacity,
            corner_radius: self.corner_radius,
            stroke: self.stroke.as_ref().map(StrokeParams::stroke).transpose()?,
            shadow: self.shadow.as_ref().map(ShadowParams::shadow).transpose()?,
            fill_placeholder,
            common: self.common.spec()?,
        };
        document.add_image(slide_number(self.slide)?, &spec)
    }
}

////////////////////////////////////////////////////////////////////////////////
// Shapes
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddShapeParams {
    /// 1-based slide number
    pub slide: u32,
    /// "rect", "rounded_rect", "ellipse", "triangle", "right_triangle", "diamond", "pentagon",
    /// "hexagon", "polygon", "star", "arrow", "double_arrow", "chevron", "plus", "callout" or
    /// "path"
    pub shape: String,
    /// required, except for a "path" placed where its coordinates say
    pub frame: Option<FrameParams>,
    /// rounded_rect and callout corners, points (defaults to 16)
    pub corner_radius: Option<f32>,
    /// polygon sides (defaults to 6)
    pub sides: Option<u32>,
    /// star points (defaults to 5)
    pub points: Option<u32>,
    /// star inner radius as a fraction of the outer one (defaults to 0.5)
    pub inner_radius: Option<f32>,
    /// SVG path data for "path", eg "M0 0 L100 0 L50 80 Z"; scaled to `frame` when given
    pub d: Option<String>,
    /// degrees clockwise
    pub rotation: Option<f32>,
    /// the theme's shape preset 0..5 to start from (defaults to 0)
    pub preset: Option<u32>,
    #[serde(flatten)]
    pub style: StyleParams,
    /// text inside the shape
    pub text: Option<TextParams>,
    #[serde(flatten)]
    pub layout: TextLayoutParams,
    #[serde(flatten)]
    pub common: CommonParams,
}

impl AddShapeParams {
    ////////////////////////////////////////////////////////////////////////////////
    fn outline(&self) -> Result<Outline> {
        let radius = self.corner_radius.unwrap_or(16.0);
        Ok(match self.shape.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "rect" | "rectangle" | "square" => Outline::Rect,
            "rounded_rect" | "rounded_rectangle" | "rounded" => Outline::RoundedRect { radius },
            "ellipse" | "circle" | "oval" => Outline::Ellipse,
            "triangle" => Outline::Triangle,
            "right_triangle" => Outline::RightTriangle,
            "diamond" | "rhombus" => Outline::Diamond,
            "pentagon" => Outline::Pentagon,
            "hexagon" => Outline::Hexagon,
            "polygon" => Outline::Polygon {
                sides: self.sides.unwrap_or(6),
            },
            "star" => Outline::Star {
                points: self.points.unwrap_or(5),
                inner: self.inner_radius.unwrap_or(0.5),
            },
            "arrow" | "right_arrow" => Outline::Arrow { double: false },
            "double_arrow" => Outline::Arrow { double: true },
            "chevron" => Outline::Chevron,
            "plus" | "cross" => Outline::Plus,
            "callout" | "speech_bubble" => Outline::Callout { radius },
            "path" => Outline::Path(
                self.d
                    .clone()
                    .ok_or_else(|| anyhow!("a path shape needs `d` with SVG path data"))?,
            ),
            other => bail!(
                "unknown shape {other:?}, use rect, rounded_rect, ellipse, triangle, right_triangle, diamond, \
                 pentagon, hexagon, polygon, star, arrow, double_arrow, chevron, plus, callout or path"
            ),
        })
    }

    pub fn apply(&self, document: &mut KeynoteDocument, _images: &Images) -> Result<u64> {
        let outline = self.outline()?;
        let frame = match (&self.frame, &outline) {
            (Some(frame), _) => frame.frame(),
            (None, Outline::Path(data)) => svg_path_bounds(data)?,
            (None, _) => bail!("a {} needs a frame", self.shape),
        };
        let spec = ShapeSpec {
            outline,
            frame,
            rotation: self.rotation.unwrap_or(0.0),
            text_box: false,
            preset: self.preset,
            style: self.style.spec()?,
            layout: self.layout.spec()?,
            text: self.text.as_ref().map(TextParams::paragraphs).transpose()?,
            common: self.common.spec()?,
        };
        document.add_shape(slide_number(self.slide)?, &spec)
    }
}

////////////////////////////////////////////////////////////////////////////////
// Text boxes
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddTextBoxParams {
    /// 1-based slide number
    pub slide: u32,
    pub frame: FrameParams,
    /// a string (one paragraph per line), a paragraph object, or a list of them; paragraphs take
    /// `text`, `markdown` or `runs` plus character formatting (font, size, bold, italic,
    /// underline, strike, color, highlight, baseline, link, monospace) and paragraph formatting
    /// (align, bullet, indent_level, line_spacing, line_height, space_before, space_after)
    pub text: TextParams,
    /// degrees clockwise
    pub rotation: Option<f32>,
    /// a text box has no fill or line unless given here
    #[serde(flatten)]
    pub style: StyleParams,
    #[serde(flatten)]
    pub layout: TextLayoutParams,
    #[serde(flatten)]
    pub common: CommonParams,
}

impl AddTextBoxParams {
    pub fn apply(&self, document: &mut KeynoteDocument, _images: &Images) -> Result<u64> {
        let spec = ShapeSpec {
            outline: Outline::Rect,
            frame: self.frame.frame(),
            rotation: self.rotation.unwrap_or(0.0),
            text_box: true,
            preset: None,
            style: self.style.spec()?,
            layout: self.layout.spec()?,
            text: Some(self.text.paragraphs()?),
            common: self.common.spec()?,
        };
        document.add_shape(slide_number(self.slide)?, &spec)
    }
}

////////////////////////////////////////////////////////////////////////////////
// Lines
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddLineParams {
    /// 1-based slide number
    pub slide: u32,
    /// start point [x, y]
    pub from: [f32; 2],
    /// end point [x, y]
    pub to: [f32; 2],
    /// defaults to black
    pub color: Option<String>,
    /// points, defaults to 2
    pub width: Option<f32>,
    /// "solid" (default), "dash", "dot" or "dash_dot"
    pub dash: Option<String>,
    /// end at `from`: "none" (default), "arrow", "filled_arrow", "circle", "square", "diamond" or "bar"
    pub head: Option<String>,
    /// end at `to`, same choices as `head`
    pub tail: Option<String>,
    /// 0..1
    pub opacity: Option<f32>,
    /// "none" or {color, blur, offset}
    pub shadow: Option<ShadowParams>,
    #[serde(flatten)]
    pub common: CommonParams,
}

impl AddLineParams {
    pub fn apply(&self, document: &mut KeynoteDocument, _images: &Images) -> Result<u64> {
        let spec = LineSpec {
            from: (self.from[0], self.from[1]),
            to: (self.to[0], self.to[1]),
            stroke: Stroke {
                color: self.color.as_deref().map(Color::parse).transpose()?.unwrap_or(Color::BLACK),
                width: self.width.unwrap_or(2.0),
                dash: self.dash.as_deref().map(Dash::parse).transpose()?.unwrap_or(Dash::Solid),
            },
            head: parse_line_end(&self.head)?,
            tail: parse_line_end(&self.tail)?,
            opacity: self.opacity,
            shadow: self.shadow.as_ref().map(ShadowParams::shadow).transpose()?,
            common: self.common.spec()?,
        };
        document.add_line(slide_number(self.slide)?, &spec)
    }
}

////////////////////////////////////////////////////////////////////////////////
// Code blocks
////////////////////////////////////////////////////////////////////////////////

/// Colours of a code block, any left out keep the default dark theme
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct CodeThemeParams {
    pub text: Option<String>,
    pub keyword: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub string: Option<String>,
    pub number: Option<String>,
    pub comment: Option<String>,
    pub function: Option<String>,
    #[serde(rename = "macro")]
    pub macro_: Option<String>,
    /// fill behind the code, "none" for no fill
    pub background: Option<String>,
    /// border colour, "none" for no border
    pub border: Option<String>,
}

impl CodeThemeParams {
    fn theme(&self) -> Result<(CodeTheme, bool)> {
        let mut theme = CodeTheme::default();
        let set = |target: &mut Color, value: &Option<String>| -> Result<()> {
            if let Some(value) = value {
                *target = Color::parse(value)?;
            }
            Ok(())
        };
        set(&mut theme.text, &self.text)?;
        set(&mut theme.keyword, &self.keyword)?;
        set(&mut theme.type_, &self.type_)?;
        set(&mut theme.string, &self.string)?;
        set(&mut theme.number, &self.number)?;
        set(&mut theme.comment, &self.comment)?;
        set(&mut theme.function, &self.function)?;
        set(&mut theme.macro_, &self.macro_)?;
        let mut background = true;
        match self.background.as_deref() {
            Some(e) if e.trim().eq_ignore_ascii_case("none") => background = false,
            other => set(&mut theme.background, &other.map(str::to_string))?,
        }
        match self.border.as_deref() {
            Some(e) if e.trim().eq_ignore_ascii_case("none") => theme.border = None,
            Some(e) => theme.border = Some(Color::parse(e)?),
            None => {}
        }
        Ok((theme, background))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddCodeParams {
    /// 1-based slide number
    pub slide: u32,
    pub frame: FrameParams,
    /// the source code; or `file`
    pub code: Option<String>,
    /// source file to read instead of `code`
    pub file: Option<String>,
    /// for highlighting: rust, c, cpp, java, kotlin, swift, go, js, ts, python, sh, … (plain
    /// text otherwise); defaults to the file extension
    pub language: Option<String>,
    pub theme: Option<CodeThemeParams>,
    /// defaults to "Menlo"
    pub font: Option<String>,
    /// points, defaults to 26
    pub size: Option<f32>,
    /// baseline to baseline, points (defaults to 1.45 × size)
    pub line_height: Option<f32>,
    /// 1-based lines shown at full strength, the others dimmed
    pub highlight_lines: Option<Vec<usize>>,
    /// number the lines
    pub line_numbers: Option<bool>,
    /// padding inside the block, points (defaults to 24)
    pub text_inset: Option<f32>,
    /// corner radius of the block, points (defaults to 12)
    pub corner_radius: Option<f32>,
    #[serde(flatten)]
    pub common: CommonParams,
}

impl AddCodeParams {
    pub fn text_paths(&self) -> Vec<String> {
        self.file.iter().cloned().collect()
    }

    pub fn apply(&self, document: &mut KeynoteDocument, images: &Images) -> Result<u64> {
        let code = match (&self.code, &self.file) {
            (Some(code), None) => code.clone(),
            (None, Some(file)) => images.text(file)?,
            _ => bail!("give the code block either `code` or `file`"),
        };
        let language = self
            .language
            .clone()
            .or_else(|| self.file.as_ref().and_then(|e| e.rsplit_once('.')).map(|e| e.1.to_string()))
            .unwrap_or_default();
        let (theme, background) = self.theme.clone().unwrap_or_default().theme()?;
        let size = self.size.unwrap_or(26.0);
        let options = CodeOptions {
            language,
            theme: theme.clone(),
            font: self.font.clone().unwrap_or_else(|| "Menlo".to_string()),
            size,
            line_height: Some(self.line_height.unwrap_or(size * 1.45)),
            highlight_lines: self.highlight_lines.clone().unwrap_or_default(),
            line_numbers: self.line_numbers.unwrap_or(false),
        };
        let paragraphs = code_paragraphs(&code, &options)?;

        let spec = ShapeSpec {
            outline: Outline::RoundedRect {
                radius: self.corner_radius.unwrap_or(12.0),
            },
            frame: self.frame.frame(),
            rotation: 0.0,
            text_box: true,
            preset: None,
            style: ShapeStyleSpec {
                fill: Some(if background { Fill::Color(theme.background) } else { Fill::None }),
                stroke: Some(theme.border.map(|color| Stroke {
                    color,
                    width: 2.0,
                    dash: Dash::Solid,
                })),
                shadow: None,
                opacity: None,
            },
            layout: TextLayoutSpec {
                inset: Some(self.text_inset.unwrap_or(24.0)),
                vertical_align: Some(crate::tools::keynote::format::objects::VerticalAlign::Top),
                shrink_to_fit: Some(false),
                columns: None,
            },
            text: Some(paragraphs),
            common: self.common.spec()?,
        };
        document.add_shape(slide_number(self.slide)?, &spec)
    }
}

////////////////////////////////////////////////////////////////////////////////
// Editing and deleting
////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct EditObjectParams {
    /// object id, "id:N" as keynote_info lists it
    pub id: String,
    /// any of x, y, w, h to change
    pub frame: Option<FramePatchParams>,
    /// degrees clockwise
    pub rotation: Option<f32>,
    /// "front", "back", "above:id:N" or "below:id:N"
    pub z: Option<String>,
    #[serde(flatten)]
    pub style: StyleParams,
    /// new text (shapes, text boxes, title and body)
    pub text: Option<TextParams>,
    #[serde(flatten)]
    pub layout: TextLayoutParams,
    pub locked: Option<bool>,
    /// accessibility description, "" removes it
    pub description: Option<String>,
    /// URL opened on click while presenting, "" removes it
    pub link: Option<String>,
}

impl EditObjectParams {
    pub fn apply(&self, document: &mut KeynoteDocument, _images: &Images) -> Result<u64> {
        let identifier = parse_object_id(&self.id)?;
        let edit = ObjectEdit {
            frame: self.frame.unwrap_or_default().patch(),
            rotation: self.rotation,
            z: self.z.as_deref().map(ZOrder::parse).transpose()?,
            style: self.style.spec()?,
            layout: self.layout.spec()?,
            text: self.text.as_ref().map(TextParams::paragraphs).transpose()?,
            locked: self.locked,
            description: self.description.clone(),
            link: self.link.clone(),
        };
        document.edit_object(identifier, &edit)?;
        Ok(identifier)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DeleteObjectsParams {
    /// object ids, "id:N" as keynote_info lists them
    pub ids: Vec<String>,
}

impl DeleteObjectsParams {
    /// Deletes the objects, returns the slides they were on
    pub fn apply(&self, document: &mut KeynoteDocument, _images: &Images) -> Result<Vec<usize>> {
        if self.ids.is_empty() {
            bail!("no ids to delete");
        }
        let mut identifiers = Vec::new();
        let mut slides = Vec::new();
        for id in &self.ids {
            let identifier = parse_object_id(id)?;
            let slide = document.slide_with_object(identifier).with_context(|| format!("deleting {id}"))?;
            identifiers.push(identifier);
            if !slides.contains(&slide) {
                slides.push(slide);
            }
        }
        document.delete_objects(&identifiers)?;
        Ok(slides)
    }
}
