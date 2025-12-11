//! Analyzer module for log parsing and visualization.
//!
//! Provides functionality for:
//! - Real-time tracking of live log streams
//! - Log file visualization with time-synchronized playback
//!
//! The analyzer communicates with the UI using the same channels as the simulation module.

pub mod log_loader;
pub mod log_parser;
pub mod task;
pub mod types;

pub use task::analyzer_task;
pub use types::{AnalyzerMode, LogEvent, NodePacketRecord};
