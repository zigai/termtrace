use std::{collections::BTreeMap, process};

use uuid::Uuid;

use termtrace_model::execution::{
    application::{ApplicationEvent, ApplicationEventKind},
    event::EventProjection,
};
use termtrace_sdk::{instrumentation::Instrumentation, Client};

use super::*;

#[test]
fn application_events_are_deduplicated_bounded_and_separate_from_terminal_bytes() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("application-events");
    let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; assert os.path.isabs(os.environ['TERMTRACE_SESSION']); assert os.path.isfile(os.path.join(os.environ['TERMTRACE_SESSION'],'session.json')); tty.setraw(0); os.write(1,b'READY'); os.read(0,1); os.write(1,b'DONE')",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn instrumented fixture");

    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("ready output");

    let data = ApplicationEvent {
        process_instance_id: Uuid::now_v7(),
        process_id: process::id(),
        monotonic_ns: 123,
        kind: ApplicationEventKind::Event,
        name: "sensitive diagnostic name".to_owned(),
        span_id: None,
        parent_span_id: None,
        attributes: BTreeMap::from([("secret".to_owned(), serde_json::json!("private value"))]),
    };
    let request = ControlRequest::new(ControlCommand::Application { data: data.clone() });
    let first = send_request_response(&session, &request).expect("application event");
    assert!(matches!(first.result, ControlResult::Ok { .. }));
    assert_eq!(
        send_request_response(&session, &request).expect("retry"),
        first
    );

    let mut oversized = data;
    oversized
        .attributes
        .insert("large".to_owned(), serde_json::json!("x".repeat(65_536)));
    let rejected = send_request_response(
        &session,
        &ControlRequest::new(ControlCommand::Application { data: oversized }),
    )
    .expect("bounded rejection");
    assert!(
        matches!(rejected.result, ControlResult::Error { code, .. } if code == "invalid_application_event")
    );

    let application = Instrumentation::new(Client::new(&session));
    let span = application
        .start_span("work", None, BTreeMap::new())
        .expect("span start");

    application
        .end_span("work", span, BTreeMap::new())
        .expect("span end");

    send_request(
        &session,
        &ControlRequest::new(ControlCommand::Input {
            data: ByteString::from(b"x".as_slice()),
            input_kind: InputKind::Text,
            caller_intent: None,
        }),
    )
    .expect("release fixture");
    assert!(child
        .wait_with_output()
        .expect("fixture completion")
        .status
        .success());
    let log = EventLog::read(session.join("events.jsonl")).expect("canonical evidence");
    let application_records = log
        .events
        .iter()
        .filter(|record| {
            return matches!(record.event, Event::Application { .. });
        })
        .collect::<Vec<_>>();
    assert_eq!(application_records.len(), 3);
    let output = log
        .events
        .iter()
        .filter_map(|record| {
            return match &record.event {
                Event::Output { data } => Some(data.as_bytes()),
                _ => None,
            };
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(output, b"READYDONE");
    let metadata =
        serde_json::to_string(&EventProjection::from_record(application_records[0], true))
            .expect("metadata projection");
    assert!(!metadata.contains("private value"));
    assert!(!metadata.contains("sensitive diagnostic name"));
    assert!(!metadata.contains("attributes"));
}
