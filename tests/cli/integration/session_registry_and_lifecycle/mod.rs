//! session registry and lifecycle CLI integration tests.

use crate::support::*;
use termtrace_model::protocol::cli_response::CliResponseEnvelope;
mod binding;
mod gc;
mod restart;
mod start;

fn table_column_starts(line: &str) -> Vec<usize> {
    if line.is_empty() {
        return Vec::new();
    }
    let mut starts = vec![0];
    let mut separator_start = None;
    for (index, byte) in line.bytes().enumerate() {
        if byte == b' ' {
            if separator_start.is_none() {
                separator_start = Some(index);
            }
        } else if let Some(separator) = separator_start.take()
            && index.saturating_sub(separator) >= 2
        {
            starts.push(index);
        }
    }
    return starts;
}

fn table_cells<'line>(line: &'line str, starts: &[usize]) -> Vec<&'line str> {
    return starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(line.len());
            return line.get(*start..end).map_or("", str::trim);
        })
        .collect();
}
