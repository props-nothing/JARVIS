//! Server-sent-event framing for a Chat Completions stream.
//!
//! A second pure module, deliberately separate from both the byte transport and the JSON
//! translation. The framing rules are small but each has a failure that looks like something else,
//! so they are tested against byte fixtures rather than only through a socket:
//!
//! - **A frame is `data:` up to a blank line.** A chunk's JSON contains no newline, but the *event*
//!   does: SSE allows one payload to span several `data:` lines that are joined with `\n`. Treating
//!   each `data:` line as a whole payload would silently truncate a multi-line chunk, and the result
//!   would be a JSON parse failure attributed to the provider.
//! - **An incomplete trailing frame is kept, not emitted.** A read boundary lands wherever the
//!   kernel put it, so a partial frame must survive until the next read completes it. Emitting it
//!   early would fail the parse on a frame the provider sent correctly.
//! - **The `[DONE]` sentinel is recognized and never parsed as JSON.** It is not JSON, so a parser
//!   that tried would report a malformed frame for the normal termination of a healthy stream.
//! - **A `:` comment and a non-`data` field are skipped.** SSE comments are legal padding, and the
//!   provider's own `obfuscation` feature exists to normalize frame sizes, so padding must be
//!   tolerated rather than refused.

/// The sentinel SDKs consume when a stream ends.
///
/// Its absence from the page this adapter's evidence note cites is recorded there as `OC-C004`
/// (UNVERIFIED), so the adapter recognizes it as a sentinel to skip and does **not** treat it as the
/// terminal — the terminal is the first non-null `finish_reason`, which is documented.
pub const DONE_SENTINEL: &str = "[DONE]";

/// The largest single SSE frame this parser will assemble.
///
/// Bounded because the buffer is fed by an untrusted endpoint: without a bound, a server that never
/// sends a blank line would grow the buffer until the process died. The bound is generous relative to
/// a real chunk (a few kilobytes of JSON) so it cannot reject a legitimate frame, and it is enforced
/// here rather than at the socket so the failure is attributed to the frame and not to the transport.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

/// One complete frame's payloads, in the order they appeared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseFrame {
    /// A payload that the caller should parse as JSON.
    Data(String),
    /// The end-of-stream sentinel.
    Done,
}

/// The outcome of feeding bytes into a [`SseBuffer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameRead {
    /// This many complete frames were produced, in order.
    Frames(Vec<SseFrame>),
    /// No complete frame yet; more bytes are needed.
    Incomplete,
    /// A frame exceeded [`MAX_FRAME_BYTES`] before it completed.
    TooLarge,
}

/// Reassembles SSE frames from an arbitrarily chunked byte stream.
///
/// Fed `&[u8]` rather than `&str` because a read boundary can split a multi-byte UTF-8 character, and
/// decoding each read independently would turn that into a spurious decode error. Bytes are
/// accumulated and decoded per completed frame, which is where the boundary is meaningful.
#[derive(Debug, Default)]
pub struct SseBuffer {
    pending: Vec<u8>,
}

impl SseBuffer {
    /// Creates an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes and returns every frame that completed.
    ///
    /// # Errors
    ///
    /// There is no error return: a malformed *payload* is the JSON layer's business, and an oversized
    /// frame is reported as [`FrameRead::TooLarge`] so the caller can fail the call with a code that
    /// names the frame rather than the transport.
    pub fn push(&mut self, bytes: &[u8]) -> FrameRead {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > MAX_FRAME_BYTES {
            // Checked before scanning, so a server that never terminates a frame is stopped here
            // rather than after the buffer has already grown.
            self.pending.clear();
            return FrameRead::TooLarge;
        }

        let mut frames = Vec::new();
        // A frame ends at a blank line. `\n\n` and `\r\n\r\n` are both accepted: the specification
        // allows either line ending, and a server that emits CRLF must not produce a frame that never
        // terminates.
        while let Some((end, next)) = find_frame_boundary(&self.pending) {
            let raw: Vec<u8> = self.pending.drain(..next).collect();
            if let Some(frame) = parse_frame(&raw[..end]) {
                frames.push(frame);
            }
        }

        if frames.is_empty() {
            FrameRead::Incomplete
        } else {
            FrameRead::Frames(frames)
        }
    }

