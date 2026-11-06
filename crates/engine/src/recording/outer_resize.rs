//! Deferred resize coordination when the recorder's outer terminal owns the grid.
//!
//! A control resize is forwarded to the supervisor that owns the outer terminal window.
//! The canonical resize events come only from the grid observed on the recorder's own
//! terminal, so the effect reported to the caller is derived from evidence:
//!
//! | Evidence at completion or deadline              | Effect        |
//! | ----------------------------------------------- | ------------- |
//! | Observed grid equals the request                | `applied`     |
//! | Observed grid changed but differs               | `applied`, postcondition unmet |
//! | Supervisor rejected, or nothing was sent        | `not_applied` |
//! | Bytes sent and no reply, or no grid change seen | `unknown`     |
//!
//! The durable journal keeps the outcome returned to the caller. A grid change observed
//! after an `unknown` answer is still recorded canonically under the request identity, and
//! only the in-memory retry cache moves to the later authoritative outcome.

use super::{
    actor::SessionActor,
    input::apply_resize,
    model::{RuntimeMessage, TerminalGeometry},
    support::{actor_boundaries, control_error_response, deliver_response, terminal_error},
};
use crate::Result;
use nix::{
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::Uid,
};
use std::{
    io::{BufRead as _, BufReader, ErrorKind, Read as _, Write as _},
    os::{
        linux::net::SocketAddrExt as _,
        unix::net::{SocketAddr, UnixStream},
    },
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use termtrace_model::{
    protocol::{
        control::{
            ControlCommand, ControlErrorDetails, ControlPayload, ControlRequest, ControlResponse,
            ControlResult, CONTROL_PROTOCOL_VERSION,
        },
        mutation::MutationEffect,
        outer_resize::{OuterResizeReply, OuterResizeRequest, MAX_OUTER_RESIZE_LINE_BYTES},
    },
    terminal::state::supported_geometry_cell_count,
};
use uuid::Uuid;

/// Bound for the supervisor exchange plus the observed grid change. It is below the client's
/// resize response timeout, so every caller receives the actor's own outcome.
pub(super) const OUTER_RESIZE_DEADLINE: Duration = Duration::from_secs(5);

/// Who owns the terminal grid of a recording.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ResizeAuthority {
    /// Control resizes change the child PTY directly.
    #[default]
    ChildPty,
    /// The recorder runs inside a supervised terminal. Control resizes are forwarded to the
    /// supervisor's socket and take effect only when the outer terminal reports the new grid.
    OuterTerminal {
        /// Linux abstract socket name served by the supervisor; peers must share this user.
        socket: String,
    },
}

/// What the dispatch worker learned from one supervisor exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Dispatch {
    /// Failed before any request byte reached the socket.
    NotSent(String),
    /// The supervisor configured the outer window.
    Configured,
    /// The supervisor did not touch the outer window.
    Rejected { code: String, message: String },
    /// Request bytes were written but no valid reply arrived.
    NoReply(String),
}

struct Worker {
    requests: mpsc::Sender<OuterResizeRequest>,
}

impl Worker {
    fn start(socket: String, sender: mpsc::SyncSender<RuntimeMessage>) -> Result<Self> {
        let (requests, inbox) = mpsc::channel::<OuterResizeRequest>();

        // Each exchange is bounded, and the thread exits once the actor drops its sender.
        thread::Builder::new()
            .name("termtrace-outer-resize".to_owned())
            .spawn(move || {
                for request in inbox {
                    let dispatch = exchange(&socket, &request, OUTER_RESIZE_DEADLINE);
                    let reply = RuntimeMessage::OuterResizeReply {
                        request_id: request.request_id,
                        dispatch,
                    };

                    if sender.send(reply).is_err() {
                        return;
                    }
                }
            })
            .map_err(|error| {
                return terminal_error("spawn outer-resize dispatcher", error.to_string());
            })?;

        return Ok(Self { requests });
    }
}

