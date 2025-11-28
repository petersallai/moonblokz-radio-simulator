// UI module for the MoonBlokz Radio Simulator
//
// This module organizes the UI into separate components:
// - `mode_selector`: Initial mode selection screen
// - `top_panel`: Top metrics and controls panel
// - `right_panel`: Node inspector and message stream panel
// - `map`: Central map display with nodes and obstacles
// - `app_state`: Application state management and main update loop

pub mod app_state;
pub mod map;
pub mod mode_selector;
pub mod right_panel;
pub mod top_panel;

use crate::simulation::{NodeMessage, Point};

pub use app_state::{AppState, color_for_message_type};

#[derive(Debug)]
pub struct NodeInfo {
    pub node_id: u32,
    pub messages: Vec<NodeMessage>,
}

#[derive(Debug)]
pub enum UIRefreshState {
    Alert(String),
    #[allow(dead_code)]
    NodeUpdated(NodeUIState),
    NodesUpdated(Vec<NodeUIState>),
    ObstaclesUpdated(Vec<crate::simulation::Obstacle>),
    NodeSentRadioMessage(u32, u8, u32), // node ID, message type, and effective distance
    NodeInfo(NodeInfo),
    RadioMessagesCountUpdated(u64, u64, u64), // total sent, total received, total collisions
    SimulationDelayWarningChanged(u32),
    NodeReachedInMeasurement(u32, u32), // node ID and measurement ID
    SimulationSpeedChanged(u32),        // new speed percent
    SendMessageInSimulation(u32),       // sequence number
    PoorAndExcellentLimits(u8, u8),     // poor and excellent limits
}

#[derive(Debug)]
pub struct NodeUIState {
    pub node_id: u32,
    pub position: Point,
    pub radio_strength: u32,
}

pub enum UICommand {
    LoadFile(String),
    RequestNodeInfo(u32),
    StartMeasurement(u32, u32),
    SetAutoSpeed(bool),
}
