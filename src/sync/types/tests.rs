//! Unit tests for `forward_stream`'s end-of-file handling.
//!
//! The behavioural suite drives real child processes, whose pipes reach end of
//! file only after the process exits. That exercises the ordinary path, but it
//! cannot show what happens when a zero-byte read is followed by another read:
//! a real pipe stays at end of file, so a comparison that kept reading would
//! spin there and the caller's join would never return. These cases drive the
//! forwarder with a scripted reader instead, so the read after end of file is
//! an error and a wrong comparison fails an assertion rather than hanging the
//! suite.

use std::{
    collections::VecDeque,
    io::{self, Read},
};

use rstest::rstest;

use super::forward_stream;

/// One scripted outcome for a [`ScriptedReader`] read call.
enum ReadStep {
    /// Supply these bytes, truncated to the caller's buffer.
    Bytes(&'static [u8]),
    /// Report end of file.
    EndOfFile,
    /// Fail with this message.
    Failed(&'static str),
}

/// An owned reader that replays a fixed script of read outcomes.
///
/// The forwarder takes the reader by value and reads from its own thread, so
/// the double owns its script and satisfies `Read + Send + 'static`. Running
/// past the end of the script is an error rather than a panic: a read the
/// script did not expect surfaces as the forwarder's own `Spawn` error, which
/// the assertion reports.
struct ScriptedReader {
    steps: VecDeque<ReadStep>,
}

impl ScriptedReader {
    /// Builds a reader that replays `steps` in order.
    #[must_use]
    fn new(steps: impl IntoIterator<Item = ReadStep>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
        }
    }
}

impl Read for ScriptedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.steps.pop_front() {
            Some(ReadStep::Bytes(bytes)) => {
                let count = bytes.len().min(buffer.len());
                let source = bytes
                    .get(..count)
                    .ok_or_else(|| io::Error::other("scripted payload is not addressable"))?;
                let target = buffer
                    .get_mut(..count)
                    .ok_or_else(|| io::Error::other("read buffer is not addressable"))?;
                target.copy_from_slice(source);
                Ok(count)
            }
            Some(ReadStep::EndOfFile) => Ok(0),
            Some(ReadStep::Failed(message)) => Err(io::Error::other(message)),
            None => Err(io::Error::other("scripted reader exhausted")),
        }
    }
}

/// A zero-byte read is end of file, and the forwarder must stop there.
///
/// The second read fails, so a forwarder that treats `0` as "keep reading"
/// cannot spin: it returns the scripted failure as
/// [`SyncError::Spawn`](crate::sync::SyncError::Spawn), and this assertion
/// fails promptly instead of the suite hanging at the join.
#[rstest]
fn zero_byte_read_ends_the_forwarder() {
    let reader = ScriptedReader::new([
        ReadStep::EndOfFile,
        ReadStep::Failed("read after end of file"),
    ]);

    let captured = forward_stream(reader, false, "scripted-reader")
        .join()
        .expect("forwarder thread should not panic");

    assert_eq!(captured, Ok(String::new()));
}

/// A payload is captured in full, and the end of file that follows ends it.
///
/// A forwarder that breaks on a non-zero read instead of a zero one returns an
/// empty capture before it writes anything, so the equality fails without
/// waiting for the scripted error that follows end of file.
#[rstest]
fn payload_is_captured_before_end_of_file() {
    let payload = b"forwarded output";
    let reader = ScriptedReader::new([
        ReadStep::Bytes(payload),
        ReadStep::EndOfFile,
        ReadStep::Failed("read after end of file"),
    ]);

    let captured = forward_stream(reader, false, "scripted-reader")
        .join()
        .expect("forwarder thread should not panic");

    assert_eq!(captured, Ok(String::from_utf8_lossy(payload).into_owned()));
}
