use std::{
    ffi::OsString,
    fs,
    os::unix::process::CommandExt as _,
    process::{Child, Command, ExitCode},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use clap::Args;
use miette::{IntoDiagnostic as _, Result};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use signal_hook::{
    consts::{SIGHUP, SIGINT, SIGTERM},
    flag, low_level,
};

#[derive(Debug, Args)]
pub(super) struct OwnedProcessArgs {
    #[arg(long)]
    parent_pid: u32,
    #[arg(long)]
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

struct OwnedGroup {
    child: Child,
    group: Pid,
}
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        let _ = killpg(self.group, Signal::SIGTERM);
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            let _ = self.child.try_wait();
            if killpg(self.group, None).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let _ = killpg(self.group, Signal::SIGKILL);
        let _ = self.child.wait();
    }
}

/// A separate process watches the owner's process identity, including zombie state.
/// This survives abrupt owner death without tying children to an owner's worker thread.
pub(super) fn run(arguments: OwnedProcessArgs) -> Result<ExitCode> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut signals = Signals(Vec::new());
    for signal in [SIGHUP, SIGINT, SIGTERM] {
        signals
            .0
            .push(flag::register(signal, Arc::clone(&cancelled)).into_diagnostic()?);
    }
    if !parent_alive(arguments.parent_pid, arguments.parent_start_ticks) {
        return Ok(ExitCode::FAILURE);
    }
    let (program, arguments_tail) = arguments
        .command
        .split_first()
        .ok_or_else(|| miette::miette!("owned process requires a command"))?;
    let mut child = Command::new(program)
        .args(arguments_tail)
        .process_group(0)
        .spawn()
        .into_diagnostic()?;
    let group = match i32::try_from(child.id()) {
        Ok(pid) => Pid::from_raw(pid),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).into_diagnostic();
        }
    };
    let mut owned = OwnedGroup { child, group };
    loop {
        if let Some(status) = owned.child.try_wait().into_diagnostic()? {
            return Ok(ExitCode::from(
                status
                    .code()
                    .and_then(|code| return u8::try_from(code).ok())
                    .unwrap_or(1),
            ));
        }
        if cancelled.load(Ordering::Relaxed)
            || !parent_alive(arguments.parent_pid, arguments.parent_start_ticks)
        {
            return Ok(ExitCode::FAILURE);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn parent_alive(pid: u32, start_ticks: u64) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, suffix)) = stat.rsplit_once(')') else {
        return false;
    };
    let mut fields = suffix.split_whitespace();
    if matches!(fields.next(), None | Some("Z" | "X")) {
        return false;
    }
    return fields
        .nth(18)
        .and_then(|ticks| return ticks.parse::<u64>().ok())
        == Some(start_ticks);
}
