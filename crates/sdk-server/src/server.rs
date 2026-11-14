use crate::failure::Failure;
use crate::methods::{self, Registry};
use crate::protocol::{self, Incoming, Responder, CANCEL, METHOD_NOT_FOUND, NOT_INITIALIZED};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::locator::WaitCancellation;

/// Server state shared by every request thread.
pub(crate) struct Server {
    terminal: OnceLock<Terminal>,
    pub(crate) recordings: Handles<methods::recording::Recording>,
    cancellations: Mutex<HashMap<String, WaitCancellation>>,
}

/// What one method call can reach.
pub(crate) struct Call<'server> {
    pub(crate) server: &'server Server,
    pub(crate) cancellation: WaitCancellation,
}

impl Call<'_> {
    pub(crate) fn terminal(&self) -> Result<Terminal, Failure> {
        return self.server.terminal().cloned();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelParams {
    id: Value,
}

impl Server {
    pub(crate) fn run(input: impl BufRead, output: impl Write + Send + 'static) -> io::Result<()> {
        let server = Arc::new(Self {
            terminal: OnceLock::new(),
            recordings: Handles::new("recording"),
            cancellations: Mutex::new(HashMap::new()),
        });

        let registry = Arc::new(methods::registry());
        let responder = Responder::new(output);

        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            match protocol::parse(&line) {
                Ok(Incoming::Request { id, method, params }) => {
                    if method == methods::INITIALIZE {
                        responder.respond(&id, server.initialize(params))?;
                        continue;
                    }

                    if server.terminal.get().is_none() {
                        let failure = Failure::protocol(
                            NOT_INITIALIZED,
                            format!("call initialize before {method}"),
                        );

                        responder.respond(&id, Err(failure))?;
                        continue;
                    }

                    server.spawn(&registry, &responder, id, method, params);
                }
                Ok(Incoming::Notification { method, params }) => {
                    if method == CANCEL
                        && let Ok(CancelParams { id }) = serde_json::from_value(params)
                    {
                        server.cancel(&id);
                    }
                }
                Err((id, failure)) => responder.respond(&id, Err(failure))?,
            }
        }

        server.recordings.clear();
        return Ok(());
    }

    fn initialize(&self, params: Value) -> Result<Value, Failure> {
        let params: methods::termtrace::InitializeParams = serde_json::from_value(params)
            .map_err(|error| return Failure::invalid_params(error.to_string()))?;
        let terminal = params.terminal();

        if self.terminal.set(terminal).is_err() {
            return Err(Failure::plain("initialize was already called".to_owned()));
        }
        return serde_json::to_value(methods::termtrace::InitializeResult::current())
            .map_err(|error| return Failure::internal(error.to_string()));
    }

    fn terminal(&self) -> Result<&Terminal, Failure> {
        return self.terminal.get().ok_or_else(|| {
            return Failure::protocol(NOT_INITIALIZED, "call initialize first".to_owned());
        });
    }

    fn spawn(
        self: &Arc<Self>,
        registry: &Arc<Registry>,
        responder: &Responder,
        id: Value,
        method: String,
        params: Value,
    ) {
        let Some(entry) = registry.get(method.as_str()).cloned() else {
            let failure = Failure::protocol(METHOD_NOT_FOUND, format!("unknown method `{method}`"));
            if let Err(error) = responder.respond(&id, Err(failure)) {
                report_write_failure(&error);
            }

            return;
        };

        let server = Arc::clone(self);
        let responder = responder.clone();
        let cancellation = WaitCancellation::default();
        server
            .lock_cancellations()
            .insert(id.to_string(), cancellation.clone());

        thread::spawn(move || {
            let call = Call {
                server: &server,
                cancellation,
            };

            let outcome =
                panic::catch_unwind(AssertUnwindSafe(|| return (entry.invoke)(&call, params)))
                    .unwrap_or_else(|panic| {
                        let reason = panic
                            .downcast_ref::<&str>()
                            .map(|text| return (*text).to_owned())
                            .or_else(|| return panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| return "method panicked".to_owned());
                        return Err(Failure::internal(format!("{method} panicked: {reason}")));
                    });

            server.lock_cancellations().remove(&id.to_string());

            if let Err(error) = responder.respond(&id, outcome) {
                report_write_failure(&error);
            }
        });
    }

    fn cancel(&self, id: &Value) {
        if let Some(cancellation) = self.lock_cancellations().get(&id.to_string()) {
            cancellation.cancel();
        }
    }

    fn lock_cancellations(&self) -> MutexGuard<'_, HashMap<String, WaitCancellation>> {
        return self
            .cancellations
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner());
    }
}

fn report_write_failure(error: &io::Error) {
    let mut stderr = io::stderr().lock();
    // The client is gone once its pipe closes; stderr is the only place left to report it.
    let _ignored = writeln!(
        stderr,
        "termtrace sdk-server: cannot write response: {error}"
    );
}

/// Live objects addressed by opaque ids.
pub(crate) struct Handles<T> {
    prefix: &'static str,
    next: AtomicU64,
    entries: Mutex<HashMap<String, Arc<T>>>,
}

impl<T> Handles<T> {
    fn new(prefix: &'static str) -> Self {
        return Self {
            prefix,
            next: AtomicU64::new(1),
            entries: Mutex::new(HashMap::new()),
        };
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<T>>> {
        return self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner());
    }

    pub(crate) fn insert(&self, value: T) -> String {
        let id = format!(
            "{}-{}",
            self.prefix,
            self.next.fetch_add(1, Ordering::Relaxed)
        );

        self.lock().insert(id.clone(), Arc::new(value));
        return id;
    }

    pub(crate) fn get(&self, id: &str) -> Result<Arc<T>, Failure> {
        return self.lock().get(id).cloned().ok_or_else(|| {
            return Failure::invalid_params(format!("unknown or closed {} `{id}`", self.prefix));
        });
    }

    pub(crate) fn remove(&self, id: &str) -> Result<(), Failure> {
        return self.lock().remove(id).map(drop).ok_or_else(|| {
            return Failure::invalid_params(format!("unknown or closed {} `{id}`", self.prefix));
        });
    }

    fn clear(&self) {
        let entries = mem::take(&mut *self.lock());
        drop(entries);
    }
}
