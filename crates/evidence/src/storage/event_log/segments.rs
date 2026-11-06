//! Bounded physical chunks behind the canonical JSONL byte stream.
//!
//! The first chunk keeps the original path. Further chunks live in its `.segments`
//! sibling directory. Sealed chunks have independently verifiable BLAKE3 digests;
//! the canonical event hash chain continues across every physical boundary.

use blake3::Hasher;
use nix::libc::O_NOFOLLOW;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::{cell::Cell, thread, time::Duration};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};
use zstd::stream::{read::Decoder, write::Encoder};

const DEFAULT_SEGMENT_BYTES: u64 = 16 * 1024 * 1024;

// Tests model a congested filesystem by delaying every data synchronization of the writers
// created on the current thread.
#[cfg(test)]
thread_local! {
    pub(super) static SYNC_DELAY: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}
const MAX_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    version: u32,
    segment_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Seal {
    index: u64,
    bytes: u64,
    blake3: String,
}

fn directory(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".segments");

    return PathBuf::from(name);
}

fn chunk_path(path: &Path, index: u64, extension: &str) -> PathBuf {
    if index == 0 && extension == "jsonl" {
        return path.to_path_buf();
    }
    return directory(path).join(format!("{index:020}.{extension}"));
}

fn invalid(reason: impl Into<String>) -> io::Error {
    return io::Error::new(io::ErrorKind::InvalidData, reason.into());
}

fn open_regular(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)?;

    if !file.metadata()?.is_file() {
        return Err(invalid("canonical segment must be a regular file"));
    }
    return Ok(file);
}

fn create_private(path: &Path) -> io::Result<File> {
    return OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path);
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let mut bytes = Vec::new();
    open_regular(path)?.take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(invalid("segment metadata exceeds 8 KiB"));
    }
    return serde_json::from_slice(&bytes).map_err(io::Error::other);
}

fn layout(path: &Path) -> io::Result<Option<Layout>> {
    match fs::symlink_metadata(directory(path)) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(invalid("canonical segment directory was replaced"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }

    let value: Layout = match read_json(&directory(path).join("layout.json")) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    if value.version != 1 || !(1024..=MAX_SEGMENT_BYTES).contains(&value.segment_bytes) {
        return Err(invalid("unsupported canonical segment layout"));
    }
    return Ok(Some(value));
}

fn publish_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let temporary = path.with_extension("pending");
    let mut file = create_private(&temporary)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()?;

    fs::rename(&temporary, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| return invalid("segment has no directory"))?;

    File::open(parent)?.sync_all()?;

    return Ok(());
}

/// Append-only physical storage; chunks never exceed the configured size.
#[derive(Debug)]
pub(super) struct SegmentWriter {
    path: PathBuf,
    root: File,
    active: File,
    layout: Layout,
    index: u64,
    bytes: u64,
    hash: Hasher,
    #[cfg(test)]
    sync_delay: Duration,
}

impl SegmentWriter {
    pub(super) fn new(path: &Path, file: File) -> io::Result<Self> {
        return Self::with_segment_bytes(path, file, DEFAULT_SEGMENT_BYTES);
    }

    fn with_segment_bytes(path: &Path, file: File, segment_bytes: u64) -> io::Result<Self> {
        return Ok(Self {
            path: path.to_path_buf(),
            root: file.try_clone()?,
            active: file,
            layout: Layout {
                version: 1,
                segment_bytes,
            },
            index: 0,
            bytes: 0,
            hash: Hasher::new(),
            #[cfg(test)]
            sync_delay: SYNC_DELAY.get(),
        });
    }

    pub(super) fn metadata(&self) -> io::Result<Metadata> {
        return self.root.metadata();
    }

