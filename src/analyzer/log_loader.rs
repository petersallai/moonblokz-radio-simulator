//! File I/O abstraction for both real-time and historical log reading.
//!
//! Provides async-compatible log file reading with two modes:
//! - Real-time tracking: Tail-follow semantics (starts at end, polls for new lines)
//! - Log visualization: Sequential reading from start

use embassy_time::{Duration, Timer};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};

use super::types::AnalyzerMode;

/// Buffer size for reading log files (8KB).
const BUFFER_SIZE: usize = 8 * 1024;

/// Poll interval for checking new lines in real-time mode (50ms).
const POLL_INTERVAL_MS: u64 = 50;

/// Log file loader with mode-aware reading behavior.
pub struct LogLoader {
    reader: BufReader<File>,
    mode: AnalyzerMode,
    eof_reached: bool,
    line_buffer: String,
}

impl LogLoader {
    /// Create a new log loader.
    ///
    /// # Parameters
    ///
    /// * `path` - Path to the log file
    /// * `mode` - Determines reading behavior
    ///
    /// # Returns
    ///
    /// `Ok(LogLoader)` if file opens successfully, `Err` otherwise.
    pub fn new(path: &str, mode: AnalyzerMode) -> Result<Self, std::io::Error> {
        let mut file = File::open(path)?;

        // In real-time mode, seek to end to only read new lines
        if mode == AnalyzerMode::RealtimeTracking {
            file.seek(SeekFrom::End(0))?;
        }

        let reader = BufReader::with_capacity(BUFFER_SIZE, file);

        Ok(Self {
            reader,
            mode,
            eof_reached: false,
            line_buffer: String::with_capacity(512),
        })
    }

    /// Read the next line from the log file.
    ///
    /// In `RealtimeTracking` mode, this will poll for new lines if at EOF.
    /// In `LogVisualization` mode, returns `None` when EOF is reached.
    ///
    /// # Returns
    ///
    /// `Some(line)` if a line is available, `None` at EOF (LogVisualization only).
    pub async fn next_line(&mut self) -> Option<String> {
        loop {
            self.line_buffer.clear();

            match self.reader.read_line(&mut self.line_buffer) {
                Ok(0) => {
                    // EOF reached
                    match self.mode {
                        AnalyzerMode::LogVisualization => {
                            self.eof_reached = true;
                            return None;
                        }
                        AnalyzerMode::RealtimeTracking => {
                            // Poll for new content
                            Timer::after(Duration::from_millis(POLL_INTERVAL_MS)).await;
                            // Continue loop to try reading again
                        }
                    }
                }
                Ok(_) => {
                    // Successfully read a line
                    let line = self.line_buffer.trim_end().to_string();
                    if !line.is_empty() {
                        return Some(line);
                    }
                    // Skip empty lines
                }
                Err(e) => {
                    log::warn!("Error reading log file: {}", e);
                    match self.mode {
                        AnalyzerMode::LogVisualization => {
                            self.eof_reached = true;
                            return None;
                        }
                        AnalyzerMode::RealtimeTracking => {
                            // Log error and retry after delay
                            Timer::after(Duration::from_millis(POLL_INTERVAL_MS * 10)).await;
                        }
                    }
                }
            }
        }
    }

    /// Check if EOF has been reached (only meaningful in LogVisualization mode).
    pub fn is_eof(&self) -> bool {
        self.eof_reached
    }

    /// Get the current mode.
    pub fn mode(&self) -> AnalyzerMode {
        self.mode
    }
}
