//! Protocol behavior of the SDK server: framing, errors, the handshake, and handles.

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::io::{self, Cursor, Write};
    use std::sync::mpsc::{self, Sender};
    use std::thread;

    struct Lines {
        pending: Vec<u8>,
        sender: Sender<Value>,
    }

    impl Write for Lines {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.pending.extend_from_slice(buf);
            while let Some(end) = self.pending.iter().position(|byte| return *byte == b'\n') {
                let line = self.pending.drain(..=end).collect::<Vec<_>>();
                let message = serde_json::from_slice(&line).map_err(io::Error::other)?;
                self.sender.send(message).map_err(io::Error::other)?;
            }
            return Ok(buf.len());
        }

        fn flush(&mut self) -> io::Result<()> {
            return Ok(());
        }
    }

    /// Serve `requests`, one per line, and collect every response once the server and its request
    /// threads finish.
    fn exchange(requests: &[Value]) -> Vec<Value> {
        let input = requests
            .iter()
            .map(|request| return format!("{request}\n"))
            .collect::<String>();
        return serve_text(&input);
    }

    fn serve_text(input: &str) -> Vec<Value> {
        let (sender, receiver) = mpsc::channel();
        let output = Lines {
            pending: Vec::new(),
            sender,
        };

        let input = input.to_owned();
        let server = thread::spawn(move || {
            return termtrace_sdk_server::serve(Cursor::new(input), output);
        });

        let responses = receiver.iter().collect();

        server.join().expect("server thread").expect("serve");
        return responses;
    }

    fn by_id(responses: Vec<Value>) -> HashMap<String, Value> {
        return responses
            .into_iter()
            .map(|response| return (response["id"].to_string(), response))
            .collect();
    }

    fn request(id: u64, method: &str, params: &Value) -> Value {
        return json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    }

    fn initialize(id: u64) -> Value {
        return request(id, "initialize", &json!({}));
    }

    fn error_code(response: &Value) -> i64 {
        return response["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("not an error: {response}"));
    }

    #[test]
    fn malformed_lines_are_answered_and_later_requests_still_run() {
        let input = format!(
            "{{not json\n{}\n{}\n{}\n{}\n",
            json!({"jsonrpc": "1.0", "id": 2, "method": "system.schemas"}),
            json!({"jsonrpc": "2.0", "id": {"nested": true}, "method": "system.schemas"}),
            initialize(4),
            request(5, "system.schemas", &json!({})),
        );
        let responses = serve_text(&input);
        let first = &responses[0];

        assert_eq!(first["id"], Value::Null);
        assert_eq!(error_code(first), -32_700);

        let responses = by_id(responses);

        assert_eq!(error_code(&responses["2"]), -32_600);
        assert_eq!(error_code(&responses[r#"{"nested":true}"#]), -32_600);
        assert!(responses["4"]["result"]["version"].is_string());
        assert!(responses["5"]["result"]
            .as_array()
            .is_some_and(|names| return !names.is_empty()));
    }

    #[test]
    fn calls_before_initialize_are_rejected_and_initialize_runs_once() {
        let responses = by_id(exchange(&[
            request(1, "system.schemas", &json!({})),
            initialize(2),
            request(3, "system.schemas", &json!({})),
            initialize(4),
        ]));

        assert_eq!(error_code(&responses["1"]), -32_002);
        assert!(responses["2"].get("error").is_none());
        assert!(responses["3"]["result"].is_array());
        assert_eq!(error_code(&responses["4"]), -32_000);
    }

    #[test]
    fn unknown_methods_and_bad_params_fail_without_stopping_the_server() {
        let responses = by_id(exchange(&[
            initialize(1),
            request(2, "screen.teleport", &json!({})),
            request(3, "recording.stop", &json!({"recording": 7})),
            request(
                4,
                "recording.stop",
                &json!({"recording": "recording-1", "extra": true}),
            ),
            request(5, "system.schemas", &json!({})),
        ]));
        assert_eq!(error_code(&responses["2"]), -32_601);
        for id in ["3", "4"] {
            let response = &responses[id];
            assert_eq!(error_code(response), -32_602);
            assert!(response["error"]["message"]
                .as_str()
                .is_some_and(|message| return message.starts_with("recording.stop:")));
        }

        assert!(responses["5"]["result"].is_array());
    }

    #[test]
    fn unknown_handles_are_rejected_by_name() {
        let responses = by_id(exchange(&[
            initialize(1),
            request(2, "recording.stop", &json!({"recording": "recording-41"})),
            request(3, "recording.is_active", &json!({"recording": "elsewhere"})),
        ]));

        for (id, handle) in [("2", "recording-41"), ("3", "elsewhere")] {
            let response = &responses[id];
            assert_eq!(error_code(response), -32_602);
            assert!(response["error"]["message"]
                .as_str()
                .is_some_and(|message| return message.contains(handle)));
        }
    }

    #[test]
    fn notifications_get_no_response_and_end_of_input_ends_the_server() {
        let responses = exchange(&[
            json!({"jsonrpc": "2.0", "method": "$/cancel", "params": {"id": 99}}),
            json!({"jsonrpc": "2.0", "method": "system.schemas"}),
        ]);
        assert!(responses.is_empty(), "{responses:?}");
    }
}
