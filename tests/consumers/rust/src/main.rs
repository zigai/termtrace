use std::{
    collections::BTreeMap,
    env,
    error::Error,
    thread,
    time::{Duration, Instant},
};
use termtrace_sdk::{
    ByteString, Client, ControlCommand, ControlRequest, ControlResult, Driver, DriverData,
    DriverMethod, DriverResult, instrumentation::Instrumentation,
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

    let binary = env::var_os("TERMTRACE_BIN").ok_or("expected TERMTRACE_BIN")?;
    let mut driver = Driver::spawn(binary)?;
    let response = driver.request(
        DriverMethod::Attach {
            session: session.into(),
        },
        Duration::from_secs(10),
    )?;
    let DriverResult::Ok {
        data: DriverData::Attached { handle, .. },
    } = response.result
    else {
        return Err("driver did not attach".into());
    };

    let response = driver.request(
        DriverMethod::WaitOutput {
            handle,
            data: ByteString::from(b"consumer ready".as_slice()),
            timeout_ms: 2000,
        },
        Duration::from_secs(5),
    )?;

    assert!(matches!(
        response.result,
        DriverResult::Ok {
            data: DriverData::OutputWait { .. }
        }
    ));

    driver.close(Duration::from_secs(10))?;
    assert!(
        client.is_reachable(),
        "closing an attachment must preserve its recorder"
    );

    let response = client.send(&ControlRequest::new(ControlCommand::Stop))?;
    assert!(matches!(response.result, ControlResult::Ok { .. }));

    Ok(())
}
