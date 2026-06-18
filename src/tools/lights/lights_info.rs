use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LightsInfoTool {}

impl LightsInfoTool {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let devices_lock = context.lights.lock_devices().await;
        let devices = devices_lock.values().collect::<Vec<_>>();

        if devices.is_empty() {
            return Ok("no devices found on the local network".to_string());
        }

        let json = serde_json::to_string(&devices).map_err(|e| {
            let message = format!("failed to serialize devices list: {e}");
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        Ok(json)
    }
}
