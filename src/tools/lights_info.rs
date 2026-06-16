use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;

use crate::context::McpAgentContext;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LightsInfoTool {}

impl LightsInfoTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let _ = context;
        Ok(format!("{:?}", ["main light", "room light", "contour",]))
    }
}
