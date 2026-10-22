//! Model Context Protocol server exposing the CLI's session workflow as typed tools.

use crate::cli::{
    command::{root, Cli},
    dispatch::dispatch,
    output::{report_message, ResponseMode, ResponseWriter},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::FromArgMatches as _;
use miette::{IntoDiagnostic as _, Result};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData, ServiceExt as _,
};
use serde_json::Value;
use std::{env, iter, process::ExitCode, result::Result as StdResult};
use tokio::{runtime::Builder, task::spawn_blocking};
use tools::{
    AnalyzeInput, BundleInput, Invocation, ScreenInput, ScreenshotInput, SendInput, StartInput,
    StatusInput, StopInput, WaitInput,
};

mod tools;

const INSTRUCTIONS: &str = "Termtrace records terminal programs and lets you drive them. \
Call `start` to launch a program in a recorded terminal, `send` to type keys and wait for the \
screen to react, `screen` or `wait` to read it, and `stop` when done. Completed sessions \
support `analyze`, `screenshot`, and `bundle`. Sessions are shared with the `termtrace` CLI \
by name.";

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
        description = "Launch a command in a new recorded background terminal session. Returns the session name to use in other tools."
    )]
    async fn start(
        &self,
        Parameters(input): Parameters<StartInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(move || return input.invocation(), false).await;
    }

    #[tool(
        description = "Send keys, text, clicks, resizes, or signals to a live session, optionally wait for a condition. Returns the resulting state hash, plus changed rows with `screen: diff`; call `screen` for the full text."
    )]
    async fn send(
        &self,
        Parameters(input): Parameters<SendInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(
            move || return input.arguments().map(Invocation::from),
            false,
        )
        .await;
    }

    #[tool(description = "Read the latest presented screen of a session, or an earlier state.")]
    async fn screen(
        &self,
        Parameters(input): Parameters<ScreenInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(move || return Ok(input.arguments().into()), false).await;
    }

    #[tool(
        description = "Wait until a live session's screen or output meets a condition, then return the screen."
    )]
    async fn wait(
        &self,
        Parameters(input): Parameters<WaitInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(move || return Ok(input.arguments().into()), false).await;
    }

    #[tool(description = "Show one session's status, or list all sessions.")]
    async fn status(
        &self,
        Parameters(input): Parameters<StatusInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(move || return Ok(input.arguments().into()), false).await;
    }

    #[tool(description = "Stop a live session and wait until its recording is complete.")]
    async fn stop(
        &self,
        Parameters(input): Parameters<StopInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(move || return Ok(input.arguments().into()), false).await;
    }

    #[tool(
        description = "Analyze a completed session: last-action triage, timeline, state diff, explain, diagnostics, integrity verification, or output text."
    )]
    async fn analyze(
        &self,
        Parameters(input): Parameters<AnalyzeInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(
            move || return input.arguments().map(Invocation::from),
            false,
        )
        .await;
    }

    #[tool(description = "Capture a PNG screenshot of a session and return the image.")]
    async fn screenshot(
        &self,
        Parameters(input): Parameters<ScreenshotInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(move || return Ok(input.arguments().into()), true).await;
    }

    #[tool(description = "Write an evidence bundle for a completed session.")]
    async fn bundle(
        &self,
        Parameters(input): Parameters<BundleInput>,
    ) -> StdResult<CallToolResult, ErrorData> {
        return call(move || return Ok(input.arguments().into()), false).await;
    }
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        return ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("termtrace", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS);
    }
}

async fn call(
    invocation: impl FnOnce() -> StdResult<Invocation, String> + Send + 'static,
    image: bool,
) -> StdResult<CallToolResult, ErrorData> {
    return spawn_blocking(move || {
        return match invocation() {
            Ok(invocation) => {
                let result = run(invocation.arguments, image);
                drop(invocation.environment);

                result
            }
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        };
    })
    .await
    .map_err(|error| return ErrorData::internal_error(error.to_string(), None));
}

fn run(arguments: Vec<String>, image: bool) -> CallToolResult {
    // Tools spawn and attach through this executable's path, which a reinstall replaces.
    match env::current_exe() {
        Ok(executable) if executable.exists() => {}
        Ok(executable) => {
            return CallToolResult::error(vec![ContentBlock::text(format!(
                "{} was replaced or removed after this MCP server started; restart the MCP server",
                executable
                    .display()
                    .to_string()
                    .trim_end_matches(" (deleted)")
            ))]);
        }
        Err(error) => return CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    }

    let matches = match root()
        .try_get_matches_from(iter::once("termtrace".to_owned()).chain(arguments))
    {
        Ok(matches) => matches,
        Err(error) => return CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    };

    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => return CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    };

    if let Err(error) = cli.validate() {
        return CallToolResult::error(vec![ContentBlock::text(error.to_string())]);
    }

    let mut response = ResponseWriter::new(ResponseMode::Capture);
    let succeeded = match dispatch(&mut response, cli.command) {
        Ok(code) => code == ExitCode::SUCCESS,
        Err(error) => {
            return CallToolResult::error(vec![ContentBlock::text(report_message(&error))]);
        }
    };
    let output = response.into_captured();
    let text = String::from_utf8_lossy(&output).into_owned();
    let structured = serde_json::from_slice::<Value>(&output)
        .ok()
        .filter(Value::is_object);

    let mut content = vec![ContentBlock::text(text)];
    if image && succeeded {
        match structured.as_ref().map(screenshot_image) {
            Some(Ok(block)) => content.push(block),
            Some(Err(message)) => return CallToolResult::error(vec![ContentBlock::text(message)]),
            None => {}
        }
    }

    let mut result = if succeeded {
        CallToolResult::success(content)
    } else {
        CallToolResult::error(content)
    };
    result.structured_content = structured;

    return result;
}

fn screenshot_image(output: &Value) -> StdResult<ContentBlock, String> {
    let path = output
        .get("image_display")
        .or_else(|| return output.get("image"))
        .and_then(Value::as_str)
        .ok_or_else(|| return "screenshot output has no image path".to_owned())?;

    let bytes = fs_err::read(path).map_err(|error| return error.to_string())?;

    return Ok(ContentBlock::image(STANDARD.encode(bytes), "image/png"));
}
