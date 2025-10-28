//! Central network task driving the simulation timeline and UI updates.
//!
//! High-level flow each loop tick:
//! 1) Compute the next "interesting" time (end of any CAD/airtime) and a
//!    periodic tick deadline (10 ms) to keep the UI responsive.
//! 2) `select3` waits for: a node event, a UI command, or the next deadline.
//! 3) On deadlines, evaluate CAD windows, process at most one pending packet
//!    per node (to preserve order), compute SINR and collisions, and deliver RX.
//! 4) Adjust simulation speed if auto-speed is enabled based on observed delay.

use anyhow::Context;
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_time::{Duration, Instant, Timer};
use moonblokz_radio_lib::{MessageType, RadioMessage, RadioPacket, ScoringMatrix};
use std::collections::{HashMap, VecDeque};
use std::fs;

use crate::{
    ui::{NodeInfo, NodeUIState, UICommand, UIRefreshState},
    UICommandChannelReceiver, UIRefreshChannelSender,
    time_driver,
};

use super::types::{
    CadItem, Node, NodeInputMessage, NodeInputQueue,
    NodeMessage, NodeOutputMessage, NodeOutputPayload, NodesOutputQueue,
    Point, Scene, AirtimeWaitingPacket, NODE_MESSAGES_CAPACITY, CAPTURE_THRESHOLD,
};
use super::signal_calculations::{
    calculate_air_time, calculate_effective_distance, calculate_rssi,
    calculate_snr_limit, dbm_to_mw, get_cad_time, mw_to_dbm,
};
use super::geometry::{distance2, distance_from_d2, is_intersect};
use super::node_task::node_task;

/// Wait for a configuration file path from UI commands.
async fn wait_for_config_file(ui_command_rx: &UICommandChannelReceiver) -> String {
    loop {
        match ui_command_rx.receive().await {
            UICommand::LoadFile(file_path) => {
                log::info!("Loaded configuration file: {:?}", file_path);
                return file_path;
            }
            _ => {}
        }
    }
}

/// Load and parse the scene configuration from a file.
async fn load_scene(config_file_path: &str, ui_refresh_tx: &UIRefreshChannelSender) -> Option<Scene> {
    let file_result = fs::read_to_string(config_file_path)
        .with_context(|| format!("Failed to read file: {config_file_path}"));

    let data = match file_result {
        Ok(data) => data,
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error reading config file: {}", err))).await;
            return None;
        }
    };

    let result = serde_json::from_str::<Scene>(&data).context("Invalid JSON format");

    match result {
        Ok(scene) => Some(scene),
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error parsing config file: {}", err))).await;
            None
        }
    }
}

/// Initialize and publish scene state to the UI.
async fn initialize_scene_ui(scene: &Scene, ui_refresh_tx: &UIRefreshChannelSender) {
    let scoring_matrix = ScoringMatrix::new_from_encoded(&scene.radio_module_config.scoring_matrix);
    let poor_limit = scoring_matrix.poor_limit;
    let excellent_limit = scoring_matrix.excellent_limit;
    
    _ = ui_refresh_tx.send(UIRefreshState::PoorAndExcellentLimits(poor_limit, excellent_limit)).await;

    // Publish initial nodes to the UI (radio_strength rendered as effective distance in world units).
    ui_refresh_tx
        .send(UIRefreshState::NodesUpdated(
            scene
                .nodes
                .iter()
                .map(|n| NodeUIState {
                    node_id: n.node_id,
                    position: Point {
                        x: n.position.x,
                        y: n.position.y,
                    },
                    radio_strength: calculate_effective_distance(
                        n.radio_strength as f32,
                        &scene.lora_parameters,
                        &scene.path_loss_parameters,
                    ) as u32,
                })
                .collect(),
        ))
        .await;

    // Publish obstacles to the UI
    ui_refresh_tx.send(UIRefreshState::ObstaclesUpdated(scene.obstacles.clone())).await;
}

