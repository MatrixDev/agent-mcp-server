use rmcp::model::ListRootsResult;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::{
    handler::server::wrapper::Parameters, model::{InitializeRequestParams, InitializeResult}, schemars::{self, JsonSchema}, service::RequestContext, tool,
    tool_handler,
    tool_router,
    transport::streamable_http_server::{session::local::LocalSessionManager, StreamableHttpService},
    ErrorData,
    RoleServer, ServerHandler, ServiceError,
    ServiceExt,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::process::Command;
use tracing::{error, info, instrument};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Deserialize, JsonSchema)]
struct CargoRunnerArgs {
    /// cargo project root directory
    project_dir: PathBuf,
    /// extra arguments added to cargo subcommand
    arguments: Vec<String>,
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

        info!("{request:?}");

        let Some(request_parts) = context.extensions.get::<http::request::Parts>() else {
            let message = "failed to parse request http parts";
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        info!("{request_parts:?}");

        let peer = context.peer.clone();
        tokio::spawn(async move {
            error!("calling list_roots");
            match peer.list_roots().await {
                Ok(e) => {
                    error!("list_roots = {e:#?}");
                }
                Err(e) => {
                    error!("list_roots = {e:?}");
                }
            };
        });

        if "".is_empty() {
            return Ok(self.get_info());
        }

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
            let message = "failed to canonicalize workspace dir";
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

    async fn run(&self, subcommand: &str, args: &CargoRunnerArgs) -> Result<String, ErrorData> {
        info!(subcommand, ?args, "run");

        let project_dir = self.resolve_workspace_dir(&args.project_dir).await?;
        let output = Command::new("cargo")
            .arg(subcommand)
            .args(&args.arguments)
            .current_dir(project_dir)
            .output()
            .await
            .map_err(|e| {
                let message = format!("failed to execute cargo: {e}");
                error!("{message}");
                ErrorData::internal_error(message, None)
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(format!("STDOUT:\n{stdout}\n\nSTDERR:\n{stderr}"))
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
    #[tool(description = "Runs `cargo fetch` command directly without a terminal shell")]
    #[instrument(skip_all, "fetch")]
    async fn fetch(&self, parameters: Parameters<CargoRunnerArgs>) -> Result<String, ErrorData> {
        self.run("fetch", &parameters.0).await
    }

    #[tool(description = "Runs `cargo build` command directly without a terminal shell")]
    #[instrument(skip_all, "build")]
    async fn build(&self, parameters: Parameters<CargoRunnerArgs>) -> Result<String, ErrorData> {
        self.run("build", &parameters.0).await
    }

    #[tool(description = "Runs `cargo test` command directly without a terminal shell")]
    #[instrument(skip_all, "test")]
    async fn test(&self, parameters: Parameters<CargoRunnerArgs>) -> Result<String, ErrorData> {
        self.run("test", &parameters.0).await
    }

    #[tool(description = "Runs `cargo check` command directly without a terminal shell")]
    #[instrument(skip_all, "check")]
    async fn check(&self, parameters: Parameters<CargoRunnerArgs>) -> Result<String, ErrorData> {
        self.run("check", &parameters.0).await
    }

    #[tool(description = "Runs `cargo clippy` command directly without a terminal shell")]
    #[instrument(skip_all, "clippy")]
    async fn clippy(&self, parameters: Parameters<CargoRunnerArgs>) -> Result<String, ErrorData> {
        self.run("clippy", &parameters.0).await
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_span_events(FmtSpan::CLOSE);

    tracing_subscriber::registry().with(fmt_layer).init();

    if std::env::args().nth(1).as_deref() == Some("stdio") {
        serve_stdio().await?;
    } else {
        serve_http().await?;
    }
    Ok(())
}

#[instrument(skip_all, "serve_stdio")]
async fn serve_stdio() -> anyhow::Result<()> {
    let service = CargoRunner::default()
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await?;
    service.waiting().await?;
    Ok(())
}

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