fn exchange(socket: &str, request: &OuterResizeRequest, timeout: Duration) -> Dispatch {
    let deadline = Instant::now() + timeout;
    let mut line = match serde_json::to_vec(request) {
        Ok(line) => line,
        Err(error) => return Dispatch::NotSent(error.to_string()),
    };
    line.push(b'\n');

    let mut stream = match SocketAddr::from_abstract_name(socket.as_bytes())
        .and_then(|address| return UnixStream::connect_addr(&address))
    {
        Ok(stream) => stream,
        Err(error) => return Dispatch::NotSent(format!("connect to supervisor: {error}")),
    };

    match getsockopt(&stream, PeerCredentials) {
        Ok(credentials) if credentials.uid() == Uid::effective().as_raw() => {}
        Ok(_) => return Dispatch::NotSent("supervisor socket belongs to another user".to_owned()),
        Err(error) => return Dispatch::NotSent(format!("authenticate supervisor: {error}")),
    }

    if let Err(error) = stream.set_write_timeout(Some(timeout)) {
        return Dispatch::NotSent(format!("configure supervisor socket: {error}"));
    }

    let mut written = 0;
    while let Some(remaining) = line.get(written..).filter(|rest| return !rest.is_empty()) {
        match stream.write(remaining) {
            Ok(0) if written == 0 => {
                return Dispatch::NotSent("supervisor socket closed".to_owned())
            }
            Ok(0) => return Dispatch::NoReply("supervisor socket closed mid-request".to_owned()),
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if written == 0 => {
                return Dispatch::NotSent(format!("write supervisor request: {error}"));
            }
            Err(error) => return Dispatch::NoReply(format!("write supervisor request: {error}")),
        }
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Dispatch::NoReply("supervisor reply deadline expired".to_owned());
    }

    if let Err(error) = stream.set_read_timeout(Some(remaining)) {
        return Dispatch::NoReply(format!("configure supervisor socket: {error}"));
    }

    let mut reply = Vec::new();
    let mut reader = BufReader::new(stream.take(MAX_OUTER_RESIZE_LINE_BYTES + 1));
    if let Err(error) = reader.read_until(b'\n', &mut reply) {
        return Dispatch::NoReply(format!("read supervisor reply: {error}"));
    }

    if reply.last() != Some(&b'\n') {
        return Dispatch::NoReply("supervisor closed without a complete reply".to_owned());
    }
    return match serde_json::from_slice::<OuterResizeReply>(&reply) {
        Ok(OuterResizeReply::Configured { request_id, .. }) if request_id == request.request_id => {
            Dispatch::Configured
        }
        Ok(OuterResizeReply::Rejected {
            request_id,
            code,
            message,
        }) if request_id == request.request_id => Dispatch::Rejected { code, message },
        Ok(_) => Dispatch::NoReply("supervisor replied for another request".to_owned()),
        Err(error) => Dispatch::NoReply(format!("malformed supervisor reply: {error}")),
    };
}

struct Pending {
    request_id: Uuid,
    command: ControlCommand,
    command_hash: String,
    requested: (u16, u16),
    // None once the caller was answered `unknown`; the request then only correlates a late change.
    response: Option<mpsc::SyncSender<ControlResponse>>,
    deadline: Instant,
    dispatch: Option<Dispatch>,
}

/// Outer-terminal resize coordination owned by the actor.
pub(super) struct OuterResize {
    worker: Worker,
    pending: Option<Pending>,
}

impl OuterResize {
    pub(super) fn start(
        authority: &ResizeAuthority,
        sender: &mpsc::SyncSender<RuntimeMessage>,
    ) -> Result<Option<Self>> {
        let ResizeAuthority::OuterTerminal { socket } = authority else {
            return Ok(None);
        };

        return Ok(Some(Self {
            worker: Worker::start(socket.clone(), sender.clone())?,
            pending: None,
        }));
    }
}

fn effect_response(
    request_id: Uuid,
    code: &str,
    message: String,
    effect: MutationEffect,
    actor: &SessionActor<'_>,
) -> Result<ControlResponse> {
    return Ok(ControlResponse {
        version: CONTROL_PROTOCOL_VERSION,
        request_id,
        result: ControlResult::Error {
            code: code.to_owned(),
            message,
            effect,
            details: Some(ControlErrorDetails {
                current_presented_state_hash: actor.live.presented_state_hash()?,
                boundaries: actor_boundaries(&actor.event_log),
                scope: "outer_terminal_resize".to_owned(),
                input_sequence: None,
                screen_frame: None,
            }),
        },
    });
}

