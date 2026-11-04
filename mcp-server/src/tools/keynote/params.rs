//! Tool parameters shared by the Keynote tools, and their conversion to document specs.
//!
//! Geometry is in slide points with the origin top-left, colours are "#RRGGBB" or
//! "#RRGGBBAA", rotation is in degrees clockwise.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::mcp_internal_error;
use crate::permissions::PermissionsGroup;
use crate::tools::keynote::format::drawing::{Color, Dash, Fill, Frame, LineEnd, Shadow, Stroke};
use crate::tools::keynote::format::media::ImageFile;
use crate::tools::keynote::format::objects::{
    BackgroundSpec, CommonSpec, Fit, FramePatch, ShapeStyleSpec, TextLayoutSpec, VerticalAlign, ZOrder,
};
use crate::tools::keynote::format::rich_text::{
    Align, Baseline, Bullet, LineSpacing, MONOSPACE_FONT, Paragraph, RunStyle, TextRun, markdown_paragraphs,
};

////////////////////////////////////////////////////////////////////////////////
// Files
////////////////////////////////////////////////////////////////////////////////

/// Image and text files a call uses, loaded up front (permission checks are async)
#[derive(Debug, Default)]
pub struct Images {
    files: HashMap<String, ImageFile>,
    texts: HashMap<String, String>,
}

