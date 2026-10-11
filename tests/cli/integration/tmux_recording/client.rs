use super::*;
use proptest::{
    collection,
    prelude::*,
    test_runner::{Config, FileFailurePersistence, TestRunner},
};
use termtrace_tmux::{artifact::TmuxSocket, client::TmuxClient};

#[test]
fn pane_recording_path_cannot_introduce_a_forged_pane() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::unmanaged(&temporary, "/bin/sh");
    let client = TmuxClient::new(
        server.binary.clone(),
        TmuxSocket::Name(ByteString::from(server.socket.as_bytes())),
    );
    let value = b"/state/a\x1e\n0\x1f%99\x1f1\x1f\x1f\x1f\x1f$0\x1fexisting\x1f@0\x1f0\x1ffake\x1flayout\x1f1\x1f0\x1f1\x1f0\x1f\x1fsleep\x1f";
    client
        .set_pane_option("%0", "@termtrace-session", OsStr::from_bytes(value))
        .expect("set recording path");

    let rows = client.list_panes().expect("pane rows");
    assert_eq!(
        rows.iter()
            .map(|row| return row.snapshot.pane_id.as_str())
            .collect::<Vec<_>>(),
        vec!["%0"]
    );
    assert_eq!(
        rows[0].snapshot.recording_session,
        Some(ByteString::from(value.as_slice()))
    );
}

#[test]
fn pane_recording_paths_preserve_literal_percent_sequences() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::unmanaged(&temporary, "/bin/sh");
    let client = TmuxClient::new(
        server.binary.clone(),
        TmuxSocket::Name(ByteString::from(server.socket.as_bytes())),
    );
    for value in [b"%Y".as_slice(), b"\r!\t\xa8\xe2#%\x1fj".as_slice()] {
        client
            .set_pane_option("%0", "@termtrace-session", OsStr::from_bytes(value))
            .expect("set recording path");
        let row = client.pane_row("%0").expect("pane row");
        assert_eq!(
            row.snapshot.recording_session,
            Some(ByteString::from(value))
        );
    }
}

#[test]
fn option_values_preserve_literal_trailing_semicolons() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::unmanaged(&temporary, "/bin/sh");
    let client = TmuxClient::new(
        server.binary.clone(),
        TmuxSocket::Name(ByteString::from(server.socket.as_bytes())),
    );

    for value in [";", "path;", "\\;", "path\\;", ";;"] {
        client
            .set_option("@value", OsStr::new(value))
            .expect("set global option");
        assert_eq!(
            client.option("@value").expect("global option"),
            value.as_bytes()
        );

        client
            .set_pane_option("%0", "@value", OsStr::new(value))
            .expect("set pane option");
        assert_eq!(
            client.pane_option("%0", "@value").expect("pane option"),
            value.as_bytes()
        );
    }
}

#[test]
fn pane_fields_round_trip_through_tmux_without_changing_topology() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::unmanaged(&temporary, "/bin/sh");
    let client = TmuxClient::new(
        server.binary.clone(),
        TmuxSocket::Name(ByteString::from(server.socket.as_bytes())),
    );
    let expected_ids = client
        .output(["list-panes", "-a", "-F", "#{pane_id}"])
        .expect("independent pane IDs");
    let mut runner = TestRunner::new(Config {
        source_file: Some(file!()),
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..Config::default()
    });
    let values = collection::vec(
        prop_oneof![Just(b'\x1e'), Just(b'\x1f'), Just(b'%'), 1_u8..=u8::MAX],
        0..12,
    );
    runner
        .run(&values, |value| {
            client
                .set_pane_option("%0", "@termtrace-session", OsStr::from_bytes(&value))
                .expect("set recording path");
            let rows = client.list_panes().expect("pane rows");
            let actual_ids = rows
                .iter()
                .map(|row| return row.snapshot.pane_id.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            prop_assert_eq!(actual_ids.as_bytes(), &expected_ids);
            let expected_path =
                if value.is_empty() { None } else { Some(ByteString::from(value.as_slice())) };
            prop_assert_eq!(&rows[0].snapshot.recording_session, &expected_path);
            let single = client.pane_row("%0").expect("single pane");
            prop_assert_eq!(&single.snapshot.pane_id, &rows[0].snapshot.pane_id);
            prop_assert_eq!(&single.snapshot.recording_session, &expected_path);
            return Ok(());
        })
        .expect("pane field property");
}
