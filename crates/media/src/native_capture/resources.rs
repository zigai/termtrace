use std::{
    collections::{BTreeSet, VecDeque},
    fs::{self, File},
    io::Read as _,
    os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    path::{Path, PathBuf},
    result::Result as StdResult,
    str,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(target_os = "linux")]
use nix::sys::stat::makedev;
use serde::{Deserialize, Serialize};
use termtrace_evidence::artifact::write_secure;
use termtrace_model::value::ByteString;

use super::{
    Error, Result,
    process::ManagedChild,
    util::{external_error, unix_now_ns},
};

const MAX_RESOURCE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceIdentity {
    path: ByteString,
    kind: String,
    byte_length: u64,
    blake3: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceReport {
    version: u32,
    observed_unix_ns: u128,
    observation: String,
    terminal_pid: Option<u32>,
    resources: Vec<ResourceIdentity>,
    limitations: Vec<String>,
}

/// Snapshot mapped font identities and the executable of this capture's owned terminal.
/// A map is evidence of resource availability, not which glyphs consumed it.
pub(super) fn capture_resources(
    process: &ManagedChild,
    staging: &Path,
    cancelled: &AtomicBool,
    observation: &str,
) -> Result<()> {
    let mut report = ResourceReport {
        version: 1,
        observed_unix_ns: unix_now_ns()?,
        observation: format!("owned_terminal_mapped_resources_{observation}"),
        terminal_pid: None,
        resources: Vec::new(),
        limitations: vec!["mapped_fonts_do_not_identify_per_glyph_resource_use".to_owned()],
    };
    match terminal_pid(process.id()) {
        Ok(Some((pid, executable))) => {
            report.terminal_pid = Some(pid);
            let mut budget = 512 * 1024 * 1024;
            match resource_identity(
                &executable,
                &PathBuf::from(format!("/proc/{pid}/exe")),
                "terminal_executable",
                None,
                &mut budget,
                cancelled,
            ) {
                Ok(identity) => report.resources.push(identity),
                Err(reason) => report.limitations.push(reason),
            }
            match mapped_fonts(pid) {
                Ok(fonts) => {
                    if fonts.is_empty() {
                        report
                            .limitations
                            .push("no_mapped_font_files_observed".to_owned());
                    }
                    for (font, inode, device) in fonts {
                        match resource_identity(
                            &font,
                            &font,
                            "mapped_font",
                            Some((inode, device)),
                            &mut budget,
                            cancelled,
                        ) {
                            Ok(identity) => report.resources.push(identity),
                            Err(reason) => report.limitations.push(reason),
                        }
                    }
                }
                Err(reason) => report.limitations.push(reason),
            }
        }
        Ok(None) => report
            .limitations
            .push("owned_terminal_process_not_observed".to_owned()),
        Err(reason) => report.limitations.push(reason),
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(external_error("original capture", "capture cancelled"));
    }
    let path = staging.join("native-resources.json");
    let bytes = serde_json::to_vec_pretty(&report).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return write_secure(&path, &bytes).map_err(Error::from);
}

fn terminal_pid(root: Option<u32>) -> StdResult<Option<(u32, PathBuf)>, String> {
    let mut queue: VecDeque<u32> = root.into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        if seen.len() > 128 {
            return Err("owned_process_inventory_exceeded_128_processes".to_owned());
        }
        if let Ok(executable) = fs::read_link(format!("/proc/{pid}/exe"))
            && executable
                .file_name()
                .is_some_and(|name| return name == "ghostty")
        {
            return Ok(Some((pid, executable)));
        }
        let path = PathBuf::from(format!("/proc/{pid}/task/{pid}/children"));
        let children = match read_bounded(&path, 64 * 1024) {
            Ok(children) => children,
            Err(_) => continue,
        };
        for child in String::from_utf8_lossy(&children).split_whitespace() {
            if let Ok(pid) = child.parse() {
                queue.push_back(pid);
            }
        }
    }
    return Ok(None);
}

