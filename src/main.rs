use eframe::egui;
use egui::Color32;
use embassy_executor::{Executor, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use env_logger::Builder;
use log::{LevelFilter, debug, info};
use std::thread;
use std::time::Duration;

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

#[derive(Debug)]
enum UIRefreshState {
    Alert(String),
    NodeUpdated(NodeUIState),
    NodesUpdated(Vec<NodeUIState>),
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
}

impl AppState {
    fn new(rx: UIRefreshChannelReceiver, tx: UICommandChannelSender) -> Self {
        Self {
            alert: None,
            ui_refresh_rx: rx,
            ui_command_tx: tx,
            selected: None,
            nodes: Vec::new(),
        }
    }
}

impl eframe::App for AppState {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Repaint periodically so background updates are visible without input
        ctx.request_repaint_after(Duration::from_millis(100));

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

        // Top: system metrics (fixed 200 px height)
        egui::TopBottomPanel::top("top_metrics").exact_height(200.0).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("System Metrics");
                ui.separator();
                ui.label("FPS: ~60");
                ui.label(format!("Nodes: {}", self.nodes.len()));
                ui.label("Throughput: 0 pkt/s");
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
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

        // Bottom-right: inspector (fixed width)
        egui::SidePanel::right("inspector_right").exact_width(300.0).show(ctx, |ui| {
            ui.heading("Inspector");
            ui.separator();
            match self.selected {
                Some(i) => {
                    let p = &self.nodes[i];
                    ui.label(format!("Selected point: #{}", p.node_id));
                    ui.label(format!("Normalized position: ({:.2}, {:.2})", p.position.x, p.position.y));
                    ui.add_space(8.0);
                    ui.label("Actions");
                    if ui.button("Center camera on point").clicked() {
                        debug!("Center on {i}");
                    }
                    if ui.button("Delete point").clicked() {
                        debug!("Delete {i}");
                    }
                }
                None => {
                    ui.label("No point selected.");
                }
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
