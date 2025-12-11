//! Main analyzer async task for log parsing and visualization.
//!
//! Runs on the Embassy executor and coordinates:
//! - Scene loading and UI initialization
//! - Log file reading with mode-appropriate behavior
//! - Time-synchronized event dispatching
//! - UI command handling

use chrono::{DateTime, Utc};
use embassy_time::{Duration, Timer};
use std::time::Instant;

use crate::common::scene::{Scene, SceneMode, load_scene};
use crate::ui::{NodeUIState, UICommand, UIRefreshState};
use crate::{UICommandQueueReceiver, UIRefreshQueueSender};

use super::log_loader::LogLoader;
use super::log_parser::parse_log_line;
use super::types::{AnalyzerMode, AnalyzerState, LogEvent, NodePacketRecord};

/// Main analyzer task that runs on the Embassy executor.
///
/// # Parameters
///
/// * `mode` - Real-time tracking or log visualization
/// * `scene_path` - Path to the scene JSON file
/// * `log_path` - Path to the log file
/// * `ui_refresh_tx` - Channel for sending UI updates
/// * `ui_command_rx` - Channel for receiving UI commands
#[embassy_executor::task]
pub async fn analyzer_task(
    mode: AnalyzerMode,
    scene_path: String,
    log_path: String,
    ui_refresh_tx: UIRefreshQueueSender,
    ui_command_rx: UICommandQueueReceiver,
) {
    log::info!("Analyzer task started in {:?} mode", mode);
    log::info!("Scene: {}, Log: {}", scene_path, log_path);

    // Load and validate scene
    let scene = match load_scene(&scene_path, SceneMode::Analyzer) {
        Ok(s) => s,
        Err(e) => {
            let _ = ui_refresh_tx.send(UIRefreshState::Alert(format!("Failed to load scene: {}", e))).await;
            return;
        }
    };

    // Initialize UI with scene data
    initialize_scene_ui(&scene, &ui_refresh_tx).await;

    // Notify UI of the operating mode
    let operating_mode = match mode {
        AnalyzerMode::RealtimeTracking => crate::ui::OperatingMode::RealtimeTracking,
        AnalyzerMode::LogVisualization => crate::ui::OperatingMode::LogVisualization,
    };
    let _ = ui_refresh_tx.send(UIRefreshState::ModeChanged(operating_mode)).await;

    // Open log file
    let mut log_loader = match LogLoader::new(&log_path, mode) {
        Ok(l) => l,
        Err(e) => {
            let _ = ui_refresh_tx.send(UIRefreshState::Alert(format!("Failed to open log file: {}", e))).await;
            return;
        }
    };

    // Initialize analyzer state
    let mut state = AnalyzerState::new();
    let mut total_sent = 0u64;
    let mut total_received = 0u64;

    // Main processing loop
    loop {
        // Check for UI commands (non-blocking)
        while let Ok(cmd) = ui_command_rx.try_receive() {
            match cmd {
                UICommand::RequestNodeInfo(_node_id) => {
                    // TODO: Build NodeInfo from packet history and send
                }
                UICommand::SeekAnalyzer(time) => {
                    // TODO: Implement seeking for log visualization
                    log::debug!("Seek request to {} (not yet implemented)", time);
                }
                _ => {
                    // Ignore other commands in analyzer mode
                }
            }
        }

        // Read next log line
        let line = match log_loader.next_line().await {
            Some(l) => l,
            None => {
                // EOF in log visualization mode
                let _ = ui_refresh_tx.send(UIRefreshState::VisualizationEnded).await;
                log::info!("Log visualization ended (EOF reached)");
                
                // Keep task alive to handle UI commands
                loop {
                    if let Ok(cmd) = ui_command_rx.try_receive() {
                        match cmd {
                            UICommand::SeekAnalyzer(_) => {
                                // TODO: Could reset and replay
                            }
                            _ => {}
                        }
                    }
                    Timer::after(Duration::from_millis(100)).await;
                }
            }
        };

        // Parse the log line
        let (timestamp, event) = match parse_log_line(&line) {
            Some(parsed) => parsed,
            None => {
                log::debug!("Unparseable log line: {}", line);
                continue;
            }
        };

        // Time synchronization
        synchronize_time(&mut state, timestamp, mode).await;

        // Process the event
        process_event(&event, timestamp, &mut state, &ui_refresh_tx, &mut total_sent, &mut total_received).await;
    }
}

/// Initialize UI with scene data (nodes, obstacles, dimensions).
async fn initialize_scene_ui(scene: &Scene, ui_refresh_tx: &UIRefreshQueueSender) {
    // Publish nodes with effective distances
    let node_states: Vec<NodeUIState> = scene
        .nodes
        .iter()
        .map(|n| NodeUIState {
            node_id: n.node_id,
            position: (&n.position).into(),
            radio_strength: n.effective_distance.unwrap_or(100),
        })
        .collect();

    let _ = ui_refresh_tx.send(UIRefreshState::NodesUpdated(node_states)).await;

    // Publish obstacles (using From trait for conversion)
    let obstacles: Vec<crate::simulation::Obstacle> = scene
        .obstacles
        .iter()
        .map(|o| o.into())
        .collect();

    let _ = ui_refresh_tx.send(UIRefreshState::ObstaclesUpdated(obstacles)).await;

    // Publish scene dimensions
    let _ = ui_refresh_tx
        .send(UIRefreshState::SceneDimensionsUpdated(
            (&scene.world_top_left).into(),
            (&scene.world_bottom_right).into(),
            scene.width,
            scene.height,
        ))
        .await;

    // Background image if present
    if let Some(ref bg_image) = scene.background_image {
        log::info!("Background image specified: {:?}", bg_image);
        let _ = ui_refresh_tx.send(UIRefreshState::BackgroundImageUpdated(Some(bg_image.clone()))).await;
    }
}

