use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

const MAX_ENTRIES: usize = 500;

#[derive(Clone)]
pub struct TuiLogBuffer {
    inner: Arc<Mutex<VecDeque<String>>>,
}

impl TuiLogBuffer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_ENTRIES))),
        }
    }

    fn push(&self, line: String) {
        let mut buf = self.inner.lock().expect("log buffer poisoned");
        if buf.len() >= MAX_ENTRIES {
            buf.pop_front();
        }
        buf.push_back(line);
    }

    pub fn lines(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("log buffer poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

pub struct TuiTracingLayer {
    buffer: TuiLogBuffer,
}

impl TuiTracingLayer {
    pub fn new(buffer: TuiLogBuffer) -> Self {
        Self { buffer }
    }
}

struct MessageVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: Vec::new(),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }
}

impl<S> Layer<S> for TuiTracingLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let level = meta.level();
        let target = meta.target();

        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);

        let mut line = format!("{level} {target}: {}", visitor.message);
        for (k, v) in &visitor.fields {
            line.push_str(&format!(" {k}={v}"));
        }
        self.buffer.push(line);
    }
}
