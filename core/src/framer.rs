//! Incremental `\n` line framer for bytes arriving off a TCP socket in arbitrary chunks.
//!
//! `docs/SPEC-puente-ingame.md` calls this "the idea" copied from H8's own record framer, not
//! the module itself (`src/platform/` is not imported here; see the spec's "Reparto"). This is
//! a fresh ~60-line one, scoped to exactly what the in-game channel needs: split on `\n`,
//! and never let an unterminated line grow without bound in memory.
//!
//! The 512-byte wire cap from the spec is a *protocol* rule, enforced in `protocol.rs` on the
//! already-framed line. This module enforces a separate, looser *memory safety* cap: if a peer
//! never sends `\n` at all, the buffer must still stop growing. `MAX_BUFFERED_BYTES` is set
//! well above the wire cap so a legitimate oversized line (which `protocol.rs` will discard
//! anyway) is framed and handed up rather than silently eaten here.

/// Upper bound on bytes held between newlines before this framer gives up on the current line
/// and resyncs at the next `\n` instead of growing forever. Comfortably above the protocol's
/// own 512-byte cap so that cap, not this one, is what normally decides "too long".
const MAX_BUFFERED_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramedLine {
    /// A complete line, `\n` excluded. Not yet validated against the wire contract.
    Complete(String),
    /// The framer gave up on a line that grew past [`MAX_BUFFERED_BYTES`] without a `\n`.
    /// Bytes up to the next `\n` are discarded so the stream can resync.
    Oversized,
}

#[derive(Debug, Default)]
pub struct LineFramer {
    buffer: Vec<u8>,
    resyncing: bool,
}

impl LineFramer {
    pub fn new() -> Self {
        Self { buffer: Vec::new(), resyncing: false }
    }

    /// Feeds a chunk of bytes, however it was split by the socket, and returns every line the
    /// chunk completed, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<FramedLine> {
        let mut out = Vec::new();
        for &byte in chunk {
            if self.resyncing {
                if byte == b'\n' {
                    self.resyncing = false;
                }
                continue;
            }
            if byte == b'\n' {
                let line = String::from_utf8_lossy(&self.buffer).into_owned();
                self.buffer.clear();
                out.push(FramedLine::Complete(line));
                continue;
            }
            self.buffer.push(byte);
            if self.buffer.len() > MAX_BUFFERED_BYTES {
                self.buffer.clear();
                self.resyncing = true;
                out.push(FramedLine::Oversized);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_a_single_line_delivered_whole() {
        let mut framer = LineFramer::new();
        assert_eq!(framer.push(b"hello\n"), vec![FramedLine::Complete("hello".into())]);
    }

    #[test]
    fn frames_a_line_split_across_chunks() {
        let mut framer = LineFramer::new();
        assert_eq!(framer.push(b"hel"), vec![]);
        assert_eq!(framer.push(b"lo\n"), vec![FramedLine::Complete("hello".into())]);
    }

    #[test]
    fn frames_multiple_lines_in_one_chunk() {
        let mut framer = LineFramer::new();
        assert_eq!(
            framer.push(b"one\ntwo\nthree\n"),
            vec![
                FramedLine::Complete("one".into()),
                FramedLine::Complete("two".into()),
                FramedLine::Complete("three".into()),
            ]
        );
    }

    #[test]
    fn an_empty_line_is_still_a_line() {
        let mut framer = LineFramer::new();
        assert_eq!(framer.push(b"\n"), vec![FramedLine::Complete(String::new())]);
    }

    #[test]
    fn a_line_that_never_terminates_stops_growing_and_resyncs() {
        let mut framer = LineFramer::new();
        let runaway = vec![b'x'; MAX_BUFFERED_BYTES + 1];
        let mut lines = framer.push(&runaway);
        // The garbage after the overflow point, before the next `\n`, produces no more
        // `Oversized` events: only one per resync, not one per byte.
        lines.extend(framer.push(b"more garbage"));
        assert_eq!(lines, vec![FramedLine::Oversized]);
        // Once the peer's next `\n` arrives, framing resumes normally.
        assert_eq!(framer.push(b"\nback to normal\n"), vec![FramedLine::Complete("back to normal".into())]);
    }
}
