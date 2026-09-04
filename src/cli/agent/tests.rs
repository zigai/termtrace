use std::{collections::BTreeMap, io::Cursor, path::PathBuf};

use termtrace_engine::Error;
use termtrace_model::{
    execution::environment::EnvironmentSpec,
    execution::event::EVENT_LOG_VERSION,
    execution::event_log::CanonicalBoundary,
    execution::session::ResourceLimits,
    protocol::control::{CONTROL_PROTOCOL_VERSION, ControlCommand, ControlErrorDetails},
    terminal::observation::ActorBoundaries,
    terminal::replay::STATE_FORMAT_VERSION,
    value::ByteString,
};
use uuid::Uuid;

use super::{
    operations::{
        actor_failure_retry, control_command_mutates, driver_error, driver_failure_from_error,
        driver_failure_with_retry, driver_response_is_cacheable,
    },
    runtime::{DRIVER_RESPONSE_CACHE_BYTES, Driver, bounded_response, serialized_size},
    serve,
};
use termtrace_model::protocol::driver::{
    DRIVER_MAX_REQUEST_BYTES, DRIVER_MAX_RESPONSE_BYTES, DRIVER_PROTOCOL_VERSION, DriverData,
    DriverMethod, DriverRequest, DriverResponse, DriverResult,
};

#[test]
fn blocked_capability_retains_remediation_in_driver_protocol() {
    let result = driver_failure_from_error(
        Error::CapabilityBlocked {
            id: "native-screenshot".to_owned(),
            code: "native_host_tools_missing".to_owned(),
            detail: "missing host tools: ghostty".to_owned(),
            remediation: "install Ghostty GTK".to_owned(),
        },
        None,
    );
    assert!(matches!(
        result.as_ref(),
        DriverResult::Error {
            code,
            message,
            ..
        } if code == "capability_blocked"
            && message.contains("native_host_tools_missing")
            && message.contains("fix: install Ghostty GTK")
            && message.contains("--require native-screenshot --explain")
    ));
}

#[test]
fn bounded_valid_frame_is_streamed_and_followed_by_another_request() {
    let large_request_id = Uuid::now_v7();
    let environment = EnvironmentSpec {
        set: BTreeMap::from([(
            "LARGE_VALUE".to_owned(),
            ByteString::new(vec![b'x'; 1024 * 1024]),
        )]),
        ..EnvironmentSpec::default()
    };
    environment.validate().expect("valid environment");
    let large_request = serde_json::to_vec(&DriverRequest {
        version: DRIVER_PROTOCOL_VERSION,
        request_id: large_request_id,
        method: DriverMethod::Launch {
            evidence_root: PathBuf::from("/unused"),
            command: Vec::new(),
            cols: 80,
            rows: 24,
            working_directory: None,
            environment,
            resource_limits: ResourceLimits::default(),
        },
    })
    .expect("large request JSON");
    assert!(large_request.len() > 1024 * 1024);

    let hello_request_id = Uuid::now_v7();
    let hello_request = serde_json::to_vec(&DriverRequest {
        version: DRIVER_PROTOCOL_VERSION,
        request_id: hello_request_id,
        method: DriverMethod::Hello,
    })
    .expect("hello request JSON");
    let mut input = vec![b'\n'];
    input.extend_from_slice(&large_request);
    input.push(b'\n');
    input.extend_from_slice(&hello_request);
    input.push(b'\n');

    let mut output = Vec::new();
    serve(Cursor::new(input), &mut output).expect("serve requests");
    let responses = output
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
        .map(|line| return serde_json::from_slice::<DriverResponse>(line).expect("response"))
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].request_id, large_request_id);
    assert!(matches!(
        &responses[0].result,
        DriverResult::Error { code, .. } if code == "invalid_launch"
    ));
    assert_eq!(responses[1].request_id, hello_request_id);
    assert!(matches!(
        &responses[1].result,
        DriverResult::Ok {
            data: DriverData::Hello {
                max_request_bytes: DRIVER_MAX_REQUEST_BYTES,
                max_response_bytes: DRIVER_MAX_RESPONSE_BYTES,
                ..
            }
        }
    ));
}

#[test]
fn oversized_frame_is_rejected_and_drained_before_the_next_request() {
    let request_id = Uuid::now_v7();
    let request = serde_json::to_vec(&DriverRequest {
        version: DRIVER_PROTOCOL_VERSION,
        request_id,
        method: DriverMethod::Hello,
    })
    .expect("request JSON");
    let mut input =
        vec![b'x'; usize::try_from(DRIVER_MAX_REQUEST_BYTES).expect("request cap") + 99];
    input.push(b'\n');
    input.extend_from_slice(&request);
    input.push(b'\n');

    let mut output = Vec::new();
    serve(Cursor::new(input), &mut output).expect("serve requests");
    let responses = output
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
        .map(|line| return serde_json::from_slice::<DriverResponse>(line).expect("response"))
        .collect::<Vec<_>>();

    assert_eq!(responses.len(), 2);
    assert!(matches!(
        &responses[0].result,
        DriverResult::Error { code, .. } if code == "request_too_large"
    ));
    assert_eq!(responses[1].request_id, request_id);
    assert!(matches!(
        responses[1].result,
        DriverResult::Ok {
            data: DriverData::Hello { .. }
        }
    ));
}