fn mapped_fonts(pid: u32) -> StdResult<BTreeSet<(PathBuf, u64, Option<u64>)>, String> {
    let maps = read_bounded(&PathBuf::from(format!("/proc/{pid}/maps")), 4 * 1024 * 1024)?;
    let text = str::from_utf8(&maps)
        .map_err(|_| return "process_maps_path_encoding_unavailable".to_owned())?;
    let mut paths = BTreeSet::new();
    for line in text.lines() {
        let Some(start) = line.find('/') else {
            continue;
        };
        let path = line
            .get(start..)
            .ok_or_else(|| return "invalid_process_map_path".to_owned())?;
        let path = PathBuf::from(path);
        if path.extension().is_some_and(|extension| {
            return matches!(extension.to_str(), Some("ttf" | "otf" | "ttc" | "otc"));
        }) {
            let mut columns = line.split_whitespace();
            let device = columns
                .nth(3)
                .and_then(|device| return device.split_once(':'))
                .and_then(|(major, minor)| {
                    return Some((
                        u64::from_str_radix(major, 16).ok()?,
                        u64::from_str_radix(minor, 16).ok()?,
                    ));
                })
                .ok_or_else(|| return "invalid_mapped_font_device".to_owned())?;
            let inode = columns
                .next()
                .and_then(|inode| return inode.parse().ok())
                .ok_or_else(|| return "invalid_mapped_font_inode".to_owned())?;
            #[cfg(target_os = "linux")]
            let device = Some(makedev(device.0, device.1));
            #[cfg(not(target_os = "linux"))]
            let device = {
                let _ = device;
                None
            };
            paths.insert((path, inode, device));
        }
        if paths.len() > 128 {
            return Err("mapped_font_inventory_exceeded_128_files".to_owned());
        }
    }
    return Ok(paths);
}

fn resource_identity(
    path: &Path,
    opened_path: &Path,
    kind: &str,
    mapping: Option<(u64, Option<u64>)>,
    budget: &mut u64,
    cancelled: &AtomicBool,
) -> StdResult<ResourceIdentity, String> {
    let mut file = File::open(opened_path).map_err(|_| format!("{kind}_unreadable"))?;
    let before = file
        .metadata()
        .map_err(|_| format!("{kind}_metadata_unavailable"))?;
    if !before.is_file() || before.len() > MAX_RESOURCE_BYTES {
        return Err(format!("{kind}_not_regular_or_exceeds_256_MiB"));
    }
    if before.len() > *budget {
        return Err("native_resource_total_exceeds_512_MiB".to_owned());
    }
    if mapping.is_some_and(|(inode, device)| {
        return inode != before.ino() || device.is_some_and(|device| return device != before.dev());
    }) {
        return Err(format!("{kind}_path_no_longer_matches_process_mapping"));
    }
    *budget = budget.saturating_sub(before.len());
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 65536].into_boxed_slice();
    let mut total = 0_u64;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("native_resource_hash_cancelled".to_owned());
        }
        let count = file
            .read(&mut buffer)
            .map_err(|_| format!("{kind}_hash_unavailable"))?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        if total > before.len() {
            return Err(format!("{kind}_changed_during_hashing"));
        }
        hash.update(
            buffer
                .get(..count)
                .ok_or_else(|| format!("{kind}_invalid_read_count"))?,
        );
    }
    let after = file
        .metadata()
        .map_err(|_| format!("{kind}_metadata_unavailable"))?;
    if total != before.len()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(format!("{kind}_changed_during_hashing"));
    }
    return Ok(ResourceIdentity {
        path: ByteString::from(path.as_os_str().as_bytes()),
        kind: kind.to_owned(),
        byte_length: after.len(),
        blake3: hash.finalize().to_hex().to_string(),
    });
}

fn read_bounded(path: &Path, limit: u64) -> StdResult<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| return file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|_| return "native_resource_inventory_unavailable".to_owned())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err("native_resource_inventory_exceeds_byte_budget".to_owned());
    }
    return Ok(bytes);
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::atomic::AtomicBool};

    use super::resource_identity;

    #[test]
    fn resource_hash_preserves_chunk_boundaries_and_cancellation() {
        let root = tempfile::tempdir().expect("resource directory");
        let path = root.path().join("resource");
        for length in [0_usize, 1, 65535, 65536, 65537, 131_073] {
            let bytes = vec![0x5a_u8; length];
            fs::write(&path, &bytes).expect("resource bytes");
            let mut budget = 200_000_u64;
            let identity = resource_identity(
                &path,
                &path,
                "fixture",
                None,
                &mut budget,
                &AtomicBool::new(false),
            )
            .expect("resource identity");
            let length = u64::try_from(length).expect("length fits u64");
            assert_eq!(identity.byte_length, length);
            assert_eq!(identity.blake3, blake3::hash(&bytes).to_hex().to_string());
            assert_eq!(budget, 200_000 - length);
        }
        let error = resource_identity(
            &path,
            &path,
            "fixture",
            None,
            &mut 200_000,
            &AtomicBool::new(true),
        )
        .err()
        .expect("cancelled hashing");
        assert_eq!(error, "native_resource_hash_cancelled");
    }
}
