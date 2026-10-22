use crate::support::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use jsonschema::meta::is_valid;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, ContentBlock},
    service::RunningService,
    transport::TokioChildProcess,
    RoleClient, ServiceExt as _,
};
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;

const BINARY: &str = env!("CARGO_BIN_EXE_termtrace");
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const WAITING_SHELL: &str = "trap 'exit 0' TERM; printf 'name? '; read n; printf 'hello %s\\n' \"$n\"; while :; do sleep 0.1; done";

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
async fn connect(state: &Path) -> RunningService<RoleClient, ()> {
    return connect_with(Path::new(BINARY), state).await;
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
async fn connect_with(binary: &Path, state: &Path) -> RunningService<RoleClient, ()> {
    let mut command = TokioCommand::new(binary);
    command.arg("mcp").env("TERMTRACE_STATE_DIR", state);
    let transport = TokioChildProcess::new(command).expect("spawn MCP server");

    return ().serve(transport).await.expect("MCP client connects");
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
async fn call(
    client: &RunningService<RoleClient, ()>,
    tool: &'static str,
    arguments: Value,
) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("tool arguments must be an object");
    };

    return client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .expect("tool call completes");
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
fn structured(result: &CallToolResult) -> &Value {
    return result
        .structured_content
        .as_ref()
        .unwrap_or_else(|| panic!("structured content: {result:?}"));
}

fn failed(result: &CallToolResult) -> bool {
    return result.is_error == Some(true);
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
fn png(result: &CallToolResult) -> Vec<u8> {
    let image = result
        .content
        .iter()
        .find_map(|block| {
            return match block {
                ContentBlock::Image(image) => Some(image),
                _ => None,
            };
        })
        .unwrap_or_else(|| panic!("image content: {result:?}"));
    assert_eq!(image.mime_type, "image/png");

    return STANDARD.decode(&image.data).expect("base64 image");
}

async fn start_waiting_shell(client: &RunningService<RoleClient, ()>, name: &str) {
    let started = call(
        client,
        "start",
        json!({
            "command": ["/bin/sh", "-c", WAITING_SHELL],
            "name": name,
            "cols": 40,
            "rows": 6,
            "until": {"kind": "screen", "text": "name?"},
            "timeout_ms": 5000,
        }),
    )
    .await;
    assert!(!failed(&started), "{started:?}");
    assert_eq!(structured(&started)["alias"], name);
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
fn cli_screen(state: &Path, name: &str) -> Value {
    let output = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("TERMTRACE_STATE_DIR", state)
        .args(["screen", name, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    return serde_json::from_slice(&output).expect("screen JSON");
}

#[tokio::test]
async fn tools_cover_the_session_workflow_with_valid_input_schemas() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;

    let tools = client.list_all_tools().await.expect("list tools");
    let names = tools
        .iter()
        .map(|tool| return tool.name.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        [
            "analyze",
            "bundle",
            "screen",
            "screenshot",
            "send",
            "start",
            "status",
            "stop",
            "wait"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );

    for tool in &tools {
        let schema = Value::Object((*tool.input_schema).clone());
        assert!(is_valid(&schema), "{}", tool.name);
    }

    client.cancel().await.expect("close client");
}

#[tokio::test]
async fn sessions_started_over_mcp_are_driven_by_the_cli_and_outlive_the_server() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let client = connect(&state).await;
    start_waiting_shell(&client, "shared").await;

    let sent = call(
        &client,
        "send",
        json!({
            "session": "shared",
            "inputs": ["text:world", "enter"],
            "until": {"kind": "screen", "text": "hello world"},
            "timeout_ms": 5000,
        }),
    )
    .await;
    assert!(!failed(&sent), "{sent:?}");

    client.cancel().await.expect("close client");

    let screen = cli_screen(&state, "shared");
    assert_eq!(screen["lifecycle"], "live");
    assert!(
        screen["screen"]
            .as_str()
            .is_some_and(|text| return text.contains("hello world")),
        "{screen}"
    );

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "shared", "--wait"])
        .assert()
        .success();
}

#[tokio::test]
async fn stale_state_guard_rejects_input_without_applying_it() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let client = connect(&state).await;
    start_waiting_shell(&client, "guarded").await;
    let before = cli_screen(&state, "guarded");

    let rejected = call(
        &client,
        "send",
        json!({"session": "guarded", "inputs": ["text:late"], "if_state": "0".repeat(64)}),
    )
    .await;
    assert!(failed(&rejected));
    assert_eq!(structured(&rejected)["code"], "stale_observation");
    assert_eq!(
        structured(&rejected)["input"]["effect"]["kind"],
        "not_applied"
    );
    assert_eq!(cli_screen(&state, "guarded")["state"], before["state"]);

    let stopped = call(&client, "stop", json!({"session": "guarded", "wait": true})).await;
    assert!(!failed(&stopped), "{stopped:?}");
    client.cancel().await.expect("close client");
}

#[tokio::test]
async fn failed_calls_return_tool_errors_and_leave_the_server_usable() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "patient").await;

    let deadline = call(
        &client,
        "wait",
        json!({"session": "patient", "for": {"kind": "screen", "text": "never"}, "timeout_ms": 200}),
    )
    .await;
    assert!(failed(&deadline));
    assert_eq!(structured(&deadline)["quality"], "deadline");

    let missing = call(&client, "status", json!({"session": "missing"})).await;
    assert!(failed(&missing));

    let protocol_input = call(
        &client,
        "send",
        json!({"session": "patient", "inputs": ["-"]}),
    )
    .await;
    assert!(failed(&protocol_input));

    let screen = call(&client, "screen", json!({"session": "patient"})).await;
    assert!(!failed(&screen), "{screen:?}");
    assert_eq!(structured(&screen)["screen"], "name?");

    let stopped = call(&client, "stop", json!({"session": "patient", "wait": true})).await;
    assert!(!failed(&stopped), "{stopped:?}");
    client.cancel().await.expect("close client");
}