/// Initialize nodes map and spawn node tasks.
fn initialize_nodes(
    spawner: &Spawner,
    scene: &Scene,
    nodes_output_channel: &'static NodesOutputQueue,
) -> HashMap<u32, Node> {
    let mut nodes_map: HashMap<u32, Node> = HashMap::new();

    for node in &scene.nodes {
        let node_input_channel = Box::leak(Box::new(NodeInputQueue::new()));
        let _ = spawner.spawn(node_task(
            *spawner,
            scene.radio_module_config.clone(),
            node.node_id,
            nodes_output_channel.sender(),
            node_input_channel.receiver(),
        ));
        
        let mut new_node = node.clone();
        new_node.node_input_queue_sender = Some(node_input_channel.sender());
        new_node.cached_effective_distance = calculate_effective_distance(
            new_node.radio_strength as f32,
            &scene.lora_parameters,
            &scene.path_loss_parameters,
        );
        
        // Ensure runtime-only fields are initialized
        if new_node.node_messages.is_empty() {
            new_node.node_messages = VecDeque::with_capacity(NODE_MESSAGES_CAPACITY.min(64));
        }
        nodes_map.insert(new_node.node_id, new_node);
    }

    nodes_map
}

/// Calculate the next interesting event time (earliest CAD or airtime completion).
fn calculate_next_event_time(nodes_map: &HashMap<u32, Node>) -> Instant {
    let mut next_event = Instant::now() + Duration::from_secs(3600);
    
    for node in nodes_map.values() {
        for airtime_packet in &node.airtime_waiting_packets {
            if !airtime_packet.processed {
                let packet_end = airtime_packet.start_time + airtime_packet.airtime;
                if packet_end < next_event {
                    next_event = packet_end;
                }
            }
        }
        
        for cad_item in &node.cad_waiting_list {
            if cad_item.end_time < next_event {
                next_event = cad_item.end_time;
            }
        }
    }
    
    next_event
}

/// Handle a radio packet transmission from a node.
async fn handle_radio_transfer(
    node_id: u32,
    packet: RadioPacket,
    nodes_map: &mut HashMap<u32, Node>,
    scene: &Scene,
    ui_refresh_tx: &UIRefreshChannelSender,
    total_sent_packets: &mut u64,
    total_received_packets: u64,
    total_collision: u64,
) {
    // Handle special message types for UI
    if packet.message_type() == MessageType::AddBlock as u8 {
        let sequence = u32::from_le_bytes([packet.data[5], packet.data[6], packet.data[7], packet.data[8]]);
        _ = ui_refresh_tx.try_send(UIRefreshState::SendMessageInSimulation(sequence)).ok();
    }

    let (node_position, node_radio_strength, node_effective_distance) = {
        let node = match nodes_map.get_mut(&node_id) {
            Some(n) => n,
            None => return,
        };

        node.push_message(NodeMessage {
            timestamp: Instant::now(),
            message_type: packet.message_type(),
            sender_node: node_id,
            packet_size: packet.length,
            packet_index: packet.packet_index(),
            link_quality: 63,
            packet_count: packet.total_packet_count(),
            collision: false,
        });

        // Enqueue the transmitter's own airtime window for collision modeling.
        let airtime_ms = (calculate_air_time(&scene.lora_parameters, packet.length) * 1000.0) as u64;
        node.airtime_waiting_packets.push(AirtimeWaitingPacket {
            packet: packet.clone(),
            sender_node_id: node_id,
            start_time: Instant::now(),
            airtime: Duration::from_millis(airtime_ms),
            rssi: calculate_rssi(0.0, node.radio_strength, &scene.path_loss_parameters),
            processed: true,
        });

        *total_sent_packets += 1;

        ui_refresh_tx
            .try_send(UIRefreshState::RadioMessagesCountUpdated(
                *total_sent_packets,
                total_received_packets,
                total_collision,
            ))
            .ok();

        (node.position.clone(), node.radio_strength, node.cached_effective_distance)
    };

    // Notify UI of transmission
    _ = ui_refresh_tx.try_send(UIRefreshState::NodeSentRadioMessage(
        node_id,
        packet.message_type(),
        node_effective_distance as u32,
    ));

    // Find target receivers within range and not occluded
    let target_node_ids = find_target_nodes(
        node_id,
        &node_position,
        node_effective_distance,
        nodes_map,
        scene,
    );

    // Queue packet reception for each target
    distribute_packet_to_targets(
        &packet,
        node_id,
        &node_position,
        node_radio_strength,
        &target_node_ids,
        nodes_map,
        scene,
    );
}

