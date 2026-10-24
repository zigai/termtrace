//! Model Context Protocol server exposing the CLI's session workflow as typed tools and resources.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use execute::{execute, Failure};
use miette::{IntoDiagnostic as _, Result};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock, Implementation, ListResourceTemplatesResult,
        ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResponse, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData, RoleServer, ServiceExt as _,
};
use serde_json::Value;
use std::{process::ExitCode, result::Result as StdResult};
use tokio::runtime::Builder;
use tools::{
    AnalyzeInput, BundleInput, Invocation, ScreenInput, ScreenshotInput, SendInput, StartInput,
    StatusInput, StopInput, Structure, WaitInput,
};

mod execute;
mod outputs;
mod resources;
mod tools;

const INSTRUCTIONS: &str = "Termtrace records terminal programs and lets you drive them. \
Call `start` to launch a program in a recorded terminal, `send` to type keys and wait for the \
screen to react, `screen` or `wait` to read it, and `stop` when done. Completed sessions \
support `analyze`, `screenshot`, and `bundle`. Sessions are shared with the `termtrace` CLI \
by name, and each one is also readable as `termtrace://sessions/{session}` resources.";

pub(in crate::cli) fn run_mcp() -> Result<ExitCode> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;

    return runtime.block_on(serve());
}

async fn serve() -> Result<ExitCode> {
    let service = Server::new()
        .serve(stdio())
        .await
        .map_err(|error| return miette::miette!("MCP initialization failed: {error}"))?;

    service.waiting().await.into_diagnostic()?;

    return Ok(ExitCode::SUCCESS);
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    fn new() -> Self {
        return Self {
            tool_router: Self::tool_router(),
        };
    }

    #[tool(
        title = "Start session",
        description = "Launch a command in a new recorded background terminal session. Returns the session name to use in other tools.",
        output_schema = outputs::start(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn start(
        &self,
        Parameters(input): Parameters<StartInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(input.invocation(), &context).await;
    }

    #[tool(
        title = "Send input",
        description = "Send keys, text, clicks, resizes, or signals to a live session, optionally wait for a condition. Returns the resulting state hash, plus changed rows with `screen: diff`; call `screen` for the full text.",
        output_schema = outputs::send(),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn send(
        &self,
        Parameters(input): Parameters<SendInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(input.invocation(), &context).await;
    }

    #[tool(
        title = "Read screen",
        description = "Read the latest presented screen of a session, or an earlier state.",
        output_schema = outputs::screen(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn screen(
        &self,
        Parameters(input): Parameters<ScreenInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(Ok(input.invocation()), &context).await;
    }

    #[tool(
        title = "Wait for condition",
        description = "Wait until a live session's screen or output meets a condition, then return the screen.",
        output_schema = outputs::wait(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn wait(
        &self,
        Parameters(input): Parameters<WaitInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(Ok(input.invocation()), &context).await;
    }

    #[tool(
        title = "Session status",
        description = "Show one session's status, or list all sessions.",
        output_schema = outputs::status(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn status(
        &self,
        Parameters(input): Parameters<StatusInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(Ok(input.invocation()), &context).await;
    }

    #[tool(
        title = "Stop session",
        description = "Stop a live session and wait until its recording is complete.",
        output_schema = outputs::stop(),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn stop(
        &self,
        Parameters(input): Parameters<StopInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(Ok(input.invocation()), &context).await;
    }

    #[tool(
        title = "Analyze recording",
        description = "Analyze a completed session: last-action triage, timeline, state diff, explain, diagnostics, integrity verification, or output text.",
        output_schema = outputs::analyze(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn analyze(
        &self,
        Parameters(input): Parameters<AnalyzeInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(input.invocation(), &context).await;
    }

    #[tool(
        title = "Capture screenshot",
        description = "Capture a PNG screenshot of a session and return the image.",
        output_schema = outputs::screenshot(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn screenshot(
        &self,
        Parameters(input): Parameters<ScreenshotInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(Ok(input.invocation()), &context).await;
    }

    #[tool(
        title = "Write evidence bundle",
        description = "Write an evidence bundle for a completed session.",
        output_schema = outputs::bundle(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn bundle(
        &self,
        Parameters(input): Parameters<BundleInput>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(Ok(input.invocation()), &context).await;
    }
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        return ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("termtrace", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS);
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> StdResult<ListResourcesResult, ErrorData> {
        return resources::list(&context).await;
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> StdResult<ListResourceTemplatesResult, ErrorData> {
        return Ok(resources::templates());
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> StdResult<ReadResourceResponse, ErrorData> {
        return resources::read(&request.uri, &context)
            .await
            .map(ReadResourceResponse::from);
    }
}

async fn call(
    invocation: StdResult<Invocation, String>,
    context: &RequestContext<RoleServer>,
) -> StdResult<CallToolResult, ErrorData> {
    let invocation = match invocation {
        Ok(invocation) => invocation,
        Err(message) => return Ok(CallToolResult::error(vec![ContentBlock::text(message)])),
    };

    let completed = match execute(&invocation.arguments, context).await {
        Ok(completed) => completed,
        Err(Failure::Cancelled) => {
            return Err(ErrorData::internal_error("request cancelled", None));
        }
        Err(Failure::Unavailable(message)) => {
            return Ok(CallToolResult::error(vec![ContentBlock::text(message)]));
        }
    };

    drop(invocation.environment);

    let text = String::from_utf8_lossy(&completed.stdout).into_owned();
    let Some(structured) = invocation.structure.parse(&completed.stdout) else {
        let message = if completed.stderr.is_empty() { text } else { completed.stderr };

        return Ok(CallToolResult::error(vec![ContentBlock::text(message)]));
    };

    let mut content = vec![ContentBlock::text(text)];
    if invocation.structure == Structure::Screenshot && completed.succeeded {
        match screenshot_png(&structured) {
            Ok(png) => content.push(ContentBlock::image(STANDARD.encode(png), "image/png")),
            Err(message) => return Ok(CallToolResult::error(vec![ContentBlock::text(message)])),
        }
    }

    let mut result = if completed.succeeded {
        CallToolResult::success(content)
    } else {
        CallToolResult::error(content)
    };
    result.structured_content = Some(structured);

    return Ok(result);
}

fn screenshot_png(output: &Value) -> StdResult<Vec<u8>, String> {
    let path = output
        .get("image_display")
        .or_else(|| return output.get("image"))
        .and_then(Value::as_str)
        .ok_or_else(|| return "screenshot output has no image path".to_owned())?;

    return fs_err::read(path).map_err(|error| return error.to_string());
}
