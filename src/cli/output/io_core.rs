#![expect(
    dead_code,
    reason = "Capture tracking utilities preserved for test harness"
)]

use super::*;

pub(in crate::cli) fn write_progress(response: &ResponseWriter, message: impl Display) {
    if response.mode() == ResponseMode::Quiet {
        return;
    }

    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = writeln!(stderr, "{message}");
}

#[derive(Default)]
struct CountingWriter(usize);

impl io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());

        return Ok(buf.len());
    }

    fn flush(&mut self) -> io::Result<()> {
        return Ok(());
    }
}

pub(super) fn capture_json(
    response: &mut ResponseWriter,
    value: &impl serde::Serialize,
) -> Result<()> {
    let encoded = serde_json::to_vec(value).into_diagnostic()?;
    let encoded_bytes = encoded.len();

    let state = &mut response.state;

    if state.captured_bytes.saturating_add(encoded_bytes) > MAX_CAPTURE_BYTES {
        state.omitted_items = state.omitted_items.saturating_add(1);

        let value: serde_json::Value = serde_json::from_slice(&encoded).into_diagnostic()?;

        if let Some(pagination) = bounded_capture_projection(&value) {
            let mut counter = CountingWriter::default();
            serde_json::to_writer(&mut counter, &pagination).into_diagnostic()?;
            state.captured_bytes = state.captured_bytes.saturating_add(counter.0);
            state.values.push(pagination);
        }

        return Ok(());
    }

    state.captured_bytes = state.captured_bytes.saturating_add(encoded_bytes);
    let value: serde_json::Value = serde_json::from_slice(&encoded).into_diagnostic()?;
    state.values.push(value);

    return Ok(());
}

pub(super) fn capture_raw(response: &mut ResponseWriter, output: &[u8]) {
    let state = &mut response.state;

    if let Some(page) = raw_pagination_projection(output) {
        state.values.push(page);
    }

    state.captured_bytes = state.captured_bytes.saturating_add(output.len());
    let remaining = MAX_CAPTURE_BYTES.saturating_sub(state.raw.len());
    let keep = remaining.min(output.len());

    if let Some(prefix) = output.get(..keep) {
        state.raw.extend_from_slice(prefix);
    }

    if keep < output.len() {
        state.omitted_items = state.omitted_items.saturating_add(1);
    }
}

pub(super) fn direct_stdout(output: &[u8]) -> Result<ExitCode> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    match stdout
        .write_all(output)
        .and_then(|()| return stdout.flush())
    {
        Ok(()) => return Ok(ExitCode::SUCCESS),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => return Err(error).into_diagnostic(),
    }
}

pub(super) fn direct_stderr(message: &str) {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = writeln!(stderr, "termtrace: {message}");
}

pub(super) fn report_message(error: &miette::Report) -> String {
    let mut message = error.to_string();

    if let Some(help) = error.help() {
        let _ = write!(message, "\nhelp: {help}");
    }

    return message;
}
