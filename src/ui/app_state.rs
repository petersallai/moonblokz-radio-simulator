//! # Application State Management
//!
//! This module implements the central `AppState` struct which manages all UI state
//! and coordinates the rendering of all UI components. It implements the `eframe::App`
//! trait to integrate with the egui application framework.
//!
//! ## Responsibilities
//!
//! - Manages the complete UI state (nodes, obstacles, selection, metrics)
//! - Processes incoming messages from the simulation via `ui_refresh_rx`
//! - Sends user commands to the simulation via `ui_command_tx`
//! - Coordinates rendering of all UI panels (top, right, map)
//! - Manages the 50 FPS render loop via `request_repaint_after`
//! - Persists user settings (last directory) across application sessions
//!
//! ## State Management
//!
//! The AppState uses an immediate-mode UI paradigm where the entire interface is
//! rebuilt every frame. State is updated by consuming messages from the simulation
//! and then rendered by delegating to specialized panel render functions.

use eframe::egui;
use egui::Color32;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

use super::{NodeInfo, NodeUIState, UICommand, UIRefreshState, mode_selector};
use crate::simulation::Obstacle;
use crate::simulation::Point;

/// Duration (in milliseconds) that a radio transmission indicator remains visible on the map.
/// The indicator fades from full opacity to transparent over this period.
pub const NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT: u64 = 1000;
/// Duration constant as a `std::time::Duration` for easy comparison with timestamps.
pub const NODE_RADIO_TRANSFER_INDICATOR_DURATION: Duration = Duration::from_millis(NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT);

/// Central application state managing all UI components and simulation coordination.
///
/// This struct maintains all state needed for rendering the UI and coordinates
/// communication between the user interface and the simulation backend. It is
/// rebuilt every frame (immediate mode) but maintains persistent state between frames.
pub struct AppState {
    /// Optional alert message to display in a modal dialog.
    pub alert: Option<String>,
    /// Receiver for UI refresh messages from the simulation.
    pub ui_refresh_rx: crate::UIRefreshQueueReceiver,
    /// Sender for commands from the UI to the simulation.
    pub ui_command_tx: crate::UICommandQueueSender,

    // Mode selection state
    /// Mode selector component for choosing simulation/tracking/log mode.
    pub mode_selector: mode_selector::ModeSelector,
    /// Whether the user has selected a mode (determines if mode selector is shown).
    pub mode_selected: bool,

    // Map visualization state
    /// Index of the currently selected node in the `nodes` vector, if any.
    pub selected: Option<usize>,
    /// All nodes in the scene with their positions and radio ranges.
    pub nodes: Vec<NodeUIState>,
    /// All obstacles (walls, circles) in the scene.
    pub obstacles: Vec<Obstacle>,
    /// Active radio transmission indicators: node_id -> (expiry_time, message_type, distance).
    pub node_radio_transfer_indicators: HashMap<u32, (Instant, u8, u32)>,
    /// Detailed info for the selected node (messages, statistics).
    pub node_info: Option<NodeInfo>,

    // Timing and metrics
    /// Simulation start time (virtual time, scaled by time driver).
    pub start_time: embassy_time::Instant,
    /// Last time the node info was refreshed (real time, for throttling updates).
    pub last_node_info_update: Instant,
    /// Total packets sent across all nodes.
    pub total_sent_packets: u64,
    /// Total packets successfully received across all nodes.
    pub total_received_packets: u64,
    /// Total packet collisions detected.
    pub total_collision: u64,
    /// Current simulation delay in milliseconds (0 = no delay warning).
    pub simulation_delay: u32,

    // Measurement state
    /// Unique identifier for the current measurement (0 = no active measurement).
    pub measurement_identifier: u32,
    /// Set of node IDs reached during the current measurement.
    pub reached_nodes: HashSet<u32>,
    /// Virtual time when the current measurement started.
    pub measurement_start_time: embassy_time::Instant,
    /// Whether a scene file has been selected (after mode selection).
    pub scene_file_selected: bool,

