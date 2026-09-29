//! tmux command construction, execution, and output parsing.

use crate::{
    artifact::{TmuxPaneSnapshot, TmuxServerIdentity, TmuxSocket},
    error::{Error, Result},
    options::{self, HOOK_INDEX, WRAPPER_BINARY_ENV},
};
use std::{
    env,
    ffi::{OsStr, OsString},
    fmt::{Display, Write as _},
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::{FileTypeExt as _, MetadataExt as _},
    },
    path::{Path, PathBuf},
    process::Command,
    str,
};
use termtrace_model::value::ByteString;

const FIELD: u8 = 0x1f;
const ROW_END: &[u8] = b"\x1e\n";

/// One tmux client invocation target.
#[derive(Clone, Debug)]
pub struct TmuxClient {
    /// tmux executable.
    pub executable: PathBuf,
    /// Server socket selection.
    pub socket: TmuxSocket,
}

/// Process marks cached on a pane by scope decisions and enabling.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PaneMarks {
    /// Process that predates enabling and is never enforced.
    pub grandfathered: Option<u32>,
    /// Process decided in scope.
    pub selected: Option<u32>,
    /// Process decided out of scope.
    pub excluded: Option<u32>,
}

/// Everything one `list-panes` row reports about a pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneRow {
    /// Whether the pane's process exited while the pane remains.
    pub dead: bool,
    /// Pane root process.
    pub pid: Option<u32>,
    /// Cached scope and enabling marks.
    pub marks: PaneMarks,
    /// Window name bytes.
    pub window_name: ByteString,
    /// Current foreground command name bytes.
    pub current_command: ByteString,
    /// Aggregate snapshot fields.
    pub snapshot: TmuxPaneSnapshot,
}

impl TmuxClient {
    /// A client for the configured socket.
    pub fn new(executable: PathBuf, socket: TmuxSocket) -> Self {
        return Self { executable, socket };
    }

    /// The client that invoked this process through a tmux hook or wrapper.
    pub fn inherited() -> Self {
        return Self {
            executable: env::var_os(WRAPPER_BINARY_ENV)
                .map_or_else(|| return PathBuf::from("tmux"), PathBuf::from),
            socket: TmuxSocket::Default,
        };
    }

    /// Socket selection arguments, e.g. `-L name`.
    pub fn socket_arguments(&self) -> Vec<OsString> {
        return match &self.socket {
            TmuxSocket::Default => Vec::new(),
            TmuxSocket::Name(name) => vec![
                OsString::from("-L"),
                OsStr::from_bytes(name.as_bytes()).to_owned(),
            ],
            TmuxSocket::Path(path) => vec![
                OsString::from("-S"),
                OsStr::from_bytes(path.as_bytes()).to_owned(),
            ],
        };
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.args(self.socket_arguments());

        return command;
    }

    /// Run a tmux command and return stdout without its final newline.
    ///
    /// # Errors
    /// Returns [`Error::Tmux`] when tmux cannot run or fails.
    pub fn output<I, S>(&self, arguments: I) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command();
        command.args(arguments);

