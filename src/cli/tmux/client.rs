use std::{
    env,
    ffi::{OsStr, OsString},
    fmt::{Display, Write as _},
    os::unix::ffi::OsStringExt as _,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    str,
};

use miette::Result;

use super::{args::TmuxConnectionArgs, TMUX_HOOK_INDEX, TMUX_WRAPPER_BINARY_ENV};

#[derive(Clone)]
pub(super) struct TmuxClient {
    pub(super) executable: PathBuf,
    pub(super) socket_name: Option<OsString>,
    pub(super) socket_path: Option<PathBuf>,
}

impl TmuxClient {
    pub(super) fn from_connection(connection: TmuxConnectionArgs) -> Self {
        return Self {
            executable: connection.tmux,
            socket_name: connection.socket_name,
            socket_path: connection.socket_path,
        };
    }

    pub(super) fn inherited() -> Self {
        return Self {
            executable: env::var_os(TMUX_WRAPPER_BINARY_ENV)
                .map_or_else(|| return PathBuf::from("tmux"), PathBuf::from),
            socket_name: None,
            socket_path: None,
        };
    }

    pub(super) fn process(&self) -> ProcessCommand {
        let mut process = ProcessCommand::new(&self.executable);
        if let Some(socket_name) = &self.socket_name {
            process.arg("-L").arg(socket_name);
        } else if let Some(socket_path) = &self.socket_path {
            process.arg("-S").arg(socket_path);
        }

        return process;
    }

    pub(super) fn output<I, S>(&self, arguments: I) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.process().args(arguments).output().map_err(|error| {
            return miette::miette!(
                "could not run tmux executable {}: {error}",
                self.executable.display()
            );
        })?;

