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
use embassy_executor::{Executor, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use env_logger::Builder;
use log::{LevelFilter, info};
use std::thread;

mod simulation;
mod time_driver;
mod ui;

pub const UI_REFRESH_CHANNEL_SIZE: usize = 500;
pub type UIRefreshChannel = embassy_sync::channel::Channel<CriticalSectionRawMutex, ui::UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;
pub type UIRefreshChannelReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, ui::UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;
pub type UIRefreshChannelSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, ui::UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;

pub const UI_COMMAND_CHANNEL_SIZE: usize = 100;
pub type UICommandChannel = embassy_sync::channel::Channel<CriticalSectionRawMutex, ui::UICommand, UI_COMMAND_CHANNEL_SIZE>;
pub type UICommandChannelReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, ui::UICommand, UI_COMMAND_CHANNEL_SIZE>;
pub type UICommandChannelSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, ui::UICommand, UI_COMMAND_CHANNEL_SIZE>;

fn embassy_init(spawner: Spawner, ui_refresh_tx: UIRefreshChannelSender, ui_command_rx: UICommandChannelReceiver) {
    let _ = spawner.spawn(simulation::network_task(spawner, ui_refresh_tx, ui_command_rx));
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
        Box::new(move |cc| Box::new(ui::AppState::new(ui_refresh_rx, ui_command_tx, cc.storage))),
    );
}