#[test]
fn oversized_response_is_replaced_with_a_bounded_error() {
    let request_id = Uuid::now_v7();
    let response =
        DriverResponse {
            version: DRIVER_PROTOCOL_VERSION,
            request_id,
            result: DriverResult::Ok {
                data: DriverData::Hello {
                    driver_protocol_version: DRIVER_PROTOCOL_VERSION,
                    control_protocol_version: CONTROL_PROTOCOL_VERSION,
                    event_log_version: EVENT_LOG_VERSION,
                    state_format_version: STATE_FORMAT_VERSION,
                    methods: vec!["x".repeat(
                        usize::try_from(DRIVER_MAX_RESPONSE_BYTES).expect("response cap") + 1,
                    )],
                    max_request_bytes: DRIVER_MAX_REQUEST_BYTES,
                    max_response_bytes: DRIVER_MAX_RESPONSE_BYTES,
                },
            },
        };

    let bounded = bounded_response(response);
    assert_eq!(bounded.request_id, request_id);
    assert!(serialized_size(&bounded).expect("bounded size") < 1024);
    assert!(matches!(
        bounded.result,
        DriverResult::Error {
            code,
            retry_safety,
            ..
        } if code == "response_too_large" && retry_safety == "safe_same_request_id"
    ));
}

#[test]
fn response_cache_is_evicted_by_total_encoded_bytes() {
    let mut driver = Driver::default();
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let response = driver_error(first, "failure", "bounded", None);

    driver.cache(
        first,
        "first".to_owned(),
        response.clone(),
        DRIVER_RESPONSE_CACHE_BYTES / 2 + 1,
    );
    driver.cache(
        second,
        "second".to_owned(),
        DriverResponse {
            request_id: second,
            ..response
        },
        DRIVER_RESPONSE_CACHE_BYTES / 2 + 1,
    );

    assert!(!driver.cached.contains_key(&first));
    assert!(driver.cached.contains_key(&second));
    assert_eq!(driver.cached_bytes, DRIVER_RESPONSE_CACHE_BYTES / 2 + 1);
}

#[test]
fn malformed_frames_are_rejected_without_echoing_payload() {
    let request = serde_json::to_vec(&DriverRequest {
        version: DRIVER_PROTOCOL_VERSION,
        request_id: Uuid::now_v7(),
        method: DriverMethod::Hello,
    })
    .expect("request JSON");
    let mut output = Vec::new();

    serve(Cursor::new(request), &mut output).expect("serve unterminated request");

    let response: DriverResponse = serde_json::from_slice(&output).expect("response JSONL");
    assert_eq!(response.request_id, Uuid::nil());
    assert!(matches!(
        response.result,
        DriverResult::Error { code, message, .. }
            if code == "invalid_request"
                && message.contains("newline")
                && !message.contains("hello")
    ));

    let malformed = br#"{"version":"SECRET_SENTINEL","request_id":"00000000-0000-0000-0000-000000000001","method":{"method":"hello"}}
"#;
    let mut output = Vec::new();
    serve(Cursor::new(malformed), &mut output).expect("serve malformed request");

    let response: DriverResponse = serde_json::from_slice(&output).expect("response JSONL");
    assert_eq!(response.request_id, Uuid::nil());
    assert!(matches!(
        response.result,
        DriverResult::Error { code, message, .. }
            if code == "invalid_request"
                && message.contains("protocol schema")
                && !message.contains("SECRET_SENTINEL")
    ));
}

#[test]
fn retryable_transport_failures_are_not_cached_by_request_id() {
    let response = DriverResponse {
        version: DRIVER_PROTOCOL_VERSION,
        request_id: Uuid::now_v7(),
        result: *driver_failure_with_retry(
            "control_timeout",
            "timed out",
            None,
            "safe_same_request_id",
            true,
            None,
        ),
    };
    assert!(!driver_response_is_cacheable(&response));

    let actor_response = DriverResponse {
        version: DRIVER_PROTOCOL_VERSION,
        request_id: Uuid::now_v7(),
        result: *driver_failure_with_retry(
            "input_sequence_incomplete",
            "prefix applied",
            None,
            "safe_same_request_id",
            true,
            None,
        ),
    };
    assert!(driver_response_is_cacheable(&actor_response));

    let deterministic = driver_error(
        Uuid::now_v7(),
        "unknown_handle",
        "session handle is not attached",
        None,
    );
    assert!(driver_response_is_cacheable(&deterministic));
}

#[test]
fn retry_classification_distinguishes_observation_from_mutation() {
    assert!(!control_command_mutates(&ControlCommand::Snapshot));
    assert!(control_command_mutates(&ControlCommand::InputSequence {
        atoms: Vec::new(),
        pace_ns: 0,
        include_atom_outcomes: false,
    }));
    assert_eq!(
        actor_failure_retry("input_sequence_incomplete", None),
        ("safe_same_request_id", false)
    );
    assert_eq!(
        actor_failure_retry("input_write_failed", None),
        ("safe_same_request_id", true)
    );
}

#[test]
fn post_apply_terminating_frame_requires_same_action_id() {
    let boundary = CanonicalBoundary {
        next_event_seq: 2,
        event_hash: "a".repeat(64),
        output_bytes: 3,
    };
    let details = ControlErrorDetails {
        current_presented_state_hash: "b".repeat(64),
        boundaries: ActorBoundaries {
            durable: boundary.clone(),
            observed: boundary,
        },
        scope: "action_terminating_screen_frame".to_owned(),
        input_sequence: None,
        screen_frame: None,
    };

    assert_eq!(
        actor_failure_retry("screen_frame_resource_limit", Some(&details)),
        ("safe_same_request_id", false)
    );
    assert_eq!(
        actor_failure_retry("screen_frame_resource_limit", None),
        ("safe_new_request", false)
    );
}
