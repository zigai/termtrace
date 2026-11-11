// A seekable zstd member: independent frames of at most `FRAME_BYTES` uncompressed bytes, then
// one `(uncompressed start, compressed offset)` pair of little-endian u64s per frame, the
// uncompressed length, the frame count, and `MAGIC`. Offsets callers store are uncompressed, so a
// reader opened at one yields exactly the bytes a plain file would from that position.

use super::super::invalid_replay;
use crate::{Error, Result};
use std::{
    fs::File,
    io::{self, BufReader, BufWriter, Read, Seek as _, SeekFrom, Take, Write},
    path::{Path, PathBuf},
};
use termtrace_evidence::artifact::{finish_writer, secure_writer};
use zstd::{bulk::Compressor, stream::read::Decoder};

const FRAME_BYTES: usize = 1 << 20;
const MAGIC: &[u8; 8] = b"TTSEEK01";
const FOOTER_BYTES: u64 = 24;

pub(crate) type SeekableReader = BufReader<Decoder<'static, BufReader<Take<File>>>>;

pub(super) struct SeekableWriter {
    writer: BufWriter<File>,
    path: PathBuf,
    compressor: Compressor<'static>,
    pending: Vec<u8>,
    frames: Vec<(u64, u64)>,
    uncompressed: u64,
    compressed: u64,
}

impl SeekableWriter {
    pub(super) fn create(path: &Path) -> Result<Self> {
        return Ok(Self {
            writer: secure_writer(path)?,
            path: path.to_path_buf(),
            compressor: Compressor::new(3).map_err(|source| {
                return Error::Filesystem {
                    path: path.to_path_buf(),
                    source,
                };
            })?,
            pending: Vec::with_capacity(FRAME_BYTES),
            frames: Vec::new(),
            uncompressed: 0,
            compressed: 0,
        });
    }

    fn write_frame(&mut self) -> io::Result<()> {
        // An empty member still holds one frame, because zstd rejects an empty stream.
        if self.pending.is_empty() && !self.frames.is_empty() {
            return Ok(());
        }

        let frame = self.compressor.compress(&self.pending)?;
        self.writer.write_all(&frame)?;
        self.frames.push((self.uncompressed, self.compressed));
        self.uncompressed = self
            .uncompressed
            .checked_add(u64::try_from(self.pending.len()).map_err(io::Error::other)?)
            .ok_or_else(|| return io::Error::other("seekable member length overflow"))?;

        self.compressed = self
            .compressed
            .checked_add(u64::try_from(frame.len()).map_err(io::Error::other)?)
            .ok_or_else(|| return io::Error::other("seekable member offset overflow"))?;

        self.pending.clear();
        return Ok(());
    }

    pub(super) fn finish(mut self) -> Result<()> {
        let result = (|| {
            self.write_frame()?;

            for (start, offset) in &self.frames {
                self.writer.write_all(&start.to_le_bytes())?;
                self.writer.write_all(&offset.to_le_bytes())?;
            }

            let count = u64::try_from(self.frames.len()).map_err(io::Error::other)?;

            self.writer.write_all(&self.uncompressed.to_le_bytes())?;
            self.writer.write_all(&count.to_le_bytes())?;
            return self.writer.write_all(MAGIC);
        })();

        result.map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;

        return Ok(finish_writer(self.writer, &self.path)?);
    }
}

impl Write for SeekableWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = FRAME_BYTES.saturating_sub(self.pending.len());
        let taken = buf.get(..room.min(buf.len())).unwrap_or(buf);
        self.pending.extend_from_slice(taken);
        if self.pending.len() >= FRAME_BYTES {
            self.write_frame()?;
        }

        return Ok(taken.len());
    }

    fn flush(&mut self) -> io::Result<()> {
        return Ok(());
    }
}

struct Table {
    frames: Vec<(u64, u64)>,
    uncompressed: u64,
    compressed: u64,
}

fn read_table(file: &mut File, path: &Path) -> Result<Table> {
    let filesystem_error = |source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    };

    let malformed = || return invalid_replay("seekable derived member is malformed");

    let length = file.metadata().map_err(filesystem_error)?.len();
    let footer_start = length.checked_sub(FOOTER_BYTES).ok_or_else(malformed)?;
    let mut footer = [0_u8; 24];
    file.seek(SeekFrom::Start(footer_start))
        .and_then(|_| return file.read_exact(&mut footer))
        .map_err(filesystem_error)?;

    let (uncompressed, rest) = footer.split_first_chunk::<8>().ok_or_else(malformed)?;
    let (count, magic) = rest.split_first_chunk::<8>().ok_or_else(malformed)?;
    if magic != MAGIC {
        return Err(malformed());
    }

    let uncompressed = u64::from_le_bytes(*uncompressed);
    let count = u64::from_le_bytes(*count);
    let compressed = count
        .checked_mul(16)
        .and_then(|table| return footer_start.checked_sub(table))
        .ok_or_else(malformed)?;

    let table_bytes =
        usize::try_from(footer_start.saturating_sub(compressed)).map_err(|source| {
            return Error::InvalidReplay {
                reason: "seekable derived member table does not fit".to_owned(),
                source: Some(Box::new(source)),
            };
        })?;

    let mut table = vec![0_u8; table_bytes];
    file.seek(SeekFrom::Start(compressed))
        .and_then(|_| return file.read_exact(&mut table))
        .map_err(filesystem_error)?;

    let frames = table
        .as_chunks::<8>()
        .0
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[start, offset]| return (u64::from_le_bytes(*start), u64::from_le_bytes(*offset)))
        .collect::<Vec<_>>();

    let ordered = frames.first() == Some(&(0, 0))
        && frames.windows(2).all(|pair| {
            return matches!(pair, [(start, offset), (next_start, next_offset)]
                if start < next_start && offset < next_offset);
        })
        && frames.last().is_some_and(|(start, offset)| {
            return *start <= uncompressed && *offset < compressed;
        });

    if !ordered {
        return Err(malformed());
    }
    return Ok(Table {
        frames,
        uncompressed,
        compressed,
    });
}

pub(crate) fn seekable_len(path: &Path) -> Result<u64> {
    let mut file = File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    return Ok(read_table(&mut file, path)?.uncompressed);
}

pub(crate) fn open_seekable(path: &Path, offset: u64) -> Result<SeekableReader> {
    let filesystem_error = |source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    };
    let mut file = File::open(path).map_err(filesystem_error)?;
    let table = read_table(&mut file, path)?;

    if offset > table.uncompressed {
        return Err(invalid_replay(
            "seekable derived member offset is past its end",
        ));
    }

    let frame = table
        .frames
        .partition_point(|(start, _)| return *start <= offset)
        .checked_sub(1)
        .and_then(|index| return table.frames.get(index))
        .copied()
        .ok_or_else(|| return invalid_replay("seekable derived member has no frame"))?;
    file.seek(SeekFrom::Start(frame.1))
        .map_err(filesystem_error)?;
    let compressed = BufReader::new(file.take(table.compressed.saturating_sub(frame.1)));
    let mut reader = BufReader::new(Decoder::with_buffer(compressed).map_err(filesystem_error)?);
    let skip = offset.saturating_sub(frame.0);
    let skipped =
        io::copy(&mut reader.by_ref().take(skip), &mut io::sink()).map_err(filesystem_error)?;

    if skipped != skip {
        return Err(invalid_replay(
            "seekable derived member ended before its offset",
        ));
    }
    return Ok(reader);
}
