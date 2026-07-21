use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::SteamCommand;

const SCRIPT: &str = "/home/matrixdev.guest/projects/ieee1905_bench.sh";

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct Ieee1905BenchTool {}

impl Ieee1905BenchTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let project_dir = context.resolve_path(".").await?;

        SteamCommand::new(request, "cargo")
            .args(["build"])
            .args(["--package", "ieee1905"])
            .args(["--target", "aarch64-unknown-linux-musl"])
            .args(["--release"])
            .current_dir(&project_dir)
            .execute_for_result()
            .await?
            .into_status_error()?;

        SteamCommand::new(request, "limactl")
            .args(["shell", "default", "sudo", "timeout", "10s", SCRIPT])
            .current_dir(&project_dir)
            .execute()
            .await
    }
}
