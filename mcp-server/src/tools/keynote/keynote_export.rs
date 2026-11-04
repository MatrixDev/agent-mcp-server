use anyhow::{Context, bail};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::permissions::PermissionsGroup;
use crate::tools::keynote::format::document::KeynoteDocument;
use crate::tools::keynote::format::render::svg_to_png;
use crate::tools::keynote::{resolve_existing, run_blocking};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteExportTool {
    /// path to the *.key file
    path: String,
    /// 1-based slide numbers to export, defaults to every slide
    slides: Option<Vec<u32>>,
    /// "png" (default) or "svg"
    format: Option<String>,
    /// image width in pixels, defaults to the slide width
    width: Option<u32>,
    /// folder the images go to (created when missing), as slide-NN.png
    out_dir: String,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Serialize)]
struct ExportedSlide {
    slide: usize,
    file: String,
}

#[derive(Debug, Serialize)]
struct ExportResult {
    files: Vec<ExportedSlide>,
    /// text that doesn't fit its frame
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// what the preview leaves out
    note: &'static str,
}

impl KeynoteExportTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let path = resolve_existing(context, &self.path, PermissionsGroup::FsRead).await?;
        let out_dir = context.resolve_path(&self.out_dir).await?;
        context.check_permissions(PermissionsGroup::FsWrite, &out_dir).await?;

        run_blocking(move || {
            let format = self.format.as_deref().unwrap_or("png").trim().to_lowercase();
            if format != "png" && format != "svg" {
                bail!("unknown format {format:?}, use png or svg");
            }
            let document = KeynoteDocument::open(&path)?;
            let count = document.slide_count()?;
            let numbers: Vec<usize> = match &self.slides {
                Some(list) => list.iter().map(|e| *e as usize).collect(),
                None => (1..=count).collect(),
            };
            if let Some(bad) = numbers.iter().find(|e| !(1..=count).contains(*e)) {
                bail!("slide {bad} is out of range, the presentation has {count} slides");
            }
            std::fs::create_dir_all(&out_dir).with_context(|| format!("failed to create {}", out_dir.display()))?;

            let digits = count.to_string().len().max(2);
            let mut files = Vec::new();
            let mut warnings = Vec::new();
            for number in numbers {
                let rendered = document.render_slide(number)?;
                warnings.extend(rendered.warnings);
                let file = out_dir.join(format!("slide-{number:0digits$}.{format}"));
                let bytes = match format.as_str() {
                    "svg" => rendered.svg.into_bytes(),
                    _ => svg_to_png(&rendered.svg, self.width.unwrap_or(rendered.width.round() as u32).clamp(16, 8192))?,
                };
                std::fs::write(&file, bytes).with_context(|| format!("failed to write {}", file.display()))?;
                files.push(ExportedSlide {
                    slide: number,
                    file: file.display().to_string(),
                });
            }
            Ok(ExportResult {
                files,
                warnings,
                note: "preview drawn without Keynote: fonts come from this machine; tables, charts, movies and \
                       effects show as labelled boxes",
            })
        })
        .await
    }
}
