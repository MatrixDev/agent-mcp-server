use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::context::McpAgentContext;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LightsSetColorTool {
    /// id of the smart light to change color for (matched case-insensitively)
    id: String,
    /// RGB color of the light, each component in 0.0 to 1.0 range
    color_rgb: [f32; 3],
}

impl LightsSetColorTool {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let device_id = self.id.trim();
        let device_url = context.lights.get_device_url(device_id).await.ok_or_else(|| {
            let message = format!("no light with id {:?} found in cache; run lights_info first", self.id);
            error!("{message}");
            ErrorData::invalid_request(message, None)
        })?;

        self.set_color(context, &device_url).await?;

        Ok(format!("set light for {:?} to rgb{:?}", device_id, self.color_rgb))
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn set_color(&self, context: &McpAgentContext, device_url: &str) -> Result<(), ErrorData> {
        #[derive(Debug, Serialize)]
        struct WledState {
            on: bool,
            seg: Vec<WledSegment>,
        }

        #[derive(Debug, Serialize)]
        struct WledSegment {
            col: Vec<[u8; 3]>,
        }

        let [r, g, b] = self.color_rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let url = format!("{device_url}/json/state");
        let state = WledState {
            on: true,
            seg: vec![WledSegment { col: vec![[r, g, b]] }],
        };

        let client = context.lights.client();
        let request = client.post(&url).json(&state);
        request
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|e| {
                let message = format!("failed to set color via {url}: {e}");
                error!("{message}");
                ErrorData::internal_error(message, None)
            })?;

        Ok(())
    }
}
