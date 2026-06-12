mod tools;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{InitializeRequestParams, InitializeResult};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::{tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler, ServiceExt};
use tracing::{error, info, instrument};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::tools::cargo_exec::CargoRunTool;
use crate::tools::directory_list::DirectoryListTool;
use crate::tools::directory_make::DirectoryMakeTool;
use crate::tools::file_edit::FileEditTool;
use crate::tools::file_move::FileMoveTool;
use crate::tools::file_read::FileReadTool;
use crate::tools::file_write::FileWriteTool;

////////////////////////////////////////////////////////////////////////////////
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().with_span_events(FmtSpan::CLOSE))
        .init();

    if std::env::args().nth(1).as_deref() == Some("stdio") {
        serve_stdio().await?;
    } else {
        serve_http().await?;
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, "serve_stdio")]
async fn serve_stdio() -> anyhow::Result<()> {
    let service = CargoRunner::default()
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await?;
    service.waiting().await?;
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, "serve_http")]
async fn serve_http() -> anyhow::Result<()> {
    let service = StreamableHttpService::new(
        || Ok::<_, std::io::Error>(CargoRunner::default()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;

    info!("streamable HTTP server listening");
    axum::serve(listener, router).await?;
    Ok(())
}

#[derive(Default, Clone)]
struct CargoRunner {
    workspace_root: OnceLock<PathBuf>,
}

impl CargoRunner {
    const HEADER_WORKSPACE_ROOT: &'static str = "x-mcp-workspace-root";

    async fn do_initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        info!("client: {} v{}", request.client_info.name, request.client_info.version);

        let Some(request_parts) = context.extensions.get::<http::request::Parts>() else {
            let message = "failed to parse request http parts";
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let Some(workspace_root) = request_parts.headers.get(Self::HEADER_WORKSPACE_ROOT) else {
            let message = format!("{} header is missing", Self::HEADER_WORKSPACE_ROOT);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let Ok(workspace_root) = workspace_root.to_str() else {
            let message = format!("{} header is invalid", Self::HEADER_WORKSPACE_ROOT);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let Ok(workspace_root) = Path::new(workspace_root).canonicalize() else {
            let message = format!("failed to canonicalize workspace root: {workspace_root}");
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        info!("working directory: {}", workspace_root.display());
        self.workspace_root.get_or_init(|| workspace_root);

        // do it dynamically? Zed doesn't support this
        // let _ = self.roots_supported.set(request.capabilities.roots.is_some());
        Ok(self.get_info())
    }

    async fn resolve_workspace_dir(&self, path: impl AsRef<Path>) -> Result<PathBuf, ErrorData> {
        let Some(workspace_root) = self.workspace_root.get() else {
            let message = "workspace root is missing";
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let Ok(path) = tokio::fs::canonicalize(path.as_ref()).await else {
            let message = format!("failed to canonicalize workspace dir: {}", path.as_ref().display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        if !path.starts_with(workspace_root) {
            let message = format!("{path:?} is not child of {workspace_root:?}");
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        Ok(path)
    }
}

#[tool_handler(router = Self::tool_router())]
impl ServerHandler for CargoRunner {
    #[instrument(skip_all, "initialize")]
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.do_initialize(request, context).await
    }
}

#[tool_router]
impl CargoRunner {
    ////////////////////////////////////////////////////////////////////////////////
    // File system
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Read a file")]
    #[instrument(skip_all, "tool/read_file")]
    pub async fn read_file(&self, args: Parameters<FileReadTool>) -> Result<String, ErrorData> {
        args.0.handle(self).await
    }

    #[tool(description = "Write a file, overwriting any existing contents")]
    #[instrument(skip_all, "tool/write_file")]
    pub async fn write_file(&self, args: Parameters<FileWriteTool>) -> Result<String, ErrorData> {
        args.0.handle(self).await
    }

    #[tool(description = "Edit a file by replacing a unique string")]
    #[instrument(skip_all, "tool/edit_file")]
    pub async fn edit_file(&self, args: Parameters<FileEditTool>) -> Result<String, ErrorData> {
        args.0.handle(self).await
    }

    #[tool(description = "Move or rename a file or directory")]
    #[instrument(skip_all, "tool/move_file")]
    pub async fn move_file(&self, args: Parameters<FileMoveTool>) -> Result<String, ErrorData> {
        args.0.handle(self).await
    }

    #[tool(description = "List the entries of a directory")]
    #[instrument(skip_all, "tool/list_directory")]
    pub async fn list_directory(&self, args: Parameters<DirectoryListTool>) -> Result<String, ErrorData> {
        args.0.handle(self).await
    }

    #[tool(description = "Create a directory, including parents")]
    #[instrument(skip_all, "tool/make_directory")]
    pub async fn make_directory(&self, args: Parameters<DirectoryMakeTool>) -> Result<String, ErrorData> {
        args.0.handle(self).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Cargo
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Runs `cargo fetch` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_fetch")]
    async fn cargo_fetch(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        args.0.handle(self, "fetch").await
    }

    #[tool(description = "Runs `cargo build` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_build")]
    async fn cargo_build(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        args.0.handle(self, "build").await
    }

    #[tool(description = "Runs `cargo test` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_test")]
    async fn cargo_test(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        args.0.handle(self, "test").await
    }

    #[tool(description = "Runs `cargo check` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_check")]
    async fn cargo_check(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        args.0.handle(self, "check").await
    }

    #[tool(description = "Runs `cargo clippy` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_clippy")]
    async fn cargo_clippy(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        args.0.handle(self, "clippy").await
    }
}
