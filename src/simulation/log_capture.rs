//! Log capture for routing moonblokz_radio_lib logs to per-node log streams.
//!
//! This module provides a custom logger that intercepts log messages from the
//! `moonblokz_radio_lib` crate, extracts the node ID from the `[N]` prefix in
//! the log message, and routes them to a global buffer for later distribution
//! to the appropriate node's log history.
//!
//! The log format from moonblokz_radio_lib is: `[node_id] message content`
//! For example: `[49] RX handler task started`

use embassy_time::Instant;
use log::{Level, Log, Metadata, Record};
use std::collections::VecDeque;
use std::sync::Mutex;

use super::types::LogLevel;

/// Maximum number of log entries to buffer before they're consumed.
const LOG_BUFFER_CAPACITY: usize = 10000;

/// A captured log entry with extracted node ID.
#[derive(Clone)]
pub struct CapturedLogEntry {
    pub node_id: u32,
    pub timestamp: Instant,
    pub content: String,
    pub level: LogLevel,
}

/// Global buffer for captured log entries.
static CAPTURED_LOGS: Mutex<Option<VecDeque<CapturedLogEntry>>> = Mutex::new(None);

/// Initialize the log capture buffer.
pub fn init_log_capture() {
    let mut guard = CAPTURED_LOGS.lock().unwrap();
    *guard = Some(VecDeque::with_capacity(LOG_BUFFER_CAPACITY));
}

/// Drain all captured log entries from the buffer.
pub fn drain_captured_logs() -> Vec<CapturedLogEntry> {
    let mut guard = CAPTURED_LOGS.lock().unwrap();
    if let Some(buffer) = guard.as_mut() {
        buffer.drain(..).collect()
    } else {
        Vec::new()
    }
}

/// Push a captured log entry to the buffer.
fn push_log_entry(entry: CapturedLogEntry) {
    let mut guard = CAPTURED_LOGS.lock().unwrap();
    if let Some(buffer) = guard.as_mut() {
        if buffer.len() >= LOG_BUFFER_CAPACITY {
            buffer.pop_front();
        }
        buffer.push_back(entry);
    }
}

/// Extract the node ID from a log message with format `[N] ...`.
/// Returns (node_id, remaining_message) if successful.
fn extract_node_id(message: &str) -> Option<(u32, &str)> {
    let trimmed = message.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }

    let end_bracket = trimmed.find(']')?;
    let node_id_str = &trimmed[1..end_bracket];
    let node_id: u32 = node_id_str.parse().ok()?;

    // Get the rest of the message after the bracket
    let rest = trimmed[end_bracket + 1..].trim_start();
    Some((node_id, rest))
}

/// Convert log::Level to our LogLevel enum.
fn convert_level(level: Level) -> LogLevel {
    match level {
        Level::Error => LogLevel::Error,
        Level::Warn => LogLevel::Warn,
        Level::Info => LogLevel::Info,
        Level::Debug => LogLevel::Debug,
        Level::Trace => LogLevel::Trace,
    }
}

/// A tee logger that forwards to the original logger and captures moonblokz_radio_lib logs.
pub struct TeeLogger {
    inner: env_logger::Logger,
}

impl TeeLogger {
    pub fn new(inner: env_logger::Logger) -> Self {
        Self { inner }
    }

    /// Get the maximum log level filter from the inner logger.
    pub fn filter(&self) -> log::LevelFilter {
        self.inner.filter()
    }
}

impl Log for TeeLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        // Always forward to the inner logger first
        self.inner.log(record);

        // Only capture logs from moonblokz_radio_lib
        if let Some(module) = record.module_path() {
            if module.starts_with("moonblokz_radio_lib") {
                let message = format!("{}", record.args());
                if let Some((node_id, content)) = extract_node_id(&message) {
                    push_log_entry(CapturedLogEntry {
                        node_id,
                        timestamp: Instant::now(),
                        content: content.to_string(),
                        level: convert_level(record.level()),
                    });
                }
            }
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_node_id() {
        assert_eq!(extract_node_id("[49] RX handler started"), Some((49, "RX handler started")));
        assert_eq!(extract_node_id("[1] Test"), Some((1, "Test")));
        assert_eq!(extract_node_id("[123] "), Some((123, "")));
        assert_eq!(extract_node_id("No bracket"), None);
        assert_eq!(extract_node_id("[abc] Not a number"), None);
    }
}