        return self.checked(command);
    }

    /// Run a tmux command for its effect.
    ///
    /// # Errors
    /// Returns [`Error::Tmux`] when tmux cannot run or fails.
    pub fn run<I, S>(&self, arguments: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let _ = self.output(arguments)?;

        return Ok(());
    }

    fn checked(&self, mut command: Command) -> Result<Vec<u8>> {
        let output = command.output().map_err(|error| {
            return Error::Tmux(format!(
                "could not run tmux executable {}: {error}",
                self.executable.display()
            ));
        })?;

        if !output.status.success() {
            return Err(Error::Tmux(format!(
                "tmux command failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        return Ok(trim_one_newline(output.stdout));
    }

    /// Read a global option.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn option(&self, name: &str) -> Result<Vec<u8>> {
        return self.output(["show-options", "-gqv", name]);
    }

    /// Read a global option as UTF-8.
    ///
    /// # Errors
    /// Returns tmux failures or non-UTF-8 values.
    pub fn option_text(&self, name: &str) -> Result<String> {
        let bytes = self.option(name)?;

        return String::from_utf8(bytes).map_err(|error| {
            return Error::Tmux(format!("tmux option {name} is not UTF-8: {error}"));
        });
    }

    /// Read a global option as a path, or none when empty.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn option_path(&self, name: &str) -> Result<Option<PathBuf>> {
        let bytes = self.option(name)?;

        return Ok((!bytes.is_empty()).then(|| return PathBuf::from(OsString::from_vec(bytes))));
    }

    /// Parse a global option.
    ///
    /// # Errors
    /// Returns tmux failures or unparsable values.
    pub fn parse_option<T>(&self, name: &str) -> Result<T>
    where
        T: str::FromStr,
        T::Err: Display,
    {
        return self
            .option_text(name)?
            .parse()
            .map_err(|error| return Error::Tmux(format!("invalid tmux option {name}: {error}")));
    }

    /// Whether the current integration version is enabled.
    pub fn integration_enabled(&self) -> bool {
        return self.option_text(options::VERSION).ok().as_deref()
            == Some(options::INTEGRATION_VERSION);
    }

    /// Read a pane option.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn pane_option(&self, pane: &str, name: &str) -> Result<Vec<u8>> {
        return self.output(["show-options", "-pqv", "-t", pane, name]);
    }

    /// Expand a format for a pane.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn pane_format(&self, pane: &str, format: &str) -> Result<Vec<u8>> {
        return self.output(["display-message", "-p", "-t", pane, format]);
    }

    /// Expand a UTF-8 format for a pane.
    ///
    /// # Errors
    /// Returns tmux failures or non-UTF-8 output.
    pub fn pane_text(&self, pane: &str, format: &str) -> Result<String> {
        return String::from_utf8(self.pane_format(pane, format)?).map_err(|error| {
            return Error::Tmux(format!("tmux format {format} is not UTF-8: {error}"));
        });
    }

    /// Expand and parse a format for a pane.
    ///
    /// # Errors
    /// Returns tmux failures or unparsable output.
    pub fn pane_number<T>(&self, pane: &str, format: &str) -> Result<T>
    where
        T: str::FromStr,
        T::Err: Display,
    {
        return self
            .pane_text(pane, format)?
            .parse()
            .map_err(|error| return Error::Tmux(format!("invalid tmux format {format}: {error}")));
    }

    /// Resolve a user target to a stable object ID through `format`.
    ///
    /// # Errors
    /// Returns unavailable targets or invalid identifiers.
    pub fn resolve_target(
        &self,
        target: &OsStr,
        format: &str,
        prefix: u8,
        kind: &str,
    ) -> Result<String> {
        let mut command = self.command();
        command
            .arg("display-message")
            .arg("-p")
            .arg("-t")
            .arg(target)
            .arg(format);
        let bytes = self.checked(command).map_err(|error| {
            return Error::Tmux(format!(
                "tmux target {} is unavailable: {error}",
                target.to_string_lossy()
            ));
        })?;

        return parse_identifier(Some(&bytes), prefix, kind);
    }

    /// Set a global option.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn set_option(&self, name: &str, value: &OsStr) -> Result<()> {
        let mut command = self.command();
        command.arg("set-option").arg("-gq").arg(name).arg(value);

        return self.checked(command).map(drop);
    }

    /// Remove a global option.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn unset_option(&self, name: &str) -> Result<()> {
        return self.run(["set-option", "-gqu", name]);
    }

    /// Set a pane option.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn set_pane_option(&self, pane: &str, name: &str, value: &OsStr) -> Result<()> {
        let mut command = self.command();
        command
            .arg("set-option")
            .arg("-pq")
            .arg("-t")
            .arg(pane)
            .arg(name)
            .arg(value);

        return self.checked(command).map(drop);
    }

    /// Remove a pane option.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn unset_pane_option(&self, pane: &str, name: &str) -> Result<()> {
        return self.run(["set-option", "-pqu", "-t", pane, name]);
    }

    /// Read Termtrace's guarded hook command.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn hook(&self, name: &str) -> Result<Vec<u8>> {
        let hook = hook_name(name);
        let output = self.output(["show-hooks", "-g", hook.as_str()])?;
        let mut prefix = hook.into_bytes();
        prefix.push(b' ');

        return Ok(output
            .strip_prefix(prefix.as_slice())
            .map_or_else(Vec::new, <[u8]>::to_vec));
    }

    /// Install Termtrace's guarded hook command.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn set_hook(&self, name: &str, value: &OsStr) -> Result<()> {
        let mut command = self.command();
        command
            .arg("set-hook")
            .arg("-g")
            .arg(hook_name(name))
            .arg(value);

        return self.checked(command).map(drop);
    }

    /// Remove Termtrace's guarded hook command.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn unset_hook(&self, name: &str) -> Result<()> {
        return self.run(["set-hook", "-gu", hook_name(name).as_str()]);
    }

    /// Run a shell command in the background under the tmux server.
    ///
    /// # Errors
    /// Returns tmux failures.
    pub fn run_shell_background(&self, value: &OsStr) -> Result<()> {
        let mut command = self.command();
        command.arg("run-shell").arg("-b").arg(value);

        return self.checked(command).map(drop);
    }

    /// Observe the answering server's PID and socket inode.
    ///
    /// # Errors
    /// Returns tmux failures or a socket path that is not a Unix socket.
    pub fn server_identity(&self) -> Result<TmuxServerIdentity> {
        let output = self.output(["display-message", "-p", "#{pid}\u{1f}#{socket_path}"])?;
        let separator = output
            .iter()
            .position(|byte| return *byte == FIELD)
            .ok_or_else(|| {
                return Error::Tmux("tmux returned an invalid server identity".to_owned());
            })?;
        let (pid, socket) = output.split_at(separator);
        let socket = socket.get(1..).unwrap_or_default();
        let pid = str::from_utf8(pid)
            .ok()
            .and_then(|pid| return pid.parse::<u32>().ok())
            .ok_or_else(|| return Error::Tmux("tmux returned an invalid server PID".to_owned()))?;

        if socket.is_empty() {
            return Err(Error::Tmux(
                "tmux returned an empty server socket path".to_owned(),
            ));
        }

        let path = Path::new(OsStr::from_bytes(socket));
        let metadata = fs_err::symlink_metadata(path)
            .map_err(|error| return Error::filesystem(path, error))?;

        if !metadata.file_type().is_socket() {
            return Err(Error::Tmux(format!(
                "tmux server path is not a Unix socket: {}",
                path.display()
            )));
        }
        return Ok(TmuxServerIdentity {
            pid,
            socket_path: ByteString::from(socket),
            socket_device: metadata.dev(),
            socket_inode: metadata.ino(),
        });
    }

    /// One snapshot of every pane on the server.
    ///
    /// # Errors
    /// Returns tmux failures or malformed rows.
    pub fn list_panes(&self) -> Result<Vec<PaneRow>> {
        let mut output = self.output(["list-panes", "-a", "-F", row_format().as_str()])?;
        if !output.is_empty() {
            output.push(b'\n');
        }

        return parse_rows(&output);
    }

    /// One snapshot of a single pane.
    ///
    /// # Errors
    /// Returns tmux failures, including an absent pane, or a malformed row.
    pub fn pane_row(&self, pane: &str) -> Result<PaneRow> {
        let mut output = self.pane_format(pane, row_format().as_str())?;
        output.push(b'\n');

        return parse_rows(&output)?
            .into_iter()
            .next()
            .ok_or_else(|| return Error::Tmux(format!("tmux pane {pane} is unavailable")));
    }
}

