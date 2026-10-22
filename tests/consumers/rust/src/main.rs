use std::{
    collections::BTreeMap,
    env,
    error::Error,
    thread,
    time::{Duration, Instant},
};
use termtrace_sdk::{
    instrumentation::Instrumentation, Client, ControlCommand, ControlRequest, ControlResult,
};

fn main() -> Result<(), Box<dyn Error>> {
    let session = env::args_os().nth(1).ok_or("expected session directory")?;
    let client = Client::new(&session);
    let deadline = Instant::now() + Duration::from_secs(10);

    while !client.is_reachable() {
        if Instant::now() >= deadline {
            return Err("recorder did not become reachable".into());
        }

        thread::sleep(Duration::from_millis(10));
    }

    let ping = ControlRequest::new(ControlCommand::Ping);
    let response = client.send_with_timeout(&ping, Duration::from_secs(2))?;

    assert_eq!(response.request_id, ping.request_id);
    assert!(matches!(response.result, ControlResult::Ok { .. }));

    let application = Instrumentation::new(client.clone());
    let span = application.start_span("installed rust SDK", None, BTreeMap::new())?;
    application.event("consumer diagnostic", BTreeMap::new())?;
    application.end_span("installed rust SDK", span, BTreeMap::new())?;

    let response = client.send(&ControlRequest::new(ControlCommand::Stop))?;
    assert!(matches!(response.result, ControlResult::Ok { .. }));

    Ok(())
}