    /// Returns whether unparsed bytes remain that never completed a frame.
    ///
    /// Used at end-of-body: a stream that ended mid-frame is a truncated response rather than a
    /// clean end, and the caller must fail the call instead of reporting a completion.
    #[must_use]
    pub fn has_partial(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// Returns `(content length, index after the boundary)` for the first complete frame.
fn find_frame_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    // `\r\n\r\n` is checked first so a CRLF stream is not mis-split by the `\n\n` case matching its
    // second and third bytes and leaving a stray `\r` at the end of the payload.
    if let Some(index) = find(bytes, b"\r\n\r\n") {
        return Some((index, index + 4));
    }
    find(bytes, b"\n\n").map(|index| (index, index + 2))
}

/// Returns the index of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Builds a frame from its raw bytes, or `None` when it carries no data.
fn parse_frame(raw: &[u8]) -> Option<SseFrame> {
    let Ok(text) = std::str::from_utf8(raw) else {
        // A frame that is not valid UTF-8 cannot be JSON, and JSON is the only payload shape this
        // protocol uses. Reported as no frame so the caller's end-of-stream handling treats the
        // stream as having produced nothing more; the missing terminal is what fails the call.
        return None;
    };

    let mut payload = String::new();
    let mut saw_data = false;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        // A comment or an empty line inside the frame. SSE padding, and the provider's own
        // `obfuscation` field is deliberate padding, so neither may be refused.
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some(value) = line.strip_prefix("data:") else {
            // `event:`, `id:`, and `retry:` are legal and unused by this protocol. Ignoring them is
            // the forward-compatible reading; refusing would break on a server that emits `event:`.
            continue;
        };
        // One optional space after the colon is part of the framing, not of the payload.
        let value = value.strip_prefix(' ').unwrap_or(value);
        if saw_data {
            payload.push('\n');
        }
        payload.push_str(value);
        saw_data = true;
    }

    if !saw_data {
        return None;
    }
    if payload == DONE_SENTINEL {
        return Some(SseFrame::Done);
    }
    Some(SseFrame::Data(payload))
}

#[cfg(test)]
mod tests {
    use super::{FrameRead, SseBuffer, SseFrame};

    #[test]
    fn a_frame_split_across_two_reads_is_assembled_rather_than_failed() {
        // The property that makes the buffer necessary: a read boundary lands wherever the kernel
        // put it, so a partial frame must survive. Emitting it early would report a JSON parse
        // failure for a frame the provider sent correctly.
        let mut buffer = SseBuffer::new();
        assert_eq!(
            buffer.push(b"data: {\"choices\":["),
            FrameRead::Incomplete,
            "an unterminated frame must produce nothing",
        );
        assert!(buffer.has_partial());
        let read = buffer.push(b"]}\n\n");
        assert_eq!(
            read,
            FrameRead::Frames(vec![SseFrame::Data("{\"choices\":[]}".to_owned())]),
        );
        assert!(!buffer.has_partial());
    }

    #[test]
    fn several_frames_in_one_read_are_all_produced() {
        let mut buffer = SseBuffer::new();
        let read = buffer.push(b"data: {\"a\":1}\n\ndata: {\"b\":2}\n\n");
        assert_eq!(
            read,
            FrameRead::Frames(vec![
                SseFrame::Data("{\"a\":1}".to_owned()),
                SseFrame::Data("{\"b\":2}".to_owned()),
            ]),
        );
    }

    #[test]
    fn crlf_framing_terminates_correctly_and_does_not_leak_a_carriage_return() {
        // A server that emits CRLF must not produce a frame that never ends, and the trailing `\r`
        // must not end up inside the JSON — which would make every payload fail to parse.
        let mut buffer = SseBuffer::new();
        let read = buffer.push(b"data: {\"a\":1}\r\n\r\n");
        assert_eq!(
            read,
            FrameRead::Frames(vec![SseFrame::Data("{\"a\":1}".to_owned())]),
        );
    }

    #[test]
    fn the_sentinel_is_recognized_and_is_not_parsed_as_json() {
        // `[DONE]` is not JSON. A parser that tried would report a malformed frame for the normal
        // termination of a healthy stream, which is the failure this branch exists to prevent.
        let mut buffer = SseBuffer::new();
        assert_eq!(
            buffer.push(b"data: [DONE]\n\n"),
            FrameRead::Frames(vec![SseFrame::Done]),
        );
    }

    #[test]
    fn comments_and_non_data_fields_are_skipped() {
        // SSE comments are legal padding and this protocol's `obfuscation` feature exists to
        // normalize frame sizes, so padding must be tolerated. A frame with only padding carries no
        // data and is not a frame.
        let mut buffer = SseBuffer::new();
        assert_eq!(
            buffer.push(b": keep-alive\n\nevent: message\ndata: {\"a\":1}\n\n"),
            FrameRead::Frames(vec![SseFrame::Data("{\"a\":1}".to_owned())]),
        );
    }

    #[test]
    fn a_multi_line_payload_is_joined_with_a_newline() {
        // SSE allows one payload to span several `data:` lines. Treating each line as a whole
        // payload would silently truncate a multi-line chunk and blame the provider for the
        // resulting parse failure.
        let mut buffer = SseBuffer::new();
        assert_eq!(
            buffer.push(b"data: {\"a\":\ndata: 1}\n\n"),
            FrameRead::Frames(vec![SseFrame::Data("{\"a\":\n1}".to_owned())]),
        );
    }

    #[test]
    fn an_oversized_frame_is_refused_rather_than_grown() {
        // Bounded because the buffer is fed by an untrusted endpoint: without this a server that
        // never sends a blank line grows the buffer until the process dies.
        let mut buffer = SseBuffer::new();
        let read = buffer.push(&vec![b'x'; super::MAX_FRAME_BYTES + 1]);
        assert_eq!(read, FrameRead::TooLarge);
    }

    #[test]
    fn a_frame_with_no_data_field_is_not_a_frame() {
        let mut buffer = SseBuffer::new();
        assert_eq!(buffer.push(b": only a comment\n\n"), FrameRead::Incomplete);
    }
}
