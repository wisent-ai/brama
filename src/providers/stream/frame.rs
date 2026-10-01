//! How arriving bytes become whole `(event, data)` pairs.
//!
//! Every provider read here ships `text/event-stream`, so the framing is one
//! subject shared by all of them and no wire module should ever see half a
//! line: chunk boundaries fall wherever the network put them, one event's
//! payload can arrive as several `data:` fields, and a line is buffered until
//! its end arrives. What a framed event then means is a different question,
//! answered per provider.

/// Line-based SSE framing: events in, complete `(event, data)` pairs out.
///
/// The wire is `event:` and `data:` fields joined by blank lines, with `:`
/// comments interleaved. Data accumulates across repeated `data:` fields of
/// one event, joined by newlines, per the SSE grammar.
pub(super) struct SseFramer {
    buffer: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseFramer {
    pub(super) fn new() -> Self {
        Self {
            buffer: String::new(),
            event: None,
            data: Vec::new(),
        }
    }

    /// Append one received chunk and yield every event it completed.
    pub(super) fn feed(&mut self, chunk: &[u8]) -> Result<Vec<(Option<String>, String)>, String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(line_end) = self.buffer.find('\n') {
            let line = self.buffer[..line_end].trim_end_matches('\r').to_string();
            self.buffer.drain(..=line_end);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push((self.event.take(), self.data.join("\n")));
                    self.data.clear();
                } else {
                    self.event = None;
                }
                continue;
            }
            if let Some(comment) = line.strip_prefix(':') {
                let _ = comment;
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_str(), ""),
            };
            match field {
                "event" => self.event = Some(value.to_string()),
                "data" => self.data.push(value.to_string()),
                _ => {}
            }
        }
        Ok(events)
    }
}
