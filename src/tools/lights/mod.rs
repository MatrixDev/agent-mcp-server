use std::collections::HashMap;
use std::ops::DerefMut;
use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::Mutex;

pub mod lights_info;
pub mod lights_set_color;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, Serialize)]
struct LedDevice {
    id: String,
    name: String,
    hostname: String,
    addresses: Vec<String>,
    port: u16,
}

impl LedDevice {
    ////////////////////////////////////////////////////////////////////////////////
    fn choose_hostname(&self) -> &str {
        self.addresses.first().unwrap_or(&self.hostname)
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn lock_cache() -> impl DerefMut<Target = HashMap<String, Self>> {
        static CACHE: OnceLock<Mutex<HashMap<String, LedDevice>>> = OnceLock::new();
        CACHE.get_or_init(Mutex::default).lock().await
    }
}
