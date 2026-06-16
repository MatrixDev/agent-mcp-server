use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;

use crate::context::McpAgentContext;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LightsSetColorTool {
    /// name of the smart light to change color for
    name: String,
    /// RGB color of the light, each component in 0.0 to 1.0 range
    color_rgb: [f32; 3],
}

impl LightsSetColorTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let _ = context;
        Ok(format!("{:?} light color changed to {:?}", self.name, self.color_rgb))
    }
}