/// Find all target nodes within radio range and not blocked by obstacles.
fn find_target_nodes(
    sender_id: u32,
    sender_position: &Point,
    sender_effective_distance: f32,
    nodes_map: &HashMap<u32, Node>,
    scene: &Scene,
) -> Vec<u32> {
    let eff2 = sender_effective_distance.powi(2);
    let mut target_ids = Vec::new();

    for (&other_id, other_node) in nodes_map.iter() {
        if other_id == sender_id {
            continue;
        }

        let d2 = distance2(sender_position, &other_node.position);
        if d2 < eff2 {
            if !is_intersect(sender_position, &other_node.position, &scene.obstacles) {
                target_ids.push(other_id);
            }
        }
    }

    target_ids
}

/// Distribute a packet to all target nodes, computing RSSI and airtime.
fn distribute_packet_to_targets(
    packet: &RadioPacket,
    sender_id: u32,
    sender_position: &Point,
    sender_radio_strength: f32,
    target_node_ids: &[u32],
    nodes_map: &mut HashMap<u32, Node>,
    scene: &Scene,
) {
    let airtime_ms = (calculate_air_time(&scene.lora_parameters, packet.length) * 1000.0) as u64;

    for &target_id in target_node_ids {
        let target_node = match nodes_map.get_mut(&target_id) {
            Some(n) => n,
            None => continue,
        };

        let d2 = distance2(sender_position, &target_node.position);
        let distance = distance_from_d2(d2);
        
        target_node.airtime_waiting_packets.push(AirtimeWaitingPacket {
            packet: packet.clone(),
            sender_node_id: sender_id,
            start_time: Instant::now(),
            airtime: Duration::from_millis(airtime_ms),
            rssi: calculate_rssi(distance as f32, sender_radio_strength, &scene.path_loss_parameters),
            processed: false,
        });
    }
}

/// Process CAD (Channel Activity Detection) requests for all nodes.
fn process_cad_requests(nodes_map: &mut HashMap<u32, Node>) {
    let now = Instant::now();
    
    for node in nodes_map.values_mut() {
        for cad_item in node.cad_waiting_list.iter() {
            if cad_item.end_time < now {
                let activity = node.airtime_waiting_packets.iter().any(|packet| {
                    packet.start_time < cad_item.end_time
                        && packet.start_time + packet.airtime > cad_item.start_time
                });

                let _ = node.node_input_queue_sender
                    .as_ref()
                    .unwrap()
                    .try_send(NodeInputMessage::CADResponse(activity));
            }
        }

        // Delete all outdated CAD items
        node.cad_waiting_list.retain(|item| item.end_time >= now);
    }
}

/// Clean up outdated airtime packets and find the next packet to process.
fn find_next_packet_to_process(node: &mut Node) -> Option<(usize, Instant, Instant, f32)> {
    // Find earliest start time of unprocessed packets
    let earliest_start_time = node
        .airtime_waiting_packets
        .iter()
        .filter(|p| !p.processed)
        .map(|p| p.start_time)
        .min()
        .unwrap_or_else(Instant::now);

    // Clean up processed packets that are no longer relevant
    node.airtime_waiting_packets.retain(|packet| {
        !packet.processed || packet.start_time + packet.airtime >= earliest_start_time
    });

    // Find the first unprocessed packet
    node.airtime_waiting_packets
        .iter()
        .enumerate()
        .find(|(_, p)| !p.processed)
        .map(|(i, p)| (i, p.start_time, p.start_time + p.airtime, p.rssi))
}

