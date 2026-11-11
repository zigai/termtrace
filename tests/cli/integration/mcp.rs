use crate::support::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use jsonschema::meta::is_valid;
use rmcp::{
    model::{
        CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, ContentBlock,
        ProgressNotificationParam, ReadResourceRequestParams, ResourceContents,
    },
    service::{NotificationContext, PeerRequestOptions, RunningService},
    transport::TokioChildProcess,
    ClientHandler, RoleClient, ServiceExt as _,
};
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};
use tokio::{process::Command as TokioCommand, time::sleep};

const BINARY: &str = env!("CARGO_BIN_EXE_termtrace");
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const WAITING_SHELL: &str = "trap 'exit 0' TERM; printf 'name? '; read n; printf 'hello %s\\n' \"$n\"; while :; do sleep 0.1; done";

type Connection = RunningService<RoleClient, Client>;

#[derive(Clone, Default)]
struct Client {
    progress: Arc<Mutex<Vec<ProgressNotificationParam>>>,
}

impl Client {
    #[allow(
        clippy::expect_used,
        reason = "a poisoned progress log means another test assertion already failed"
    )]
    fn progress(&self) -> Vec<ProgressNotificationParam> {
        return self.progress.lock().expect("progress log").clone();
    }
}

impl ClientHandler for Client {
    #[allow(
        clippy::expect_used,
        reason = "a poisoned progress log means another test assertion already failed"
    )]
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.progress.lock().expect("progress log").push(params);
    }
}

async fn connect(state: &Path) -> Connection {
    return connect_with(Path::new(BINARY), state).await;
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
async fn connect_with(binary: &Path, state: &Path) -> Connection {
    let mut command = TokioCommand::new(binary);
    command.arg("mcp").env("TERMTRACE_STATE_DIR", state);
    let transport = TokioChildProcess::new(command).expect("spawn MCP server");
    return Client::default()
        .serve(transport)
        .await
        .expect("MCP client connects");
}

#[allow(
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
fn arguments(value: Value) -> Map<String, Value> {
    let Value::Object(arguments) = value else {
        panic!("tool arguments must be an object");
    };
    return arguments;
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
async fn call(client: &Connection, tool: &'static str, input: Value) -> CallToolResult {
    let result = client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments(input)))
        .await
        .expect("tool call completes");

    let Some(structured) = &result.structured_content else {
        assert!(
            failed(&result),
            "{tool} succeeded without structured content"
        );

        return result;
    };

    let tools = client.list_all_tools().await.expect("list tools");
    let schema = tools
        .iter()
        .find(|candidate| return candidate.name == tool)
        .and_then(|candidate| return candidate.output_schema.clone())
        .unwrap_or_else(|| panic!("{tool} declares no output schema"));

    let validator =
        jsonschema::validator_for(&Value::Object((*schema).clone())).expect("output schema");

    let violations = validator
        .iter_errors(structured)
        .map(|error| return error.to_string())
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "{tool} output violates its schema: {violations:?}\n{structured}"
    );

    return result;
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

async fn start_waiting_shell(client: &Connection, name: &str) {
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
async fn tools_cover_the_session_workflow_with_valid_schemas() {
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
        let input = Value::Object((*tool.input_schema).clone());
        assert!(is_valid(&input), "{}", tool.name);

        let output = tool
            .output_schema
            .as_ref()
            .map(|schema| return Value::Object((**schema).clone()));
        assert!(output.as_ref().is_some_and(is_valid), "{}", tool.name);
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
async fn send_ignores_typed_matches_and_returns_the_deadline_screen() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "echo").await;

    let sent = call(
        &client,
        "send",
        json!({
            "session": "echo",
            "inputs": ["text:COMPLETE"],
            "until": {"kind": "screen_regex", "pattern": "COMPLETE"},
            "timeout_ms": 100,
        }),
    )
    .await;
    let marked = call(
        &client,
        "send",
        json!({
            "session": "echo", "inputs": ["paste:RESULT", "mark:submitted"],
            "until": {"kind": "screen", "text": "RESULT"}, "timeout_ms": 100,
        }),
    )
    .await;
    let reply = call(
        &client,
        "send",
        json!({
            "session": "echo", "inputs": ["enter"],
            "until": {"kind": "screen", "text": "hello COMPLETERESULT"}, "timeout_ms": 1000,
            "settle_ms": 100,
        }),
    )
    .await;
    let stopped = call(&client, "stop", json!({"session": "echo", "wait": true})).await;
    client.cancel().await.expect("close client");

    assert!(!failed(&stopped), "{stopped:?}");
    assert!(failed(&sent), "{sent:?}");
    assert_eq!(structured(&sent)["postcondition"]["outcome"], "deadline");
    assert_eq!(structured(&sent)["screen"], "name? COMPLETE");
    assert!(failed(&marked), "{marked:?}");
    assert_eq!(structured(&marked)["postcondition"]["outcome"], "deadline");
    assert_eq!(structured(&marked)["screen"], "name? COMPLETERESULT");
    assert!(!failed(&reply), "{reply:?}");
    assert_eq!(structured(&reply)["postcondition"]["matched_row"], 1);
    assert_eq!(
        structured(&reply)["postcondition"]["matched_text"],
        "hello COMPLETERESULT"
    );
    assert_eq!(structured(&reply)["postcondition"]["outcome"], "matched");
}

