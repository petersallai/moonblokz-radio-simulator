//! Network simulation core module.
//!
//! This module provides the complete simulation infrastructure for a multi-node
//! radio network. It integrates:
//! - Scene loading and configuration
//! - Per-node async tasks interfacing with the radio library
//! - Discrete-time event loop for CAD/airtime windows
//! - Radio reachability with line-of-sight obstacle checks
//! - Signal propagation and collision modeling
//!
//! ## Module Organization
//!
//! - `types`: Core data structures (Scene, Node, messages, channels)
//! - `signal_calculations`: Radio signal and timing calculations
//! - `geometry`: Line-of-sight and obstacle intersection logic
//! - `node_task`: Per-node task managing radio communication
//! - `network_task`: Central simulation task coordinating all nodes
//!
//! ## Public API
//!
//! The main entry point is `network_task`, which should be spawned by the
//! Embassy executor. It communicates with the UI via channels defined in
//! the parent module.

pub mod geometry;
pub mod log_capture;
pub mod network;
pub mod node_task;
pub mod signal_calculations;
pub mod types;

// Re-export the main network task for convenience
pub use network::network_task;

// Re-export commonly used types
pub use types::{NodeMessage, Obstacle, Point};

