use eframe::egui;
use egui::Color32;
use embassy_executor::{Executor, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use env_logger::Builder;
use log::{LevelFilter, debug, info};
use std::collections::HashMap;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use crate::network::NodeMessage;
use crate::network::Point;

mod network;

const UI_REFRESH_CHANNEL_SIZE: usize = 10;
type UIRefreshChannel = embassy_sync::channel::Channel<CriticalSectionRawMutex, UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;
type UIRefreshChannelReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;
type UIRefreshChannelSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;

const UI_COMMAND_CHANNEL_SIZE: usize = 10;
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
    NodeUpdated(NodeUIState),
    NodesUpdated(Vec<NodeUIState>),
    NodeSentRadioMessage(u32, u8, u32), // node ID, message type, and effective distance
    NodeInfo(NodeInfo),
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
}

struct AppState {
    alert: Option<String>,
    ui_refresh_rx: UIRefreshChannelReceiver,
    ui_command_tx: UICommandChannelSender,
    // Map state
    selected: Option<usize>,
    nodes: Vec<NodeUIState>,
    node_radio_transfer_indicators: HashMap<u32, (Instant, u8, u32)>,
    node_info: Option<NodeInfo>,
    start_time: Instant,
    last_node_info_update: Instant,
}

impl AppState {
    fn new(rx: UIRefreshChannelReceiver, tx: UICommandChannelSender) -> Self {
        Self {
            alert: None,
            ui_refresh_rx: rx,
            ui_command_tx: tx,
            selected: None,
            nodes: Vec::new(),
            node_radio_transfer_indicators: HashMap::new(),
            node_info: None,
            start_time: Instant::now(),
            last_node_info_update: Instant::now(),
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Repaint periodically so background updates are visible without input
        ctx.request_repaint_after(Duration::from_millis(50));

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
                UIRefreshState::NodeSentRadioMessage(node_id, message_type, distance) => {
                    self.node_radio_transfer_indicators
                        .insert(node_id, (Instant::now() + NODE_RADIO_TRANSFER_INDICATOR_DURATION, message_type, distance));
                }
                UIRefreshState::NodeInfo(node_info) => {
                    self.node_info = Some(node_info);
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
            ui.columns(3, |cols| {
                // Column 1: Title + core metrics
                cols[0].vertical(|ui| {
                    ui.heading("System Metrics");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Simulation time:");
                        ui.label(egui::RichText::new(Instant::now().duration_since(self.start_time).as_secs().to_string()).strong());
                    });

                    ui.horizontal(|ui| {
                        ui.label("Nodes:");
                        ui.label(egui::RichText::new(self.nodes.len().to_string()).strong());
                    });
                    ui.horizontal(|ui| {
                        ui.label("Throughput:");
                        ui.label(egui::RichText::new(format!("{}", 100)).strong());
                        ui.label(" total packets/minutes");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Collision rate:");
                        ui.label(egui::RichText::new(format!("{}", 3)).strong());
                        ui.label("%");
                    });
                });

                // Column 2: Measured distribution
                cols[1].vertical(|ui| {
                    ui.heading("Measured distribution");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Measurement time: ");
                        ui.label(egui::RichText::new(format!("{}", 23)).strong());
                        ui.label(" seconds");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Total nodes accessed: ");
                        ui.label(egui::RichText::new(format!("{}", 75)).strong());
                    });
                    ui.horizontal(|ui| {
                        ui.label("Distribution percentage: ");
                        ui.label(egui::RichText::new(format!("{}", 75)).strong());
                        ui.label("%");
                    });
                });

                // Column 3: Controls
                cols[2].vertical(|ui| {
                    ui.heading("Controls");
                    ui.separator();
                    if ui.button("Start").clicked() {
                        debug!("Start clicked");
                    }
                    if ui.button("Stop").clicked() {
                        debug!("Stop clicked");
                    }
                    if ui.button("Reset").clicked() {
                        debug!("Reset clicked");
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
                    ui.heading(format!("Message stream for #{}", node_info.node_id));
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
                            if ui.add_sized([button_w, button_h], egui::Button::new("Send Echo Request")).clicked() {
                                debug!("Start measurement for {i}");
                            }
                        });
                        ui.horizontal(|ui| {
                            let pad = (avail_w - button_w).max(0.0) / 2.0;
                            ui.add_space(pad);
                            if ui.add_sized([button_w, button_h], egui::Button::new("Start Measurement")).clicked() {
                                debug!("Delete {i}");
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
                                        .column(Column::initial(90.0).at_least(70.0)) // Timestamp
                                        .column(Column::initial(70.0).at_least(60.0)) // From
                                        .column(Column::remainder()) // Type
                                        .column(Column::initial(80.0).at_least(60.0)) // Packet
                                        .column(Column::initial(70.0).at_least(60.0)) // Size
                                        .header(row_height, |mut header| {
                                            header.col(|ui| {
                                                ui.strong("Timestamp");
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
                                                let row_color = if is_self { Color32::RED } else { Color32::LIGHT_GREEN };
                                                let type_string = match msg.message_type {
                                                    1 => "Request echo",
                                                    2 => "Echo",
                                                    3 => "Echo result",
                                                    4 => "Request block",
                                                    5 => "Request block part",
                                                    6 => "Add block",
                                                    7 => "Add transaction",
                                                    8 => "Mempool request",
                                                    9 => "Support",
                                                    _ => "Unknown",
                                                };
                                                let from_string = if is_self { "-".to_string() } else { format!("#{}", msg.sender_node) };
                                                let secs = msg.timestamp.duration_since(self.start_time).as_secs();
                                                let message_type_color = color_for_message_type(msg.message_type, 1.0);

                                                row.col(|ui| {
                                                    ui.colored_label(row_color, format!("{} s", secs));
                                                });
                                                row.col(|ui| {
                                                    ui.colored_label(row_color, from_string);
                                                });
                                                row.col(|ui| {
                                                    ui.colored_label(message_type_color, type_string);
                                                });
                                                row.col(|ui| {
                                                    ui.colored_label(row_color, format!("{}/{}", msg.packet_index + 1, msg.packet_count));
                                                });
                                                row.col(|ui| {
                                                    ui.colored_label(row_color, format!("{} B", msg.packet_size));
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

            // Draw points scaled into rect
            let radius = 4.0;
            for (i, p) in self.nodes.iter().enumerate() {
                let pos = egui::pos2(
                    egui::lerp(rect.left()..=rect.right(), p.position.x as f32 / 10000f32),
                    egui::lerp(rect.top()..=rect.bottom(), p.position.y as f32 / 10000f32),
                );
                let color = if Some(i) == self.selected {
                    let scale_x = rect.width() / 10000.0;
                    let scale_y = rect.height() / 10000.0;
                    let units_to_pixels = scale_x.min(scale_y);
                    let radius = p.radio_strength as f32 * units_to_pixels;
                    painter.circle_filled(pos, radius, Color32::from_rgba_unmultiplied(0, 128, 255, 50));
                    Color32::from_rgb(0, 128, 255)
                } else {
                    ui.visuals().widgets.inactive.fg_stroke.color
                };

                painter.circle_filled(pos, radius, color);

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

            // Handle selection
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
    let _embassy_handle = thread::Builder::new()
        .stack_size(192 * 1024 * 1024)
        .name("embassy-executor".to_string())
        .spawn(move || {
            // Leak the executor to satisfy the 'static lifetime required by run()
            let executor: &'static mut Executor = Box::leak(Box::new(Executor::new()));
            executor.run(|spawner| embassy_init(spawner, ui_refresh_tx, ui_command_rx));
        })
        .expect("failed to spawn embassy thread");

    let _ = ui_command_tx.try_send(UICommand::LoadFile("test_simulation.json".to_string()));
    // Start the GUI on the main thread (required on macOS)
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default(),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "MoonBlokz Radio Simulator",
        native_options,
        Box::new(move |_cc| Box::new(AppState::new(ui_refresh_rx, ui_command_tx))),
    );
}