#[tokio::test]
async fn wrapped_typed_matches_do_not_satisfy_a_send() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    let started = call(
        &client,
        "start",
        json!({
            "name": "wrapped", "command": ["/bin/sh", "-c", WAITING_SHELL],
            "cols": 12, "rows": 6, "until": {"kind": "screen", "text": "name?"}, "timeout_ms": 1000,
        }),
    )
    .await;
    let sent = call(
        &client,
        "send",
        json!({
            "session": "wrapped", "inputs": ["text:prefix WRAPPEDTARGET end"],
            "until": {"kind": "screen", "text": "WRAPPEDTARGET"}, "timeout_ms": 100,
        }),
    )
    .await;
    let spaced = call(
        &client,
        "send",
        json!({
            "session": "wrapped", "inputs": ["text:hello world"],
            "until": {"kind": "screen_regex", "pattern": "helloworld"}, "timeout_ms": 100,
        }),
    )
    .await;
    let separated = call(
        &client,
        "send",
        json!({
            "session": "wrapped", "inputs": ["enter", "text:world", "enter", "text:world"],
            "until": {"kind": "screen_regex", "pattern": "(?m)^world$"}, "timeout_ms": 100,
        }),
    )
    .await;
    let stopped = call(&client, "stop", json!({"session": "wrapped", "wait": true})).await;
    client.cancel().await.expect("close client");

    assert!(!failed(&started), "{started:?}");
    assert!(!failed(&stopped), "{stopped:?}");
    assert!(failed(&sent), "{sent:?}");
    assert_eq!(structured(&sent)["postcondition"]["outcome"], "deadline");
    assert_eq!(
        structured(&sent)["screen"]
            .as_str()
            .expect("screen")
            .replace('\n', ""),
        "name? prefix WRAPPEDTARGET end"
    );
    assert!(failed(&spaced), "{spaced:?}");
    assert_eq!(structured(&spaced)["postcondition"]["outcome"], "deadline");
    assert!(failed(&separated), "{separated:?}");
    assert_eq!(
        structured(&separated)["postcondition"]["outcome"],
        "deadline"
    );
}

#[tokio::test]
async fn pending_sends_allow_reads_and_are_preempted_by_stop() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "pending").await;
    let request = CallToolRequestParams::new("send").with_arguments(arguments(json!({
        "session": "pending", "inputs": ["text:hello"],
        "until": {"kind": "screen", "text": "unreachable"}, "timeout_ms": 20_000,
    })));
    let handle = client
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(request)),
            PeerRequestOptions::no_options(),
        )
        .await
        .expect("send input");

    let waited = call(&client, "wait", json!({"session": "pending", "for": {"kind": "screen", "text": "hello"}, "timeout_ms": 1000})).await;
    let screen = call(&client, "screen", json!({"session": "pending"})).await;
    let status = call(&client, "status", json!({"session": "pending"})).await;
    let stopped = call(&client, "stop", json!({"session": "pending", "wait": true})).await;
    let sent = handle
        .await_response()
        .await
        .expect("interrupted send response");
    let sent = serde_json::to_value(sent).expect("send JSON");
    client.cancel().await.expect("close client");

    assert!(!failed(&waited), "{waited:?}");
    assert_eq!(structured(&screen)["screen"], "name? hello");
    assert_eq!(structured(&status)["lifecycle"], "live");
    assert!(!failed(&stopped), "{stopped:?}");
    assert_eq!(sent["structuredContent"]["code"], "action_stopped");
    assert_eq!(
        sent["structuredContent"]["input"]["effect"]["kind"],
        "applied"
    );
}

