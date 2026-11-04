//! Keynote presentations (*.key) read and edited in pure Rust, without Keynote installed.
//!
//! New slides are made from one of the theme's layouts, or duplicated from an existing slide.
//! New presentations start from a template presentation providing the theme. Slides take styled
//! text, backgrounds, images, shapes, lines, text boxes and code blocks; transitions between
//! slides and builds (animations) within a slide can be set on any slide. Slides render to
//! PNG/SVG previews for visual checks.

mod code;
#[cfg(test)]
mod explore;
mod format;
pub mod keynote_add_code;
pub mod keynote_add_image;
pub mod keynote_add_line;
pub mod keynote_add_shape;
pub mod keynote_add_slides;
pub mod keynote_add_text_box;
pub mod keynote_batch;
pub mod keynote_create;
pub mod keynote_delete_objects;
pub mod keynote_delete_slides;
pub mod keynote_edit_object;
pub mod keynote_edit_slide;
pub mod keynote_export;
pub mod keynote_info;
pub mod keynote_move_slide;
mod operations;
mod params;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_rich;
use std::path::PathBuf;

use anyhow::{Result, bail};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::mcp_internal_error;
use crate::permissions::PermissionsGroup;
use crate::tools::keynote::format::document::{BuildSpec, KeynoteDocument, SlideContent, SlideInfo, TransitionSpec};
use crate::tools::keynote::params::{BackgroundParams, Images, TextParams, TextValue};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Default, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NewSlideSpec {
    /// layout name (see keynote_info) the new slide is made from
    layout: Option<String>,
    /// 1-based number of a slide to copy with all its content, animations included, instead of a layout
    duplicate: Option<u32>,
    #[serde(flatten)]
    content: SlideContentParams,
}

/// What a slide shows; everything left out stays as it is
#[derive(Debug, Default, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SlideContentParams {
    /// text of the title placeholder: a string, or styled paragraphs (see keynote_add_text_box)
    title: Option<TextParams>,
    /// text of the body placeholder: a string with one paragraph (bullet) per line, or styled
    /// paragraphs
    body: Option<TextParams>,
    /// presenter notes
    notes: Option<String>,
    /// whether the slide is skipped when presenting
    skipped: Option<bool>,
    /// "#17161B", "layout" (the layout's own), {"gradient": {"angle": 135, "stops":
    /// [["#17161B", 0], ["#2A1A12", 1]]}} or {"image": "art/texture.jpg", "fit": "cover"}
    background: Option<BackgroundParams>,
    /// show or hide the layout's title placeholder
    show_title: Option<bool>,
    /// show or hide the layout's body placeholder
    show_body: Option<bool>,
    /// show or hide the slide number
    show_slide_number: Option<bool>,
    /// transition played when moving on from this slide to the next
    transition: Option<TransitionParams>,
    /// builds (animations) of the slide's objects in playback order, replacing any it has;
    /// an empty list removes them
    builds: Option<Vec<BuildParams>>,
}

