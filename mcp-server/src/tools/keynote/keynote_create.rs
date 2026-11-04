use std::path::Path;

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::helpers::resources::mcp_resource_path;
use crate::mcp_internal_error;
use crate::permissions::PermissionsGroup;
use crate::tools::keynote::format::document::{DocumentInfo, KeynoteDocument};
use crate::tools::keynote::params::Images;
use crate::tools::keynote::{NewSlideSpec, add_slides, resolve_existing, resolve_missing, run_blocking};

////////////////////////////////////////////////////////////////////////////////
const DEFAULT_TEMPLATE: &str = "keynote/template.key";

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteCreateTool {
    /// path of the new *.key file, must not exist yet
    path: String,
    /// presentation whose theme (layouts, styles) the new one uses, defaults to
    /// mcp-resources/keynote/template.key; `duplicate` numbers refer to its slides
    template: Option<String>,
    /// slides of the new presentation, the template's own slides are left out
    slides: Vec<NewSlideSpec>,
}

impl KeynoteCreateTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        if !self.path.ends_with(".key") {
            return Err(mcp_internal_error!("path must have a .key extension: {}", self.path));
        }
        if self.slides.is_empty() {
            return Err(mcp_internal_error!("a presentation needs at least one slide"));
        }

        let path = resolve_missing(context, &self.path, PermissionsGroup::FsWrite).await?;
        let template = match &self.template {
            Some(e) => resolve_existing(context, e, PermissionsGroup::FsRead).await?,
            None => mcp_resource_path(DEFAULT_TEMPLATE).await.map_err(|_| {
                mcp_internal_error!(
                    "no template: pass `template` (any .key presentation with the theme to use) \
                     or add mcp-resources/{DEFAULT_TEMPLATE}"
                )
            })?,
        };
        let image_paths = self.slides.iter().flat_map(|e| e.content.image_paths());
        let images = Images::load(context, image_paths.collect::<Vec<_>>(), Vec::new()).await?;

        run_blocking(move || create_presentation(&template, &path, &self.slides, &images)).await
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Builds a presentation at `path` from `template`'s theme, the template's own slides are
/// dropped once the new ones are in
pub(super) fn create_presentation(
    template: &Path,
    path: &Path,
    slides: &[NewSlideSpec],
    images: &Images,
) -> anyhow::Result<DocumentInfo> {
    let mut document = KeynoteDocument::open(template)?;
    let samples = document.slide_count()?;

    add_slides(&mut document, slides, samples + 1, images)?;
    for _ in 0..samples {
        document.delete_slide(1)?;
    }

    document.reset_identity();
    document.save(path)?;
    document.info()
}