    // Persistence
    /// Last directory used for file picker (persisted across sessions).
    pub last_open_dir: Option<String>,

    // Statistics
    /// Count of echo result messages observed.
    pub echo_result_count: u32,

    // Speed control
    /// Current simulation speed as a percentage (100 = real-time, 200 = 2x, etc.).
    pub speed_percent: u32,
    /// Whether automatic speed adjustment is enabled.
    pub auto_speed_enabled: bool,

    // Measurement milestones
    /// Virtual time when 50% of nodes were reached (seconds).
    pub measurement_50_time: u64,
    /// Virtual time when 90% of nodes were reached (seconds).
    pub measurement_90_time: u64,
    /// Virtual time when 100% of nodes were reached (seconds).
    pub measurement_100_time: u64,
    /// Number of packets sent when 50% distribution was reached.
    pub measurement_50_message_count: u32,
    /// Number of packets sent when 90% distribution was reached.
    pub measurement_90_message_count: u32,
    /// Number of packets sent when 100% distribution was reached.
    pub measurement_100_message_count: u32,
    /// Total elapsed time for the current measurement (seconds).
    pub measurement_total_time: u64,
    /// Total packets sent during the current measurement.
    pub measurement_total_message_count: u32,

    // Link quality thresholds
    /// Link quality value considered "poor" (from scoring matrix).
    pub poor_limit: u8,
    /// Link quality value considered "excellent" (from scoring matrix).
    pub excellent_limit: u8,

    // Map display options
    /// Whether to display node IDs as text labels on the map.
    pub show_node_ids: bool,
    /// Top-left corner of the world coordinate system.
    pub world_top_left: Point,
    /// Bottom-right corner of the world coordinate system.
    pub world_bottom_right: Point,
    /// Width of the world in meters.
    pub width: f64,
    /// Height of the world in meters.
    pub height: f64,
}

/// Settings persisted across application sessions.
///
/// Currently only stores the last directory used for file selection,
/// improving UX by remembering the user's working directory.
#[derive(Default, Serialize, Deserialize)]
struct PersistedSettings {
    last_open_dir: Option<String>,
}

impl AppState {
    /// Create a new AppState, loading persisted settings if available.
    ///
    /// # Parameters
    ///
    /// * `rx` - Receiver for UI refresh messages from the simulation
    /// * `tx` - Sender for commands to the simulation
    /// * `storage` - Optional persistent storage for loading saved settings
    ///
    /// # Returns
    ///
    /// A fully initialized AppState ready for rendering.
    pub fn new(rx: crate::UIRefreshQueueReceiver, tx: crate::UICommandQueueSender, storage: Option<&dyn eframe::Storage>) -> Self {
        // Load persisted settings if available
        let persisted: PersistedSettings = storage.and_then(|s| eframe::get_value(s, "app_settings")).unwrap_or_default();

        Self {
            alert: None,
            ui_refresh_rx: rx,
            ui_command_tx: tx,
            mode_selector: mode_selector::ModeSelector::new(),
            mode_selected: false,
            selected: None,
            nodes: Vec::new(),
            obstacles: Vec::new(),
            node_radio_transfer_indicators: HashMap::new(),
            node_info: None,
            start_time: embassy_time::Instant::now(),
            last_node_info_update: Instant::now(),
            total_sent_packets: 0,
            total_received_packets: 0,
            total_collision: 0,
            simulation_delay: 0,
            measurement_identifier: 0,
            reached_nodes: HashSet::new(),
            measurement_start_time: embassy_time::Instant::now(),
            scene_file_selected: false,
            last_open_dir: persisted.last_open_dir,
            echo_result_count: 0,
            speed_percent: crate::time_driver::get_simulation_speed_percent(),
            auto_speed_enabled: false,
            measurement_50_time: 0,
            measurement_90_time: 0,
            measurement_100_time: 0,
            measurement_50_message_count: 0,
            measurement_90_message_count: 0,
            measurement_100_message_count: 0,
            measurement_total_time: 0,
            measurement_total_message_count: 0,
            poor_limit: 0,
            excellent_limit: 0,
            show_node_ids: false,
            world_top_left: Point { x: 0.0, y: 0.0 },
            world_bottom_right: Point { x: 100.0, y: 100.0 },
            width: 1.0,
            height: 1.0,
        }
    }

