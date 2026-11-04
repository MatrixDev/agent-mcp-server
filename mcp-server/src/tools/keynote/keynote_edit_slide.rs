use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::tools::keynote::{SlideContentParams, checked_slide, edit_presentation};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteEditSlideTool {
    /// path to the *.key file
    path: String,
    /// 1-based number of the slide to edit
    slide: u32,
    #[serde(flatten)]
    content: SlideContentParams,
}

impl KeynoteEditSlideTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let content = self.content;
        let number = self.slide as usize;
        edit_presentation(context, &self.path, content.image_paths(), Vec::new(), move |document, images| {
            let content = content.content(images)?;
            document.edit_slide(number, &content)?;
            checked_slide(document, number)
        })
        .await
    }
}