/// Synchronize timing between log timestamps and real time.
async fn synchronize_time(state: &mut AnalyzerState, timestamp: DateTime<Utc>, mode: AnalyzerMode) {
    match (state.reference_timestamp, state.reference_instant) {
        (None, _) | (_, None) => {
            // First timestamp - set as reference
            state.reference_timestamp = Some(timestamp);
            state.reference_instant = Some(Instant::now());
        }
        (Some(ref_ts), Some(ref_instant)) => {
            let elapsed = ref_instant.elapsed();
            let log_offset = timestamp.signed_duration_since(ref_ts);
            let log_offset_duration = log_offset.to_std().unwrap_or(std::time::Duration::ZERO);

            if log_offset_duration > elapsed {
                // Log is ahead of real time - wait
                let wait_duration = log_offset_duration - elapsed;
                
                // Cap wait time to prevent very long waits
                let max_wait = std::time::Duration::from_secs(10);
                let actual_wait = wait_duration.min(max_wait);
                
                if mode == AnalyzerMode::LogVisualization {
                    Timer::after(Duration::from_micros(actual_wait.as_micros() as u64)).await;
                }
                // In real-time mode, don't wait - process immediately
            } else if elapsed > log_offset_duration + std::time::Duration::from_secs(3600) {
                // Log is more than 1 hour behind - reset reference
                log::warn!("Log timestamp significantly behind, resetting reference");
                state.reference_timestamp = Some(timestamp);
                state.reference_instant = Some(Instant::now());
            }
            // Otherwise, log is behind real time - process immediately
        }
    }

    state.last_processed_timestamp = Some(timestamp);
}

/// Process a parsed log event and update UI.
async fn process_event(
    event: &LogEvent,
    timestamp: DateTime<Utc>,
    state: &mut AnalyzerState,
    ui_refresh_tx: &UIRefreshQueueSender,
    total_sent: &mut u64,
    total_received: &mut u64,
) {
    match event {
        LogEvent::SendPacket { node_id, message_type, .. } => {
            *total_sent += 1;

            // Store in history
            state.add_packet_record(*node_id, NodePacketRecord {
                timestamp,
                event: event.clone(),
            });

            // Notify UI of transmission (use a default distance of 100 for now)
            let _ = ui_refresh_tx
                .try_send(UIRefreshState::NodeSentRadioMessage(*node_id, *message_type, 100))
                .ok();

            // Update counters
            let _ = ui_refresh_tx
                .try_send(UIRefreshState::RadioMessagesCountUpdated(*total_sent, *total_received, 0))
                .ok();
        }
        LogEvent::ReceivePacket { node_id, .. } => {
            *total_received += 1;

            // Store in history
            state.add_packet_record(*node_id, NodePacketRecord {
                timestamp,
                event: event.clone(),
            });

            // Update counters
            let _ = ui_refresh_tx
                .try_send(UIRefreshState::RadioMessagesCountUpdated(*total_sent, *total_received, 0))
                .ok();
        }
        LogEvent::StartMeasurement { node_id, sequence } => {
            state.active_measurement_id = Some(*sequence);
            log::info!("Measurement started by node {} with sequence {}", node_id, sequence);
        }
        LogEvent::ReceivedFullMessage { node_id, message_type, sequence, .. } => {
            // If this is an AddBlock message (type 6), it might be part of a measurement
            if *message_type == 6 {
                if let Some(active_id) = state.active_measurement_id {
                    if active_id == *sequence {
                        let _ = ui_refresh_tx
                            .try_send(UIRefreshState::NodeReachedInMeasurement(*node_id, *sequence))
                            .ok();
                    }
                }
            }
        }
        LogEvent::Position { x, y } => {
            log::debug!("Position event: ({}, {})", x, y);
            // Position updates could be handled here if needed
        }
    }

    // In real-time mode, calculate and send delay
    if state.reference_instant.is_some() && state.reference_timestamp.is_some() {
        if let (Some(ref_instant), Some(ref_ts)) = (state.reference_instant, state.reference_timestamp) {
            let elapsed = ref_instant.elapsed();
            let log_offset = timestamp.signed_duration_since(ref_ts);
            
            if let Ok(log_offset_duration) = log_offset.to_std() {
                if elapsed > log_offset_duration {
                    let delay_ms = (elapsed - log_offset_duration).as_millis() as u64;
                    let _ = ui_refresh_tx.try_send(UIRefreshState::AnalyzerDelay(delay_ms)).ok();
                }
            }
        }
    }
}
