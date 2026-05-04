//! Reader-task framing helpers and assistant-delta coalescer.
//!
//! Claude streams partial assistant text token-by-token. Persisting every
//! token would explode the event log; the coalescer buffers deltas keyed by
//! `message_id` and flushes a single `AssistantMessage` event when one of:
//!   - `MAX_DELTAS` (5) deltas are pending, OR
//!   - `MAX_BUFFERED_BYTES` (200 KiB) is exceeded, OR
//!   - `FLUSH_INTERVAL` (250 ms) has elapsed since the last flush AND a new
//!     delta arrives (reactive — `push_delta` checks the elapsed time), OR
//!   - `finalize` is called for the `message_id`.
//!
//! `tick()` is provided for a future *proactive* periodic-flush hookup
//! (slice 2 wires a `tokio::time::interval`); it is not yet called by the
//! reader task in slice 1a, so a buffer with pending deltas but no further
//! arrivals will sit until either `finalize` runs or the next delta lands.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::Utc;
use nephila_core::session_event::SessionEvent;

const MAX_DELTAS: usize = 5;
const MAX_BUFFERED_BYTES: usize = 200 * 1024;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct Coalescer {
    buffers: HashMap<String, MessageBuffer>,
}

struct MessageBuffer {
    seq_in_message: u32,
    pending_text: String,
    pending_count: usize,
    last_flush: Instant,
}

impl Coalescer {
    pub(crate) fn push_delta(&mut self, message_id: &str, text: &str) -> Option<SessionEvent> {
        let buf = self
            .buffers
            .entry(message_id.to_owned())
            .or_insert_with(|| MessageBuffer {
                seq_in_message: 0,
                pending_text: String::new(),
                pending_count: 0,
                last_flush: Instant::now(),
            });
        buf.pending_text.push_str(text);
        buf.pending_count += 1;
        if buf.pending_count >= MAX_DELTAS
            || buf.pending_text.len() >= MAX_BUFFERED_BYTES
            || buf.last_flush.elapsed() >= FLUSH_INTERVAL
        {
            return Some(emit(buf, message_id, false));
        }
        None
    }

    pub(crate) fn finalize(&mut self, message_id: &str) -> Option<SessionEvent> {
        self.buffers
            .remove(message_id)
            .map(|mut buf| emit(&mut buf, message_id, true))
    }

    // Periodic flush hook; not yet plumbed in slice 1a (slice 2 wires a
    // 250ms ticker into the reader task). Kept public-crate so its unit
    // test exercises the same surface.
    #[allow(dead_code)]
    pub(crate) fn tick(&mut self, now: Instant) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        for (id, buf) in &mut self.buffers {
            if !buf.pending_text.is_empty() && now.duration_since(buf.last_flush) >= FLUSH_INTERVAL
            {
                out.push(emit(buf, id, false));
            }
        }
        out
    }
}

fn emit(buf: &mut MessageBuffer, message_id: &str, is_final: bool) -> SessionEvent {
    let seq = buf.seq_in_message;
    buf.seq_in_message += 1;
    buf.pending_count = 0;
    buf.last_flush = Instant::now();
    let text = std::mem::take(&mut buf.pending_text);
    SessionEvent::AssistantMessage {
        message_id: message_id.to_owned(),
        seq_in_message: seq,
        delta_text: text,
        is_final,
        // Slice 1b: payload truncation is observability-only; not implemented
        // in the reader path. `metrics::SESSION_EVENT_PAYLOAD_TRUNCATED` will
        // count truncations once a downstream codepath introduces them.
        truncated: false,
        ts: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;

    use super::*;

    fn unwrap_assistant(ev: SessionEvent) -> (String, u32, String, bool) {
        if let SessionEvent::AssistantMessage {
            message_id,
            seq_in_message,
            delta_text,
            is_final,
            ..
        } = ev
        {
            (message_id, seq_in_message, delta_text, is_final)
        } else {
            panic!("expected AssistantMessage, got {}", ev.kind());
        }
    }

    #[test]
    fn fewer_than_max_deltas_buffers_silently() {
        let mut c = Coalescer::default();
        for _ in 0..(MAX_DELTAS - 1) {
            assert!(c.push_delta("msg-1", "x").is_none());
        }
    }

    #[test]
    fn fifth_delta_emits_non_final() {
        let mut c = Coalescer::default();
        for _ in 0..(MAX_DELTAS - 1) {
            assert!(c.push_delta("msg-1", "x").is_none());
        }
        let ev = c
            .push_delta("msg-1", "x")
            .expect("fifth delta should flush");
        let (id, seq, text, is_final) = unwrap_assistant(ev);
        assert_eq!(id, "msg-1");
        assert_eq!(seq, 0);
        assert_eq!(text, "xxxxx");
        assert!(!is_final);
    }

    #[test]
    fn two_messages_are_independent() {
        let mut c = Coalescer::default();
        for _ in 0..(MAX_DELTAS - 1) {
            assert!(c.push_delta("a", "1").is_none());
            assert!(c.push_delta("b", "2").is_none());
        }
        let a_flush = c.push_delta("a", "1").unwrap();
        let b_flush = c.push_delta("b", "2").unwrap();
        let (a_id, _, a_text, _) = unwrap_assistant(a_flush);
        let (b_id, _, b_text, _) = unwrap_assistant(b_flush);
        assert_eq!(a_id, "a");
        assert_eq!(b_id, "b");
        assert_eq!(a_text, "11111");
        assert_eq!(b_text, "22222");
    }

    #[test]
    fn finalize_empties_buffer_and_marks_final() {
        let mut c = Coalescer::default();
        c.push_delta("msg-1", "hello");
        let ev = c.finalize("msg-1").expect("finalize should emit");
        let (id, seq, text, is_final) = unwrap_assistant(ev);
        assert_eq!(id, "msg-1");
        assert_eq!(seq, 0);
        assert_eq!(text, "hello");
        assert!(is_final);
        assert!(c.finalize("msg-1").is_none(), "buffer should be removed");
    }

    #[test]
    fn tick_after_flush_interval_flushes() {
        let mut c = Coalescer::default();
        c.push_delta("msg-1", "abc");
        sleep(FLUSH_INTERVAL + Duration::from_millis(20));
        let out = c.tick(Instant::now());
        assert_eq!(out.len(), 1);
        let (id, _, text, is_final) = unwrap_assistant(out.into_iter().next().unwrap());
        assert_eq!(id, "msg-1");
        assert_eq!(text, "abc");
        assert!(!is_final);
    }
}
