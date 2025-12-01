//! # MoonBlokz Radio Simulator - Main Entry Point
//!
//! This is the main entry point for the MoonBlokz Radio Simulator, a desktop application
//! that simulates radio network behavior for testing the MoonBlokz mesh protocol.
//!
//! ## Purpose
//!
//! The simulator validates multi-node mesh networking behavior without requiring physical
//! hardware deployment. It runs hundreds of simulated nodes in a single process, each
//! executing the same embedded codebase used in real radio modules.
//!
//! ## Architecture Overview
//!
//! The application has two main components running on separate threads:
//!
//! 1. **UI Thread (main)**: Runs the egui/eframe GUI on the main thread (required for macOS).
//!    Displays:
//!    - Top panel: System metrics and simulation controls (speed, auto-speed)
//!    - Right panel: Node inspector showing detailed message streams
//!    - Central map: Visual representation of nodes, obstacles, and radio transmissions
//!
//! 2. **Embassy Executor Thread**: Runs the async simulation tasks using Embassy runtime.
//!    Manages all simulated nodes and the central network coordination task.
//!
//! ## Communication Channels
//!
//! Two bounded channels coordinate between the UI and simulation:
//! - `UIRefreshChannel`: Network → UI updates (node states, metrics, events)
//! - `UICommandChannel`: UI → Network commands (load scene, select node, start measurement)
//!
//! ## Design Rationale
//!
//! This lightweight multi-node simulation architecture avoids the overhead of VM-based
//! emulation while preserving the exact async task model, timing logic, and radio behavior
//! from the embedded codebase. It enables rapid iteration, large-scale testing, and
//! controlled experiments with arbitrary topologies without deploying physical hardware.

use eframe::egui;
use embassy_executor::{Executor, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use env_logger::Builder;
use log::{LevelFilter, error, info};
use std::thread;

mod simulation;
mod time_driver;
mod ui;

/// Capacity of the UI refresh channel (network → UI).
/// Large enough to handle bursts of node updates without blocking the simulation.
pub const UI_REFRESH_CHANNEL_SIZE: usize = 500;
/// Bounded channel for sending UI state updates from the network task to the UI.
pub type UIRefreshQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, ui::UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;
/// Receiver side of the UI refresh channel.
pub type UIRefreshQueueReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, ui::UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;
/// Sender side of the UI refresh channel.
pub type UIRefreshQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, ui::UIRefreshState, UI_REFRESH_CHANNEL_SIZE>;

/// Capacity of the UI command channel (UI → network).
/// Smaller than refresh channel as user commands are infrequent.
pub const UI_COMMAND_CHANNEL_SIZE: usize = 100;
/// Bounded channel for sending commands from the UI to the network task.
pub type UICommandQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, ui::UICommand, UI_COMMAND_CHANNEL_SIZE>;
/// Receiver side of the UI command channel.
pub type UICommandQueueReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, ui::UICommand, UI_COMMAND_CHANNEL_SIZE>;
/// Sender side of the UI command channel.
pub type UICommandQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, ui::UICommand, UI_COMMAND_CHANNEL_SIZE>;

/// Initialize the Embassy executor and spawn the main network task.
///
/// This function is called once the Embassy executor is running on its dedicated thread.
/// It spawns the central `network_task` which coordinates all simulated nodes.
///
/// # Parameters
///
/// * `spawner` - Embassy spawner for creating async tasks
/// * `ui_refresh_tx` - Channel sender for pushing UI updates
/// * `ui_command_rx` - Channel receiver for receiving UI commands
fn embassy_init(spawner: Spawner, ui_refresh_tx: UIRefreshQueueSender, ui_command_rx: UICommandQueueReceiver) {
    let _ = spawner.spawn(simulation::network_task(spawner, ui_refresh_tx, ui_command_rx));
}

/// Embedded PNG icon data for the application window.
const APP_ICON_BYTES: &[u8] = include_bytes!("../icons/moonblokz_icon.png");

/// Load and decode the application icon from embedded PNG data.
///
/// # Returns
///
/// `Some(IconData)` if the icon loads successfully, `None` if decoding fails.
/// Failure is logged but non-fatal (the app will run without a custom icon).
fn load_app_icon() -> Option<egui::IconData> {
    match image::load_from_memory(APP_ICON_BYTES) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = (rgba.width(), rgba.height());
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            })
        }
        Err(err) => {
            error!("Failed to decode embedded app icon: {err}");
            None
        }
    }
}

fn main() {
    // Initialize logging subsystem with appropriate filter levels
    Builder::new()
        .filter_level(LevelFilter::Info)
        .filter(Some("moonblokz_radio_simulator"), LevelFilter::Debug)
        .filter(Some("moonblokz_radio_lib"), LevelFilter::Info)
        .init();

    info!("Starting up");

    // Create communication channels using Box::leak to satisfy 'static lifetime requirements.
    // These channels coordinate between the UI thread and the Embassy executor thread.
    // The leak is intentional and safe here: these channels live for the entire program lifetime
    // and are automatically cleaned up when the process terminates. This solution is required to
    // satisfy the 'static lifetime constraints of the Embassy executor and UI tasks.
    let ui_refresh_channel: &'static UIRefreshQueue = Box::leak(Box::new(UIRefreshQueue::new()));
    let ui_command_channel: &'static UICommandQueue = Box::leak(Box::new(UICommandQueue::new()));

    let ui_refresh_tx = ui_refresh_channel.sender();
    let ui_refresh_rx = ui_refresh_channel.receiver();
    let ui_command_tx = ui_command_channel.sender();
    let ui_command_rx = ui_command_channel.receiver();

    // Spawn Embassy executor on a dedicated background thread.
    // Large stack size (192 MB) is needed to accommodate the state of hundreds or thousands
    // of simulated nodes, each with their own async tasks and queues.
    let _embassy_handle = thread::Builder::new()
        .stack_size(192 * 1024 * 1024)
        .name("embassy-executor".to_string())
        .spawn(move || {
            // INTENTIONAL LEAK: Box::leak provides 'static lifetime for Embassy executor.
            // This allows the embedded moonblokz-radio-lib code to run unmodified in the simulator.
            // The executor lives for the entire program lifetime and is cleaned up on process exit.
            let executor: &'static mut Executor = Box::leak(Box::new(Executor::new()));
            executor.run(|spawner| embassy_init(spawner, ui_refresh_tx, ui_command_rx));
        })
        .expect("failed to spawn embassy thread");

    // Start the GUI on the main thread (required on macOS for AppKit integration).
    // Configure the window with minimum size and custom icon.
    let mut viewport = egui::ViewportBuilder::default().with_min_inner_size([1000.0, 800.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    // Run the eframe event loop with our AppState managing UI updates
    let _ = eframe::run_native(
        "MoonBlokz Radio Simulator",
        native_options,
        Box::new(move |cc| Box::new(ui::AppState::new(ui_refresh_rx, ui_command_tx, cc.storage))),
    );
}
