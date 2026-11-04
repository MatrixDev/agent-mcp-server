use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::tools::keynote::operations::AddImageParams;
use crate::tools::keynote::{edit_presentation, object_result};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteAddImageTool {
    /// path to the *.key file
    path: String,
    #[serde(flatten)]
    params: AddImageParams,
}

impl KeynoteAddImageTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let params = self.params;
        edit_presentation(context, &self.path, params.image_paths(), Vec::new(), move |document, images| {
            let identifier = params.apply(document, images)?;
            object_result(document, identifier)
        })
        .await
    }
}