fn row_format() -> String {
    let fields = [
        "#{pane_dead}".to_owned(),
        "#{pane_id}".to_owned(),
        "#{pane_pid}".to_owned(),
        format!("#{{{}}}", options::GRANDFATHERED_PID),
        format!("#{{{}}}", options::SELECTED_PID),
        format!("#{{{}}}", options::EXCLUDED_PID),
        "#{session_id}".to_owned(),
        "#{session_name}".to_owned(),
        "#{window_id}".to_owned(),
        "#{window_index}".to_owned(),
        "#{window_name}".to_owned(),
        "#{window_layout}".to_owned(),
        "#{window_active}".to_owned(),
        "#{window_zoomed_flag}".to_owned(),
        "#{pane_active}".to_owned(),
        "#{pane_in_mode}".to_owned(),
        "#{pane_mode}".to_owned(),
        "#{pane_current_command}".to_owned(),
        format!("#{{{}}}", options::PANE_SESSION),
    ];

    return format!("{}\u{1e}", fields.join("\u{1f}"));
}

/// Parse `list-panes` output framed by [`row_format`]; option values may contain newlines.
fn parse_rows(output: &[u8]) -> Result<Vec<PaneRow>> {
    let mut rows = Vec::new();
    let mut rest = output;

    while !rest.is_empty() {
        let end = rest
            .windows(ROW_END.len())
            .position(|window| return window == ROW_END)
            .ok_or_else(|| {
                return Error::Tmux("tmux returned an unterminated pane row".to_owned());
            })?;
        let (row, tail) = rest.split_at(end);
        rest = tail.get(ROW_END.len()..).unwrap_or_default();
        rows.push(parse_row(row)?);
    }

    return Ok(rows);
}

