//! Main analyzer async task for log parsing and visualization.
//!
//! Runs on the Embassy executor and coordinates:
//! - Scene loading and UI initialization
//! - Log file reading with mode-appropriate behavior
//! - Time-synchronized event dispatching with adaptive delay recovery
//! - UI command handling
//!
//! The main loop uses a two-phase select approach:
//! 1. Wait for log line OR UI command
//! 2. If log line needs delay, wait for remaining time OR UI command
//!
//! This provides stable delay visualization while allowing the system to
//! gradually catch up when the average network latency is better than spikes.

use chrono::{DateTime, Utc};
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use crate::common::scene::{Scene, SceneMode, load_scene};
use crate::simulation::types::{FullMessage, LogLine, NodeMessage};
use crate::ui::{NodeInfo, NodeUIState, UICommand, UIRefreshState};
use crate::{UICommandQueueReceiver, UIRefreshQueueSender};

use super::log_loader::LogLoader;
use super::log_parser::{parse_log_line, parse_raw_log_line};
use super::types::{AnalyzerMode, AnalyzerState, LogEvent, NodePacketRecord};

/// Size of the sliding window for calculating average delay.
const DELAY_HISTORY_SIZE: usize = 100;

/// Tracks the delay between log timestamp and processing time for adaptive catch-up.
struct DelayTracker {
    /// Recent delay samples (real_processing_time - log_timestamp) in milliseconds
    delay_samples: VecDeque<i64>,
}

impl DelayTracker {
    fn new() -> Self {
        Self {
            delay_samples: VecDeque::with_capacity(DELAY_HISTORY_SIZE),
        }
    }

    /// Add a delay sample and return the current average.
    fn add_sample(&mut self, delay_ms: i64) -> i64 {
        if self.delay_samples.len() >= DELAY_HISTORY_SIZE {
            self.delay_samples.pop_front();
        }
        self.delay_samples.push_back(delay_ms);
        self.average()
    }

    /// Calculate the average delay over the last N samples.
    fn average(&self) -> i64 {
        if self.delay_samples.is_empty() {
            return 0;
        }
        let sum: i64 = self.delay_samples.iter().sum();
        sum / self.delay_samples.len() as i64
    }
}

/// Main analyzer task that runs on the Embassy executor.
///
/// Uses a two-phase select approach:
/// 1. Wait for log line OR UI command
/// 2. If log line needs time sync delay, wait for remaining time OR UI command
///
/// The delay tracking system maintains stable visualization while allowing
/// gradual catch-up when network performance is better than observed spikes.
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

    // Build node effective distances map for radio message visualization
    let node_effective_distances: HashMap<u32, u32> = scene.nodes.iter().map(|n| (n.node_id, n.effective_distance.unwrap_or(100))).collect();

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
    let mut delay_tracker = DelayTracker::new();

    // Timing state
    let mut last_log_timestamp: Option<DateTime<Utc>> = None;
    let mut last_process_time: Option<Instant> = None;

    // Main processing loop
    loop {
        // Phase 1: Wait for log line OR UI command
        match select(log_loader.next_line(), ui_command_rx.receive()).await {
            Either::First(line_result) => {
                match line_result {
                    Some(line) => {
                        // First, try to capture the raw log line for Log Stream tab
                        // This captures ALL log lines with a [node_id] pattern
                        if let Some((node_id, raw_log)) = parse_raw_log_line(&line) {
                            state.add_log_line(node_id, raw_log);
                        }

                        // Then, parse for structured events (Radio Stream tab)
                        if let Some((timestamp, event)) = parse_log_line(&line) {
                            // Check if this is the first log line
                            if last_log_timestamp.is_none() {
                                // First log line - process immediately and establish reference
                                last_log_timestamp = Some(timestamp);
                                last_process_time = Some(Instant::now());

                                process_event(
                                    &event,
                                    timestamp,
                                    &mut state,
                                    &ui_refresh_tx,
                                    &mut total_sent,
                                    &mut total_received,
                                    &node_effective_distances,
                                )
                                .await;

                                state.last_processed_timestamp = Some(timestamp);
                            } else {
                                // Not the first log line - apply time synchronization
                                let prev_ts = last_log_timestamp.unwrap();
                                let prev_process = last_process_time.unwrap();

                                // Calculate time difference between log lines
                                let log_line_diff = timestamp.signed_duration_since(prev_ts);
                                let mut log_line_diff_ms = log_line_diff.num_milliseconds().max(0);

                                // Calculate time spent since we processed the last message
                                let real_elapsed = prev_process.elapsed();
                                let real_elapsed_ms = real_elapsed.as_millis() as i64;

                                // Calculate current delay (how far we are behind the log timeline)
                                let current_delay = real_elapsed_ms - log_line_diff_ms;
                                let average_delay = delay_tracker.add_sample(current_delay);

                                // Adaptive catch-up: if average delay is less than current delay,
                                // speed up by multiplying wait time by 0.9
                                if average_delay < current_delay && log_line_diff_ms > 0 {
                                    log_line_diff_ms = (log_line_diff_ms * 9) / 10;
                                }

                                // Calculate remaining wait time
                                let remaining_wait_ms = log_line_diff_ms - real_elapsed_ms;

                                if remaining_wait_ms > 0 && mode == AnalyzerMode::LogVisualization {
                                    // Phase 2: Wait for remaining time OR UI command
                                    let wait_duration = Duration::from_millis(remaining_wait_ms as u64);
                                    match select(Timer::after(wait_duration), ui_command_rx.receive()).await {
                                        Either::First(_) => {
                                            // Timer expired - process the event
                                            last_log_timestamp = Some(timestamp);
                                            last_process_time = Some(Instant::now());

                                            process_event(
                                                &event,
                                                timestamp,
                                                &mut state,
                                                &ui_refresh_tx,
                                                &mut total_sent,
                                                &mut total_received,
                                                &node_effective_distances,
                                            )
                                            .await;

                                            state.last_processed_timestamp = Some(timestamp);
                                        }
                                        Either::Second(cmd) => {
                                            // UI command arrived during wait - handle it
                                            // The event is lost in this case (spec says start next iteration)
                                            handle_ui_command(cmd, &state, &ui_refresh_tx);
                                            continue;
                                        }
                                    }
                                } else {
                                    // No wait needed or in real-time mode - process immediately
                                    last_log_timestamp = Some(timestamp);
                                    last_process_time = Some(Instant::now());

                                    process_event(
                                        &event,
                                        timestamp,
                                        &mut state,
                                        &ui_refresh_tx,
                                        &mut total_sent,
                                        &mut total_received,
                                        &node_effective_distances,
                                    )
                                    .await;

                                    state.last_processed_timestamp = Some(timestamp);
                                }
                            }
                        }
                        // If parsing fails, continue to next iteration
                    }
                    None => {
                        // EOF reached in log visualization mode
                        let _ = ui_refresh_tx.send(UIRefreshState::VisualizationEnded).await;
                        log::info!("Log visualization ended (EOF reached)");

                        // After EOF, just keep responding to UI commands
                        loop {
                            let cmd = ui_command_rx.receive().await;
                            handle_ui_command(cmd, &state, &ui_refresh_tx);
                        }
                    }
                }
            }
            Either::Second(cmd) => {
                // UI command received - handle it and continue to next iteration
                handle_ui_command(cmd, &state, &ui_refresh_tx);
            }
        }
    }
}

