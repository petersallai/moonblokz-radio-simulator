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
enum UIRefreshState {
    Alert(String),
    NodeUpdated(NodeUIState),
    NodesUpdated(Vec<NodeUIState>),
    NodeSentRadioMessage(u32, u32), // Node ID of the node that sent a message
}
#[derive(Debug)]
struct NodeUIState {
    node_id: u32,
    position: Point,
    radio_strength: u32,
}

enum UICommand {
    LoadFile(String),
}

struct AppState {
    alert: Option<String>,
    ui_refresh_rx: UIRefreshChannelReceiver,
    ui_command_tx: UICommandChannelSender,
    // Map state
    selected: Option<usize>,
    nodes: Vec<NodeUIState>,
    node_radio_transfer_indicators: HashMap<u32, (Instant, u32)>,
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
        }
    }
}

impl eframe::App for AppState {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Repaint periodically so background updates are visible without input
        ctx.request_repaint_after(Duration::from_millis(50));

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
                UIRefreshState::NodeSentRadioMessage(node_id, distance) => {
                    self.node_radio_transfer_indicators
                        .insert(node_id, (Instant::now() + NODE_RADIO_TRANSFER_INDICATOR_DURATION, distance));
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

        // Bottom-right: inspector (fixed 200 px) with top-left info and bottom-aligned centered buttons
        egui::SidePanel::right("inspector_right").exact_width(200.0).show(ctx, |ui| {
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
                    ui.label(egui::RichText::new(format!("{}", 3400)).strong());
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Sent messages:");
                    ui.label(egui::RichText::new(format!("{}", 45)).strong());
                });
                ui.horizontal(|ui| {
                    ui.label("Received messages:");
                    ui.label(egui::RichText::new(format!("{}", 30)).strong());
                });
            } else {
                ui.label("No point selected.");
            }

            // Push buttons to the bottom using a spacer, then center them at 80% width
            let avail_h = ui.available_height();
            let avail_w = ui.available_width();
            let button_h = ui.spacing().interact_size.y;
            let spacing_y = ui.spacing().item_spacing.y;
            // Three button rows: height = 3 * button_h + 2 * spacing between rows
            let total_buttons_h = if self.selected.is_some() { button_h * 3.0 + spacing_y * 2.0 } else { 0.0 };
            let bottom_padding = 20.0; // extra space under the bottom button
            let spacer_h = (avail_h - total_buttons_h - if self.selected.is_some() { bottom_padding } else { 0.0 }).max(0.0);
            ui.add_space(spacer_h);
            if let Some(i) = self.selected {
                let button_w = (avail_w * 0.8).max(0.0);
                // Top-most of the three (added last): Start Measurement
                ui.horizontal(|ui| {
                    let pad = (avail_w - button_w).max(0.0) / 2.0;
                    ui.add_space(pad);
                    if ui.add_sized([button_w, button_h], egui::Button::new("Send Echo Request")).clicked() {
                        debug!("Start measurement for {i}");
                    }
                });
                // Bottom button row: centered manually with left padding
                ui.horizontal(|ui| {
                    let pad = (avail_w - button_w).max(0.0) / 2.0;
                    ui.add_space(pad);
                    if ui.add_sized([button_w, button_h], egui::Button::new("Start Measurement")).clicked() {
                        debug!("Delete {i}");
                    }
                });
                // Above bottom row: centered camera button
                ui.horizontal(|ui| {
                    let pad = (avail_w - button_w).max(0.0) / 2.0;
                    ui.add_space(pad);
                    if ui.add_sized([button_w, button_h], egui::Button::new("Send Message...")).clicked() {
                        debug!("Center on {i}");
                    }
                });
                // Bottom padding under buttons
                ui.add_space(bottom_padding);
            }
        });

        // Map fills the remaining space
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Map");
            ui.label("Click to select a point");
            ui.separator();

            // Reserve drawing area to fill all remaining space
            let available = ui.available_size();
            let size = egui::vec2(available.x, available.y);
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            let painter = ui.painter_at(rect);

            // Draw background
            painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);

            // Draw points scaled into rect
            let radius = 4.0;
            for (i, p) in self.nodes.iter().enumerate() {
                let pos = egui::pos2(
                    egui::lerp(rect.left()..=rect.right(), p.position.x as f32 / 10000f32),
                    egui::lerp(rect.top()..=rect.bottom(), p.position.y as f32 / 10000f32),
                );
                let color = if Some(i) == self.selected {
                    Color32::from_rgb(0, 128, 255)
                } else {
                    ui.visuals().widgets.inactive.fg_stroke.color
                };
                painter.circle_filled(pos, radius, color);

                // Draw radio transfer indicator
                if let Some((expiry, distance)) = self.node_radio_transfer_indicators.get(&p.node_id) {
                    let remaining = *expiry - Instant::now();
                    if remaining > Duration::from_millis(0) {
                        let alpha = (remaining.as_millis() as f32 / NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT as f32).clamp(0.0, 1.0);
                        // Convert world distance to pixels like we do for coordinates (range 0..10000)
                        let scale_x = rect.width() / 10000.0;
                        let scale_y = rect.height() / 10000.0;
                        let units_to_pixels = scale_x.min(scale_y);
                        let radius = (*distance as f32 * units_to_pixels) * (1.0 - alpha);
                        painter.circle_stroke(pos, radius, egui::Stroke::new(1.0, Color32::from_white_alpha((alpha * 255.0) as u8)));
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
                    self.selected = best.map(|(i, _)| i);
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
