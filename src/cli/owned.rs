use clap::Args;
use miette::{IntoDiagnostic as _, Result};
use signal_hook::{
    consts::{SIGHUP, SIGINT, SIGTERM},
    flag, low_level,
};
use std::{
    ffi::OsString,
    process::{Command, ExitCode},
    sync::{atomic::AtomicBool, Arc},
};
use termtrace_process::{owned, ProcessIdentity};

#[derive(Debug, Args)]
pub(super) struct OwnedProcessArgs {
    #[arg(long = owned::PARENT_PID)]
    parent_pid: u32,
    #[arg(long = owned::PARENT_START_TICKS)]
    parent_start_ticks: u64,
    #[arg(last = true, required = true)]
    command: Vec<OsString>,
}

struct Signals(Vec<signal_hook::SigId>);

impl Drop for Signals {
    fn drop(&mut self) {
        for signal in self.0.drain(..) {
            low_level::unregister(signal);
        }
    }
}

pub(super) fn run(arguments: OwnedProcessArgs) -> Result<ExitCode> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut signals = Signals(Vec::new());

    for signal in [SIGHUP, SIGINT, SIGTERM] {
        signals
            .0
            .push(flag::register(signal, Arc::clone(&cancelled)).into_diagnostic()?);
    }

    let (program, arguments_tail) = arguments
        .command
        .split_first()
        .ok_or_else(|| miette::miette!("owned process requires a command"))?;

    let owner = ProcessIdentity {
        pid: arguments.parent_pid,
        start_ticks: arguments.parent_start_ticks,
    };
    let status = owned::supervise(
        owner,
        Command::new(program).args(arguments_tail),
        &cancelled,
    )
    .into_diagnostic()?;

    return Ok(status.map_or(ExitCode::FAILURE, |status| {
        return ExitCode::from(
            status
                .code()
                .and_then(|code| return u8::try_from(code).ok())
                .unwrap_or(1),
        );
    }));
}
