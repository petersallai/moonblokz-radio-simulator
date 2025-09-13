//! GUI for the MoonBlokz Radio Simulator.
//!
//! Uses eframe/egui to render:
//! - Top metrics and controls (including speed and auto-speed toggles)
//! - Right-side inspector for node details and message stream
//! - Central map with obstacles, nodes, optional IDs, and radio pulse indicators
//!
//! The UI exchanges state with the network task via two bounded channels:
//! `UIRefreshChannel` (network → UI) and `UICommandChannel` (UI → network).

use eframe::egui;
use egui::Color32;
use embassy_executor::{Executor, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use env_logger::Builder;
use log::{LevelFilter, debug, info};
use moonblokz_radio_lib::MessageType;
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::cmp::max;
use std::collections::HashMap;
use std::collections::HashSet;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use crate::network::NodeMessage;
use crate::network::Obstacle;
use crate::network::Point;

mod network;
mod signal_calculations;
mod time_driver;

const UI_REFRESH_CHANNEL_SIZE: usize = 500;
type UIRefreshChannel = embassy_sync::channel::Channel<CriticalSectionRawMutex, UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;
type UIRefreshChannelReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;
type UIRefreshChannelSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;

const UI_COMMAND_CHANNEL_SIZE: usize = 100;
type UICommandChannel = embassy_sync::channel::Channel<CriticalSectionRawMutex, UICommand, UI_COMMAND_CHANNEL_SIZE>;
type UICommandChannelReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, UICommand, UI_COMMAND_CHANNEL_SIZE>;
type UICommandChannelSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, UICommand, UI_COMMAND_CHANNEL_SIZE>;

const NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT: u64 = 1000;
const NODE_RADIO_TRANSFER_INDICATOR_DURATION: Duration = Duration::from_millis(NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT);

#[derive(Debug)]
pub struct NodeInfo {
    pub node_id: u32,
    pub messages: Vec<NodeMessage>,
}

#[derive(Debug)]
enum UIRefreshState {
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
struct NodeUIState {
    node_id: u32,
    position: Point,
    radio_strength: u32,
}

enum UICommand {
    LoadFile(String),
    RequestNodeInfo(u32),
    StartMeasurement(u32, u32),
    SetAutoSpeed(bool),
}

struct AppState {
    alert: Option<String>,
    ui_refresh_rx: UIRefreshChannelReceiver,
    ui_command_tx: UICommandChannelSender,
    // Map state
    selected: Option<usize>,
    nodes: Vec<NodeUIState>,
    obstacles: Vec<Obstacle>,
    node_radio_transfer_indicators: HashMap<u32, (Instant, u8, u32)>,
    node_info: Option<NodeInfo>,
    start_time: embassy_time::Instant,
    last_node_info_update: Instant,
    total_sent_packets: u64,
    total_received_packets: u64,
    total_collision: u64,
    simulation_delay: u32,
    measurement_identifier: u32,
    reached_nodes: HashSet<u32>,
    measurement_start_time: embassy_time::Instant,
    scene_file_selected: bool,
    // Persistence: last directory used for scene file chooser
    last_open_dir: Option<String>,
    echo_result_count: u32,
    // Simulation speed control (percentage)
    speed_percent: u32,
    // Auto speed control (network-side scaler)
    auto_speed_enabled: bool,
    measurement_50_time: u64,
    measurement_90_time: u64,
    measurement_100_time: u64,
    measurement_50_message_count: u32,
    measurement_90_message_count: u32,
    measurement_100_message_count: u32,
    measurement_total_time: u64,
    measurement_total_message_count: u32,
    poor_limit: u8,
    excellent_limit: u8,
    // Map options
    show_node_ids: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct PersistedSettings {
    last_open_dir: Option<String>,
}

impl AppState {
    fn new(rx: UIRefreshChannelReceiver, tx: UICommandChannelSender, storage: Option<&dyn eframe::Storage>) -> Self {
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

fn color_for_message_type(message_type: u8, alpha: f32) -> Color32 {
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
            let mut dialog = FileDialog::new().add_filter("text", &["json"]);
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
                    if message_type == MessageType::EchoResult as u8 {
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
                //                .modal(true)
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

        // Top: system metrics (fixed 200 px height) arranged into 3 vertical stacks
        egui::TopBottomPanel::top("top_metrics").exact_height(150.0).show(ctx, |ui| {
            let throughput_tx = if self.start_time.elapsed().as_secs() > 0 {
                ((self.total_sent_packets as f64 / self.start_time.elapsed().as_secs() as f64) * 60.0) as u64
            } else {
                0
            };

            let throughput_rx = if self.start_time.elapsed().as_secs() > 0 {
                ((self.total_received_packets as f64 / self.start_time.elapsed().as_secs() as f64) * 60.0) as u64
            } else {
                0
            };

            let collision_rate = if self.total_received_packets > 0 {
                (self.total_collision as f64 / (self.total_received_packets as f64 + self.total_collision as f64)) * 100.0
            } else {
                0.0
            };

            ui.columns(3, |cols| {
                // Column 1: Title + core metrics
                cols[0].vertical(|ui| {
                    ui.heading("System Metrics");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Sim time:");
                        // Fixed-width, monospace time so following labels don't shift horizontally
                        // Use embassy_time Instant for simulation time (scaled by driver)
                        let sim_secs = embassy_time::Instant::now().as_secs();
                        let sim_secs_with_s = format!("{}s", sim_secs);
                        let sim_time_str = format!("{:<6}", sim_secs_with_s); // fixed 6 chars, left-aligned (e.g., "42    ")
                        ui.label(egui::RichText::new(sim_time_str).monospace().strong());
                        ui.label("Total TX: ");
                        ui.label(egui::RichText::new(self.total_sent_packets.to_string()).strong());
                    });

                    let nodes_count_str = format!("{:<7}", self.nodes.len()); // fixed 7 chars, left-aligned (e.g., "42    ")

                    ui.horizontal(|ui| {
                        ui.label("Nodes:");
                        ui.label(egui::RichText::new(nodes_count_str).monospace().strong());
                        ui.label("  Echo results: ");
                        ui.label(egui::RichText::new(self.echo_result_count.to_string()).strong());
                    });
                    ui.horizontal(|ui| {
                        ui.label("Throughput(TX):");
                        ui.label(egui::RichText::new(format!("{}", throughput_tx)).strong());
                        ui.label("packets/minutes");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Throughput(RX):");
                        ui.label(egui::RichText::new(format!("{}", throughput_rx)).strong());
                        ui.label("packets/minutes");
                    });

                    ui.horizontal(|ui| {
                        ui.label("Collision rate:");
                        ui.label(egui::RichText::new(format!("{:.2}", collision_rate)).strong());
                        ui.label("%");
                    });
                });

                // Column 2: Measured distribution
                cols[1].vertical(|ui| {
                    let measurement_duration_string = if self.measurement_identifier > 0 {
                        let measurement_total_time_with_s = format!("{}s", self.measurement_total_time);
                        format!("{:<7}", measurement_total_time_with_s)
                    } else {
                        "-".into()
                    };

                    let distribution_percentage = if self.nodes.len() > 0 && self.measurement_identifier > 0 {
                        (self.reached_nodes.len() as f64 / self.nodes.len() as f64) * 100.0
                    } else {
                        0.0
                    };

                    let distribution_percentage_string = if self.nodes.len() > 0 && self.measurement_identifier > 0 {
                        format!("{:.0}", distribution_percentage)
                    } else {
                        "-".into()
                    };

                    if distribution_percentage >= 50.0 && self.measurement_50_time == 0 {
                        self.measurement_50_time = self.measurement_start_time.elapsed().as_secs();
                        self.measurement_50_message_count = self.measurement_total_message_count;
                    }

                    if distribution_percentage >= 90.0 && self.measurement_90_time == 0 {
                        self.measurement_90_time = self.measurement_start_time.elapsed().as_secs();
                        self.measurement_90_message_count = self.measurement_total_message_count;
                    }

                    if distribution_percentage >= 99.9 && self.measurement_100_time == 0 {
                        self.measurement_100_time = self.measurement_start_time.elapsed().as_secs();
                        self.measurement_100_message_count = self.measurement_total_message_count;
                    }

                    let measurement_50_time_string = if self.measurement_50_time > 0 {
                        format!("{}s", self.measurement_50_time)
                    } else {
                        "-".into()
                    };
                    let measurement_90_time_string = if self.measurement_90_time > 0 {
                        format!("{}s", self.measurement_90_time)
                    } else {
                        "-".into()
                    };
                    let measurement_100_time_string = if self.measurement_100_time > 0 {
                        format!("{}s", self.measurement_100_time)
                    } else {
                        "-".into()
                    };

                    let p_per_n_string = if self.measurement_total_message_count > 0 && self.nodes.len() > 0 {
                        format!("{}", (self.measurement_total_message_count * 100) / self.nodes.len() as u32)
                    } else {
                        "-".into()
                    };

                    let p_per_n_50_string = if self.measurement_50_message_count > 0 && self.nodes.len() > 0 {
                        format!("{}", (self.measurement_50_message_count * 100) / self.nodes.len() as u32)
                    } else {
                        "-".into()
                    };

                    let p_per_n_90_string = if self.measurement_90_message_count > 0 && self.nodes.len() > 0 {
                        format!("{}", (self.measurement_90_message_count * 100) / self.nodes.len() as u32)
                    } else {
                        "-".into()
                    };

                    let p_per_n_100_string = if self.measurement_100_message_count > 0 && self.nodes.len() > 0 {
                        format!("{}", (self.measurement_100_message_count * 100) / self.nodes.len() as u32)
                    } else {
                        "-".into()
                    };

                    ui.heading("Measured data");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Total time: ");
                        ui.label(egui::RichText::new(measurement_duration_string).strong().monospace());
                        ui.label("packets: ");
                        ui.label(egui::RichText::new(format!("{}", self.measurement_total_message_count)).strong());
                    });
                    ui.horizontal(|ui| {
                        ui.label("Distribution: ");
                        ui.label(egui::RichText::new(format!("{}", distribution_percentage_string)).strong().monospace());
                        ui.label("% ");
                        ui.label("  P/N: ");
                        ui.label(egui::RichText::new(p_per_n_string).strong());
                        ui.label("%");
                    });
                    ui.horizontal(|ui| {
                        ui.label("50% time: ");
                        ui.label(egui::RichText::new(format!("{}", measurement_50_time_string)).strong());
                        ui.label("   P/N:");
                        ui.label(egui::RichText::new(p_per_n_50_string).strong());
                        ui.label("%");
                    });
                    ui.horizontal(|ui| {
                        ui.label("90% time: ");
                        ui.label(egui::RichText::new(format!("{}", measurement_90_time_string)).strong());
                        ui.label("   P/N:");
                        ui.label(egui::RichText::new(p_per_n_90_string).strong());
                        ui.label("%");
                    });
                    ui.horizontal(|ui| {
                        ui.label("100% time: ");
                        ui.label(egui::RichText::new(format!("{}", measurement_100_time_string)).strong());
                        ui.label("   P/N:");
                        ui.label(egui::RichText::new(p_per_n_100_string).strong());
                        ui.label("%");
                    });
                });

                // Column 3: Controls
                cols[2].vertical(|ui| {
                    ui.heading("Controls");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Speed:");
                        let mut speed = self.speed_percent as f64;
                        // Keep UI slider in sync with autoscaler bounds
                        if ui.add(egui::Slider::new(&mut speed, 20.0..=1000.0).suffix("%")).changed() {
                            self.speed_percent = speed.round() as u32;
                            crate::time_driver::set_simulation_speed_percent(self.speed_percent);
                        }
                    });
                    let mut auto = self.auto_speed_enabled;
                    if ui.checkbox(&mut auto, "Auto speed").changed() {
                        self.auto_speed_enabled = auto;
                        let _ = self.ui_command_tx.try_send(UICommand::SetAutoSpeed(self.auto_speed_enabled));
                    }
                    if ui.button("Reset").clicked() {
                        self.speed_percent = 100;
                        crate::time_driver::set_simulation_speed_percent(self.speed_percent);
                    }
                    ui.separator();
                    let mut show_ids = self.show_node_ids;
                    if ui.checkbox(&mut show_ids, "Show node IDs").changed() {
                        self.show_node_ids = show_ids;
                    }
                    if self.simulation_delay > 10 {
                        ui.separator();
                        let warn_text = format!("Warning! Simulation delay is more than 10 milliseconds ({} ms)", self.simulation_delay);
                        ui.add(egui::Label::new(egui::RichText::new(warn_text).color(egui::Color32::RED)).wrap(true));
                    }
                });
            });
        });

        // Bottom-right: inspector (fixed 400 px) with top-left info and bottom-aligned centered buttons
        egui::SidePanel::right("inspector_right").exact_width(400.0).show(ctx, |ui| {
            // Top content (default top-down, left-aligned)
            ui.heading("Inspector");
            ui.separator();
            if let Some(i) = self.selected {
                let p = &self.nodes[i];
                ui.horizontal(|ui| {
                    ui.label("Selected point:");
                    ui.label(egui::RichText::new(format!("#{}", p.node_id)).strong().color(Color32::from_rgb(0, 128, 255)));
                });
                ui.horizontal(|ui| {
                    ui.label("Position: (");
                    ui.label(egui::RichText::new(format!("{:.2}", p.position.x)).strong());
                    ui.label(",");
                    ui.label(egui::RichText::new(format!("{:.2}", p.position.y)).strong());
                    ui.label(")");
                });
                ui.horizontal(|ui| {
                    ui.label("Radio strength:");
                    ui.label(egui::RichText::new(format!("{}", p.radio_strength)).strong());
                });
                ui.separator();

                let mut sent_messages_count = 0;
                let mut received_messages_count = 0;

                if let Some(node_info) = &self.node_info {
                    for msg in &node_info.messages {
                        if msg.sender_node == p.node_id {
                            sent_messages_count += 1;
                        } else {
                            received_messages_count += 1;
                        }
                    }
                }

                ui.horizontal(|ui| {
                    ui.label("Sent messages:");
                    ui.label(egui::RichText::new(format!("{}", sent_messages_count)).strong());
                });
                ui.horizontal(|ui| {
                    ui.label("Received messages:");
                    ui.label(egui::RichText::new(format!("{}", received_messages_count)).strong());
                });

                // Messages header (outside of bottom-up so it doesn't steal table space)
                if let Some(node_info) = &self.node_info {
                    ui.separator();
                    ui.heading(format!("Radio stream for #{}", node_info.node_id));
                    ui.add_space(4.0);
                }

                // Remaining area: bottom-up so buttons stick to the bottom and table fills above
                let avail_w = ui.available_width();
                let button_h = ui.spacing().interact_size.y;
                ui.allocate_ui_with_layout(egui::vec2(avail_w, ui.available_height()), egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    // Bottom buttons, centered at 80% width
                    if let Some(i) = self.selected {
                        let button_w = (avail_w * 0.8).max(0.0);

                        // 20px padding at the very bottom under the last (bottom-most) button
                        ui.add_space(20.0);

                        ui.horizontal(|ui| {
                            let pad = (avail_w - button_w).max(0.0) / 2.0;
                            ui.add_space(pad);
                            let measurement_button_title: String = if self.measurement_identifier > 0 {
                                "Reset Measurement".into()
                            } else {
                                "Start Measurement".into()
                            };
                            if ui.add_sized([button_w, button_h], egui::Button::new(measurement_button_title)).clicked() {
                                if self.measurement_identifier == 0 {
                                    self.measurement_identifier = max(rand::random::<u32>(), 1);
                                    self.measurement_start_time = embassy_time::Instant::now();
                                    let _ = self.ui_command_tx.try_send(UICommand::StartMeasurement(p.node_id, self.measurement_identifier));
                                    self.reached_nodes.clear();
                                    self.reached_nodes.insert(p.node_id);
                                } else {
                                    self.reached_nodes.clear();
                                    self.measurement_identifier = 0;
                                    self.measurement_50_time = 0;
                                    self.measurement_90_time = 0;
                                    self.measurement_100_time = 0;
                                    self.measurement_total_time = 0;
                                    self.measurement_total_message_count = 0;
                                    self.measurement_50_message_count = 0;
                                    self.measurement_90_message_count = 0;
                                    self.measurement_100_message_count = 0;
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            let pad = (avail_w - button_w).max(0.0) / 2.0;
                            ui.add_space(pad);
                            if ui.add_sized([button_w, button_h], egui::Button::new("Send Message...")).clicked() {
                                debug!("Center on {i}");
                            }
                        });

                        // 5px spacing above the first (top-most) button, separating it from the table
                        ui.add_space(5.0);
                    }

                    // Table area above buttons: fill whatever is left
                    let table_h = ui.available_height().max(0.0);
                    if table_h > 0.0 {
                        ui.allocate_ui_with_layout(egui::vec2(avail_w, table_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                            if let Some(node_info) = &self.node_info {
                                if node_info.node_id == p.node_id {
                                    use egui_extras::{Column, TableBuilder};

                                    let row_height = ui.text_style_height(&egui::TextStyle::Body) * 1.3;
                                    // Ensure total table height (header + body) fits in the allocated space,
                                    // otherwise the body would push into the buttons area by ~header height.
                                    let header_h = row_height;
                                    let body_min_h = (table_h - header_h).max(0.0);
                                    TableBuilder::new(ui)
                                        .striped(true)
                                        .resizable(true)
                                        .vscroll(true)
                                        .min_scrolled_height(body_min_h)
                                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                                        .column(Column::initial(60.0).at_least(40.0)) // Timestamp
                                        .column(Column::initial(50.0).at_least(40.0)) // From
                                        .column(Column::remainder()) // Type
                                        .column(Column::initial(50.0).at_least(40.0)) // Packet
                                        .column(Column::initial(50.0).at_least(40.0)) // Size
                                        .column(Column::initial(50.0).at_least(40.0)) // Link Quality
                                        .header(row_height, |mut header| {
                                            header.col(|ui| {
                                                ui.strong("TS");
                                            });
                                            header.col(|ui| {
                                                ui.strong("From");
                                            });
                                            header.col(|ui| {
                                                ui.strong("Type");
                                            });
                                            header.col(|ui| {
                                                ui.strong("Packet");
                                            });
                                            header.col(|ui| {
                                                ui.strong("Size");
                                            });
                                            header.col(|ui| {
                                                ui.strong("LQ");
                                            });
                                        })
                                        .body(|body| {
                                            // Virtualized rows: only build visible rows; keep newest-first order
                                            let row_count = node_info.messages.len();
                                            body.rows(row_height, row_count, |mut row| {
                                                // Map visible row index to reversed (newest-first) index
                                                let row_index = row.index();
                                                let msg_idx = row_count - 1 - row_index;
                                                let msg = &node_info.messages[msg_idx];

                                                // Color rows red if from this node, else green
                                                let is_self = node_info.node_id == msg.sender_node;
                                                let mut row_color = if is_self { Color32::YELLOW } else { Color32::LIGHT_GREEN };
                                                let mut collision_fill: Option<Color32> = None;
                                                if msg.collision {
                                                    // Paint whole row red for collisions and use white text for contrast
                                                    collision_fill = Some(Color32::from_rgb(255, 0, 0));
                                                    row_color = Color32::WHITE;
                                                }
                                                let type_string = match msg.message_type {
                                                    1 => "Req echo",
                                                    2 => "Echo",
                                                    3 => "Echo result",
                                                    4 => "Req block",
                                                    5 => "Req blk prt",
                                                    6 => "Add block",
                                                    7 => "Add trans",
                                                    8 => "Req mempool",
                                                    9 => "Support",
                                                    _ => "Unknown",
                                                };
                                                let from_string = if is_self { "Sent msg".to_string() } else { format!("#{}", msg.sender_node) };
                                                let secs = msg.timestamp.duration_since(self.start_time).as_secs();
                                                let message_type_color = color_for_message_type(msg.message_type, 1.0);
                                                let link_quality_string = if is_self { "-".to_string() } else { format!("{}", msg.link_quality) };

                                                row.col(|ui| {
                                                    if let Some(fill) = collision_fill {
                                                        let rect = ui.available_rect_before_wrap();
                                                        ui.painter().rect_filled(rect, 0.0, fill);
                                                    }
                                                    ui.colored_label(row_color, format!("{} s", secs));
                                                });
                                                row.col(|ui| {
                                                    if let Some(fill) = collision_fill {
                                                        let rect = ui.available_rect_before_wrap();
                                                        ui.painter().rect_filled(rect, 0.0, fill);
                                                    }
                                                    ui.colored_label(row_color, from_string);
                                                });
                                                row.col(|ui| {
                                                    if let Some(fill) = collision_fill {
                                                        let rect = ui.available_rect_before_wrap();
                                                        ui.painter().rect_filled(rect, 0.0, fill);
                                                    }
                                                    ui.colored_label(message_type_color, type_string);
                                                });
                                                row.col(|ui| {
                                                    if let Some(fill) = collision_fill {
                                                        let rect = ui.available_rect_before_wrap();
                                                        ui.painter().rect_filled(rect, 0.0, fill);
                                                    }
                                                    ui.colored_label(row_color, format!("{}/{}", msg.packet_index + 1, msg.packet_count));
                                                });
                                                row.col(|ui| {
                                                    if let Some(fill) = collision_fill {
                                                        let rect = ui.available_rect_before_wrap();
                                                        ui.painter().rect_filled(rect, 0.0, fill);
                                                    }
                                                    ui.colored_label(row_color, format!("{} B", msg.packet_size));
                                                });
                                                row.col(|ui| {
                                                    if let Some(fill) = collision_fill {
                                                        let rect = ui.available_rect_before_wrap();
                                                        ui.painter().rect_filled(rect, 0.0, fill);
                                                    }

                                                    let link_quality_color;

                                                    if self.poor_limit > 0 && self.excellent_limit > 0 {
                                                        if msg.link_quality <= self.poor_limit {
                                                            link_quality_color = Color32::RED;
                                                        } else if msg.link_quality >= self.excellent_limit {
                                                            link_quality_color = Color32::GREEN;
                                                        } else {
                                                            link_quality_color = Color32::YELLOW;
                                                        }
                                                    } else {
                                                        link_quality_color = Color32::WHITE;
                                                    }

                                                    ui.colored_label(link_quality_color, link_quality_string);
                                                });
                                            });
                                        });
                                }
                            }
                        });
                    }
                });
            } else {
                // Center the info label both horizontally and vertically within the remaining panel space
                ui.centered_and_justified(|ui| {
                    ui.label("No node selected. Click on a node on the map to select it.");
                });
            }
        });

        // Map fills the remaining space
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Map");
            ui.separator();

            // Reserve a square drawing area using the smaller of available width/height, centered both horizontally and vertically
            let avail_rect = ui.available_rect_before_wrap();
            let side = avail_rect.width().min(avail_rect.height());
            let x = avail_rect.center().x - side / 2.0;
            let y = avail_rect.center().y - side / 2.0;
            let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(side, side));
            let response = ui.interact(rect, egui::Id::new("map_canvas"), egui::Sense::click());
            let painter = ui.painter_at(rect);

            // Draw background
            painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);

            // Draw grid: dark blue lines every 1000 world units (0..=10000)
            let grid_color = Color32::from_rgb(0, 0, 100);
            let grid_stroke = egui::Stroke::new(1.0, grid_color);
            for i in 0..=10 {
                let t = i as f32 / 10.0; // 0.0 ..= 1.0 in 0.1 steps
                // Vertical line at x = i * 1000
                let x = egui::lerp(rect.left()..=rect.right(), t);
                painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], grid_stroke);
                // Horizontal line at y = i * 1000
                let y = egui::lerp(rect.top()..=rect.bottom(), t);
                painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], grid_stroke);
            }

            // Draw obstacles before nodes so nodes appear on top
            let obstacle_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 255);
            let obstacle_stroke = egui::Stroke::new(1.5, Color32::from_rgb(255, 255, 255));
            for obs in &self.obstacles {
                match obs {
                    Obstacle::Rectangle { position, .. } => {
                        // Compute bounds from corners in world units
                        let l = position.top_left.x.min(position.bottom_right.x);
                        let r = position.top_left.x.max(position.bottom_right.x);
                        let t = position.top_left.y.min(position.bottom_right.y);
                        let b = position.top_left.y.max(position.bottom_right.y);

                        // Map world 0..10000 to rect coordinates
                        let left = egui::lerp(rect.left()..=rect.right(), l as f32 / 10000.0);
                        let right = egui::lerp(rect.left()..=rect.right(), r as f32 / 10000.0);
                        let top = egui::lerp(rect.top()..=rect.bottom(), t as f32 / 10000.0);
                        let bottom = egui::lerp(rect.top()..=rect.bottom(), b as f32 / 10000.0);
                        let rect_px = egui::Rect::from_min_max(egui::pos2(left.min(right), top.min(bottom)), egui::pos2(left.max(right), top.max(bottom)));
                        painter.rect_filled(rect_px, 0.0, obstacle_fill);
                        painter.rect_stroke(rect_px, 0.0, obstacle_stroke);
                    }
                    Obstacle::Circle { position, .. } => {
                        let cx = egui::lerp(rect.left()..=rect.right(), position.center.x as f32 / 10000.0);
                        let cy = egui::lerp(rect.top()..=rect.bottom(), position.center.y as f32 / 10000.0);
                        // Uniform scale for radius: take min scale to keep circle round in non-square rects
                        let scale_x = rect.width() / 10000.0;
                        let scale_y = rect.height() / 10000.0;
                        let units_to_pixels = scale_x.min(scale_y);
                        let r = position.radius as f32 * units_to_pixels;
                        let center_px = egui::pos2(cx, cy);
                        painter.circle_filled(center_px, r, obstacle_fill);
                        painter.circle_stroke(center_px, r, obstacle_stroke);
                    }
                }
            }

            // Draw nodes scaled into rect
            let radius = 4.0;
            for (i, p) in self.nodes.iter().enumerate() {
                let pos = egui::pos2(
                    egui::lerp(rect.left()..=rect.right(), p.position.x as f32 / 10000f32),
                    egui::lerp(rect.top()..=rect.bottom(), p.position.y as f32 / 10000f32),
                );

                let mut color = ui.visuals().widgets.inactive.fg_stroke.color;

                if self.measurement_identifier != 0 && self.reached_nodes.contains(&p.node_id) {
                    color = Color32::from_rgb(0, 255, 0); // Green if reached in current measurement
                }

                if self.selected == Some(i) {
                    let scale_x = rect.width() / 10000.0;
                    let scale_y = rect.height() / 10000.0;
                    let units_to_pixels = scale_x.min(scale_y);
                    let radius = p.radio_strength as f32 * units_to_pixels;
                    painter.circle_filled(pos, radius, Color32::from_rgba_unmultiplied(0, 128, 255, 50));
                    color = Color32::from_rgb(0, 128, 255);
                }

                painter.circle_filled(pos, radius, color);

                // Optional ID label next to each node
                if self.show_node_ids {
                    let label_pos = egui::pos2(pos.x + 6.0, pos.y - 6.0);
                    painter.text(
                        label_pos,
                        egui::Align2::LEFT_BOTTOM,
                        format!("#{}", p.node_id),
                        egui::FontId::monospace(12.0),
                        ui.visuals().text_color(),
                    );
                }

                // Draw radio transfer indicator
                if let Some((expiry, message_type, distance)) = self.node_radio_transfer_indicators.get(&p.node_id) {
                    let remaining = *expiry - Instant::now();
                    if remaining > Duration::from_millis(0) {
                        let alpha = (remaining.as_millis() as f32 / NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT as f32).clamp(0.0, 1.0);
                        // Convert world distance to pixels like we do for coordinates (range 0..10000)
                        let scale_x = rect.width() / 10000.0;
                        let scale_y = rect.height() / 10000.0;
                        let units_to_pixels = scale_x.min(scale_y);
                        let radius = (*distance as f32 * units_to_pixels) * (1.0 - alpha);
                        let color = color_for_message_type(*message_type, alpha);
                        painter.circle_stroke(pos, radius, egui::Stroke::new(1.0, color));
                    } else {
                        self.node_radio_transfer_indicators.remove(&p.node_id);
                    }
                }
            }

            // Handle selection by nearest node (squared-distance comparison)
            if response.clicked() {
                if let Some(click_pos) = response.interact_pointer_pos() {
                    let mut best: Option<(usize, f32)> = None;
                    for (i, p) in self.nodes.iter().enumerate() {
                        let pos = egui::pos2(
                            egui::lerp(rect.left()..=rect.right(), p.position.x as f32 / 10000f32),
                            egui::lerp(rect.top()..=rect.bottom(), p.position.y as f32 / 10000f32),
                        );
                        let d2 = pos.distance_sq(click_pos);
                        if best.map_or(true, |(_, bd)| d2 < bd) {
                            best = Some((i, d2));
                        }
                    }
                    let new_selected = best.map(|(i, _)| i);
                    if new_selected != self.selected {
                        self.selected = new_selected;
                        if let Some(new_selected) = new_selected {
                            let node_id = &self.nodes[new_selected].node_id;
                            self.ui_command_tx.try_send(UICommand::RequestNodeInfo(node_id.clone())).ok();
                        }
                    } else {
                        self.selected = None;
                    }
                }
            }
            // Draw obstacles
        });
    }
}

