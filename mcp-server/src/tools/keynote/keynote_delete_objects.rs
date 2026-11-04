use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::tools::keynote::operations::DeleteObjectsParams;
use crate::tools::keynote::{checked_slide, edit_presentation};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteDeleteObjectsTool {
    /// path to the *.key file
    path: String,
    #[serde(flatten)]
    params: DeleteObjectsParams,
}

impl KeynoteDeleteObjectsTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let params = self.params;
        edit_presentation(context, &self.path, Vec::new(), Vec::new(), move |document, images| {
            let slides = params.apply(document, images)?;
            slides.into_iter().map(|e| checked_slide(document, e)).collect::<anyhow::Result<Vec<_>>>()
        })
        .await
    }
}