fn parse_row(row: &[u8]) -> Result<PaneRow> {
    let fields = row.split(|byte| return *byte == FIELD).collect::<Vec<_>>();
    let [dead, pane_id, pid, grandfathered, selected, excluded, session_id, session_name, window_id, window_index, window_name, layout, window_active, zoomed, pane_active, in_mode, pane_mode, current_command, recording_session] =
        fields.as_slice()
    else {
        return Err(Error::Tmux("tmux returned an invalid pane row".to_owned()));
    };

    let text = |field: &[u8]| -> Result<String> {
        return String::from_utf8(field.to_vec()).map_err(|source| {
            return Error::invalid_with_source("tmux returned a non-UTF-8 pane field", source);
        });
    };

    let number = |field: &[u8]| -> Option<u32> {
        return str::from_utf8(field).ok()?.parse().ok();
    };

    return Ok(PaneRow {
        dead: *dead == b"1",
        pid: number(pid),
        marks: PaneMarks {
            grandfathered: number(grandfathered),
            selected: number(selected),
            excluded: number(excluded),
        },
        window_name: ByteString::from(*window_name),
        current_command: ByteString::from(*current_command),
        snapshot: TmuxPaneSnapshot {
            session_id: parse_identifier(Some(session_id), b'$', "session")?,
            session_name: text(session_name)?,
            window_id: parse_identifier(Some(window_id), b'@', "window")?,
            window_index: number(window_index).ok_or_else(|| {
                return Error::Tmux("tmux returned an invalid window index".to_owned());
            })?,
            layout: text(layout)?,
            window_active: *window_active == b"1",
            zoomed: *zoomed == b"1",
            pane_id: parse_identifier(Some(pane_id), b'%', "pane")?,
            pane_active: *pane_active == b"1",
            copy_mode: *in_mode != b"0",
            pane_mode: text(pane_mode)?,
            recording_session: (!recording_session.is_empty())
                .then(|| return ByteString::from(*recording_session)),
        },
    });
}

/// Whether both paths name the same executable after canonicalization.
pub fn same_executable(left: &Path, right: &Path) -> bool {
    let left = fs_err::canonicalize(left).unwrap_or_else(|_| return left.to_path_buf());
    let right = fs_err::canonicalize(right).unwrap_or_else(|_| return right.to_path_buf());

    return left == right;
}

/// Whether `value` is a stable tmux ID such as `%3` with the given prefix.
pub fn valid_identifier(value: &str, prefix: u8) -> bool {
    let bytes = value.as_bytes();

    return bytes.first() == Some(&prefix)
        && bytes.len() > 1
        && bytes
            .get(1..)
            .is_some_and(|suffix| return suffix.iter().all(u8::is_ascii_digit));
}

/// Parse a stable tmux ID field.
///
/// # Errors
/// Returns an invalid identifier.
pub fn parse_identifier(value: Option<&[u8]>, prefix: u8, kind: &str) -> Result<String> {
    return value
        .and_then(|value| return str::from_utf8(value).ok())
        .filter(|value| return valid_identifier(value, prefix))
        .map(str::to_owned)
        .ok_or_else(|| return Error::Tmux(format!("tmux returned an invalid {kind} ID")));
}

/// Guarded hook name at Termtrace's reserved index.
pub fn hook_name(name: &str) -> String {
    return format!("{name}[{HOOK_INDEX}]");
}

fn trim_one_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }

    return bytes;
}

/// Quote bytes for a POSIX shell.
pub fn shell_quote(bytes: &[u8]) -> Vec<u8> {
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

/// Quote bytes as a tmux configuration string with octal escapes.
pub fn config_quote(bytes: &[u8]) -> String {
    let mut quoted = String::from("\"");
    for byte in bytes {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "`fmt::Write` for `String` never fails, as documented on `format!`, and `drop_non_drop` rejects dropping the unit result"
        )]
        let _ = write!(quoted, "\\{byte:03o}");
    }

    quoted.push('"');

    return quoted;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_keep_newlines_inside_option_values() {
        let row = |pane: &str, session: &[u8]| {
            let mut bytes = format!(
                "0\x1f{pane}\x1f42\x1f\x1f42\x1f\x1f$1\x1fmain\x1f@2\x1f0\x1fshell\x1flayout\x1f1\x1f0\x1f1\x1f0\x1f\x1fsh\x1f"
            )
            .into_bytes();
            bytes.extend_from_slice(session);
            bytes.extend_from_slice(ROW_END);

            return bytes;
        };
        let mut output = row("%1", b"/state/a\nb");
        output.extend(row("%2", b""));
        let rows = parse_rows(&output).expect("framed rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.first()
                .and_then(|row| return row.snapshot.recording_session.clone()),
            Some(ByteString::from(b"/state/a\nb".as_slice()))
        );
        assert_eq!(
            rows.first().map(|row| return row.marks.selected),
            Some(Some(42))
        );
        assert!(rows
            .get(1)
            .is_some_and(|row| return row.snapshot.recording_session.is_none()));
        assert!(parse_rows(b"0\x1f%1\x1e\n").is_err());
    }
}
