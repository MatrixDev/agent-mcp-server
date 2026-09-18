use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, RwLockReadGuard};
use tokio::task::JoinSet;
use tracing::{error, info, instrument};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Serialize)]
pub struct LedDevice {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub port: u16,
    #[serde(skip)]
    tasks: JoinSet<()>,
}

impl LedDevice {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn prepare_base_url(&self) -> String {
        let host = self.addresses.first().unwrap_or(&self.hostname);
        let authority = if host.contains(':') {
            // Bracket IPv6 literals for the URL authority.
            format!("[{host}]:{}", self.port)
        } else {
            format!("{host}:{}", self.port)
        };
        format!("http://{authority}")
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Clone)]
pub struct LightsController {
    shared: LedDeviceShared,
    _tasks: Arc<JoinSet<()>>,
}

impl LightsController {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|e| anyhow!("failed to build HTTP client: {e}"))?;

        let shared = LedDeviceShared {
            client,
            devices: Default::default(),
        };

        let actor = LightsControllerActor { shared: shared.clone() };

        let mut tasks = JoinSet::new();
        tasks.spawn(actor.track_devices_worker());

        Ok(Self {
            shared,
            _tasks: Arc::new(tasks),
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn client(&self) -> &reqwest::Client {
        &self.shared.client
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn lock_devices(&self) -> RwLockReadGuard<'_, HashMap<String, LedDevice>> {
        self.shared.devices.read().await
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn get_device_url(&self, id: &str) -> Option<String> {
        Some(self.shared.devices.read().await.get(id)?.prepare_base_url())
    }
}

impl Debug for LightsController {
    ////////////////////////////////////////////////////////////////////////////////
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "LightsController")
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Clone)]
struct LedDeviceShared {
    client: reqwest::Client,
    devices: Arc<RwLock<HashMap<String, LedDevice>>>,
}

////////////////////////////////////////////////////////////////////////////////
struct LightsControllerActor {
    shared: LedDeviceShared,
}

impl LightsControllerActor {
    const MDNS_SERVICE: &str = "_wled._tcp.local.";

    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, name = "lights_tracker")]
    async fn track_devices_worker(self) {
        info!("search started: {}", Self::MDNS_SERVICE);
        loop {
            if let Err(e) = self.track_devices().await {
                error!("find_devices failed: {e}");
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn track_devices(&self) -> anyhow::Result<()> {
        let daemon = ServiceDaemon::new().map_err(|e| anyhow!("failed to start mDNS daemon: {e}"))?;

        let receiver = daemon
            .browse(Self::MDNS_SERVICE)
            .map_err(|e| anyhow!("failed to browse mDNS for WLED devices: {e}"))?;

        while let Ok(event) = receiver.recv_async().await {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    self.register_device(info).await;
                }
                ServiceEvent::ServiceRemoved(_service_type, full_name) => {
                    if self.shared.devices.write().await.remove(&full_name).is_some() {
                        info!("device removed: {full_name}");
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn register_device(&self, info: Box<ResolvedService>) {
        let full_name = info.get_fullname();

        // The mDNS instance label is the user-configured WLED name,
        // e.g. "main light._wled._tcp.local." -> "main light".
        let name = full_name
            .strip_suffix(&format!(".{}", Self::MDNS_SERVICE))
            .unwrap_or(&full_name)
            .to_string();

        let devices = &mut *self.shared.devices.write().await;
        let device = devices.entry(full_name.to_string()).or_insert_with_key(|e| {
            info!("device discovered: {full_name}");
            LedDevice {
                id: e.to_string(),
                name,
                hostname: Default::default(),
                addresses: Default::default(),
                port: Default::default(),
                tasks: Default::default(),
            }
        });

        device.hostname = info.get_hostname().trim_end_matches('.').to_string();
        device.addresses = info.get_addresses().iter().map(|e| e.to_string()).collect();
        device.port = info.get_port();

        if device.tasks.is_empty() {
            let shared = self.shared.clone();
            let device_id = device.id.clone();
            device.tasks.spawn(Self::refresh_device_info_worker(shared, device_id));
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, name = "lights_info_updater", fields(id = device_id))]
    async fn refresh_device_info_worker(shared: LedDeviceShared, device_id: String) {
        loop {
            let device_url = match shared.devices.read().await.get(&device_id) {
                None => break,
                Some(e) => e.prepare_base_url(),
            };

            if let Err(e) = Self::refresh_device_info(&shared, &device_id, &device_url).await {
                error!("refresh_device_info failed: {e}");
            }

            tokio::time::sleep(Duration::from_mins(1)).await;
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn refresh_device_info(shared: &LedDeviceShared, device_id: &str, device_url: &str) -> anyhow::Result<()> {
        /// https://kno.wled.ge/interfaces/json-api/
        #[derive(Debug, Deserialize)]
        struct WledInfo {
            #[serde(default)]
            name: Option<String>,
        }

        let url = format!("{device_url}/json/info");
        let request = shared.client.get(&url);
        let response = request
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|e| anyhow!("request failed {url}: {e}"))?;

        let info = response
            .json::<WledInfo>()
            .await
            .map_err(|e| anyhow!("failed to parse /json/info from {url}: {e}"))?;

        if let Some(device) = shared.devices.write().await.get_mut(device_id)
            && let Some(name) = info.name
            && name != device.name
        {
            info!("resolved name for {}: {name}", device.id);
            device.name = name;
        }
        Ok(())
    }
}
