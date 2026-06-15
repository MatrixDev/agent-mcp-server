use std::path::{Path, PathBuf};

use anyhow::anyhow;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use tracing::error;

use crate::path_resolver::PathResolver;
use crate::permissions::{Permissions, PermissionsGroup, PermissionsKind};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug)]
pub struct McpAgentContext {
    path_resolver: PathResolver,
    pub permissions: Permissions,
}

impl McpAgentContext {
    const HEADER_WORKSPACE_ROOT: &'static str = "x-mcp-workspace-root";

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn new(context: &RequestContext<RoleServer>) -> Result<Self, ErrorData> {
        let path_resolver = match Self::prepare_path_resolver(context).await {
            Ok(e) => e,
            Err(e) => {
                let message = format!("failed to initialize path resolver: {e}");
                error!("{message}");
                return Err(ErrorData::internal_error(message, None));
            }
        };

        let permissions = match Self::prepare_permissions(&path_resolver).await {
            Ok(e) => e,
            Err(e) => {
                let message = format!("failed to initialize permissions provider: {e}");
                error!("{message}");
                return Err(ErrorData::internal_error(message, None));
            }
        };

        Ok(Self {
            path_resolver,
            permissions,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn resolve_path(&self, path: impl AsRef<Path>) -> Result<PathBuf, ErrorData> {
        self.path_resolver.resolve(path).await.map_err(|e| {
            let message = format!("resolve_path failed: {e}");
            error!("{message}");
            ErrorData::invalid_request(e.to_string(), None)
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn check_permissions(&self, group: PermissionsGroup, path: impl AsRef<Path>) -> Result<(), ErrorData> {
        self.permissions.check(group, path).await.map_err(|e| {
            let message = format!("permissions denied: {e}");
            error!("{message}");
            ErrorData::invalid_request(message, None)
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn prepare_path_resolver(context: &RequestContext<RoleServer>) -> anyhow::Result<PathResolver> {
        let request_parts = context
            .extensions
            .get::<http::request::Parts>()
            .ok_or_else(|| anyhow!("failed to parse request http parts"))?;

        let workspace_root = request_parts
            .headers
            .get(Self::HEADER_WORKSPACE_ROOT)
            .ok_or_else(|| anyhow!("{} header is missing", Self::HEADER_WORKSPACE_ROOT))?;

        let workspace_root = workspace_root
            .to_str()
            .map_err(|e| anyhow!("{} header is invalid: {e}", Self::HEADER_WORKSPACE_ROOT))?;

        let workspace_root = tokio::fs::canonicalize(workspace_root)
            .await
            .map_err(|e| anyhow!("failed to canonicalize workspace root {workspace_root}: {e}"))?;

        Ok(PathResolver::new(workspace_root)?)
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn prepare_permissions(path_resolver: &PathResolver) -> anyhow::Result<Permissions> {
        let mut permissions = Permissions::new(path_resolver.clone());

        // fs read deny
        for path in [".git/"] {
            permissions
                .register_fs_entry(PermissionsGroup::FsRead, PermissionsKind::Deny, path)
                .await?;
        }

        // fs read allow
        for path in ["./", "/Users/matrixdev/.cargo/"] {
            permissions
                .register_fs_entry(PermissionsGroup::FsRead, PermissionsKind::Allow, path)
                .await?;
        }

        // fs write deny
        for path in [".git/"] {
            permissions
                .register_fs_entry(PermissionsGroup::FsWrite, PermissionsKind::Allow, path)
                .await?;
        }

        // fs write allow
        for path in ["./"] {
            permissions
                .register_fs_entry(PermissionsGroup::FsWrite, PermissionsKind::Allow, path)
                .await?;
        }

        Ok(permissions)
    }
}