#[tokio::test]
async fn cancelling_a_send_releases_its_pending_postcondition() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "cancel-send").await;
    let request = CallToolRequestParams::new("send").with_arguments(arguments(json!({
        "session": "cancel-send", "inputs": ["text:world"],
        "until": {"kind": "screen", "text": "unreachable"}, "timeout_ms": 20_000,
    })));
    let handle = client
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(request)),
            PeerRequestOptions::no_options(),
        )
        .await
        .expect("send input");
    let waited = call(&client, "wait", json!({"session": "cancel-send", "for": {"kind": "screen", "text": "world"}, "timeout_ms": 1000})).await;
    handle.cancel(None).await.expect("cancel send");
    let screen = call(
        &client,
        "screen",
        json!({"session": "cancel-send", "stable_for_ms": 100, "timeout_ms": 1000}),
    )
    .await;
    let submitted = call(&client, "send", json!({"session": "cancel-send", "inputs": ["enter"], "until": {"kind": "screen", "text": "hello world"}, "timeout_ms": 1000})).await;
    let stopped = call(
        &client,
        "stop",
        json!({"session": "cancel-send", "wait": true}),
    )
    .await;
    client.cancel().await.expect("close client");

    assert!(!failed(&waited), "{waited:?}");
    assert!(!failed(&screen), "{screen:?}");
    assert!(!failed(&submitted), "{submitted:?}");
    assert!(!failed(&stopped), "{stopped:?}");
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

fn waiting_on(session: &str) -> bool {
    let Ok(processes) = fs::read_dir("/proc") else {
        return false;
    };

    return processes.flatten().any(|process| {
        let Ok(command_line) = fs::read(process.path().join("cmdline")) else {
            return false;
        };
        let words = command_line
            .split(|byte| return *byte == 0)
            .collect::<Vec<_>>();

        return words.get(1) == Some(&b"wait".as_slice())
            && words.get(2) == Some(&session.as_bytes());
    });
}

#[tokio::test]
#[allow(
    clippy::expect_used,
    reason = "test setup failures should stop the test with context"
)]
async fn long_waits_report_progress_and_stop_when_cancelled() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "cancelled").await;

    let request = CallToolRequestParams::new("wait").with_arguments(arguments(json!({
        "session": "cancelled",
        "for": {"kind": "screen", "text": "never"},
        "timeout_ms": 60_000,
    })));
    let handle = client
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(request)),
            PeerRequestOptions::no_options(),
        )
        .await
        .expect("send wait");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !client
        .service()
        .progress()
        .iter()
        .any(|progress| return progress.progress_token == handle.progress_token)
    {
        assert!(
            Instant::now() < deadline,
            "no progress for the running wait"
        );
        sleep(Duration::from_millis(50)).await;
    }
    assert!(waiting_on("cancelled"), "the wait is not running");

    handle.cancel(None).await.expect("cancel wait");

    let deadline = Instant::now() + Duration::from_secs(10);
    while waiting_on("cancelled") {
        assert!(Instant::now() < deadline, "the cancelled wait kept running");
        sleep(Duration::from_millis(50)).await;
    }

    let screen = call(&client, "screen", json!({"session": "cancelled"})).await;
    assert!(!failed(&screen), "{screen:?}");

    let stopped = call(
        &client,
        "stop",
        json!({"session": "cancelled", "wait": true}),
    )
    .await;
    assert!(!failed(&stopped), "{stopped:?}");
    client.cancel().await.expect("close client");
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "MCP test helpers stop the test when the server or its output is unusable"
)]
async fn read(client: &Connection, uri: &str) -> ResourceContents {
    let mut result = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .unwrap_or_else(|error| panic!("read {uri}: {error}"));
    assert_eq!(result.contents.len(), 1, "{uri}");

    return result.contents.remove(0);
}

