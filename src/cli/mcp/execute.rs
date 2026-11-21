use nix::sys::signal::Signal;
use rmcp::{model::ProgressNotificationParam, service::RequestContext, RoleServer};
use std::{
    env, io,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};
use termtrace_process::ProcessGroup;
use tokio::{
    process::Command,
    time::{self, Instant},
};

const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const DELETED_SUFFIX: &str = " (deleted)";

pub(super) struct Completed {
    pub(super) succeeded: bool,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: String,
}

pub(super) enum Failure {
    Cancelled,
    Unavailable(String),
}

pub(super) async fn execute(
    arguments: &[String],
    context: &RequestContext<RoleServer>,
) -> Result<Completed, Failure> {
    // Invocations spawn and attach through this executable's path, which a reinstall replaces.
    let current = env::current_exe().map_err(|error| return unavailable(&error))?;
    let executable = installed_path(&current);
    if !executable.exists() {
        return Err(Failure::Unavailable(format!(
            "{} was replaced or removed after this MCP server started; restart the MCP server",
            executable.display()
        )));
    }

    // Standard input carries the MCP protocol, so invocations never read it.
    let child = Command::new(executable)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| return unavailable(&error))?;

    let group = child.id().and_then(ProcessGroup::from_leader);
    let output = child.wait_with_output();

    tokio::pin!(output);

    let progress_token = context.meta.get_progress_token();
    let started = Instant::now();
    let mut ticks = time::interval_at(started + PROGRESS_INTERVAL, PROGRESS_INTERVAL);
    loop {
        tokio::select! {
            output = &mut output => {
                return output.map(completed).map_err(|error| return unavailable(&error));
            }
            () = context.ct.cancelled() => {
                if let Some(group) = group {
                    let _signal = group.signal(Signal::SIGTERM);
                    if time::timeout(TERMINATION_GRACE, &mut output).await.is_err() {
                        let _kill = group.kill();
                        let _output = output.await;
                    }

                    // Helpers the invocation left behind die with it.
                    let _kill = group.kill();
                }

                return Err(Failure::Cancelled);
            }
            _ = ticks.tick() => {
                if let Some(token) = &progress_token {
                    let elapsed = started.elapsed();
                    let notification =
                        ProgressNotificationParam::new(token.clone(), elapsed.as_secs_f64())
                            .with_message(format!("running for {}s", elapsed.as_secs()));

                    // A client that stopped listening still receives the result.
                    let _sent = context.peer.notify_progress(notification).await;
                }
            }
        }
    }
}

// A replaced executable keeps running from its unlinked inode, which the kernel marks.
fn installed_path(current: &Path) -> PathBuf {
    return current
        .to_str()
        .and_then(|text| return text.strip_suffix(DELETED_SUFFIX))
        .map_or_else(|| return current.to_path_buf(), PathBuf::from);
}

fn completed(output: Output) -> Completed {
    return Completed {
        succeeded: output.status.success(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr)
            .trim_end()
            .to_owned(),
    };
}

fn unavailable(error: &io::Error) -> Failure {
    return Failure::Unavailable(error.to_string());
}
