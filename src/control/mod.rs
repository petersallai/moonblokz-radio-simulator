//! Control module for sending commands to nodes via the Telemetry Hub.
//!
//! This module implements the same protocol as the Telemetry CLI, allowing
//! the simulator's real-time analyzer to send commands to remote nodes.

pub mod client;
pub mod command;
pub mod config;

pub use client::TelemetryClient;
pub use command::{CommandPayload, ControlCommand, LogLevel};
pub use config::ControlConfig;
