use super::{
    operations::{
        driver_error, driver_failure_from_error, driver_failure_with_effect,
        driver_response_is_cacheable,
    },
    runtime::{bounded_response, serialized_size, DRIVER_RESPONSE_CACHE_BYTES},
    serve,
};
use std::{collections::BTreeMap, io, io::Cursor, path::PathBuf};
use termtrace_control::Error as ControlError;
use termtrace_engine::Error;
use termtrace_model::protocol::driver::{
    DriverMethod, DriverPayload, DriverRequest, DriverResponse, DriverResult,
    DRIVER_MAX_REQUEST_BYTES, DRIVER_MAX_RESPONSE_BYTES, DRIVER_PROTOCOL_VERSION,
};
use termtrace_model::{
    execution::environment::EnvironmentSpec, execution::event::EVENT_LOG_VERSION,
    execution::session::ResourceLimits, protocol::control::CONTROL_PROTOCOL_VERSION,
    protocol::mutation::MutationEffect, terminal::replay::STATE_FORMAT_VERSION, value::ByteString,
};
use uuid::Uuid;

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
            data: DriverPayload::Hello {
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
            data: DriverPayload::Hello { .. }
        }
    ));
}

#[test]
fn oversized_response_is_replaced_with_a_bounded_error() {
    let request_id = Uuid::now_v7();
    let response = DriverResponse {
        version: DRIVER_PROTOCOL_VERSION,
        request_id,
        result: DriverResult::Ok {
            data: DriverPayload::Hello {
                driver_protocol_version: DRIVER_PROTOCOL_VERSION,
                control_protocol_version: CONTROL_PROTOCOL_VERSION,
                event_log_version: EVENT_LOG_VERSION,
                state_format_version: STATE_FORMAT_VERSION,
                methods: vec!["x"
                    .repeat(usize::try_from(DRIVER_MAX_RESPONSE_BYTES).expect("response cap") + 1)],
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
            effect: MutationEffect::NotApplied,
            ..
        } if code == "response_too_large"
    ));
}

#[test]
fn response_cache_is_evicted_by_total_encoded_bytes() {
    let requests = (1..=6)
        .map(|id| {
            return DriverRequest {
                version: DRIVER_PROTOCOL_VERSION,
                request_id: Uuid::from_u128(id),
                method: DriverMethod::Launch {
                    evidence_root: PathBuf::from(format!("/{}", "x".repeat(3 * 1024 * 1024))),
                    command: Vec::new(),
                    cols: 80,
                    rows: 24,
                    working_directory: None,
                    environment: EnvironmentSpec::default(),
                    resource_limits: ResourceLimits::default(),
                },
            };
        })
        .collect::<Vec<_>>();

    let mut input = Vec::new();
    for request in &requests {
        serde_json::to_writer(&mut input, request).expect("request JSON");
        input.push(b'\n');
    }

    // The second response remains cached after the first has been evicted.
    serde_json::to_writer(&mut input, &requests[1]).expect("retry JSON");
    input.push(b'\n');

    for id in [2, 1] {
        serde_json::to_writer(
            &mut input,
            &DriverRequest {
                version: DRIVER_PROTOCOL_VERSION,
                request_id: Uuid::from_u128(id),
                method: DriverMethod::Hello,
            },
        )
        .expect("changed method JSON");
        input.push(b'\n');
    }

    let mut output = Vec::new();
    serve(Cursor::new(input), &mut output).expect("serve cache pressure and retries");
    let lines = output
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 9);

    let response_bytes = lines
        .iter()
        .take(6)
        .map(|line| return line.len())
        .sum::<usize>();
    assert!(response_bytes > DRIVER_RESPONSE_CACHE_BYTES);
    assert!(response_bytes - lines[0].len() <= DRIVER_RESPONSE_CACHE_BYTES);

    for line in lines.iter().take(6) {
        let response: DriverResponse = serde_json::from_slice(line).expect("launch response");
        assert!(matches!(
            response.result,
            DriverResult::Error { code, .. } if code == "invalid_launch"
        ));
    }

    assert_eq!(lines[1], lines[6]);

    let collision: DriverResponse = serde_json::from_slice(lines[7]).expect("collision response");
    assert_eq!(collision.request_id, Uuid::from_u128(2));
    assert!(matches!(
        collision.result,
        DriverResult::Error { code, .. } if code == "request_id_collision"
    ));

    let evicted: DriverResponse = serde_json::from_slice(lines[8]).expect("evicted response");
    assert_eq!(evicted.request_id, Uuid::from_u128(1));
    assert!(matches!(
        evicted.result,
        DriverResult::Ok {
            data: DriverPayload::Hello { .. }
        }
    ));
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
        result: *driver_failure_with_effect(
            "control_timeout",
            "timed out",
            None,
            MutationEffect::Unknown,
            None,
        ),
    };

    assert!(!driver_response_is_cacheable(&response));

    let actor_response = DriverResponse {
        version: DRIVER_PROTOCOL_VERSION,
        request_id: Uuid::now_v7(),
        result: *driver_failure_with_effect(
            "input_sequence_incomplete",
            "prefix applied",
            None,
            MutationEffect::Partial { completed_atoms: 1 },
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
fn transport_failures_carry_the_effect_implied_by_delivery() {
    let failure = |effect| {
        return driver_failure_from_error(
            ControlError::Timeout {
                session: PathBuf::from("/session"),
                step: "read response",
                timeout_ms: 1,
                effect,
                source: io::Error::from(io::ErrorKind::TimedOut),
            },
            Some(PathBuf::from("/session")),
        );
    };

    for effect in [MutationEffect::NotApplied, MutationEffect::Unknown] {
        assert!(matches!(
            failure(effect).as_ref(),
            DriverResult::Error { code, effect: actual, .. }
                if code == "control_timeout" && *actual == effect
        ));
    }
}
