use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::helpers::errors::McpResult;
use crate::helpers::resources::mcp_resource_path;
use crate::mcp_internal_error;
use crate::tools::other::rdk_qemu::AGENT_BINARY;

////////////////////////////////////////////////////////////////////////////////
pub struct RdkQemuFiles {
    wic_qcow2: PathBuf,
    u_boot_bin: PathBuf,
    agent_socket: PathBuf,
    agent_folder: PathBuf,
}

impl RdkQemuFiles {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn prepare(wic_qcow2_source: &Path, u_boot_bin_source: &Path) -> McpResult<Self> {
        let wic_qcow2 = PathBuf::from("./qemu-rdk.wic.qcow2");
        let u_boot_bin = PathBuf::from("./qemu-rdk-u-boot.bin");
        let agent_socket = PathBuf::from("./qemu-rdk-agent.sock");
        let agent_folder = mcp_resource_path("qemu").await?;

        let agent_binary = agent_folder.join(AGENT_BINARY);
        match tokio::fs::try_exists(&agent_binary).await {
            Ok(true) => {}
            Ok(false) => return Err(mcp_internal_error!("agent has a broken symlink {agent_binary:?}")),
            Err(e) => return Err(mcp_internal_error!("missing resource {agent_binary:?}: {e}")),
        }

        let targets = [wic_qcow2.as_path(), u_boot_bin.as_path()];
        ensure_not_a_working_copy(wic_qcow2_source, &targets).await?;
        ensure_not_a_working_copy(u_boot_bin_source, &targets).await?;

        if let Err(e) = tokio::fs::remove_file(&agent_socket).await
            && e.kind() != ErrorKind::NotFound
        {
            return Err(mcp_internal_error!(
                "failed to remove the stale agent socket {agent_socket:?}: {e}"
            ));
        }

        let files = Self {
            wic_qcow2,
            u_boot_bin,
            agent_socket,
            agent_folder,
        };

        copy_image(wic_qcow2_source, &files.wic_qcow2).await?;
        copy_image(u_boot_bin_source, &files.u_boot_bin).await?;
        Ok(files)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn wic_qcow2(&self) -> &Path {
        &self.wic_qcow2
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn u_boot_bin(&self) -> &Path {
        &self.u_boot_bin
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn agent_socket(&self) -> &Path {
        &self.agent_socket
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn agent_folder(&self) -> &Path {
        &self.agent_folder
    }
}

////////////////////////////////////////////////////////////////////////////////
impl Drop for RdkQemuFiles {
    fn drop(&mut self) {
        for path in [&self.wic_qcow2, &self.u_boot_bin, &self.agent_socket] {
            match std::fs::remove_file(path) {
                Ok(()) => info!("removed {}", path.display()),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => warn!("failed to remove {path:?}: {e}"),
            }
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
async fn ensure_not_a_working_copy(source: &Path, targets: &[&Path]) -> McpResult<()> {
    let Ok(source_path) = tokio::fs::canonicalize(source).await else {
        return Ok(());
    };

    for target in targets {
        if tokio::fs::canonicalize(target).await.is_ok_and(|e| e == source_path) {
            return Err(mcp_internal_error!("{source:?} is a working copy qemu boots from"));
        }
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
async fn copy_image(source: &Path, target: &Path) -> McpResult<()> {
    info!("copying {source:?} to {target:?}");
    tokio::fs::copy(source, target)
        .await
        .map_err(|e| mcp_internal_error!("failed to copy {source:?} to {target:?}: {e}"))?;
    Ok(())
}