/// Process packet reception, including collision detection and SINR calculation.
async fn process_packet_reception(
    node: &mut Node,
    packet_index: usize,
    packet_start: Instant,
    packet_end: Instant,
    packet_rssi: f32,
    scene: &Scene,
    total_received_packets: &mut u64,
    total_collision: &mut u64,
    ui_refresh_tx: &UIRefreshChannelSender,
    total_sent_packets: u64,
) {
    node.airtime_waiting_packets[packet_index].processed = true;

    let snr_limit = calculate_snr_limit(&scene.lora_parameters);
    let mut sum_noise = dbm_to_mw(scene.path_loss_parameters.noise_floor);
    let mut collision = false;
    let mut destructive_collision = false;

    // Check for overlapping packets and collisions
    for (i, other_packet) in node.airtime_waiting_packets.iter().enumerate() {
        if i == packet_index {
            continue;
        }

        let other_start = other_packet.start_time;
        let other_end = other_start + other_packet.airtime;

        // Check if packets overlap in time
        if other_start < packet_end && other_end > packet_start {
            // Preamble/header lock lost if earlier packet is above SNR limit
            if other_start < packet_start && other_packet.rssi > snr_limit {
                destructive_collision = true;
            }

            // Capture effect: later stronger packet captures the receiver
            if other_start >= packet_start && packet_rssi - other_packet.rssi > CAPTURE_THRESHOLD {
                destructive_collision = true;
            }

            sum_noise += dbm_to_mw(other_packet.rssi);
            collision = true;
        }
    }

    let total_noise = mw_to_dbm(sum_noise);
    let sinr = packet_rssi - total_noise;
    let link_quality = moonblokz_radio_lib::calculate_link_quality(packet_rssi as i16, sinr as i16);

    let packet = &node.airtime_waiting_packets[packet_index];

    // Successful reception
    if sinr >= snr_limit && !destructive_collision {
        if let Some(sender) = &node.node_input_queue_sender {
            let _ = sender
                .send(NodeInputMessage::RadioTransfer(moonblokz_radio_lib::ReceivedPacket {
                    packet: packet.packet.clone(),
                    link_quality,
                }))
                .await;
        }

        *total_received_packets += 1;

        node.push_message(NodeMessage {
            timestamp: Instant::now(),
            message_type: packet.packet.message_type(),
            sender_node: packet.sender_node_id,
            packet_size: packet.packet.length,
            packet_index: packet.packet.packet_index(),
            packet_count: packet.packet.total_packet_count(),
            collision: false,
            link_quality,
        });

        ui_refresh_tx
            .try_send(UIRefreshState::RadioMessagesCountUpdated(
                total_sent_packets,
                *total_received_packets,
                *total_collision,
            ))
            .ok();
    } else if collision {
        // Collision detected
        *total_collision += 1;

        node.push_message(NodeMessage {
            timestamp: Instant::now(),
            message_type: packet.packet.message_type(),
            sender_node: packet.sender_node_id,
            packet_size: packet.packet.length,
            packet_index: packet.packet.packet_index(),
            packet_count: packet.packet.total_packet_count(),
            collision: true,
            link_quality,
        });

        ui_refresh_tx
            .try_send(UIRefreshState::RadioMessagesCountUpdated(
                total_sent_packets,
                *total_received_packets,
                *total_collision,
            ))
            .ok();
    }
}

/// Process all pending packet receptions across all nodes.
async fn process_all_packet_receptions(
    nodes_map: &mut HashMap<u32, Node>,
    scene: &Scene,
    total_received_packets: &mut u64,
    total_collision: &mut u64,
    ui_refresh_tx: &UIRefreshChannelSender,
    total_sent_packets: u64,
) {
    for node in nodes_map.values_mut() {
        if let Some((packet_index, packet_start, packet_end, packet_rssi)) = find_next_packet_to_process(node) {
            process_packet_reception(
                node,
                packet_index,
                packet_start,
                packet_end,
                packet_rssi,
                scene,
                total_received_packets,
                total_collision,
                ui_refresh_tx,
                total_sent_packets,
            )
            .await;
        }
    }
}

