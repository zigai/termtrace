pub(super) use termtrace_model::protocol::driver::{
    DRIVER_MAX_REQUEST_BYTES, DRIVER_MAX_RESPONSE_BYTES, DRIVER_PROTOCOL_VERSION, DriverData,
    DriverMethod, DriverRequest, DriverResponse, DriverResult, DriverScreenshotRenderer,
};

pub(super) const DRIVER_RESPONSE_CACHE_BYTES: usize = 16 * 1024 * 1024;