fn embassy_init(spawner: Spawner, ui_refresh_tx: UIRefreshChannelSender, ui_command_rx: UICommandChannelReceiver) {
    let _ = spawner.spawn(network::network_task(spawner, ui_refresh_tx, ui_command_rx));
}

fn main() {
    // Logging setup
    Builder::new()
        .filter_level(LevelFilter::Info)
        .filter(Some("moonblokz_radio_simulator"), LevelFilter::Debug)
        .filter(Some("moonblokz_radio_lib"), LevelFilter::Debug)
        .init();

    info!("Starting up");

    let ui_refresh_channel: &'static UIRefreshChannel = Box::leak(Box::new(UIRefreshChannel::new()));
    let ui_command_channel: &'static UICommandChannel = Box::leak(Box::new(UICommandChannel::new()));

    let ui_refresh_tx = ui_refresh_channel.sender();
    let ui_refresh_rx = ui_refresh_channel.receiver();
    let ui_command_tx = ui_command_channel.sender();
    let ui_command_rx = ui_command_channel.receiver();

    // Spawn Embassy executor on a dedicated background thread
    // Use very large stack size to handle very large number of simulated nodes
    let _embassy_handle = thread::Builder::new()
        .stack_size(192 * 1024 * 1024)
        .name("embassy-executor".to_string())
        .spawn(move || {
            // Leak the executor to satisfy the 'static lifetime required by run()
            let executor: &'static mut Executor = Box::leak(Box::new(Executor::new()));
            executor.run(|spawner| embassy_init(spawner, ui_refresh_tx, ui_command_rx));
        })
        .expect("failed to spawn embassy thread");

    // Start the GUI on the main thread (required on macOS)
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default(),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "MoonBlokz Radio Simulator",
        native_options,
        Box::new(move |cc| Box::new(AppState::new(ui_refresh_rx, ui_command_tx, cc.storage))),
    );
}