/// Handle a UI command.
fn handle_ui_command(cmd: UICommand, state: &AnalyzerState, ui_refresh_tx: &UIRefreshQueueSender) {
    match cmd {
        UICommand::RequestNodeInfo(node_id) => {
            // Build NodeInfo from packet history
            let node_info = build_node_info(node_id, state);
            let _ = ui_refresh_tx.try_send(UIRefreshState::NodeInfo(node_info)).ok();
        }
        _ => {
            // Ignore other commands in analyzer mode
        }
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
    let obstacles: Vec<crate::simulation::Obstacle> = scene.obstacles.iter().map(|o| o.into()).collect();

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

/// Process a parsed log event and update UI.
async fn process_event(
    event: &LogEvent,
    timestamp: DateTime<Utc>,
    state: &mut AnalyzerState,
    ui_refresh_tx: &UIRefreshQueueSender,
    total_sent: &mut u64,
    total_received: &mut u64,
    node_effective_distances: &HashMap<u32, u32>,
) {
    match event {
        LogEvent::SendPacket { node_id, message_type, .. } => {
            *total_sent += 1;

            // Store in history
            state.add_packet_record(
                *node_id,
                NodePacketRecord {
                    timestamp,
                    event: event.clone(),
                },
            );

            // Notify UI of transmission using the node's effective distance
            let effective_distance = node_effective_distances.get(node_id).copied().unwrap_or(100);
            let _ = ui_refresh_tx
                .try_send(UIRefreshState::NodeSentRadioMessage(*node_id, *message_type, effective_distance))
                .ok();

            // Update counters
            let _ = ui_refresh_tx
                .try_send(UIRefreshState::RadioMessagesCountUpdated(*total_sent, *total_received, 0))
                .ok();
        }
        LogEvent::ReceivePacket { node_id, .. } => {
            *total_received += 1;

            // Store in history
            state.add_packet_record(
                *node_id,
                NodePacketRecord {
                    timestamp,
                    event: event.clone(),
                },
            );

            // Update counters
            let _ = ui_refresh_tx
                .try_send(UIRefreshState::RadioMessagesCountUpdated(*total_sent, *total_received, 0))
                .ok();
        }
        LogEvent::StartMeasurement { node_id, sequence } => {
            state.active_measurement_id = Some(*sequence);
            log::info!("Measurement started by node {} with sequence {}", node_id, sequence);
        }
        LogEvent::ReceivedFullMessage {
            node_id,
            message_type,
            sequence,
            ..
        } => {
            // If this is an AddBlock message (type 6), it might be part of a measurement
            if *message_type == 6 {
                if let Some(active_id) = state.active_measurement_id {
                    if active_id == *sequence {
                        let _ = ui_refresh_tx.try_send(UIRefreshState::NodeReachedInMeasurement(*node_id, *sequence)).ok();
                    }
                }
            }
        }
        LogEvent::AddBlockReceived { node_id, sender_id, sequence, length } => {
            // AddBlock fully received - check if part of active measurement
            if let Some(active_id) = state.active_measurement_id {
                if active_id == *sequence {
                    let _ = ui_refresh_tx.try_send(UIRefreshState::NodeReachedInMeasurement(*node_id, *sequence)).ok();
                }
            }

            // Store as FullMessage for Message Stream tab
            state.add_full_message(*node_id, FullMessage {
                timestamp: convert_to_embassy_instant(timestamp),
                message_type: 6, // AddBlock
                sender_node: *sender_id,
                sequence: *sequence,
                length: *length,
                is_outgoing: false, // Received
            });
        }
        LogEvent::AddBlockSent { node_id, sender_id, sequence, length } => {
            // Store as FullMessage for Message Stream tab
            state.add_full_message(*node_id, FullMessage {
                timestamp: convert_to_embassy_instant(timestamp),
                message_type: 6, // AddBlock
                sender_node: *sender_id,
                sequence: *sequence,
                length: *length,
                is_outgoing: true, // Sent
            });
        }
        LogEvent::Position { x, y } => {
            log::debug!("Position event: ({}, {})", x, y);
            // Position updates could be handled here if needed
        }
    }

    // Send timestamp update for UI display (as embassy_time::Instant from Unix epoch)
    let _ = ui_refresh_tx.try_send(UIRefreshState::TimeUpdated(convert_to_embassy_instant(timestamp))).ok();
}

/// Build NodeInfo from the analyzer's packet history for a given node.
///
/// Converts stored `NodePacketRecord` entries into `NodeMessage` format
/// expected by the UI.
///
/// # Parameters
///
/// * `node_id` - The node to build info for
/// * `state` - Analyzer state containing packet histories
///
/// # Returns
///
/// A `NodeInfo` struct with the node's message history.
fn build_node_info(node_id: u32, state: &AnalyzerState) -> NodeInfo {
    log::info!("Building NodeInfo for node {}", node_id);
    
    // Build radio packets from packet history
    let messages = if let Some(history) = state.node_packet_histories.get(&node_id) {
        history
            .iter()
            .filter_map(|record| {
                // Convert DateTime<Utc> to embassy_time::Instant based on absolute timestamp
                let timestamp = convert_to_embassy_instant(record.timestamp);

                match &record.event {
                    LogEvent::SendPacket {
                        message_type,
                        sequence,
                        packet_index,
                        packet_count,
                        length,
                        ..
                    } => Some(NodeMessage {
                        timestamp,
                        message_type: *message_type,
                        packet_size: *length,
                        packet_count: *packet_count,
                        packet_index: *packet_index,
                        sender_node: node_id, // Self-sent
                        link_quality: 0,
                        collision: false,
                        sequence: *sequence,
                    }),
                    LogEvent::ReceivePacket {
                        sender_id,
                        message_type,
                        sequence,
                        packet_index,
                        packet_count,
                        length,
                        link_quality,
                        ..
                    } => Some(NodeMessage {
                        timestamp,
                        message_type: *message_type,
                        packet_size: *length,
                        packet_count: *packet_count,
                        packet_index: *packet_index,
                        sender_node: *sender_id,
                        link_quality: *link_quality,
                        collision: false,
                        sequence: *sequence,
                    }),
                    _ => None, // Skip other event types
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    // Build log lines from raw log history
    let log_lines = if let Some(history) = state.node_log_histories.get(&node_id) {
        history
            .iter()
            .map(|raw_log| LogLine {
                timestamp: convert_to_embassy_instant(raw_log.timestamp),
                content: raw_log.content.clone(),
                level: raw_log.level,
            })
            .collect()
    } else {
        Vec::new()
    };

    // Build full messages from TM6/TM7 event history
    let full_messages = if let Some(history) = state.node_full_messages.get(&node_id) {
        history.iter().cloned().collect()
    } else {
        Vec::new()
    };

    NodeInfo {
        node_id,
        radio_packets: messages,
        messages: full_messages,
        log_lines,
    }
}

/// Convert a DateTime<Utc> timestamp to an embassy_time::Instant.
///
/// Creates an instant based on the absolute timestamp by converting
/// it to milliseconds since Unix epoch.
fn convert_to_embassy_instant(timestamp: DateTime<Utc>) -> embassy_time::Instant {
    // Convert absolute timestamp to milliseconds since Unix epoch
    let timestamp_ms = timestamp.timestamp_millis().max(0) as u64;
    embassy_time::Instant::from_millis(timestamp_ms)
}
