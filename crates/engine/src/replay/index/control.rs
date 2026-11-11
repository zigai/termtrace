use super::{
    super::{invalid_replay, json_error},
    seekable::SeekableWriter,
    source::IndexSource,
    write_json_line,
};
use crate::{Error, Result};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};
use termtrace_model::{
    execution::event::Event,
    terminal::{
        replay::{ControlSourceIndexEntry, SourceRange},
        vt::VtToken,
    },
};
use termtrace_terminal::VtFramer;

pub(super) fn write_control_index(
    directory: &Path,
    source: &IndexSource<'_>,
    range: Option<SourceRange>,
) -> Result<()> {
    let path = directory.join(super::CONTROL_INDEX);
    let source_path = directory.join(super::CONTROL_SOURCE_INDEX);
    let mut output = ControlWriter {
        writer: SeekableWriter::create(&path)?,
        path,
        sources: SeekableWriter::create(&source_path)?,
        source_path,
        current_source: None,
        file_offset: 0,
        range,
    };

    let mut framer = VtFramer::with_default_bound()?;

    for record in source.records()? {
        let record = record?;

        if let Event::Output { data } = &record.event {
            for token in framer.feed(record.seq, record.monotonic_ns, data.as_bytes())? {
                output.append(token)?;
            }
        }
    }

    for token in framer.finish()? {
        output.append(token)?;
    }

    if let Some(entry) = output.current_source.take() {
        write_json_line(&mut output.sources, &entry, &output.source_path)?;
    }

    output.writer.finish()?;

    return output.sources.finish();
}

struct ControlWriter {
    writer: SeekableWriter,
    path: PathBuf,
    sources: SeekableWriter,
    source_path: PathBuf,
    current_source: Option<ControlSourceIndexEntry>,
    file_offset: u64,
    range: Option<SourceRange>,
}

impl ControlWriter {
    fn append(&mut self, token: VtToken) -> Result<()> {
        if token.source.is_empty()
            || self.range.is_some_and(|range| {
                return !token
                    .source
                    .iter()
                    .any(|segment| return range.contains(segment.monotonic_ns));
            })
        {
            return Ok(());
        }

        // Framing consumes canonical bytes in order. Once a later source event
        // appears, no later token can add an earlier event's first file offset.
        for source in &token.source {
            match self.current_source.as_ref() {
                Some(current) if current.event_seq > source.event_seq => {
                    return Err(invalid_replay("control token source order moved backwards"));
                }
                Some(current) if current.event_seq == source.event_seq => continue,
                _ => {}
            }

            if let Some(entry) = self.current_source.take() {
                write_json_line(&mut self.sources, &entry, &self.source_path)?;
            }

            self.current_source = Some(ControlSourceIndexEntry {
                event_seq: source.event_seq,
                control_index_offset: self.file_offset,
            });
        }

        let encoded = serde_json::to_vec(&token)
            .map_err(|source| return json_error(self.path.clone(), source))?;
        self.writer
            .write_all(&encoded)
            .and_then(|()| return self.writer.write_all(b"\n"))
            .map_err(|source| {
                return Error::Filesystem {
                    path: self.path.clone(),
                    source,
                };
            })?;

        self.file_offset = self
            .file_offset
            .checked_add(u64::try_from(encoded.len()).unwrap_or(u64::MAX))
            .and_then(|offset| return offset.checked_add(1))
            .ok_or_else(|| return invalid_replay("control index byte offset overflow"))?;

        return Ok(());
    }
}
