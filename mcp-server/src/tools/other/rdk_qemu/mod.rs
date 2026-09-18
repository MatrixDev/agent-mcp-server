mod agent;
mod console_buffer;
mod files;
mod instance;

use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use instance::RdkQemuInstance;
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::debug;

use crate::context::McpAgentContext;
use crate::helpers::errors::McpResult;
use crate::mcp_internal_error;
use crate::permissions::PermissionsGroup;
use crate::tools::other::rdk_qemu::files::RdkQemuFiles;

////////////////////////////////////////////////////////////////////////////////
const BOOT_TIMEOUT_DEFAULT: Duration = Duration::from_secs(300);
const EXEC_TIMEOUT_DEFAULT: Duration = Duration::from_secs(120);
const AGENT_LIVENESS_TIMEOUT: Duration = Duration::from_secs(5);

const CONSOLE_CAPACITY: usize = 4096;
const CONSOLE_PROMPT_READY: &str = "root@rdkb-arm:~#";
const AGENT_BINARY: &str = "agent-aarch64-unknown-linux-musl";

////////////////////////////////////////////////////////////////////////////////
#[derive(Clone, Default)]
pub struct RdkQemuManager {
    instance: Arc<Mutex<Option<RdkQemuInstance>>>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RdkQemuStartTool {
    /// path to *.wic.qcow2 file, copied into the current folder and booted from there
    wic_qcow2_path: String,
    /// path to u-boot.bin file, copied into the current folder and booted from there
    u_boot_bin_path: String,
    /// seconds to wait for the QEMU to start, defaults to 300
    timeout_secs: Option<u64>,
}

impl RdkQemuStartTool {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
        manager: &RdkQemuManager,
    ) -> McpResult<String> {
        let wic_qcow2 = context.resolve_path(&self.wic_qcow2_path).await?;
        context.check_permissions(PermissionsGroup::FsRead, &wic_qcow2).await?;
        Self::ensure_regular_file(&wic_qcow2, "wic_qcow2_path").await?;

        let u_boot_bin = context.resolve_path(&self.u_boot_bin_path).await?;
        context.check_permissions(PermissionsGroup::FsRead, &u_boot_bin).await?;
        Self::ensure_regular_file(&u_boot_bin, "u_boot_bin_path").await?;

        let mut lock = manager.instance.lock().await;
        if let Some(instance) = lock.take() {
            debug!("stopping previous QEMU instance");
            let result = instance.shutdown().await;
            debug!("stopped previous QEMU instance: {result}");
        }

        let timeout = self.timeout_secs.map_or(BOOT_TIMEOUT_DEFAULT, Duration::from_secs);
        let started_at = Instant::now();

        let files = RdkQemuFiles::prepare(&wic_qcow2, &u_boot_bin).await?;
        let instance = RdkQemuInstance::spawn(request, timeout, files).await?;

        let elapsed = started_at.elapsed().as_secs();
        let message = format!("QEMU started after {elapsed} seconds");

        *lock = Some(instance);
        Ok(message)
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn ensure_regular_file(path: &Path, argument: &str) -> Result<(), ErrorData> {
        let message = match tokio::fs::metadata(path).await {
            Ok(metadata) if metadata.is_file() => return Ok(()),
            Ok(_) => format!("{argument} is not a regular file: {}", path.display()),
            Err(e) => format!("{argument} is not accessible: {}: {e}", path.display()),
        };
        Err(mcp_internal_error!("{message}"))
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RdkQemuExecTool {
    /// shell command to run inside the running qemu instance
    command: String,
    /// seconds to wait before cancelling the command in the guest, defaults to 120
    timeout_secs: Option<u64>,
}

impl RdkQemuExecTool {
    pub async fn handle(self, request: &RequestContext<RoleServer>, manager: &RdkQemuManager) -> McpResult<String> {
        let agent = {
            let mut slot = manager.instance.lock().await;
            let Some(instance) = slot.as_mut() else {
                return Err(mcp_internal_error!("QEMU is not running, call rdk_qemu_start first"));
            };

            if let Some(status) = instance.exit_status()? {
                let console = instance.console_dump_after_exit().await;
                *slot = None;

                return Err(mcp_internal_error!(
                    "QEMU exited: {status}, last messages:\n\n{console}"
                ));
            }

            instance.agent()
        };

        if !agent.ping(AGENT_LIVENESS_TIMEOUT).await {
            return Err(mcp_internal_error!("the RDK QEMU agent stopped answering"));
        }

        let timeout = self.timeout_secs.map_or(EXEC_TIMEOUT_DEFAULT, Duration::from_secs);
        agent.execute(request, self.command, timeout).await
    }
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RdkQemuStopTool {}

impl RdkQemuStopTool {
    pub async fn handle(self, manager: &RdkQemuManager) -> McpResult<String> {
        match manager.instance.lock().await.take() {
            Some(e) => Ok(e.shutdown().await),
            None => Ok("QEMU is not running".to_string()),
        }
    }
}