#[tokio::test]
async fn screenshot_of_a_completed_session_returns_png_image_content() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "pictured").await;
    let stopped = call(
        &client,
        "stop",
        json!({"session": "pictured", "wait": true}),
    )
    .await;
    assert!(!failed(&stopped), "{stopped:?}");

    let screenshot = call(
        &client,
        "screenshot",
        json!({"session": "pictured", "out": temporary.path().join("shot")}),
    )
    .await;
    assert!(!failed(&screenshot), "{screenshot:?}");

    assert!(png(&screenshot).starts_with(PNG_SIGNATURE));

    client.cancel().await.expect("close client");
}

#[test]
fn handshake_clients_are_served_until_standard_input_closes() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut server = ProcessCommand::new(BINARY)
        .arg("mcp")
        .env("TERMTRACE_STATE_DIR", temporary.path().join("state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn MCP server");
    let mut input = server.stdin.take().expect("server stdin");

    for message in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"},
        }}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ] {
        writeln!(input, "{message}").expect("write request");
    }

    let mut output = BufReader::new(server.stdout.take().expect("server stdout"));
    let mut tools = None;

    for _ in 0..2 {
        let mut line = String::new();
        output.read_line(&mut line).expect("read response");
        let response: Value = serde_json::from_str(&line).expect("JSON-RPC response");

        if response["id"] == 2 {
            tools = response["result"]["tools"].as_array().map(Vec::len);
        }
    }

    assert_eq!(tools, Some(9));

    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = server.try_wait().expect("poll server") {
            break status;
        }

        assert!(Instant::now() < deadline, "server kept running after EOF");

        thread::sleep(Duration::from_millis(20));
    };

    assert!(status.success(), "{status:?}");
}

#[tokio::test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick and FFprobe"]
async fn native_screenshot_leaves_no_native_processes_behind_the_server() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "native").await;
    let stopped = call(&client, "stop", json!({"session": "native", "wait": true})).await;
    assert!(!failed(&stopped), "{stopped:?}");

    let destination = temporary.path().join("native");
    let screenshot = call(
        &client,
        "screenshot",
        json!({"session": "native", "out": destination, "renderer": "native"}),
    )
    .await;
    assert!(!failed(&screenshot), "{screenshot:?}");
    assert!(png(&screenshot).starts_with(PNG_SIGNATURE));

    let resources: Value = serde_json::from_slice(
        &fs::read(destination.join("native-resources.json")).expect("resources"),
    )
    .expect("resources JSON");
    let terminal = resources["terminal_pid"].as_u64().expect("terminal PID");
    assert!(!Path::new(&format!("/proc/{terminal}")).exists());

    let status = call(&client, "status", json!({})).await;
    assert!(!failed(&status), "{status:?}");
    client.cancel().await.expect("close client");
}

#[tokio::test]
async fn reinstalled_binary_asks_for_a_server_restart() {
    let temporary = TempDir::new().expect("temporary directory");
    let binary = temporary.path().join("termtrace");
    fs::copy(BINARY, &binary).expect("copy binary");
    let client = connect_with(&binary, &temporary.path().join("state")).await;

    fs::remove_file(&binary).expect("remove running binary");
    fs::copy(BINARY, &binary).expect("install replacement");

    let started = call(
        &client,
        "start",
        json!({"command": ["/bin/sh", "-c", "sleep 5"], "name": "replaced"}),
    )
    .await;
    assert!(failed(&started));
    let ContentBlock::Text(message) = &started.content[0] else {
        panic!("text error: {started:?}");
    };
    assert!(
        message.text.contains("restart the MCP server"),
        "{}",
        message.text
    );

    client.cancel().await.expect("close client");
}

#[tokio::test]
async fn start_passes_environment_variables_to_the_command() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;

    let started = call(
        &client,
        "start",
        json!({
            "command": ["/bin/sh", "-c", "printf 'value=%s' \"$GREETING\"; while :; do sleep 0.1; done"],
            "name": "environment",
            "env": {"GREETING": "hello there"},
            "until": {"kind": "screen", "text": "value=hello there"},
            "timeout_ms": 5000,
        }),
    )
    .await;
    assert!(!failed(&started), "{started:?}");
    assert_eq!(structured(&started)["postcondition"]["matched"], true);

    let stopped = call(
        &client,
        "stop",
        json!({"session": "environment", "wait": true}),
    )
    .await;
    assert!(!failed(&stopped), "{stopped:?}");
    client.cancel().await.expect("close client");
}
