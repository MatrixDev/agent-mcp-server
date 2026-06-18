use std::collections::HashMap;
use std::time::Duration;

use anyhow::anyhow;
use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent};
use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::tools::lights::LedDevice;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LightsInfoTool {
    /// seconds to scan the local network for WLED devices (default 3, clamped to 1..=30)
    #[serde(default)]
    timeout_secs: Option<u64>,
}

impl LightsInfoTool {
    const MDNS_WLED_SERVICE: &str = "_wled._tcp.local.";

    const DURATION_DEFAULT: Duration = Duration::from_secs(3);
    const DURATION_MIN: Duration = Duration::from_secs(1);
    const DURATION_MAX: Duration = Duration::from_secs(30);

    /// Per-device timeout for the `/json/info` HTTP request.
    const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn handle(self) -> Result<String, ErrorData> {
        let devices = self.find_mdns_devices().await?;
        info!("discovered {} WLED device(s)", devices.len());

        if devices.is_empty() {
            return Ok(format!(
                "no devices found on the local network via {}",
                Self::MDNS_WLED_SERVICE,
            ));
        }

        let client = reqwest::Client::builder()
            .timeout(Self::HTTP_TIMEOUT)
            .build()
            .map_err(|e| {
                let message = format!("failed to build HTTP client: {e}");
                error!("{message}");
                ErrorData::internal_error(message, None)
            })?;

        let mut join_set = JoinSet::new();
        for mut device in devices.into_values() {
            let client = client.clone();
            join_set.spawn(async move {
                if let Err(e) = Self::update_device_info(&client, &mut device).await {
                    warn!("failed to fetch device {} info: {e}", device.hostname);
                }
                device
            });
        }

        // format output

        let devices = join_set.join_all().await;
        let json = serde_json::to_string(&devices).map_err(|e| {
            let message = format!("failed to serialize devices list: {e}");
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        *LedDevice::lock_cache().await = devices.into_iter().map(|e| (e.id.clone(), e)).collect();

        Ok(json)
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn find_mdns_devices(&self) -> Result<HashMap<String, LedDevice>, ErrorData> {
        let window = self.timeout_secs.map_or(Self::DURATION_DEFAULT, |e| {
            Duration::from_secs(e.clamp(1, 30)).clamp(Self::DURATION_MIN, Self::DURATION_MAX)
        });

        let daemon = ServiceDaemon::new().map_err(|e| {
            let message = format!("failed to start mDNS daemon: {e}");
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let receiver = daemon.browse(Self::MDNS_WLED_SERVICE).map_err(|e| {
            let message = format!("failed to browse mDNS for WLED devices: {e}");
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let mut devices = HashMap::new();
        let _ = tokio::time::timeout(window, Self::track_mdns_events(receiver, &mut devices)).await;
        let _ = daemon.shutdown();
        Ok(devices)
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn track_mdns_events(receiver: Receiver<ServiceEvent>, devices: &mut HashMap<String, LedDevice>) {
        while let Ok(event) = receiver.recv_async().await {
            if let ServiceEvent::ServiceResolved(info) = event {
                // The mDNS instance label is the user-configured WLED name,
                // e.g. "main light._wled._tcp.local." -> "main light".
                let full_name = info.get_fullname();
                let id = full_name
                    .strip_suffix(&format!(".{}", Self::MDNS_WLED_SERVICE))
                    .unwrap_or(full_name)
                    .to_string();

                let addresses = info.get_addresses().iter().map(|e| e.to_string()).collect();
                let device = LedDevice {
                    id: id.clone(),
                    name: full_name.to_string(),
                    hostname: info.get_hostname().trim_end_matches('.').to_string(),
                    addresses,
                    port: info.get_port(),
                };

                devices.insert(id, device);
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn update_device_info(client: &reqwest::Client, device: &mut LedDevice) -> anyhow::Result<()> {
        /// https://kno.wled.ge/interfaces/json-api/
        #[derive(Debug, Deserialize)]
        struct WledInfo {
            #[serde(default)]
            name: Option<String>,
        }

        let host = device.choose_hostname();

        // Bracket IPv6 literals for the URL authority.
        let authority = if host.contains(':') {
            format!("[{host}]:{}", device.port)
        } else {
            format!("{host}:{}", device.port)
        };

        let url = format!("http://{authority}/json/info");
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow!("failed to query {url}: {e}"))?;

        let info = response
            .json::<WledInfo>()
            .await
            .map_err(|e| anyhow!("failed to parse /json/info from {url}: {e}"))?;

        if let Some(name) = info.name {
            device.name = name;
        }
        Ok(())
    }
}
