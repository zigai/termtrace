//! Explicit, fallible application instrumentation over authenticated local IPC.

use crate::Client;
use serde_json::Value;
use std::{collections::BTreeMap, process, time::Instant};
use termtrace_control::{Error, Result};
pub use termtrace_model::execution::application::{ApplicationEvent, ApplicationEventKind};
use termtrace_model::protocol::control::ControlCommand;
use uuid::Uuid;

/// One emitting process instance and its monotonic clock origin.
///
/// Construct a new instance after a fork. Calls block until acknowledged or the normal
/// control deadline expires. No event is emitted on drop and no error is silently swallowed.
pub struct Instrumentation {
    client: Client,
    process_instance_id: Uuid,
    started: Instant,
}

impl Instrumentation {
    /// Select a live session and establish an independent process-instance identity.
    #[must_use]
    pub fn new(client: Client) -> Self {
        return Self {
            client,
            process_instance_id: Uuid::now_v7(),
            started: Instant::now(),
        };
    }

    /// Identity shared by this emitter's events and spans.
    #[must_use]
    pub fn process_instance_id(&self) -> Uuid {
        return self.process_instance_id;
    }

    /// Append one discrete event independently of the terminal output stream.
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidRequest` when the event fails validation or the monotonic clock
    /// overflows, and the control error when the session cannot receive it.
    pub fn event(&self, name: &str, attributes: BTreeMap<String, Value>) -> Result<()> {
        return self.emit(ApplicationEventKind::Event, name, None, None, attributes);
    }

    /// Begin a span; retain its UUID and explicitly call [`Self::end_span`].
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidRequest` when the event fails validation or the monotonic clock
    /// overflows, and the control error when the session cannot receive it.
    pub fn start_span(
        &self,
        name: &str,
        parent_span_id: Option<Uuid>,
        attributes: BTreeMap<String, Value>,
    ) -> Result<Uuid> {
        let span_id = Uuid::now_v7();
        self.emit(
            ApplicationEventKind::SpanStart,
            name,
            Some(span_id),
            parent_span_id,
            attributes,
        )?;

        return Ok(span_id);
    }

    /// End a span. Missing ends remain incomplete evidence if the application crashes.
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidRequest` when the event fails validation or the monotonic clock
    /// overflows, and the control error when the session cannot receive it.
    pub fn end_span(
        &self,
        name: &str,
        span_id: Uuid,
        attributes: BTreeMap<String, Value>,
    ) -> Result<()> {
        return self.emit(
            ApplicationEventKind::SpanEnd,
            name,
            Some(span_id),
            None,
            attributes,
        );
    }

    fn emit(
        &self,
        kind: ApplicationEventKind,
        name: &str,
        span_id: Option<Uuid>,
        parent_span_id: Option<Uuid>,
        attributes: BTreeMap<String, Value>,
    ) -> Result<()> {
        let data = ApplicationEvent {
            process_instance_id: self.process_instance_id,
            process_id: process::id(),
            monotonic_ns: u64::try_from(self.started.elapsed().as_nanos()).map_err(|source| {
                return Error::InvalidRequest {
                    reason: "application monotonic clock overflow".to_owned(),
                    source: Some(Box::new(source)),
                };
            })?,
            kind,
            name: name.to_owned(),
            span_id,
            parent_span_id,
            attributes,
        };

        data.validate().map_err(|reason| {
            return Error::InvalidRequest {
                reason: reason.to_owned(),
                source: None,
            };
        })?;

        self.client.request(ControlCommand::Application { data })?;

        return Ok(());
    }
}