        if !output.status.success() {
            return Err(miette::miette!(
                "tmux command failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        return Ok(trim_one_newline(output.stdout));
    }

    pub(super) fn run<I, S>(&self, arguments: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let _ = self.output(arguments)?;

        return Ok(());
    }

    pub(super) fn option(&self, name: &str) -> Result<Vec<u8>> {
        return self.output(["show-options", "-gqv", name]);
    }

    pub(super) fn pane_option(&self, pane: &str, name: &str) -> Result<Vec<u8>> {
        return self.output(["show-options", "-pqv", "-t", pane, name]);
    }

    pub(super) fn pane_format(&self, pane: &str, format: &str) -> Result<Vec<u8>> {
        return self.output(["display-message", "-p", "-t", pane, format]);
    }

    pub(super) fn target_format(&self, target: &OsStr, format: &str) -> Result<Vec<u8>> {
        let mut process = self.process();
        process
            .arg("display-message")
            .arg("-p")
            .arg("-t")
            .arg(target)
            .arg(format);
        let output = process.output().map_err(|error| {
            return miette::miette!(
                "could not run tmux executable {}: {error}",
                self.executable.display()
            );
        })?;

        if !output.status.success() {
            return Err(miette::miette!(
                "tmux target {} is unavailable: {}",
                target.to_string_lossy(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        return Ok(trim_one_newline(output.stdout));
    }

    pub(super) fn set_option(&self, name: &str, value: &OsStr) -> Result<()> {
        let mut process = self.process();
        process.arg("set-option").arg("-gq").arg(name).arg(value);

        return checked_status(process, &self.executable);
    }

    pub(super) fn unset_option(&self, name: &str) -> Result<()> {
        return self.run(["set-option", "-gqu", name]);
    }

    pub(super) fn set_pane_option(&self, pane: &str, name: &str, value: &OsStr) -> Result<()> {
        let mut process = self.process();
        process
            .arg("set-option")
            .arg("-pq")
            .arg("-t")
            .arg(pane)
            .arg(name)
            .arg(value);

        return checked_status(process, &self.executable);
    }

    pub(super) fn unset_pane_option(&self, pane: &str, name: &str) -> Result<()> {
        return self.run(["set-option", "-pqu", "-t", pane, name]);
    }

    pub(super) fn hook(&self, name: &str) -> Result<Vec<u8>> {
        let hook = hook_name(name);
        let output = self.output(["show-hooks", "-g", hook.as_str()])?;
        let mut prefix = hook.into_bytes();
        prefix.push(b' ');

        return Ok(output
            .strip_prefix(prefix.as_slice())
            .map_or_else(Vec::new, <[u8]>::to_vec));
    }

    pub(super) fn set_hook(&self, name: &str, command: &OsStr) -> Result<()> {
        let mut process = self.process();
        process
            .arg("set-hook")
            .arg("-g")
            .arg(hook_name(name))
            .arg(command);

        return checked_status(process, &self.executable);
    }

    pub(super) fn unset_hook(&self, name: &str) -> Result<()> {
        return self.run(["set-hook", "-gu", hook_name(name).as_str()]);
    }

    pub(super) fn run_shell_background(&self, command: &OsStr) -> Result<()> {
        let mut process = self.process();
        process.arg("run-shell").arg("-b").arg(command);

        return checked_status(process, &self.executable);
    }
}

pub(super) fn same_executable(left: &Path, right: &Path) -> bool {
    let left = fs_err::canonicalize(left).unwrap_or_else(|_| return left.to_path_buf());
    let right = fs_err::canonicalize(right).unwrap_or_else(|_| return right.to_path_buf());

    return left == right;
}

pub(super) fn option_text(client: &TmuxClient, name: &str) -> Result<String> {
    let bytes = client.option(name)?;

    return str::from_utf8(&bytes)
        .map(str::to_owned)
        .map_err(|error| return miette::miette!("tmux option {name} is not UTF-8: {error}"));
}

pub(super) fn option_path(client: &TmuxClient, name: &str) -> Result<Option<PathBuf>> {
    let bytes = client.option(name)?;

    return Ok((!bytes.is_empty()).then(|| return PathBuf::from(OsString::from_vec(bytes))));
}

pub(super) fn parse_option<T>(client: &TmuxClient, name: &str) -> Result<T>
where
    T: str::FromStr,
    T::Err: Display,
{
    let value = option_text(client, name)?;

    return value
        .parse()
        .map_err(|error| return miette::miette!("invalid tmux option {name}: {error}"));
}

pub(super) fn query_text(client: &TmuxClient, pane: &str, format: &str) -> Result<String> {
    let bytes = client.pane_format(pane, format)?;

    return str::from_utf8(&bytes)
        .map(str::to_owned)
        .map_err(|error| return miette::miette!("tmux format {format} is not UTF-8: {error}"));
}

pub(super) fn query_number<T>(client: &TmuxClient, pane: &str, format: &str) -> Result<T>
where
    T: str::FromStr,
    T::Err: Display,
{
    let value = query_text(client, pane, format)?;

    return value
        .parse()
        .map_err(|error| return miette::miette!("invalid tmux format {format}: {error}"));
}

pub(super) fn valid_tmux_identifier(value: &str, prefix: u8) -> bool {
    let bytes = value.as_bytes();

    return bytes.first() == Some(&prefix)
        && bytes.len() > 1
        && bytes
            .get(1..)
            .is_some_and(|suffix| return suffix.iter().all(u8::is_ascii_digit));
}

pub(super) fn hook_name(name: &str) -> String {
    return format!("{name}[{TMUX_HOOK_INDEX}]");
}

pub(super) fn trim_one_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }

    return bytes;
}

pub(super) fn shell_quote(bytes: &[u8]) -> Vec<u8> {
    let mut quoted = Vec::with_capacity(bytes.len().saturating_add(2));
    quoted.push(b'\'');

    for byte in bytes {
        if *byte == b'\'' {
            quoted.extend_from_slice(b"'\\''");
        } else {
            quoted.push(*byte);
        }
    }

    quoted.push(b'\'');

    return quoted;
}

pub(super) fn tmux_config_quote(bytes: &[u8]) -> String {
    let mut quoted = String::from("\"");
    for byte in bytes {
        let _ = write!(quoted, "\\{byte:03o}");
    }

    quoted.push('"');

    return quoted;
}

pub(super) fn checked_status(mut process: ProcessCommand, executable: &Path) -> Result<()> {
    let output = process.output().map_err(|error| {
        return miette::miette!(
            "could not run tmux executable {}: {error}",
            executable.display()
        );
    })?;

    if !output.status.success() {
        return Err(miette::miette!(
            "tmux command failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    return Ok(());
}
