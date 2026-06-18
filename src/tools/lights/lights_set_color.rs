use std::time::Duration;

use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::tools::lights::LedDevice;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LightsSetColorTool {
    /// id of the smart light to change color for (matched case-insensitively)
    id: String,
    /// RGB color of the light, each component in 0.0 to 1.0 range
    color_rgb: [f32; 3],
}

impl LightsSetColorTool {
    const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn handle(self) -> Result<String, ErrorData> {
        let rgb = self.color_rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);

        // Resolve the target from the cache populated by `lights_info` — no rescan.
        let target = self.id.trim();
        let Some(device) = LedDevice::lock_cache().await.get(target).cloned() else {
            let message = format!("no light with id {:?} found in cache; run lights_info first", self.id);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let client = reqwest::Client::builder()
            .timeout(Self::HTTP_TIMEOUT)
            .build()
            .map_err(|e| {
                let message = format!("failed to build HTTP client: {e}");
                error!("{message}");
                ErrorData::internal_error(message, None)
            })?;

        Self::set_color(&client, &device, rgb).await?;

        Ok(format!("set light for {} to rgb{rgb:?}", device.id))
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn set_color(client: &reqwest::Client, device: &LedDevice, [r, g, b]: [u8; 3]) -> Result<(), ErrorData> {
        /// https://kno.wled.ge/interfaces/json-api/
        #[derive(Debug, Serialize)]
        struct WledState {
            on: bool,
            seg: Vec<WledSegment>,
        }

        #[derive(Debug, Serialize)]
        struct WledSegment {
            col: Vec<[u8; 3]>,
        }

        let host = device.choose_hostname();

        // Bracket IPv6 literals for the URL authority.
        let authority = if host.contains(':') {
            format!("[{host}]:{}", device.port)
        } else {
            format!("{host}:{}", device.port)
        };

        let url = format!("http://{authority}/json/state");
        let state = WledState {
            on: true,
            seg: vec![WledSegment { col: vec![[r, g, b]] }],
        };

        client
            .post(&url)
            .json(&state)
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
