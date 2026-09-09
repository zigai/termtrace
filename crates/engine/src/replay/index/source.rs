use super::super::invalid_replay;
use crate::{Error, Result};
use std::{borrow::Cow, iter::from_fn, path::Path};
#[cfg(test)]
use termtrace_evidence::event_log::EventLog;
use termtrace_evidence::event_log::{EventLogCursor, EventLogSummary};
use termtrace_model::execution::event::{EventLogHeader, EventRecord};

pub(super) type Records<'source> =
    Box<dyn Iterator<Item = Result<Cow<'source, EventRecord>>> + 'source>;

pub(super) enum IndexSource<'source> {
    #[cfg(test)]
    Memory(&'source EventLog),
    File(&'source Path, &'source EventLogSummary),
}

impl IndexSource<'_> {
    pub(super) fn header(&self) -> &EventLogHeader {
        return match self {
            #[cfg(test)]
            Self::Memory(log) => &log.header,
            Self::File(_, summary) => &summary.header,
        };
    }

    pub(super) fn final_hash(&self) -> Option<&str> {
        return match self {
            #[cfg(test)]
            Self::Memory(log) => log.events.last().map(|record| return record.hash.as_str()),
            Self::File(_, summary) => summary
                .terminal
                .as_ref()
                .map(|record| return record.hash.as_str()),
        };
    }

    /// Each pass must end at the same validated prefix, even while the live file grows.
    pub(super) fn records(&self) -> Result<Records<'_>> {
        match self {
            #[cfg(test)]
            Self::Memory(log) => {
                return Ok(Box::new(
                    log.events
                        .iter()
                        .map(|record| return Ok(Cow::Borrowed(record))),
                ));
            }
            Self::File(path, summary) => {
                let mut cursor = EventLogCursor::open(path)?;

                if cursor.header() != &summary.header {
                    return Err(invalid_replay("index source header changed"));
                }

                let mut read = 0_u64;
                let mut finished = false;

                return Ok(Box::new(from_fn(move || {
                    if finished {
                        return None;
                    }

                    if read == summary.event_count {
                        finished = true;
                        let validation = summary.terminal.as_ref().map_or_else(
                            || return cursor.validate_source_identity(),
                            |terminal| {
                                return cursor
                                    .validate_boundary(summary.event_count, &terminal.hash);
                            },
                        );
                        return validation.err().map(|error| return Err(Error::from(error)));
                    }

                    match cursor.next_record() {
                        Ok(Some(record)) => {
                            read += 1;
                            return Some(Ok(Cow::Owned(record)));
                        }
                        Ok(None) => {
                            finished = true;
                            return Some(Err(invalid_replay(
                                "index source ended before its validated boundary",
                            )));
                        }
                        Err(error) => {
                            finished = true;
                            return Some(Err(Error::from(error)));
                        }
                    }
                })));
            }
        }
    }
}
