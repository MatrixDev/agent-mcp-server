use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::tools::keynote::operations::AddCodeParams;
use crate::tools::keynote::{edit_presentation, object_result};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeynoteAddCodeTool {
    /// path to the *.key file
    path: String,
    #[serde(flatten)]
    params: AddCodeParams,
}

impl KeynoteAddCodeTool {
    pub async fn handle(self, context: &McpAgentContext) -> McpResult<String> {
        let params = self.params;
        edit_presentation(context, &self.path, Vec::new(), params.text_paths(), move |document, images| {
            let identifier = params.apply(document, images)?;
            object_result(document, identifier)
        })
        .await
    }
}
