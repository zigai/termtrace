#![no_main]

use libfuzzer_sys::fuzz_target;
use termtrace_model::protocol::control::{ControlRequest, ControlResponse};

fuzz_target!(|bytes: &[u8]| {
    if let Ok(request) = serde_json::from_slice::<ControlRequest>(bytes) {
        let encoded = serde_json::to_vec(&request).expect("serialize parsed control request");
        let decoded = serde_json::from_slice::<ControlRequest>(&encoded)
            .expect("decode serialized control request");
        assert_eq!(decoded, request);
    }
    if let Ok(response) = serde_json::from_slice::<ControlResponse>(bytes) {
        let encoded = serde_json::to_vec(&response).expect("serialize parsed control response");
        let decoded = serde_json::from_slice::<ControlResponse>(&encoded)
            .expect("decode serialized control response");
        assert_eq!(decoded, response);
    }
});