#[tokio::test]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test setup failures should stop the test with context"
)]
async fn sessions_are_readable_as_resources() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "listed").await;

    let listed = client
        .list_all_resources()
        .await
        .expect("list resources")
        .into_iter()
        .map(|resource| return resource.uri)
        .collect::<BTreeSet<_>>();
    assert!(listed.contains("termtrace://sessions/listed"), "{listed:?}");
    assert!(
        listed.contains("termtrace://sessions/listed/screen"),
        "{listed:?}"
    );

    let ResourceContents::TextResourceContents { text, .. } =
        read(&client, "termtrace://sessions/listed/screen").await
    else {
        panic!("screen is text");
    };
    assert_eq!(text, "name?");

    let ResourceContents::TextResourceContents { text, .. } =
        read(&client, "termtrace://sessions/listed").await
    else {
        panic!("status is text");
    };
    let status: Value = serde_json::from_str(&text).expect("status JSON");
    assert_eq!(status["lifecycle"], "live");

    let stopped = call(&client, "stop", json!({"session": "listed", "wait": true})).await;
    assert!(!failed(&stopped), "{stopped:?}");

    let ResourceContents::TextResourceContents { text, .. } =
        read(&client, "termtrace://sessions/listed/output").await
    else {
        panic!("output is text");
    };
    assert!(text.contains("name?"), "{text}");

    let ResourceContents::BlobResourceContents {
        blob, mime_type, ..
    } = read(&client, "termtrace://sessions/listed/screenshot").await
    else {
        panic!("screenshot is a blob");
    };
    assert_eq!(mime_type.as_deref(), Some("image/png"));
    assert!(STANDARD
        .decode(blob)
        .expect("base64 image")
        .starts_with(PNG_SIGNATURE));

    for uri in [
        "termtrace://sessions/missing",
        "termtrace://sessions/listed/unknown",
    ] {
        let missing = client
            .read_resource(ReadResourceRequestParams::new(uri))
            .await;
        assert!(missing.is_err(), "{uri}: {missing:?}");
    }

    client.cancel().await.expect("close client");
}

#[tokio::test]
#[allow(
    clippy::expect_used,
    reason = "test setup failures should stop the test with context"
)]
async fn every_report_of_a_recorded_session_matches_its_output_schema() {
    let temporary = TempDir::new().expect("temporary directory");
    let client = connect(&temporary.path().join("state")).await;
    start_waiting_shell(&client, "reported").await;

    let before = call(&client, "screen", json!({"session": "reported"})).await;
    let sent = call(
        &client,
        "send",
        json!({
            "session": "reported",
            "inputs": ["text:world", "enter"],
            "until": {"kind": "idle", "ms": 200},
            "screen": "diff",
        }),
    )
    .await;
    assert!(!failed(&sent), "{sent:?}");

    let idle = call(
        &client,
        "wait",
        json!({"session": "reported", "for": {"kind": "idle", "ms": 200}}),
    )
    .await;
    assert!(!failed(&idle), "{idle:?}");

    let listed = call(&client, "status", json!({})).await;
    assert!(!failed(&listed), "{listed:?}");
    assert_eq!(structured(&listed)["sessions"][0]["alias"], "reported");

    let live = call(&client, "status", json!({"session": "reported"})).await;
    assert!(!failed(&live), "{live:?}");

    let stopped = call(
        &client,
        "stop",
        json!({"session": "reported", "wait": true}),
    )
    .await;
    assert!(!failed(&stopped), "{stopped:?}");

    let first = structured(&before)["state"].clone();
    let path = structured(&before)["session"].clone();
    let last =
        structured(&call(&client, "screen", json!({"session": "reported"})).await)["state"].clone();

    for report in [
        json!({"session": "reported", "kind": "triage"}),
        json!({"session": "reported", "kind": "timeline", "limit": 2}),
        json!({"session": "reported", "kind": "diff", "from_state_hash": first, "to_state_hash": last}),
        json!({"session": "reported", "kind": "explain", "state_hash": last}),
        json!({"session": "reported", "kind": "diagnose"}),
        json!({"session": path, "kind": "verify"}),
        json!({"session": "reported", "kind": "output"}),
    ] {
        let analyzed = call(&client, "analyze", report).await;
        assert!(!failed(&analyzed), "{analyzed:?}");
    }

    for (name, options) in [
        ("screen", json!({})),
        ("debug", json!({"profile": "debug"})),
        ("archive", json!({"archive": true})),
    ] {
        let mut input = arguments(options);
        input.insert("session".to_owned(), json!("reported"));
        input.insert("out".to_owned(), json!(temporary.path().join(name)));
        let bundled = call(&client, "bundle", Value::Object(input)).await;
        assert!(!failed(&bundled), "{bundled:?}");
    }

    client.cancel().await.expect("close client");
}
