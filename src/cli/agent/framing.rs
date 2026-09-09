use serde::Deserialize as _;
use std::{
    io::{BufRead, BufReader, Error as IoError, Read, Result as IoResult},
    path::PathBuf,
};
use termtrace_model::protocol::driver::{DriverRequest, DRIVER_MAX_REQUEST_BYTES};
use termtrace_sdk::{Error, Result};

pub(super) enum DriverFrame {
    Request(Box<DriverRequest>),
    Invalid { line: usize, column: usize },
    TooLarge,
    Unterminated,
}

pub(super) fn read_frame(input: &mut impl BufRead) -> Result<Option<DriverFrame>> {
    loop {
        let available = input.fill_buf().map_err(driver_input_error)?;
        if available.is_empty() {
            return Ok(None);
        }

        if available.first() != Some(&b'\n') {
            break;
        }

        input.consume(1);
    }

    let mut frame = NewlineFrameReader::new(input);
    let parsed = {
        let mut buffered = BufReader::new(&mut frame);
        let mut deserializer = serde_json::Deserializer::from_reader(&mut buffered);

        DriverRequest::deserialize(&mut deserializer)
            .and_then(|request| return deserializer.end().map(|()| return request))
    };

    if let Some(source) = frame.take_input_error() {
        return Err(driver_input_error(source));
    }

    frame.drain().map_err(driver_input_error)?;

    if frame.exceeded() {
        return Ok(Some(DriverFrame::TooLarge));
    }

    if !frame.terminated() {
        return Ok(Some(DriverFrame::Unterminated));
    }

    return Ok(Some(match parsed {
        Ok(request) => DriverFrame::Request(Box::new(request)),
        Err(error) => DriverFrame::Invalid {
            line: error.line(),
            column: error.column(),
        },
    }));
}

fn driver_input_error(source: IoError) -> Error {
    return Error::Filesystem {
        path: PathBuf::from("<driver-stdin>"),
        source,
    };
}

struct NewlineFrameReader<'input, R> {
    input: &'input mut R,
    terminated: bool,
    exceeded: bool,
    content_bytes: usize,
    input_error: Option<IoError>,
}

impl<'input, R> NewlineFrameReader<'input, R> {
    fn new(input: &'input mut R) -> Self {
        return Self {
            input,
            terminated: false,
            exceeded: false,
            content_bytes: 0,
            input_error: None,
        };
    }

    fn drain(&mut self) -> IoResult<()>
    where
        R: BufRead,
    {
        while !self.terminated {
            let available = self.input.fill_buf()?;
            if available.is_empty() {
                break;
            }

            let newline = available.iter().position(|byte| return *byte == b'\n');
            let (consumed, content_bytes, terminated) =
                newline.map_or((available.len(), available.len(), false), |index| {
                    return (index.saturating_add(1), index, true);
                });
            self.content_bytes = self.content_bytes.saturating_add(content_bytes);
            self.exceeded = self.exceeded
                || u64::try_from(self.content_bytes).unwrap_or(u64::MAX) > DRIVER_MAX_REQUEST_BYTES;

            self.input.consume(consumed);
            self.terminated = terminated;
        }

        return Ok(());
    }

    fn take_input_error(&mut self) -> Option<IoError> {
        return self.input_error.take();
    }

    fn terminated(&self) -> bool {
        return self.terminated;
    }

    fn exceeded(&self) -> bool {
        return self.exceeded;
    }
}

impl<R: BufRead> Read for NewlineFrameReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.terminated || self.exceeded || buf.is_empty() {
            return Ok(0);
        }

        let available = match self.input.fill_buf() {
            Ok(available) => available,
            Err(source) => {
                self.input_error = Some(IoError::new(source.kind(), source.to_string()));
                return Err(source);
            }
        };

        if available.is_empty() {
            return Ok(0);
        }

        let maximum = usize::try_from(DRIVER_MAX_REQUEST_BYTES).unwrap_or(usize::MAX);
        let newline = available.iter().position(|byte| return *byte == b'\n');
        let frame_bytes = match newline {
            Some(index) if self.content_bytes.saturating_add(index) <= maximum => {
                index.saturating_add(1)
            }
            _ => maximum
                .saturating_add(1)
                .saturating_sub(self.content_bytes)
                .min(available.len()),
        };

        let consumed = frame_bytes.min(buf.len());
        let (Some(destination), Some(source)) =
            (buf.get_mut(..consumed), available.get(..consumed))
        else {
            return Err(IoError::other(
                "driver frame buffer bounds are inconsistent",
            ));
        };
        destination.copy_from_slice(source);
        self.input.consume(consumed);

        let content_consumed = newline.map_or(consumed, |index| return index.min(consumed));
        self.content_bytes = self.content_bytes.saturating_add(content_consumed);
        self.exceeded = self.content_bytes > maximum;
        self.terminated = newline.is_some_and(|index| return consumed > index);
        return Ok(consumed);
    }
}