    pub(super) fn sync_data(&self) -> io::Result<()> {
        #[cfg(test)]
        thread::sleep(self.sync_delay);

        return self.active.sync_data();
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.active.sync_all()?;
        let parent = directory(&self.path);

        if self.index == 0 {
            fs::DirBuilder::new().mode(0o700).create(&parent)?;
            publish_json(&parent.join("layout.json"), &self.layout)?;
            File::open(
                parent
                    .parent()
                    .ok_or_else(|| return invalid("segment root has no parent"))?,
            )?
            .sync_all()?;
        }

        let seal = Seal {
            index: self.index,
            bytes: self.bytes,
            blake3: self.hash.finalize().to_hex().to_string(),
        };

        // Keep the root file inode stable for authenticated live cursors. Every
        // subsequent sealed chunk is compressed with bounded streaming buffers.
        if self.index > 0 {
            let compressed = chunk_path(&self.path, self.index, "zst");
            let temporary = compressed.with_extension("zst.pending");
            let mut input = open_regular(&chunk_path(&self.path, self.index, "jsonl"))?;
            let mut encoder = Encoder::new(create_private(&temporary)?, 3)?;
            io::copy(&mut input, &mut encoder)?;
            encoder.finish()?.sync_all()?;

            fs::rename(&temporary, &compressed)?;
        }

        publish_json(&chunk_path(&self.path, self.index, "seal"), &seal)?;

        // Publication precedes raw removal: after a crash either representation
        // is sufficient. Unpublished compressed temporary files are ignored.
        if self.index > 0 {
            fs::remove_file(chunk_path(&self.path, self.index, "jsonl"))?;
        }

        let next = self
            .index
            .checked_add(1)
            .ok_or_else(|| return invalid("segment index overflow"))?;

        let active = create_private(&chunk_path(&self.path, next, "jsonl"))?;

        File::open(&parent)?.sync_all()?;

        self.active = active;
        self.index = next;
        self.bytes = 0;
        self.hash = Hasher::new();

        return Ok(());
    }
}

impl Write for SegmentWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        if self.bytes == self.layout.segment_bytes {
            self.rotate()?;
        }

        let remaining =
            usize::try_from(self.layout.segment_bytes - self.bytes).map_err(io::Error::other)?;

        let selected = buf
            .get(..buf.len().min(remaining))
            .ok_or_else(|| return invalid("invalid segment write range"))?;
        let written = self.active.write(selected)?;
        self.hash.update(
            selected
                .get(..written)
                .ok_or_else(|| return invalid("invalid segment write count"))?,
        );

        self.bytes += u64::try_from(written).map_err(io::Error::other)?;

        return Ok(written);
    }

    fn flush(&mut self) -> io::Result<()> {
        return self.active.flush();
    }
}

/// Seekable canonical JSONL bytes spanning bounded physical chunks.
///
/// A compressed chunk is verified and inflated into one anonymous temporary file;
/// memory and temporary disk use do not grow with session lifetime.
pub struct CanonicalLogFile {
    path: PathBuf,
    root: File,
    active: File,
    layout: Option<Layout>,
    index: u64,
    position: u64,
}

impl CanonicalLogFile {
    /// Open the original JSONL file and discover its optional segment layout.
    ///
    /// # Errors
    ///
    /// Returns an error when the file or its active chunk is not a regular file, or the segment
    /// layout is replaced or unreadable.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let root = open_regular(&path)?;
        let active = root.try_clone()?;
        let layout = layout(&path)?;

        let mut result = Self {
            path,
            root,
            active,
            layout,
            index: 0,
            position: 0,
        };

        if result.layout.is_some() {
            result.active = result.open_chunk(0)?;
        }

        return Ok(result);
    }

    /// Immutable root descriptor metadata for live cursor authentication.
    ///
    /// # Errors
    ///
    /// Returns an error when the root descriptor cannot be inspected.
    pub fn metadata(&self) -> io::Result<Metadata> {
        return self.root.metadata();
    }

    /// Current logical byte length, including the active uncompressed tail.
    ///
    /// # Errors
    ///
    /// Returns an error when the segment layout, seal, or active chunk cannot be read, a chunk
    /// exceeds its declared size, or the total length overflows.
    pub fn len(&self) -> io::Result<u64> {
        let Some(layout) = layout(&self.path)? else {
            return Ok(self.root.metadata()?.len());
        };

        let index = last_index(&self.path)?;
        let bytes = match read_json::<Seal>(&chunk_path(&self.path, index, "seal")) {
            Ok(seal) => seal.bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                open_regular(&chunk_path(&self.path, index, "jsonl"))?
                    .metadata()?
                    .len()
            }
            Err(error) => return Err(error),
        };

        if bytes > layout.segment_bytes {
            return Err(invalid("canonical chunk exceeds its declared size"));
        }
        return index
            .checked_mul(layout.segment_bytes)
            .and_then(|offset| return offset.checked_add(bytes))
            .ok_or_else(|| return invalid("canonical length overflow"));
    }

    /// Whether the canonical stream contains no bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the logical length cannot be determined.
    pub fn is_empty(&self) -> io::Result<bool> {
        return Ok(self.len()? == 0);
    }

    fn open_chunk(&self, index: u64) -> io::Result<File> {
        let raw = chunk_path(&self.path, index, "jsonl");
        let seal: Seal = match read_json(&chunk_path(&self.path, index, "seal")) {
            Ok(seal) => seal,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return open_regular(&raw),
            Err(error) => return Err(error),
        };

        let layout = self
            .layout
            .ok_or_else(|| return invalid("sealed chunk has no layout"))?;

        if seal.index != index || seal.bytes != layout.segment_bytes {
            return Err(invalid(
                "canonical chunk seal has an invalid index or length",
            ));
        }

        let mut file = if index == 0 {
            open_regular(&raw)?
        } else {
            let compressed = open_regular(&chunk_path(&self.path, index, "zst"))?;
            let mut decoder = Decoder::new(compressed)?.take(seal.bytes + 1);
            let mut inflated = tempfile::tempfile()?;
            let count = io::copy(&mut decoder, &mut inflated)?;

            if count != seal.bytes {
                return Err(invalid("inflated canonical chunk length differs from seal"));
            }

            inflated.seek(SeekFrom::Start(0))?;

            inflated
        };

        let mut digest = Hasher::new();
        let count = io::copy(&mut file, &mut digest)?;

        if count != seal.bytes || digest.finalize().to_hex().as_str() != seal.blake3 {
            return Err(invalid("canonical chunk digest differs from seal"));
        }

        file.seek(SeekFrom::Start(0))?;

        return Ok(file);
    }
}