impl Images {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn load(
        context: &McpAgentContext,
        images: impl IntoIterator<Item = String>,
        texts: impl IntoIterator<Item = String>,
    ) -> McpResult<Self> {
        let mut loaded = Self::default();
        for path in images {
            if loaded.files.contains_key(&path) {
                continue;
            }
            let resolved: PathBuf = context.resolve_path(&path).await?;
            context.check_permissions(PermissionsGroup::FsRead, &resolved).await?;
            let file = tokio::task::spawn_blocking(move || ImageFile::load(&resolved))
                .await
                .map_err(|e| mcp_internal_error!("image task failed: {e}"))?
                .map_err(|e| mcp_internal_error!("{e:#}"))?;
            loaded.files.insert(path, file);
        }
        for path in texts {
            if loaded.texts.contains_key(&path) {
                continue;
            }
            let resolved: PathBuf = context.resolve_path(&path).await?;
            context.check_permissions(PermissionsGroup::FsRead, &resolved).await?;
            let text = tokio::fs::read_to_string(&resolved)
                .await
                .map_err(|e| mcp_internal_error!("failed to read {}: {e}", resolved.display()))?;
            loaded.texts.insert(path, text);
        }
        Ok(loaded)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn get(&self, path: &str) -> Result<ImageFile> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| anyhow!("image {path:?} was not loaded"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn text(&self, path: &str) -> Result<String> {
        self.texts
            .get(path)
            .cloned()
            .ok_or_else(|| anyhow!("file {path:?} was not loaded"))
    }
}

////////////////////////////////////////////////////////////////////////////////
// Text
////////////////////////////////////////////////////////////////////////////////

/// Character formatting
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct RunStyleParams {
    /// font name, eg "Helvetica Neue", "Inter", "Menlo"
    pub font: Option<String>,
    /// points
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// "#RRGGBB" or "#RRGGBBAA"
    pub color: Option<String>,
    /// background colour behind the text
    pub highlight: Option<String>,
    /// "super", "sub" or "normal"
    pub baseline: Option<String>,
    /// URL the text links to
    pub link: Option<String>,
    /// shortcut for a monospace font (Menlo)
    pub monospace: Option<bool>,
}

impl RunStyleParams {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn style(&self) -> Result<RunStyle> {
        let color = |e: &Option<String>| e.as_deref().map(Color::parse).transpose();
        let baseline = match self.baseline.as_deref().map(|e| e.trim().to_lowercase()) {
            None => None,
            Some(e) if e == "super" || e == "superscript" => Some(Baseline::Super),
            Some(e) if e == "sub" || e == "subscript" => Some(Baseline::Sub),
            Some(e) if e == "normal" || e == "none" => Some(Baseline::Normal),
            Some(e) => bail!("unknown baseline {e:?}, use super, sub or normal"),
        };
        let font = match (&self.font, self.monospace) {
            (Some(font), _) => Some(font.clone()),
            (None, Some(true)) => Some(MONOSPACE_FONT.to_string()),
            (None, _) => None,
        };
        if self.size.is_some_and(|e| e <= 0.0) {
            bail!("font size must be positive");
        }
        Ok(RunStyle {
            font,
            size: self.size,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            color: color(&self.color)?,
            highlight: color(&self.highlight)?,
            baseline,
            link: self.link.clone(),
        })
    }
}

/// A run of text with its own formatting
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RunParams {
    pub text: String,
    #[serde(flatten)]
    pub style: RunStyleParams,
}

/// A paragraph: `text`, `markdown` or `runs`, plus paragraph formatting; character formatting
/// given here applies to all of its runs
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ParagraphParams {
    /// plain text; newlines become line breaks within the paragraph
    pub text: Option<String>,
    /// Markdown-lite: **bold**, *italic*, ~~strike~~, `code`, [text](url); every line is a
    /// paragraph, "- " / "1. " start bullets, two spaces per nesting level
    pub markdown: Option<String>,
    /// runs with their own formatting
    pub runs: Option<Vec<RunParams>>,
    #[serde(flatten)]
    pub style: RunStyleParams,
    /// "left", "center", "right" or "justify"
    pub align: Option<String>,
    /// "none", "disc", "number" or "letter"
    pub bullet: Option<String>,
    /// 0-based list level
    pub indent_level: Option<u32>,
    /// line height as a multiple, eg 1.2
    pub line_spacing: Option<f32>,
    /// exact baseline to baseline distance in points, instead of `line_spacing`
    pub line_height: Option<f32>,
    /// points
    pub space_before: Option<f32>,
    /// points
    pub space_after: Option<f32>,
}

impl ParagraphParams {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn paragraphs(&self) -> Result<Vec<Paragraph>> {
        let base = self.style.style()?;
        let code = RunStyle {
            font: Some(MONOSPACE_FONT.to_string()),
            ..Default::default()
        };
        let mut paragraphs = match (&self.text, &self.markdown, &self.runs) {
            (Some(text), None, None) => vec![Paragraph {
                runs: vec![TextRun {
                    text: text.clone(),
                    style: base,
                }],
                ..Default::default()
            }],
            (None, Some(markdown), None) => markdown_paragraphs(markdown, &base, &code),
            (None, None, Some(runs)) => {
                let mut list = Vec::new();
                for run in runs {
                    list.push(TextRun {
                        text: run.text.clone(),
                        style: base.merged(&run.style.style()?),
                    });
                }
                vec![Paragraph {
                    runs: list,
                    ..Default::default()
                }]
            }
            (None, None, None) => vec![Paragraph {
                runs: vec![TextRun {
                    text: String::new(),
                    style: base,
                }],
                ..Default::default()
            }],
            _ => bail!("give a paragraph one of text, markdown or runs"),
        };

        let align = self.align.as_deref().map(Align::parse).transpose()?;
        let bullet = self.bullet.as_deref().map(Bullet::parse).transpose()?;
        let line_spacing = match (self.line_spacing, self.line_height) {
            (Some(_), Some(_)) => bail!("give line_spacing or line_height, not both"),
            (Some(e), None) => Some(LineSpacing::Relative(e)),
            (None, Some(e)) => Some(LineSpacing::Exact(e)),
            (None, None) => None,
        };
        for paragraph in &mut paragraphs {
            paragraph.align = paragraph.align.or(align);
            paragraph.bullet = paragraph.bullet.or(bullet);
            paragraph.level = paragraph.level.or(self.indent_level);
            paragraph.line_spacing = paragraph.line_spacing.or(line_spacing);
            paragraph.space_before = paragraph.space_before.or(self.space_before);
            paragraph.space_after = paragraph.space_after.or(self.space_after);
        }
        Ok(paragraphs)
    }
}

/// Text: a plain string (one paragraph per line), one paragraph object (whose `markdown` may
/// hold several lines), or a list of paragraph objects
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum TextParams {
    Plain(String),
    Paragraphs(Vec<ParagraphParams>),
    Paragraph(ParagraphParams),
}

/// Text ready for the document
#[derive(Debug, Clone)]
pub enum TextValue {
    Plain(String),
    Rich(Vec<Paragraph>),
}

impl TextParams {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn value(&self) -> Result<TextValue> {
        Ok(match self {
            TextParams::Plain(text) => TextValue::Plain(text.clone()),
            TextParams::Paragraph(paragraph) => TextValue::Rich(paragraph.paragraphs()?),
            TextParams::Paragraphs(list) => {
                let mut paragraphs = Vec::new();
                for (index, paragraph) in list.iter().enumerate() {
                    paragraphs.extend(paragraph.paragraphs().with_context(|| format!("paragraph {}", index + 1))?);
                }
                TextValue::Rich(paragraphs)
            }
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn paragraphs(&self) -> Result<Vec<Paragraph>> {
        Ok(match self.value()? {
            TextValue::Plain(text) => crate::tools::keynote::format::rich_text::plain_paragraphs(&text),
            TextValue::Rich(paragraphs) => paragraphs,
        })
    }
}

////////////////////////////////////////////////////////////////////////////////
// Geometry and styling
////////////////////////////////////////////////////////////////////////////////

/// Rectangle in slide points, origin top-left
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
pub struct FrameParams {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl FrameParams {
    pub fn frame(&self) -> Frame {
        Frame {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

/// Any part of a frame to change
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
pub struct FramePatchParams {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub w: Option<f32>,
    pub h: Option<f32>,
}

impl FramePatchParams {
    pub fn patch(&self) -> FramePatch {
        FramePatch {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

/// Linear gradient
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GradientParams {
    /// degrees clockwise, 0 runs left to right, 90 top to bottom
    pub angle: Option<f32>,
    /// [colour, position 0..1] pairs
    pub stops: Vec<(String, f32)>,
}

impl GradientParams {
    fn fill(&self) -> Result<Fill> {
        if self.stops.len() < 2 {
            bail!("a gradient needs at least two stops");
        }
        let mut stops = Vec::new();
        for (color, position) in &self.stops {
            stops.push((Color::parse(color)?, position.clamp(0.0, 1.0)));
        }
        Ok(Fill::Gradient {
            angle: self.angle.unwrap_or(0.0),
            stops,
        })
    }
}

/// A fill: a colour "#RRGGBB[AA]", "none", or {"gradient": {...}}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum FillParams {
    Color(String),
    Gradient { gradient: GradientParams },
}

impl FillParams {
    pub fn fill(&self) -> Result<Fill> {
        match self {
            FillParams::Color(text) if text.trim().eq_ignore_ascii_case("none") => Ok(Fill::None),
            FillParams::Color(text) => Ok(Fill::Color(Color::parse(text)?)),
            FillParams::Gradient { gradient } => gradient.fill(),
        }
    }
}

/// A line: "none" or {color, width, dash}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum StrokeParams {
    None(String),
    Line {
        /// defaults to black
        color: Option<String>,
        /// points, defaults to 1
        width: Option<f32>,
        /// "solid" (default), "dash", "dot" or "dash_dot"
        dash: Option<String>,
    },
}

impl StrokeParams {
    pub fn stroke(&self) -> Result<Option<Stroke>> {
        match self {
            StrokeParams::None(text) if text.trim().eq_ignore_ascii_case("none") => Ok(None),
            StrokeParams::None(text) => bail!("invalid stroke {text:?}, use \"none\" or {{color, width, dash}}"),
            StrokeParams::Line { color, width, dash } => Ok(Some(Stroke {
                color: color.as_deref().map(Color::parse).transpose()?.unwrap_or(Color::BLACK),
                width: width.unwrap_or(1.0),
                dash: dash.as_deref().map(Dash::parse).transpose()?.unwrap_or(Dash::Solid),
            })),
        }
    }
}

/// A drop shadow: "none" or {color, blur, offset}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ShadowParams {
    None(String),
    Shadow {
        /// colour with alpha as opacity, defaults to "#00000080"
        color: Option<String>,
        /// points, defaults to 8
        blur: Option<f32>,
        /// [dx, dy] in points, defaults to [0, 4]
        offset: Option<[f32; 2]>,
    },
}

impl ShadowParams {
    pub fn shadow(&self) -> Result<Option<Shadow>> {
        match self {
            ShadowParams::None(text) if text.trim().eq_ignore_ascii_case("none") => Ok(None),
            ShadowParams::None(text) => bail!("invalid shadow {text:?}, use \"none\" or {{color, blur, offset}}"),
            ShadowParams::Shadow { color, blur, offset } => {
                let [dx, dy] = offset.unwrap_or([0.0, 4.0]);
                Ok(Some(Shadow {
                    color: Color::parse(color.as_deref().unwrap_or("#00000080"))?,
                    blur: blur.unwrap_or(8.0),
                    dx,
                    dy,
                }))
            }
        }
    }
}

/// Fill, line, shadow and opacity
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct StyleParams {
    /// "#RRGGBB[AA]", "none" or {"gradient": {"angle": 135, "stops": [["#17161B", 0], ["#2A1A12", 1]]}}
    pub fill: Option<FillParams>,
    /// "none" or {"color": "#2E2D35", "width": 2, "dash": "solid"}
    pub stroke: Option<StrokeParams>,
    /// "none" or {"color": "#00000080", "blur": 12, "offset": [0, 4]}
    pub shadow: Option<ShadowParams>,
    /// 0..1
    pub opacity: Option<f32>,
}

impl StyleParams {
    pub fn spec(&self) -> Result<ShapeStyleSpec> {
        Ok(ShapeStyleSpec {
            fill: self.fill.as_ref().map(FillParams::fill).transpose()?,
            stroke: self.stroke.as_ref().map(StrokeParams::stroke).transpose()?,
            shadow: self.shadow.as_ref().map(ShadowParams::shadow).transpose()?,
            opacity: self.opacity,
        })
    }
}

/// Layout of text inside a shape or text box
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct TextLayoutParams {
    /// padding between the frame and the text on every side, points
    pub text_inset: Option<f32>,
    /// "top", "middle" or "bottom"
    pub vertical_align: Option<String>,
    /// "none" or "shrink" (shrink the text to fit the frame)
    pub autosize: Option<String>,
    /// number of text columns
    pub columns: Option<u32>,
    /// gap between columns, points (defaults to 24)
    pub column_gap: Option<f32>,
}

impl TextLayoutParams {
    pub fn spec(&self) -> Result<TextLayoutSpec> {
        let shrink_to_fit = match self.autosize.as_deref().map(|e| e.trim().to_lowercase()) {
            None => None,
            Some(e) if e == "shrink" || e == "shrink-to-fit" || e == "shrink_to_fit" => Some(true),
            Some(e) if e == "none" => Some(false),
            Some(e) => bail!("unknown autosize {e:?}, use none or shrink"),
        };
        Ok(TextLayoutSpec {
            inset: self.text_inset,
            vertical_align: self.vertical_align.as_deref().map(VerticalAlign::parse).transpose()?,
            shrink_to_fit,
            columns: self.columns.map(|e| (e, self.column_gap.unwrap_or(24.0))),
        })
    }
}

/// Settings of every new object
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct CommonParams {
    /// "front" (default), "back", "above:id:N" or "below:id:N"
    pub z: Option<String>,
    pub locked: Option<bool>,
    /// accessibility description (alt text)
    pub description: Option<String>,
    /// URL opened when the object is clicked while presenting
    pub link: Option<String>,
}

impl CommonParams {
    pub fn spec(&self) -> Result<CommonSpec> {
        Ok(CommonSpec {
            z: self.z.as_deref().map(ZOrder::parse).transpose()?.unwrap_or(ZOrder::Front),
            locked: self.locked.unwrap_or(false),
            description: self.description.clone(),
            link: self.link.clone(),
        })
    }
}

////////////////////////////////////////////////////////////////////////////////
pub fn parse_line_end(text: &Option<String>) -> Result<LineEnd> {
    text.as_deref().map(LineEnd::parse).transpose().map(|e| e.unwrap_or(LineEnd::None))
}

////////////////////////////////////////////////////////////////////////////////
pub fn parse_fit(text: &Option<String>, default: Fit) -> Result<Fit> {
    text.as_deref().map(Fit::parse).transpose().map(|e| e.unwrap_or(default))
}

////////////////////////////////////////////////////////////////////////////////
// Background
////////////////////////////////////////////////////////////////////////////////

/// Slide background: a colour "#17161B", "layout" (the layout's own), or
/// {"color"}, {"gradient": {...}} or {"image": "file.jpg", "fit": "cover"}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum BackgroundParams {
    Keyword(String),
    Fill {
        color: Option<String>,
        gradient: Option<GradientParams>,
        /// image file
        image: Option<String>,
        /// for images: "cover" (default), "contain", "stretch" or "original"
        fit: Option<String>,
    },
}

impl BackgroundParams {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn image_path(&self) -> Option<String> {
        match self {
            BackgroundParams::Fill { image, .. } => image.clone(),
            _ => None,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn spec(&self, images: &Images) -> Result<BackgroundSpec> {
        match self {
            BackgroundParams::Keyword(text) if text.trim().eq_ignore_ascii_case("layout") => Ok(BackgroundSpec::Layout),
            BackgroundParams::Keyword(text) if text.trim().eq_ignore_ascii_case("none") => {
                Ok(BackgroundSpec::Fill(Fill::None))
            }
            BackgroundParams::Keyword(text) => Ok(BackgroundSpec::Fill(Fill::Color(Color::parse(text)?))),
            BackgroundParams::Fill {
                color,
                gradient,
                image,
                fit,
            } => match (color, gradient, image) {
                (Some(color), None, None) => Ok(BackgroundSpec::Fill(Fill::Color(Color::parse(color)?))),
                (None, Some(gradient), None) => Ok(BackgroundSpec::Fill(gradient.fill()?)),
                (None, None, Some(image)) => Ok(BackgroundSpec::Image {
                    file: images.get(image)?,
                    fit: parse_fit(fit, Fit::Cover)?,
                }),
                _ => bail!("give a background one of color, gradient or image"),
            },
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_forms() {
        let plain: TextParams = serde_json::from_str(r#""One\nTwo""#).unwrap();
        assert!(matches!(plain.value().unwrap(), TextValue::Plain(_)));

        let rich: TextParams = serde_json::from_str(
            r##"[{"runs": [{"text": "76% → 24%", "size": 84, "bold": true, "color": "#6CC38B"}]},
                 {"text": "Android bugs", "size": 26, "color": "#A8A39A", "bullet": "none", "space_before": 8}]"##,
        )
        .unwrap();
        let paragraphs = rich.paragraphs().unwrap();
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].style.size, Some(84.0));
        assert_eq!(paragraphs[1].bullet, Some(Bullet::None));
        assert_eq!(paragraphs[1].runs[0].style.color, Some(Color::parse("#A8A39A").unwrap()));

        let markdown: TextParams =
            serde_json::from_str(r##"{"markdown": "**76%** of bugs\n- `unsafe` is ~4%", "size": 30}"##).unwrap();
        let paragraphs = markdown.paragraphs().unwrap();
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].style.bold, Some(true));
        assert_eq!(paragraphs[0].runs[0].style.size, Some(30.0));
        assert_eq!(paragraphs[1].runs[0].style.font.as_deref(), Some(MONOSPACE_FONT));
    }

    #[test]
    fn parses_styles_and_backgrounds() {
        let style: StyleParams = serde_json::from_str(
            r##"{"fill": {"gradient": {"angle": 90, "stops": [["#000000", 0], ["#FFFFFF", 1]]}},
                "stroke": "none", "shadow": {"blur": 12}}"##,
        )
        .unwrap();
        let spec = style.spec().unwrap();
        assert!(matches!(spec.fill, Some(Fill::Gradient { .. })));
        assert_eq!(spec.stroke, Some(None));
        assert!(matches!(spec.shadow, Some(Some(_))));

        let images = Images::default();
        let color: BackgroundParams = serde_json::from_str(r##""#17161B""##).unwrap();
        assert!(matches!(color.spec(&images).unwrap(), BackgroundSpec::Fill(Fill::Color(_))));
        let layout: BackgroundParams = serde_json::from_str(r#""layout""#).unwrap();
        assert!(matches!(layout.spec(&images).unwrap(), BackgroundSpec::Layout));
        let object: BackgroundParams = serde_json::from_str(r##"{"color": "#17161B"}"##).unwrap();
        assert!(object.spec(&images).is_ok());
        let bad: BackgroundParams = serde_json::from_str(r#""blue""#).unwrap();
        assert!(bad.spec(&images).is_err());
    }
}
