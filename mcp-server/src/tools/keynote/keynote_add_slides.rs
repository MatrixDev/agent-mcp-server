use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::tools::keynote::{NewSlideSpec, add_slides, checked_slide, edit_presentation};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteAddSlidesTool {
    /// path to the *.key file
    path: String,
    /// slides to add, in order
    slides: Vec<NewSlideSpec>,
    /// 1-based slide number the first added slide ends up at, defaults to the end
    position: Option<u32>,
}

impl KeynoteAddSlidesTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let images = self.slides.iter().flat_map(|e| e.content.image_paths()).collect();
        let (slides, position) = (self.slides, self.position);
        edit_presentation(context, &self.path, images, Vec::new(), move |document, images| {
            let position = match position {
                Some(e) => e as usize,
                None => document.slide_count()? + 1,
            };
            add_slides(document, &slides, position, images)?;
            // only the new slides, the rest is unchanged
            (position..position + slides.len())
                .map(|e| checked_slide(document, e))
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .await
    }
}