impl Read for CanonicalLogFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let count = self.active.read(buf)?;
            if count > 0 {
                self.position = self
                    .position
                    .checked_add(u64::try_from(count).map_err(io::Error::other)?)
                    .ok_or_else(|| return invalid("canonical position overflow"))?;

                return Ok(count);
            }

            self.layout = layout(&self.path)?;
            let Some(layout) = self.layout else {
                return Ok(0);
            };

            let next = self
                .index
                .checked_add(1)
                .ok_or_else(|| return invalid("segment index overflow"))?;
            if self.position
                != next
                    .checked_mul(layout.segment_bytes)
                    .ok_or_else(|| return invalid("segment offset overflow"))?
            {
                return Ok(0);
            }

            // A previously active file may have been sealed since it was opened.
            // Verify that publication before crossing into its successor.
            drop(self.open_chunk(self.index)?);
            let active = match self.open_chunk(next) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if last_index(&self.path)? >= next {
                        return Err(invalid(
                            "canonical segment is missing before the active tail",
                        ));
                    }
                    return Ok(0);
                }
                Err(error) => return Err(error),
            };
            self.active = active;
            self.index = next;
        }
    }
}

impl Seek for CanonicalLogFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(target) => target,
            SeekFrom::Current(delta) => self
                .position
                .checked_add_signed(delta)
                .ok_or_else(|| return invalid("invalid canonical relative seek"))?,
            SeekFrom::End(delta) => self
                .len()?
                .checked_add_signed(delta)
                .ok_or_else(|| return invalid("invalid canonical end seek"))?,
        };

        self.layout = layout(&self.path)?;
        let (mut index, mut offset) = self.layout.map_or((0, target), |layout| {
            return (target / layout.segment_bytes, target % layout.segment_bytes);
        });

        if index != self.index {
            match self.open_chunk(index) {
                Ok(file) => self.active = file,
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound && offset == 0 && index > 0 =>
                {
                    // A cursor exactly at EOF may precede creation of the next chunk.
                    index -= 1;
                    offset = self
                        .layout
                        .ok_or_else(|| return invalid("missing segment layout"))?
                        .segment_bytes;

                    self.active = self.open_chunk(index)?;
                }
                Err(error) => return Err(error),
            }

            self.index = index;
        }

        self.active.seek(SeekFrom::Start(offset))?;
        self.position = target;

        return Ok(target);
    }
}

fn last_index(path: &Path) -> io::Result<u64> {
    let mut last = 0;
    let mut entries = 0_u64;

    for entry in fs::read_dir(directory(path))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some((number, extension)) = name.split_once('.') else {
            continue;
        };

        if number.len() == 20 && matches!(extension, "jsonl" | "seal") {
            entries = entries.saturating_add(1);
            let index = number.parse::<u64>().map_err(io::Error::other)?;
            last = last.max(index);
        }
    }

    if last > entries {
        return Err(invalid("canonical segment inventory contains an index gap"));
    }
    return Ok(last);
}

