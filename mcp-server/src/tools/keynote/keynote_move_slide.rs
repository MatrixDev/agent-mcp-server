use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::permissions::PermissionsGroup;
use crate::tools::keynote::format::document::KeynoteDocument;
use crate::tools::keynote::{resolve_existing, run_blocking};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteMoveSlideTool {
    /// path to the *.key file
    path: String,
    /// 1-based number of the slide to move
    slide: u32,
    /// 1-based slide number it ends up at
    position: u32,
}

impl KeynoteMoveSlideTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let path = resolve_existing(context, &self.path, PermissionsGroup::FsWrite).await?;
        run_blocking(move || {
            let mut document = KeynoteDocument::open(&path)?;
            document.move_slide(self.slide as usize, self.position as usize)?;
            document.save(&path)?;
            document.info()
        })
        .await
    }
}
