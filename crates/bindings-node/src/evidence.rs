//! Canonical event and output namespaces.

use crate::{
    blocking_failure,
    host::{self, Arg, Model},
    to_napi_err,
};
use napi_derive::napi;
use std::path::PathBuf;
use termtrace_model::{execution::paging::EventFollowPosition, value::ExactPath};
use termtrace_sdk::{
    events::{EventPageCursor, EventsService, ProjectedEventPage},
    inspection::WaitOutcome,
    output::{ByteOutputPage, OutputPageCursor, OutputService},
};

#[napi(js_name = "Output")]
pub struct JsOutput {
    pub(crate) path: PathBuf,
}

#[napi]
impl JsOutput {
    #[napi(constructor)]
    pub fn new(path: Arg<ExactPath>) -> Self {
        return Self { path: path.0 .0 };
    }

    #[napi]
    pub async fn page(
        &self,
        cursor: Option<Arg<OutputPageCursor>>,
        limit: Option<Arg<u32>>,
    ) -> napi::Result<Model<ByteOutputPage>> {
        let path = self.path.clone();
        return tokio::task::spawn_blocking(move || {
            return OutputService::page(
                &path,
                cursor.map(|v| v.0),
                limit.map_or(65536, |v| v.0 as usize),
            )
            .map(Model)
            .map_err(to_napi_err);
        })
        .await
        .map_err(blocking_failure)?;
    }

    #[napi]
    pub async fn wait_for(
        &self,
        pattern: Arg<termtrace_sdk::ByteString>,
        timeout_ms: Option<f64>,
    ) -> napi::Result<Model<WaitOutcome>> {
        let timeout = host::millis(timeout_ms, termtrace_sdk::defaults::WAIT_TIMEOUT)?;
        let path = self.path.clone();
        return tokio::task::spawn_blocking(move || {
            return OutputService::wait_for_bytes(&path, pattern.0.as_bytes(), timeout)
                .map(Model)
                .map_err(to_napi_err);
        })
        .await
        .map_err(blocking_failure)?;
    }

    #[napi]
    pub fn cursor_to_json(&self, cursor: Arg<OutputPageCursor>) -> napi::Result<String> {
        return serde_json::to_string(&cursor.0).map_err(|e| host::type_error(e.to_string()));
    }

    #[napi]
    pub fn cursor_from_json(&self, text: String) -> napi::Result<Model<OutputPageCursor>> {
        return serde_json::from_str(&text)
            .map(Model)
            .map_err(|e| host::type_error(e.to_string()));
    }
}

#[napi(js_name = "Events")]
pub struct JsEvents {
    pub(crate) path: PathBuf,
}

#[napi]
impl JsEvents {
    #[napi(constructor)]
    pub fn new(path: Arg<ExactPath>) -> Self {
        return Self { path: path.0 .0 };
    }

    #[napi]
    pub async fn page(
        &self,
        cursor: Option<Arg<EventPageCursor>>,
        limit: Option<Arg<u16>>,
        metadata_only: Option<bool>,
        event_types: Option<Vec<String>>,
    ) -> napi::Result<Model<ProjectedEventPage>> {
        let path = self.path.clone();
        return tokio::task::spawn_blocking(move || {
            return EventsService::page(
                &path,
                cursor.map(|v| v.0),
                limit.map_or(100, |v| v.0),
                metadata_only.unwrap_or(false),
                &event_types.unwrap_or_default(),
            )
            .map(Model)
            .map_err(to_napi_err);
        })
        .await
        .map_err(blocking_failure)?;
    }

    #[napi]
    pub async fn follow(
        &self,
        next_event_seq: Option<Arg<u64>>,
    ) -> napi::Result<Model<EventFollowPosition>> {
        let path = self.path.clone();
        return tokio::task::spawn_blocking(move || {
            let session =
                termtrace_sdk::Session::attach(&path, "termtrace").map_err(to_napi_err)?;
            return session
                .events()
                .follow(next_event_seq.map_or(0, |v| v.0))
                .map(|v| Model(v.into()))
                .map_err(to_napi_err);
        })
        .await
        .map_err(blocking_failure)?;
    }

    #[napi]
    pub fn cursor_to_json(&self, cursor: Arg<EventPageCursor>) -> napi::Result<String> {
        return serde_json::to_string(&cursor.0).map_err(|e| host::type_error(e.to_string()));
    }

    #[napi]
    pub fn cursor_from_json(&self, text: String) -> napi::Result<Model<EventPageCursor>> {
        return serde_json::from_str(&text)
            .map(Model)
            .map_err(|e| host::type_error(e.to_string()));
    }
}