/// Bounded-memory fingerprint of every physical member's local kernel identity.
///
/// This supports locally authenticated caches, not portable content verification.
///
/// # Errors
///
/// Returns an error when a physical member cannot be inspected or the segment layout is replaced or
/// unreadable.
pub fn canonical_source_fingerprint(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    let mut digest = Hasher::new();
    fingerprint_file(&mut digest, path)?;

    if layout(path)?.is_some() {
        fingerprint_file(&mut digest, &directory(path).join("layout.json"))?;

        for index in 0..=last_index(path)? {
            let sealed = chunk_path(path, index, "seal");

            match fingerprint_file(&mut digest, &sealed) {
                Ok(()) => fingerprint_file(
                    &mut digest,
                    &chunk_path(path, index, if index == 0 { "jsonl" } else { "zst" }),
                )?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fingerprint_file(&mut digest, &chunk_path(path, index, "jsonl"))?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    return Ok(digest.finalize().to_hex().to_string());
}

fn fingerprint_file(digest: &mut Hasher, path: &Path) -> io::Result<()> {
    let metadata = open_regular(path)?.metadata()?;
    for value in [metadata.dev(), metadata.ino(), metadata.len()] {
        digest.update(&value.to_le_bytes());
    }

    for value in [
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    ] {
        digest.update(&value.to_le_bytes());
    }

    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use termtrace_model::execution::layout::SessionLayout;

    fn writer(path: &Path) -> SegmentWriter {
        return SegmentWriter::with_segment_bytes(path, create_private(path).expect("root"), 1024)
            .expect("writer");
    }

    #[test]
    fn compressed_chunks_preserve_arbitrary_byte_boundaries_and_seeks() {
        let temporary = TempDir::new().expect("directory");
        let path = SessionLayout::new(temporary.path()).event_log();
        let bytes: Vec<u8> = (0_u32..6500)
            .map(|value| return u8::try_from(value % 251).expect("byte"))
            .collect();

        let mut writer = writer(&path);
        writer.write_all(&bytes).expect("write all chunks");
        writer.sync_data().expect("sync");

        assert_eq!(fs::metadata(&path).expect("root metadata").len(), 1024);
        assert!(chunk_path(&path, 1, "zst").is_file());
        assert!(!chunk_path(&path, 1, "jsonl").exists());

        let mut reader = CanonicalLogFile::open(&path).expect("reader");
        assert_eq!(reader.len().expect("logical length"), 6500);

        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).expect("complete read");

        assert_eq!(actual, bytes);

        for offset in [0, 1023, 1024, 2049, 6144, 6499, 6500] {
            reader.seek(SeekFrom::Start(offset)).expect("seek");
            actual.clear();
            reader.read_to_end(&mut actual).expect("suffix read");

            assert_eq!(
                actual.as_slice(),
                bytes
                    .get(usize::try_from(offset).expect("offset")..)
                    .expect("suffix")
            );
        }
    }

    #[test]
    fn open_live_cursor_follows_new_chunks_and_boundary_eof() {
        let temporary = TempDir::new().expect("directory");
        let path = SessionLayout::new(temporary.path()).event_log();
        let mut writer = writer(&path);
        writer.write_all(&[b'a'; 1024]).expect("first chunk");

        let mut reader = CanonicalLogFile::open(&path).expect("reader");
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).expect("initial read");

        assert_eq!(actual.len(), 1024);

        reader
            .seek(SeekFrom::Start(1024))
            .expect("seek EOF before rotation");

        writer
            .write_all(&[b'b'; 2048])
            .expect("rotate and compress");

        actual.clear();
        reader
            .read_to_end(&mut actual)
            .expect("follow later chunks");

        assert_eq!(actual, [b'b'; 2048]);

        reader
            .seek(SeekFrom::Start(3072))
            .expect("seek EOF after rotation");

        writer.write_all(b"tail").expect("new active chunk");
        actual.clear();
        reader.read_to_end(&mut actual).expect("follow tail");

        assert_eq!(actual, b"tail");
    }

    #[test]
    fn corrupted_or_missing_compressed_chunks_are_rejected() {
        let temporary = TempDir::new().expect("directory");
        let path = SessionLayout::new(temporary.path()).event_log();
        let mut writer = writer(&path);
        writer.write_all(&[b'a'; 3500]).expect("chunks");

        let fingerprint = canonical_source_fingerprint(&path).expect("fingerprint");
        fs::write(chunk_path(&path, 1, "zst"), b"corrupt").expect("tamper");

        assert_ne!(
            fingerprint,
            canonical_source_fingerprint(&path).expect("changed fingerprint")
        );

        let mut reader = CanonicalLogFile::open(&path).expect("reader");
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
        fs::remove_file(chunk_path(&path, 1, "zst")).expect("remove chunk");
        let mut reader = CanonicalLogFile::open(&path).expect("reader");
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
    }
}
