// UI module for the MoonBlokz Radio Simulator
//
// This module organizes the UI into separate components:
// - `top_panel`: Top metrics and controls panel
// - `right_panel`: Node inspector and message stream panel
// - `map`: Central map display with nodes and obstacles

pub mod top_panel;
pub mod right_panel;
pub mod map;

use eframe::egui;
use egui::Color32;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

use crate::simulation::{NodeMessage, Obstacle, Point};

pub const NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT: u64 = 1000;
pub const NODE_RADIO_TRANSFER_INDICATOR_DURATION: Duration = Duration::from_millis(NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT);

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
    ObstaclesUpdated(Vec<Obstacle>),
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

pub struct AppState {
    pub alert: Option<String>,
    pub ui_refresh_rx: crate::UIRefreshChannelReceiver,
    pub ui_command_tx: crate::UICommandChannelSender,
    // Map state
    pub selected: Option<usize>,
    pub nodes: Vec<NodeUIState>,
    pub obstacles: Vec<Obstacle>,
    pub node_radio_transfer_indicators: HashMap<u32, (Instant, u8, u32)>,
    pub node_info: Option<NodeInfo>,
    pub start_time: embassy_time::Instant,
    pub last_node_info_update: Instant,
    pub total_sent_packets: u64,
    pub total_received_packets: u64,
    pub total_collision: u64,
    pub simulation_delay: u32,
    pub measurement_identifier: u32,
    pub reached_nodes: HashSet<u32>,
    pub measurement_start_time: embassy_time::Instant,
    pub scene_file_selected: bool,
    // Persistence: last directory used for scene file chooser
    pub last_open_dir: Option<String>,
    pub echo_result_count: u32,
    // Simulation speed control (percentage)
    pub speed_percent: u32,
    // Auto speed control (network-side scaler)
    pub auto_speed_enabled: bool,
    pub measurement_50_time: u64,
    pub measurement_90_time: u64,
    pub measurement_100_time: u64,
    pub measurement_50_message_count: u32,
    pub measurement_90_message_count: u32,
    pub measurement_100_message_count: u32,
    pub measurement_total_time: u64,
    pub measurement_total_message_count: u32,
    pub poor_limit: u8,
    pub excellent_limit: u8,
    // Map options
    pub show_node_ids: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct PersistedSettings {
    last_open_dir: Option<String>,
}

impl AppState {
    pub fn new(rx: crate::UIRefreshChannelReceiver, tx: crate::UICommandChannelSender, storage: Option<&dyn eframe::Storage>) -> Self {
        // Load persisted settings if available
        let persisted: PersistedSettings = storage.and_then(|s| eframe::get_value(s, "app_settings")).unwrap_or_default();

        Self {
            alert: None,
            ui_refresh_rx: rx,
            ui_command_tx: tx,
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
        }
    }
}

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
        if !self.scene_file_selected {
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
            }
        }

        // Repaint periodically so background updates are visible without input
        ctx.request_repaint_after(Duration::from_millis(20));

        if self.last_node_info_update.elapsed() > Duration::from_secs(1) {
            if let Some(node_info) = &self.node_info {
                self.last_node_info_update = Instant::now();
                _ = self.ui_command_tx.try_send(UICommand::RequestNodeInfo(node_info.node_id));
            }
        }

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
        top_panel::render(ctx, self);
        right_panel::render(ctx, self);
        map::render(ctx, self);
    }
}
