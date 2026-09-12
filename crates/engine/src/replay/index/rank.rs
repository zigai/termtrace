use super::{
    super::{invalid_replay, json_error, timeline_event},
    source::Records,
};
use crate::{Error, Result};
use std::{
    fs::File,
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
};
use termtrace_model::terminal::{replay::SourceRange, vt::VtToken};

pub(super) type TimelineKey = (u64, u64, u8, u64);

/// Merge monotonically ordered canonical/control cursors instead of retaining all keys.
pub(super) struct TimelineRanks<'source> {
    canonical: Records<'source>,
    canonical_next: Option<TimelineKey>,
    canonical_count: u64,
    controls: BufReader<File>,
    controls_path: PathBuf,
    control_next: Option<(TimelineKey, u64)>,
    control_count: u64,
    control_offset: u64,
    source_range: Option<SourceRange>,
}

impl<'source> TimelineRanks<'source> {
    pub(super) fn new(
        directory: &Path,
        canonical: Records<'source>,
        source_range: Option<SourceRange>,
    ) -> Result<Self> {
        let path = directory.join("control-index.jsonl");
        let file = File::open(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        let mut ranks = Self {
            canonical,
            canonical_next: None,
            canonical_count: 0,
            controls: BufReader::new(file),
            controls_path: path,
            control_next: None,
            control_count: 0,
            control_offset: 0,
            source_range,
        };
        ranks.advance_canonical()?;
        ranks.advance_control()?;

        return Ok(ranks);
    }

    fn advance_canonical(&mut self) -> Result<()> {
        self.canonical_next = None;

        for record in self.canonical.by_ref() {
            let record = record?;
            if self
                .source_range
                .is_none_or(|range| return range.contains(record.monotonic_ns))
                && timeline_event(&record.event)
            {
                self.canonical_next = Some((record.monotonic_ns, record.seq, 0, 0));
                break;
            }
        }

        return Ok(());
    }

    fn advance_control(&mut self) -> Result<()> {
        let mut line = String::new();
        let bytes = self.controls.read_line(&mut line).map_err(|source| {
            return Error::Filesystem {
                path: self.controls_path.clone(),
                source,
            };
        })?;

        self.control_next = None;

        if bytes == 0 {
            return Ok(());
        }

        let token: VtToken = serde_json::from_str(&line)
            .map_err(|source| return json_error(self.controls_path.clone(), source))?;

        let first = token
            .source
            .first()
            .ok_or_else(|| return invalid_replay("control index token has no source"))?;
        self.control_next = Some((
            (first.monotonic_ns, first.event_seq, 1, token.token_id),
            self.control_offset,
        ));

        self.control_offset = self
            .control_offset
            .checked_add(u64::try_from(bytes).unwrap_or(u64::MAX))
            .ok_or_else(|| return invalid_replay("control index byte offset overflow"))?;

        return Ok(());
    }

    pub(super) fn before(&mut self, key: TimelineKey, state_id: u64) -> Result<(u64, u64, u64)> {
        while self.canonical_next.is_some_and(|next| return next < key) {
            self.canonical_count = self
                .canonical_count
                .checked_add(1)
                .ok_or_else(|| return invalid_replay("canonical timeline rank overflow"))?;
            self.advance_canonical()?;
        }

        while self.control_next.is_some_and(|(next, _)| return next < key) {
            self.control_count = self
                .control_count
                .checked_add(1)
                .ok_or_else(|| return invalid_replay("control timeline rank overflow"))?;
            self.advance_control()?;
        }

        let rank = state_id
            .checked_add(self.canonical_count)
            .and_then(|rank| return rank.checked_add(self.control_count))
            .ok_or_else(|| return invalid_replay("checkpoint timeline rank overflow"))?;

        return Ok((
            rank,
            self.control_next
                .map_or(self.control_offset, |(_, offset)| return offset),
            self.control_count,
        ));
    }
}
