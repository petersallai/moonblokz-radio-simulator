//! # UI Module for MoonBlokz Radio Simulator
//!
//! This module implements the complete user interface using the egui immediate-mode GUI framework.
//! The UI is built with eframe (egui's native application framework) and renders at 50 FPS.
//!
//! ## Module Organization
//!
//! - `mode_selector`: Initial mode selection screen for choosing between Simulation, Real-time Tracking, or Log Visualization
//! - `app_state`: Central application state management and main update loop coordinating all UI components
//! - `top_panel`: Top metrics bar displaying system statistics, measurements, and simulation controls
//! - `right_panel`: Node inspector showing detailed message streams and measurement controls
//! - `map`: Central 2D map visualization with nodes, obstacles, and animated radio transmissions
//!
//! ## Communication Protocol
//!
//! The UI communicates with the simulation layer through two bounded channels:
//! - `UIRefreshState`: Events pushed from the network task to update the UI (node states, metrics)
//! - `UICommand`: User commands sent from the UI to the network task (load scene, select node)
//!
//! ## Immediate Mode Architecture
//!
//! egui uses an immediate-mode paradigm where the entire UI is rebuilt every frame.
//! This simplifies state synchronization but requires the render loop to be efficient.
//! The UI maintains minimal state and queries the latest data from channels each frame.

pub mod app_state;
pub mod map;
pub mod mode_selector;
pub mod right_panel;
pub mod top_panel;

use crate::simulation::{NodeMessage, Point};

pub use app_state::{AppState, color_for_message_type};

/// The three operational modes available in the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingMode {
    /// Full simulation with physics modeling.
    Simulation,
    /// Real-time tracking of live log streams.
    RealtimeTracking,
    /// Playback of historical log files.
    LogVisualization,
}

/// Detailed information about a selected node, including its complete message history.
///
/// This struct is sent from the network task to the UI when a node is selected,
/// providing the full list of sent and received messages for display in the inspector panel.
#[derive(Debug)]
pub struct NodeInfo {
    /// Unique identifier of the node.
    pub node_id: u32,
    /// Complete message history for this node (both sent and received).
    pub messages: Vec<NodeMessage>,
}

/// Events pushed from the network task to update the UI state.
///
/// These messages flow through the `UIRefreshChannel` and are processed by
/// the `AppState::update` method each frame. Each variant represents a different
/// type of update that requires UI changes.
#[derive(Debug)]
pub enum UIRefreshState {
    /// Display an alert dialog with the given message.
    Alert(String),
    /// Update a single node's state (currently unused but kept for future use).
    #[allow(dead_code)]
    NodeUpdated(NodeUIState),
    /// Replace the entire node list with a new set (typically on scene load).
    NodesUpdated(Vec<NodeUIState>),
    /// Replace the obstacle list with a new set (typically on scene load).
    ObstaclesUpdated(Vec<crate::simulation::Obstacle>),
    /// A node transmitted a radio message. Parameters: node ID, message type, effective distance.
    NodeSentRadioMessage(u32, u8, u32),
    /// Detailed information about a selected node and its message history.
    NodeInfo(NodeInfo),
    /// Update global packet counters. Parameters: total sent, total received, total collisions.
    RadioMessagesCountUpdated(u64, u64, u64),
    /// Update the simulation delay warning. Parameter: delay in milliseconds (0 = no warning).
    SimulationDelayWarningChanged(u32),
    /// A node was reached during a measurement. Parameters: node ID, measurement ID.
    NodeReachedInMeasurement(u32, u32),
    /// The simulation speed percentage changed (e.g., via auto-speed control).
    SimulationSpeedChanged(u32),
    /// A message was sent during an active measurement. Parameter: sequence number.
    SendMessageInSimulation(u32),
    /// Link quality thresholds from the scoring matrix. Parameters: poor limit, excellent limit.
    PoorAndExcellentLimits(u8, u8),
    SceneDimensionsUpdated(Point, Point, f64, f64),
    BackgroundImageUpdated(Option<String>),
    /// Delay between real clock and last processed log timestamp (real-time tracking only).
    AnalyzerDelay(u64),
    /// Log visualization has reached end of file.
    VisualizationEnded,
    /// Current operating mode changed.
    ModeChanged(OperatingMode),
}

/// UI-specific representation of a node's state.
///
/// This contains only the information needed for rendering the node on the map.
/// Note that `radio_strength` is stored as the calculated effective distance in
/// world units (not dBm) for direct use in rendering.
#[derive(Debug)]
pub struct NodeUIState {
    /// Unique identifier of the node.
    pub node_id: u32,
    /// 2D position in world coordinates (0..10000).
    pub position: Point,
    /// Pre-calculated effective radio range in world units for rendering.
    pub radio_strength: u32,
}

/// Commands sent from the UI to the network task.
///
/// These messages flow through the `UICommandChannel` and are processed by
/// the network task's main loop.
#[derive(Debug)]
pub enum UICommand {
    /// Load a scene configuration file at the given path.
    LoadFile(String),
    /// Request detailed information about a specific node.
    RequestNodeInfo(u32),
    /// Start a measurement from a specific node. Parameters: node ID, measurement identifier.
    StartMeasurement(u32, u32),
    /// Enable or disable automatic speed adjustment.
    SetAutoSpeed(bool),
    /// Start the application in a specific mode with file paths.
    StartMode {
        mode: OperatingMode,
        scene_path: String,
        log_path: Option<String>,
    },
    /// Seek to a specific time in log visualization (future enhancement).
    SeekAnalyzer(u64),
}
