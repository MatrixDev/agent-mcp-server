use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::permissions::PermissionsGroup;
use crate::tools::keynote::format::document::{KeynoteDocument, LayoutInfo, SlideSummary};
use crate::tools::keynote::{resolve_existing, run_blocking};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteInfoTool {
    /// path to the *.key file
    path: String,
    /// only these slides, [from, to] 1-based and inclusive
    slides: Option<[u32; 2]>,
    /// one line per slide (layout, title, object and build counts) instead of everything
    summary: Option<bool>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Serialize)]
struct Summary {
    width: Option<f32>,
    height: Option<f32>,
    layouts: Vec<LayoutInfo>,
    slide_count: usize,
    slides: Vec<SlideSummary>,
}

impl KeynoteInfoTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let path = resolve_existing(context, &self.path, PermissionsGroup::FsRead).await?;
        let range = self.slides.map(|[from, to]| (from as usize, to as usize));
        let summary = self.summary.unwrap_or(false);
        run_blocking(move || {
            let info = KeynoteDocument::open(&path)?.info_range(range)?;
            let value = match summary {
                true => serde_json::to_value(Summary {
                    width: info.width,
                    height: info.height,
                    layouts: info.layouts,
                    slide_count: info.slide_count,
                    slides: info.slides.iter().map(SlideSummary::from).collect(),
                })?,
                false => serde_json::to_value(info)?,
            };
            Ok(value)
        })
        .await
    }
}