impl SessionActor<'_> {
    /// Same-ID retry of a resize whose outcome has not been decided yet.
    pub(in crate::recording) fn pending_outer_resize_retry(
        &self,
        request: &ControlRequest,
        command_hash: &str,
    ) -> Option<ControlResponse> {
        let pending = self.outer_resize.as_ref()?.pending.as_ref()?;
        if pending.request_id != request.request_id || pending.response.is_none() {
            return None;
        }

        if pending.command_hash != command_hash {
            return Some(control_error_response(
                request.request_id,
                "request_id_collision",
                "request identifier was already used for a different mutation",
                MutationEffect::NotApplied,
            ));
        }
        return Some(control_error_response(
            request.request_id,
            "resize_pending",
            "the outer terminal has not reported this resize yet; retry with the same request ID",
            MutationEffect::Unknown,
        ));
    }

    pub(in crate::recording) fn owns_outer_resize(&self) -> bool {
        return self.outer_resize.is_some();
    }

    /// Admit a resize that the outer terminal must apply; the answer is delivered later.
    pub(in crate::recording) fn admit_outer_resize(
        &mut self,
        request: ControlRequest,
        command_hash: String,
        response: mpsc::SyncSender<ControlResponse>,
    ) -> Result<()> {
        let ControlCommand::Resize {
            cols,
            rows,
            pixel_width,
            pixel_height,
        } = request.command
        else {
            return Err(terminal_error(
                "admit outer-terminal resize",
                "command is not a resize",
            ));
        };
        let rejection = if supported_geometry_cell_count(cols, rows).is_none()
            || pixel_width == 0
            || pixel_height == 0
        {
            Some((
                "invalid_geometry",
                "terminal geometry exceeds the supported row, column, or cell limit, or has zero pixels".to_owned(),
            ))
        } else if self
            .outer_resize
            .as_ref()
            .and_then(|outer| return outer.pending.as_ref())
            .is_some_and(|pending| return pending.response.is_some())
        {
            Some((
                "resize_busy",
                "another outer-terminal resize is still awaiting its observed grid".to_owned(),
            ))
        } else {
            match self.mutation_history.reserve_operation(request.request_id) {
                Ok(()) => None,
                Err(crate::Error::ControlRejected { message, .. }) => {
                    Some(("operation_id_collision", message))
                }
                Err(error) => return Err(error),
            }
        };
        if let Some((code, message)) = rejection {
            let answer = effect_response(
                request.request_id,
                code,
                message,
                MutationEffect::NotApplied,
                self,
            )?;

            return self.answer_outer_resize(
                &request.command,
                request.request_id,
                command_hash,
                None,
                answer,
                &response,
            );
        }

        let outer = self.outer_resize.as_mut().ok_or_else(|| {
            return terminal_error("admit outer-terminal resize", "no outer terminal");
        })?;

        // A late request is no longer correlated once the caller moves on to a new one.
        outer.pending = None;

        let dispatched = outer.worker.requests.send(OuterResizeRequest {
            request_id: request.request_id,
            cols,
            rows,
        });

        if dispatched.is_err() {
            let answer = effect_response(
                request.request_id,
                "outer_resize_unavailable",
                "the outer-resize dispatcher stopped before sending the request".to_owned(),
                MutationEffect::NotApplied,
                self,
            )?;

            return self.answer_outer_resize(
                &request.command,
                request.request_id,
                command_hash,
                None,
                answer,
                &response,
            );
        }

        outer.pending = Some(Pending {
            request_id: request.request_id,
            command: request.command,
            command_hash,
            requested: (cols, rows),
            response: Some(response),
            deadline: Instant::now() + OUTER_RESIZE_DEADLINE,
            dispatch: None,
        });

        return Ok(());
    }

    fn answer_outer_resize(
        &mut self,
        command: &ControlCommand,
        request_id: Uuid,
        command_hash: String,
        intent_event_seq: Option<u64>,
        answer: ControlResponse,
        response: &mpsc::SyncSender<ControlResponse>,
    ) -> Result<()> {
        let end = self.event_log.next_seq();
        self.mutation_history.remember(
            request_id,
            command,
            command_hash,
            intent_event_seq.unwrap_or(end),
            end,
            self.event_log.current_hash(),
            answer.clone(),
        )?;

        deliver_response(response, answer);

        return Ok(());
    }

    pub(in crate::recording) fn handle_outer_resize_reply(
        &mut self,
        request_id: Uuid,
        dispatch: Dispatch,
    ) -> Result<()> {
        let Some(pending) = self
            .outer_resize
            .as_mut()
            .and_then(|outer| return outer.pending.as_mut())
            .filter(|pending| return pending.request_id == request_id)
        else {
            return Ok(());
        };
        let (code, message) = match dispatch {
            Dispatch::NotSent(reason) => ("outer_resize_unavailable".to_owned(), reason),
            Dispatch::Rejected { code, message } => (code, message),
            dispatch @ (Dispatch::Configured | Dispatch::NoReply(_)) => {
                pending.dispatch = Some(dispatch);

                return Ok(());
            }
        };

        let Some(pending) = self
            .outer_resize
            .as_mut()
            .and_then(|outer| return outer.pending.take())
        else {
            return Ok(());
        };
        let answer = effect_response(request_id, &code, message, MutationEffect::NotApplied, self)?;

        return match pending.response {
            Some(response) => self.answer_outer_resize(
                &pending.command,
                request_id,
                pending.command_hash,
                None,
                answer,
                &response,
            ),
            None => {
                self.mutation_history
                    .evolve(request_id, pending.command_hash, answer);

                Ok(())
            }
        };
    }

    /// Record a grid observed on the recorder's terminal, correlated to a pending request.
    pub(in crate::recording) fn observe_outer_grid(
        &mut self,
        geometry: TerminalGeometry,
    ) -> Result<()> {
        let correlated = self
            .outer_resize
            .as_ref()
            .and_then(|outer| return outer.pending.as_ref())
            .filter(|pending| {
                return !matches!(
                    pending.dispatch,
                    Some(Dispatch::NotSent(_) | Dispatch::Rejected { .. })
                );
            })
            .map(|pending| return pending.request_id);
        let operation_id = match correlated {
            Some(request_id) => request_id,
            None => {
                let operation_id = Uuid::now_v7();
                self.mutation_history.reserve_operation(operation_id)?;

                operation_id
            }
        };

        let intent_event_seq = self.event_log.next_seq();
        apply_resize(
            self.spawned.master.as_mut(),
            &mut self.event_log,
            operation_id,
            geometry,
        )?;

        self.live.apply_resize(geometry)?;

        let Some(pending) = correlated.and_then(|_| {
            return self
                .outer_resize
                .as_mut()
                .and_then(|outer| return outer.pending.take());
        }) else {
            return Ok(());
        };
        let answer = if (geometry.cols, geometry.rows) == pending.requested {
            ControlResponse {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: pending.request_id,
                result: ControlResult::Ok {
                    data: ControlPayload::Acknowledged {
                        boundaries: actor_boundaries(&self.event_log),
                    },
                },
            }
        } else {
            effect_response(
                pending.request_id,
                "resize_postcondition_unmet",
                format!(
                    "requested {}x{} cells; the outer terminal reported {}x{} cells",
                    pending.requested.0, pending.requested.1, geometry.cols, geometry.rows
                ),
                MutationEffect::Applied,
                self,
            )?
        };

        return match pending.response {
            Some(response) => self.answer_outer_resize(
                &pending.command,
                pending.request_id,
                pending.command_hash,
                Some(intent_event_seq),
                answer,
                &response,
            ),
            None => {
                self.mutation_history
                    .evolve(pending.request_id, pending.command_hash, answer);

                Ok(())
            }
        };
    }

    pub(in crate::recording) fn outer_resize_deadline(&self) -> Option<Instant> {
        return self
            .outer_resize
            .as_ref()
            .and_then(|outer| return outer.pending.as_ref())
            .filter(|pending| return pending.response.is_some())
            .map(|pending| return pending.deadline);
    }

    /// Answer `unknown` once the deadline passes without an observed grid change.
    pub(in crate::recording) fn expire_outer_resize(&mut self, ended: bool) -> Result<()> {
        let Some(pending) = self
            .outer_resize
            .as_mut()
            .and_then(|outer| return outer.pending.as_mut())
            .filter(|pending| {
                return pending.response.is_some() && (ended || Instant::now() >= pending.deadline);
            })
        else {
            return Ok(());
        };
        let Some(response) = pending.response.take() else {
            return Ok(());
        };

        let request_id = pending.request_id;
        let command = pending.command.clone();
        let command_hash = pending.command_hash.clone();
        let message = match (&pending.dispatch, ended) {
            (_, true) => "the recording ended before the outer terminal reported a new grid".to_owned(),
            (Some(Dispatch::Configured), false) => "the outer window was configured, but no new grid was observed before the deadline; a late change is still recorded under this request ID".to_owned(),
            (Some(Dispatch::NoReply(reason)), false) => format!("the request reached the supervisor but no reply arrived ({reason})"),
            _ => "the supervisor did not answer before the deadline".to_owned(),
        };

        let answer = effect_response(
            request_id,
            "resize_outcome_unknown",
            format!("{message}; inspect canonical resize events for this request ID, or retry with the same request ID"),
            MutationEffect::Unknown,
            self,
        )?;

        return self.answer_outer_resize(
            &command,
            request_id,
            command_hash,
            None,
            answer,
            &response,
        );
    }
}