impl SlideContentParams {
    ////////////////////////////////////////////////////////////////////////////////
    fn image_paths(&self) -> Vec<String> {
        self.background.iter().filter_map(BackgroundParams::image_path).collect()
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn content(&self, images: &Images) -> Result<SlideContent> {
        let split = |text: &Option<TextParams>| -> Result<(Option<String>, Option<Vec<format::rich_text::Paragraph>>)> {
            Ok(match text.as_ref().map(TextParams::value).transpose()? {
                None => (None, None),
                Some(TextValue::Plain(e)) => (Some(e), None),
                Some(TextValue::Rich(e)) => (None, Some(e)),
            })
        };
        let (title, title_rich) = split(&self.title)?;
        let (body, body_rich) = split(&self.body)?;
        Ok(SlideContent {
            title,
            body,
            title_rich,
            body_rich,
            notes: self.notes.clone(),
            skipped: self.skipped,
            background: self.background.as_ref().map(|e| e.spec(images)).transpose()?,
            show_title: self.show_title,
            show_body: self.show_body,
            show_slide_number: self.show_slide_number,
            transition: self.transition.as_ref().map(TransitionParams::spec),
            builds: self.builds.as_ref().map(|e| e.iter().map(BuildParams::spec).collect()),
        })
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TransitionParams {
    /// Keynote transition effect, "none" removes the transition; seen in real presentations:
    /// "apple:dissolve", "apple:fade-and-move", "apple:magic-move-implied-motion-path"
    effect: String,
    /// seconds, defaults to 1
    duration: Option<f64>,
    /// seconds before an automatic transition starts, defaults to 0
    delay: Option<f64>,
    /// move on to the next slide by itself instead of on click, defaults to false
    automatic: Option<bool>,
}

impl TransitionParams {
    ////////////////////////////////////////////////////////////////////////////////
    fn spec(&self) -> TransitionSpec {
        TransitionSpec {
            effect: self.effect.clone(),
            duration: self.duration.unwrap_or(1.0),
            delay: self.delay.unwrap_or(0.0),
            automatic: self.automatic.unwrap_or(false),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BuildParams {
    /// what to animate: "title", "body", "id:N" (an object id from keynote_info or an add tool)
    /// or "object N" (1-based position in the slide's objects)
    target: String,
    /// "In" to make it appear or "Out" to make it disappear, defaults to "In"
    kind: Option<String>,
    /// Keynote build effect, defaults to "apple:dissolve"; seen in real presentations:
    /// "apple:dissolve", "apple:dissolve character", "com.apple.iWork.Keynote.FromDarkness"
    effect: Option<String>,
    /// "All at Once" (default), or "By Bullet" / "By Highlighted Bullet" to take one step per
    /// paragraph of the target's text (a code block then reveals line by line)
    delivery: Option<String>,
    /// seconds, defaults to 1
    duration: Option<f64>,
    /// seconds before the (first) step starts, defaults to 0
    delay: Option<f64>,
    /// start the (first) step right after the previous one instead of on click, defaults to false
    automatic: Option<bool>,
}

impl BuildParams {
    ////////////////////////////////////////////////////////////////////////////////
    fn spec(&self) -> BuildSpec {
        BuildSpec {
            target: self.target.clone(),
            kind: self.kind.clone().unwrap_or_else(|| "In".to_string()),
            effect: self.effect.clone().unwrap_or_else(|| "apple:dissolve".to_string()),
            delivery: self.delivery.clone().unwrap_or_else(|| "All at Once".to_string()),
            duration: self.duration.unwrap_or(1.0),
            delay: self.delay.unwrap_or(0.0),
            automatic: self.automatic.unwrap_or(false),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Inserts `slides` starting at 1-based `position`, `duplicate` numbers refer to slides as
/// numbered before the insertion. Every slide is checked before anything changes.
fn add_slides(document: &mut KeynoteDocument, slides: &[NewSlideSpec], position: usize, images: &Images) -> Result<()> {
    let count = document.slide_count()?;
    if !(1..=count + 1).contains(&position) {
        bail!("position {position} is out of range, the presentation has {count} slides");
    }

    // resolve every duplicate and check every slide up front, later insertions shift numbers
    let layouts = document.layout_names()?;
    let mut sources = Vec::new();
    let mut contents = Vec::new();
    let mut problems = Vec::new();
    for (index, slide) in slides.iter().enumerate() {
        let number = index + 1;
        let source = match (&slide.layout, slide.duplicate) {
            (Some(_), Some(_)) => {
                problems.push(format!("slide {number}: pass either layout or duplicate, not both"));
                None
            }
            (None, None) => {
                problems.push(format!("slide {number}: pass a layout or a slide to duplicate"));
                None
            }
            (Some(layout), None) => {
                if !layouts.contains(layout) {
                    problems.push(format!(
                        "slide {number}: unknown layout {layout:?}, available: {}",
                        layouts.join(", ")
                    ));
                }
                None
            }
            (None, Some(source)) if (1..=count).contains(&(source as usize)) => Some(source as usize),
            (None, Some(source)) => {
                problems.push(format!(
                    "slide {number}: slide {source} to duplicate is out of range, the presentation has {count} slides"
                ));
                None
            }
        };
        sources.push(source);
        match slide.content.content(images) {
            Ok(content) => contents.push(content),
            Err(e) => {
                problems.push(format!("slide {number}: {e:#}"));
                contents.push(SlideContent::default());
            }
        }
    }
    if !problems.is_empty() {
        bail!("nothing was changed:\n{}", problems.join("\n"));
    }

    for (index, slide) in slides.iter().enumerate() {
        let target = position + index;
        match (&slide.layout, sources[index]) {
            (Some(layout), _) => document.new_slide_from_layout(layout, target)?,
            (None, Some(source)) => document.duplicate_slide(source, target)?,
            (None, None) => unreachable!("checked above"),
        }
        for source in sources.iter_mut().flatten() {
            if *source >= target {
                *source += 1;
            }
        }
        document
            .edit_slide(target, &contents[index])
            .map_err(|e| anyhow::anyhow!("slide {} (layout {:?}): {e:#}", index + 1, slide.layout.as_deref().unwrap_or("copy")))?;
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
/// A slide with the problems a visual check finds on it
#[derive(Debug, Serialize)]
pub struct CheckedSlide {
    #[serde(flatten)]
    pub slide: SlideInfo,
    /// text that doesn't fit its frame, measured with the system's fonts
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

////////////////////////////////////////////////////////////////////////////////
fn checked_slide(document: &KeynoteDocument, number: usize) -> Result<CheckedSlide> {
    Ok(CheckedSlide {
        slide: document.slide(number)?,
        warnings: document.text_warnings(number).unwrap_or_default(),
    })
}

////////////////////////////////////////////////////////////////////////////////
/// Opens a presentation, applies `work` with the files it needs loaded, saves it and returns
/// the result as JSON; nothing is saved when `work` fails
async fn edit_presentation<T>(
    context: &McpAgentContext,
    path: &str,
    images: Vec<String>,
    texts: Vec<String>,
    work: impl FnOnce(&mut KeynoteDocument, &Images) -> Result<T> + Send + 'static,
) -> McpResult<String>
where
    T: Serialize + Send + 'static,
{
    let path = resolve_existing(context, path, PermissionsGroup::FsWrite).await?;
    let images = Images::load(context, images, texts).await?;
    run_blocking(move || {
        let mut document = KeynoteDocument::open(&path)?;
        let result = work(&mut document, &images)?;
        document.save(&path)?;
        Ok(result)
    })
    .await
}

////////////////////////////////////////////////////////////////////////////////
/// The id of an object and its slide, as add and edit tools return them
fn object_result(document: &KeynoteDocument, identifier: u64) -> Result<operations::ObjectResult> {
    let number = document.slide_with_object(identifier)?;
    Ok(operations::ObjectResult {
        id: format!("id:{identifier}"),
        slide: checked_slide(document, number)?,
    })
}

////////////////////////////////////////////////////////////////////////////////
async fn resolve_existing(context: &McpAgentContext, path: &str, group: PermissionsGroup) -> McpResult<PathBuf> {
    let path = context.resolve_path(path).await?;
    context.check_permissions(group, &path).await?;
    match tokio::fs::try_exists(&path).await {
        Ok(true) => Ok(path),
        Ok(false) => Err(mcp_internal_error!("file does not exist: {}", path.display())),
        Err(e) => Err(mcp_internal_error!("file is not accessible: {}: {e}", path.display())),
    }
}

////////////////////////////////////////////////////////////////////////////////
async fn resolve_missing(context: &McpAgentContext, path: &str, group: PermissionsGroup) -> McpResult<PathBuf> {
    let path = context.resolve_path(path).await?;
    context.check_permissions(group, &path).await?;
    match tokio::fs::try_exists(&path).await {
        Ok(false) => Ok(path),
        Ok(true) => Err(mcp_internal_error!("file already exists: {}", path.display())),
        Err(e) => Err(mcp_internal_error!("file is not accessible: {}: {e}", path.display())),
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Runs file work off the async runtime, returns its result as pretty JSON
async fn run_blocking<T>(work: impl FnOnce() -> Result<T> + Send + 'static) -> McpResult<String>
where
    T: Serialize + Send + 'static,
{
    let value = match tokio::task::spawn_blocking(work).await {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => return Err(mcp_internal_error!("{e:#}")),
        Err(e) => return Err(mcp_internal_error!("keynote task failed: {e}")),
    };
    serde_json::to_string_pretty(&value).map_err(|e| mcp_internal_error!("failed to serialize the result: {e}"))
}
