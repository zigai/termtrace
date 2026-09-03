#![no_main]

use libfuzzer_sys::fuzz_target;
use termtrace_model::protocol::{
    control::{ControlRequest, ControlResponse},
    driver::{DriverRequest, DriverResponse},
};

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
    if let Ok(request) = serde_json::from_slice::<DriverRequest>(bytes) {
        let encoded = serde_json::to_vec(&request).expect("serialize parsed driver request");
        let decoded = serde_json::from_slice::<DriverRequest>(&encoded)
            .expect("decode serialized driver request");
        assert_eq!(decoded, request);
    }
    if let Ok(response) = serde_json::from_slice::<DriverResponse>(bytes) {
        let encoded = serde_json::to_vec(&response).expect("serialize parsed driver response");
        let decoded = serde_json::from_slice::<DriverResponse>(&encoded)
            .expect("decode serialized driver response");
        assert_eq!(decoded, response);
    }
});