    /// Open a native file picker dialog for selecting a scene JSON file.
    ///
    /// This method displays a file picker filtered to JSON files, starting in the
    /// last used directory if available. Upon selection, sends a LoadFile command
    /// to the simulation and updates the last directory for next time.
    ///
    /// If the user cancels the picker, returns to the mode selection screen.
    pub fn open_file_selector(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter("text", &["json"]);
        if let Some(dir) = &self.last_open_dir {
            dialog = dialog.set_directory(dir);
        }
        let files = dialog.pick_file();
        if let Some(file) = files {
            let _ = self.ui_command_tx.try_send(UICommand::LoadFile(file.to_str().unwrap().to_string()));
            self.scene_file_selected = true;
            // Remember directory for next time
            if let Some(parent) = file.parent() {
                self.last_open_dir = Some(parent.to_string_lossy().to_string());
            }
        } else {
            // User cancelled the picker; return to mode selection screen
            self.mode_selected = false;
            self.scene_file_selected = false;
        }
    }
}

/// Map a message type code to a color for visualization.
///
/// Each message type in the MoonBlokz protocol is assigned a distinct color
/// for easy identification on the map and in the message stream.
///
/// # Parameters
///
/// * `message_type` - The numeric message type code (1-9)
/// * `alpha` - Opacity multiplier (0.0 = transparent, 1.0 = fully opaque)
///
/// # Returns
///
/// An egui Color32 with the specified alpha blended in.
///
/// # Message Type Mapping
///
/// - 1: Green (Echo Request)
/// - 2: Yellow (Echo)
/// - 3: Orange-Brown (Echo Result)
/// - 4: Blue (Request Block)
/// - 5: Magenta (Request Block Part)
/// - 6: Orange (Add Block)
/// - 7: Cyan (Add Transaction)
/// - 8: Purple (Request Mempool)
/// - 9: Pink (Support)
/// - Unknown: White
pub fn color_for_message_type(message_type: u8, alpha: f32) -> Color32 {
    match message_type {
        1 => Color32::from_rgba_unmultiplied(0, 255, 0, (alpha * 255.0) as u8),   // Type 1: Green
        2 => Color32::from_rgba_unmultiplied(255, 255, 0, (alpha * 255.0) as u8), // Type 2: Yellow
        3 => Color32::from_rgba_unmultiplied(200, 100, 50, (alpha * 255.0) as u8), // Type 3: Red
        4 => Color32::from_rgba_unmultiplied(0, 0, 255, (alpha * 255.0) as u8),   // Type 4: Blue
        5 => Color32::from_rgba_unmultiplied(255, 0, 255, (alpha * 255.0) as u8), // Type 5: Magenta
        6 => Color32::from_rgba_unmultiplied(255, 165, 0, (alpha * 255.0) as u8), // Type 6: Orange
        7 => Color32::from_rgba_unmultiplied(0, 255, 255, (alpha * 255.0) as u8), // Type 7: Cyan
        8 => Color32::from_rgba_unmultiplied(128, 0, 128, (alpha * 255.0) as u8), // Type 8: Purple
        9 => Color32::from_rgba_unmultiplied(255, 192, 203, (alpha * 255.0) as u8), // Type 9: Pink
        _ => Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8), // Unknown type: White
    }
}

impl eframe::App for AppState {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let settings = PersistedSettings {
            last_open_dir: self.last_open_dir.clone(),
        };
        eframe::set_value(storage, "app_settings", &settings);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Show mode selector first if mode not yet selected
        if !self.mode_selected {
            if let Some(_mode) = self.mode_selector.render(ctx) {
                self.mode_selected = true;
                // All modes currently open the file selector
                self.open_file_selector();
            }
            return;
        }

