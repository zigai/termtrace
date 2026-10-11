use super::*;
use termtrace_tmux::{artifact::TmuxSocket, client::TmuxClient};

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