/// Adjust simulation speed based on processing delay (auto-speed controller).
fn adjust_auto_speed(
    time_delay: Duration,
    upcounter: &mut u32,
    auto_speed_min_percent: u32,
    auto_speed_max_percent: u32,
    ui_refresh_tx: &UIRefreshChannelSender,
) {
    // Increase speed slowly when we have headroom
    if time_delay < Duration::from_millis(8) {
        *upcounter += 1;
        if *upcounter > 5 {
            let mut percent = time_driver::get_simulation_speed_percent();
            if percent < auto_speed_max_percent {
                percent += 1;
                time_driver::set_simulation_speed_percent(percent);
                _ = ui_refresh_tx.try_send(UIRefreshState::SimulationSpeedChanged(percent));
            }
            *upcounter = 0;
        }
    } else {
        *upcounter = 0;
    }

    // Decrease speed within safe bounds to avoid near-zero speeds
    if time_delay > Duration::from_millis(8) {
        let mut percent = time_driver::get_simulation_speed_percent();
        if percent > auto_speed_min_percent {
            percent -= 1;
            time_driver::set_simulation_speed_percent(percent);
            _ = ui_refresh_tx.try_send(UIRefreshState::SimulationSpeedChanged(percent));
        }
    }
}

/// Central network task driving the simulation timeline and UI updates.
///
/// High-level flow each loop tick:
/// 1) Compute the next "interesting" time (end of any CAD/airtime) and a
///    periodic tick deadline (10 ms) to keep the UI responsive.
/// 2) `select3` waits for: a node event, a UI command, or the next deadline.
/// 3) On deadlines, evaluate CAD windows, process at most one pending packet
///    per node (to preserve order), compute SINR and collisions, and deliver RX.
/// 4) Adjust simulation speed if auto-speed is enabled based on observed delay.
#[embassy_executor::task]
pub async fn network_task(spawner: Spawner, ui_refresh_tx: UIRefreshChannelSender, ui_command_rx: UICommandChannelReceiver) {
    // Global counters for the UI
    let mut total_collision = 0;
    let mut total_received_packets = 0;
    let mut total_sent_packets = 0;

    // Wait for configuration file
    let config_file_path = wait_for_config_file(&ui_command_rx).await;

    // Load and parse scene
    let scene = match load_scene(&config_file_path, &ui_refresh_tx).await {
        Some(s) => s,
        None => return,
    };

    // Initialize UI with scene data
    initialize_scene_ui(&scene, &ui_refresh_tx).await;

    // Set up nodes and spawn tasks
    let nodes_output_channel = Box::leak(Box::new(NodesOutputQueue::new()));
    let mut nodes_map = initialize_nodes(&spawner, &scene, nodes_output_channel);

    let mut delay_warning_issued = false;
    let cad_time = get_cad_time(&scene.lora_parameters);

    let mut upcounter = 0;
    let mut auto_speed_enabled = false;
    // Auto-speed guardrails to avoid stalling the simulation
    let auto_speed_min_percent: u32 = 20; // don't go below 20%
    let auto_speed_max_percent: u32 = 1000; // don't exceed UI slider's max

    loop {
        // Calculate the next interesting event time
        let next_airtime_event = calculate_next_event_time(&nodes_map);

        // Keep the loop responsive even when no events are near by ticking every 10 ms
        let tick_deadline = Instant::now() + Duration::from_millis(10);
        let wait_deadline = if next_airtime_event < tick_deadline {
            next_airtime_event
        } else {
            tick_deadline
        };

        // Await forwarded messages from any node or UI commands
        match select3(nodes_output_channel.receiver().receive(), ui_command_rx.receive(), Timer::at(wait_deadline)).await {
            Either3::First(NodeOutputMessage { node_id, payload }) => match payload {
                NodeOutputPayload::RadioTransfer(packet) => {
                    handle_radio_transfer(
                        node_id,
                        packet,
                        &mut nodes_map,
                        &scene,
                        &ui_refresh_tx,
                        &mut total_sent_packets,
                        total_received_packets,
                        total_collision,
                    )
                    .await;
                }
                NodeOutputPayload::MessageReceived(_message) => {
                    // TODO: handle message receipt UI/state if needed
                }
                NodeOutputPayload::RequestCAD => {
                    if let Some(node) = nodes_map.get_mut(&node_id) {
                        node.cad_waiting_list.push(CadItem {
                            start_time: Instant::now(),
                            end_time: Instant::now() + cad_time,
                        });
                    }
                }
                NodeOutputPayload::NodeReachedInMeasurement(measurement_id) => {
                    ui_refresh_tx.try_send(UIRefreshState::NodeReachedInMeasurement(node_id, measurement_id)).ok();
                }
            },
            Either3::Second(cmd) => match cmd {
                UICommand::LoadFile(path) => {
                    log::warn!("LoadFile command received after initialization: {} (ignored)", path);
                }
                UICommand::RequestNodeInfo(node_id) => {
                    if let Some(node) = nodes_map.get(&node_id) {
                        let _ = ui_refresh_tx.try_send(UIRefreshState::NodeInfo(NodeInfo {
                            node_id: node.node_id,
                            messages: node.node_messages.iter().cloned().collect(),
                        }));
                    }
                }
                UICommand::StartMeasurement(node_id, measurement_identifier) => {
                    if let Some(node) = nodes_map.get(&node_id) {
                        if let Some(sender) = &node.node_input_queue_sender {
                            let message_body: [u8; 2000] = [22; 2000];
                            let message = RadioMessage::add_block_with(node_id, measurement_identifier, &message_body);
                            let _ = sender.send(NodeInputMessage::SendMessage(message)).await;
                        }
                    }
                }
                UICommand::SetAutoSpeed(enabled) => {
                    auto_speed_enabled = enabled;
                }
            },
            Either3::Third(_) => {
                // Determine whether the real event was reached or this was just the periodic tick
                let now = Instant::now();
                let event_reached = now >= next_airtime_event;

                let mut delay_for_autospeed: Option<Duration> = None;
                if event_reached {
                    // Check and report processing delay relative to the scheduled event
                    let time_delay = now.duration_since(next_airtime_event);
                    delay_for_autospeed = Some(time_delay);
                    if time_delay > Duration::from_millis(10) {
                        let delay_ms = time_delay.as_millis() as u32;
                        if !delay_warning_issued {
                            delay_warning_issued = true;
                            let _ = ui_refresh_tx.try_send(UIRefreshState::SimulationDelayWarningChanged(delay_ms));
                        }
                    } else if delay_warning_issued {
                        delay_warning_issued = false;
                        let _ = ui_refresh_tx.try_send(UIRefreshState::SimulationDelayWarningChanged(0));
                    }
                }

                if auto_speed_enabled {
                    if let Some(time_delay) = delay_for_autospeed {
                        adjust_auto_speed(
                            time_delay,
                            &mut upcounter,
                            auto_speed_min_percent,
                            auto_speed_max_percent,
                            &ui_refresh_tx,
                        );
                    }
                }
                
                // Only run event processing when the actual event deadline was reached
                if event_reached {
                    // Process CAD requests
                    process_cad_requests(&mut nodes_map);
                    
                    // Process all pending packet receptions
                    process_all_packet_receptions(
                        &mut nodes_map,
                        &scene,
                        &mut total_received_packets,
                        &mut total_collision,
                        &ui_refresh_tx,
                        total_sent_packets,
                    )
                    .await;
                } // event_reached
            }
        }
    }
}