        if !self.scene_file_selected {
            // File selector dialog is non-blocking, so we just wait for it to complete
            return;
        }

        // Repaint periodically so background updates are visible without input
        ctx.request_repaint_after(Duration::from_millis(20));

        if self.last_node_info_update.elapsed() > Duration::from_secs(1) {
            if let Some(node_info) = &self.node_info {
                self.last_node_info_update = Instant::now();
                _ = self.ui_command_tx.try_send(UICommand::RequestNodeInfo(node_info.node_id));
            }
        }

        // Clean up expired radio transfer indicators to prevent unbounded HashMap growth
        let now = Instant::now();
        self.node_radio_transfer_indicators.retain(|_, (expiry_time, _, _)| *expiry_time > now);

        while let Ok(msg) = self.ui_refresh_rx.try_receive() {
            match msg {
                UIRefreshState::Alert(alert_msg) => {
                    self.alert = Some(alert_msg);
                }
                UIRefreshState::NodeUpdated(node) => {
                    if let Some(existing) = self.nodes.iter_mut().find(|n| n.node_id == node.node_id) {
                        *existing = node;
                    } else {
                        self.nodes.push(node);
                    }
                }
                UIRefreshState::NodesUpdated(nodes) => {
                    self.nodes = nodes;
                }
                UIRefreshState::ObstaclesUpdated(obstacles) => {
                    self.obstacles = obstacles;
                }
                UIRefreshState::NodeSentRadioMessage(node_id, message_type, distance) => {
                    self.node_radio_transfer_indicators
                        .insert(node_id, (Instant::now() + NODE_RADIO_TRANSFER_INDICATOR_DURATION, message_type, distance));
                    if message_type == moonblokz_radio_lib::MessageType::EchoResult as u8 {
                        self.echo_result_count += 1;
                    }
                }
                UIRefreshState::NodeInfo(node_info) => {
                    self.node_info = Some(node_info);
                }
                UIRefreshState::RadioMessagesCountUpdated(total_sent_packets, total_received_packets, total_collision) => {
                    self.total_sent_packets = total_sent_packets;
                    self.total_received_packets = total_received_packets;
                    self.total_collision = total_collision;
                }
                UIRefreshState::SimulationDelayWarningChanged(delay) => {
                    self.simulation_delay = delay;
                }
                UIRefreshState::NodeReachedInMeasurement(node_id, measurement_id) => {
                    if self.measurement_identifier == measurement_id {
                        self.reached_nodes.insert(node_id);
                    }
                }
                UIRefreshState::SimulationSpeedChanged(new_speed) => {
                    self.speed_percent = new_speed;
                }
                UIRefreshState::SendMessageInSimulation(measurement_id) => {
                    if self.measurement_identifier == measurement_id {
                        self.measurement_total_message_count += 1;
                        self.measurement_total_time = self.measurement_start_time.elapsed().as_secs();
                    }
                }
                UIRefreshState::PoorAndExcellentLimits(poor, excellent) => {
                    self.poor_limit = poor;
                    self.excellent_limit = excellent;
                }
                UIRefreshState::SceneDimensionsUpdated(top_left, bottom_right, width, height) => {
                    self.world_top_left = top_left;
                    self.world_bottom_right = bottom_right;
                    self.width = width;
                    self.height = height;
                }
            }
        }

        if self.alert.is_some() {
            egui::Window::new("Alert")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.label(self.alert.as_ref().unwrap());
                        ui.add_space(20.0);

                        if ui.button("OK").clicked() {
                            self.alert = None; // Reset alert state
                        }
                        ui.add_space(10.0);
                    });
                });
        }

        // Panels layout: top (fixed), right (fixed), map fills the remaining using CentralPanel
        super::top_panel::render(ctx, self);
        super::right_panel::render(ctx, self);
        super::map::render(ctx, self);
    }
}
