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
use moonblokz_radio_lib::{MessageType, RadioMessage, ScoringMatrix};
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
    let mut config_file_path_option = Option::None;

    while config_file_path_option.is_none() {
        match ui_command_rx.receive().await {
            UICommand::LoadFile(file_path) => {
                config_file_path_option = Some(file_path);
            }
            _ => {}
        }
    }

    let config_file_path = config_file_path_option.as_ref().unwrap();

    log::info!("Loaded configuration file: {:?}", config_file_path);

    let file_result = fs::read_to_string(&config_file_path).with_context(|| format!("Failed to read file: {config_file_path}"));

    let data = match file_result {
        Ok(data) => data,
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error reading config file: {}", err))).await;
            return;
        }
    };

    let result = serde_json::from_str::<Scene>(&data).context("Invalid JSON format");

    let scene = match result {
        Ok(scene) => scene,
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error parsing config file: {}", err))).await;
            return;
        }
    };

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
                    radio_strength: calculate_effective_distance(n.radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters) as u32,
                })
                .collect(),
        ))
        .await;

    // Publish obstacles to the UI
    ui_refresh_tx.send(UIRefreshState::ObstaclesUpdated(scene.obstacles.clone())).await;

    let nodes_output_channel = Box::leak(Box::new(NodesOutputQueue::new()));

    let mut nodes_map: HashMap<u32, Node> = HashMap::new();

    // Build nodes and spawn a task per node that owns its manager
    for node in scene.nodes {
        let node_input_channel = Box::leak(Box::new(NodeInputQueue::new()));
        let _ = spawner.spawn(node_task(
            spawner,
            scene.radio_module_config.clone(),
            node.node_id,
            nodes_output_channel.sender(),
            node_input_channel.receiver(),
        ));
        let mut new_node = node.clone();
        new_node.node_input_queue_sender = Some(node_input_channel.sender());
        new_node.cached_effective_distance = calculate_effective_distance(new_node.radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters);
        // Ensure runtime-only fields are initialized
        if new_node.node_messages.is_empty() {
            new_node.node_messages = VecDeque::with_capacity(NODE_MESSAGES_CAPACITY.min(64));
        }
        nodes_map.insert(new_node.node_id, new_node);
    }
    let mut delay_warning_issued = false;
    let cad_time = get_cad_time(&scene.lora_parameters);

    let mut upcounter = 0;
    let mut auto_speed_enabled = false;
    // Auto-speed guardrails to avoid stalling the simulation
    let auto_speed_min_percent: u32 = 20; // don't go below 20%
    let auto_speed_max_percent: u32 = 1000; // don't exceed UI slider's max

    loop {
        let mut next_airtime_event = Instant::now() + Duration::from_secs(3600);
        for node in nodes_map.values_mut() {
            for airtime_waiting_packet in node.airtime_waiting_packets.iter() {
                if !airtime_waiting_packet.processed {
                    let packet_end_time = airtime_waiting_packet.start_time + airtime_waiting_packet.airtime;
                    if packet_end_time < next_airtime_event {
                        next_airtime_event = packet_end_time;
                    }
                }
            }
            for cad_waiting_item in node.cad_waiting_list.iter() {
                if cad_waiting_item.end_time < next_airtime_event {
                    next_airtime_event = cad_waiting_item.end_time;
                }
            }
        }

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
                    // Handle radio packet transfer simulation
                    if packet.message_type() == MessageType::AddBlock as u8 {
                        let sequence = u32::from_le_bytes([packet.data[5], packet.data[6], packet.data[7], packet.data[8]]);
                        _ = ui_refresh_tx.try_send(UIRefreshState::SendMessageInSimulation(sequence)).ok();
                    }

                    let node_position;
                    let node_radio_strength;
                    let node_effective_distance;
                    if let Some(node) = nodes_map.get_mut(&node_id) {
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

                        total_sent_packets += 1;

                        ui_refresh_tx
                            .try_send(UIRefreshState::RadioMessagesCountUpdated(
                                total_sent_packets,
                                total_received_packets,
                                total_collision,
                            ))
                            .ok();

                        node_position = Some(node.position.clone());
                        node_radio_strength = Some(node.radio_strength);
                        node_effective_distance = Some(node.cached_effective_distance);
                    } else {
                        continue;
                    }

                    if let (Some(node_position), Some(node_radio_strength)) = (node_position, node_radio_strength) {
                        // Collect target receivers within range and not occluded by obstacles.
                        let mut target_node_ids: Vec<u32> = vec![];
                        _ = ui_refresh_tx.try_send(UIRefreshState::NodeSentRadioMessage(
                            node_id,
                            packet.message_type(),
                            node_effective_distance.unwrap_or_else(|| {
                                calculate_effective_distance(node_radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters)
                            }) as u32,
                        ));

                        // Compare squared distances to avoid sqrt in the hot path.
                        let eff2 = (node_effective_distance
                            .unwrap_or_else(|| calculate_effective_distance(node_radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters)))
                        .powi(2);

                        for (_other_node_id, other_node) in nodes_map.iter().filter_map(|(id, node)| if *id != node_id { Some((id, node)) } else { None }) {
                            let d2 = distance2(&node_position, &other_node.position);
                            if d2 < eff2 {
                                if !is_intersect(&node_position, &other_node.position, &scene.obstacles) {
                                    target_node_ids.push(other_node.node_id);
                                }
                            }
                        }

                        for target_node_id in target_node_ids {
                            if let Some(target_node) = nodes_map.get_mut(&target_node_id) {
                                let d2 = distance2(&node_position, &target_node.position);
                                if d2
                                    < (node_effective_distance
                                        .unwrap_or_else(|| {
                                            calculate_effective_distance(node_radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters)
                                        })
                                        .powi(2))
                                {
                                    // Compute actual distance only for RSSI calculation (sqrt here).
                                    let distance = distance_from_d2(d2);
                                    let airtime_ms = (calculate_air_time(&scene.lora_parameters, packet.length) * 1000.0) as u64;
                                    target_node.airtime_waiting_packets.push(AirtimeWaitingPacket {
                                        packet: packet.clone(),
                                        sender_node_id: node_id,
                                        start_time: Instant::now(),
                                        airtime: Duration::from_millis(airtime_ms),
                                        rssi: calculate_rssi(distance as f32, node_radio_strength, &scene.path_loss_parameters),
                                        processed: false,
                                    });
                                }
                            }
                        }
                    } else {
                        log::warn!("Received radio packet from unknown node {}", node_id);
                    }
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
                    // Simple feedback controller:
                    // - Speed up gradually when delays are small
                    // - Slow down (bounded) when delays exceed threshold
                    if let Some(time_delay) = delay_for_autospeed {
                        // Increase speed slowly when we have headroom
                        if time_delay < Duration::from_millis(8) {
                            upcounter += 1;
                            //log::debug!("Auto-speed upcounter: {}", upcounter);
                            if upcounter > 5 {
                                let mut percent = time_driver::get_simulation_speed_percent();
                                if percent < auto_speed_max_percent {
                                    percent += 1;
                                    time_driver::set_simulation_speed_percent(percent);
                                    _ = ui_refresh_tx.try_send(UIRefreshState::SimulationSpeedChanged(percent));
                                }
                                upcounter = 0;
                            }
                        } else {
                            upcounter = 0;
                        }

                        // Decrease speed only within safe bounds to avoid near-zero speeds
                        if time_delay > Duration::from_millis(8) {
                            let mut percent = time_driver::get_simulation_speed_percent();
                            if percent > auto_speed_min_percent {
                                percent -= 1;
                                time_driver::set_simulation_speed_percent(percent);
                                _ = ui_refresh_tx.try_send(UIRefreshState::SimulationSpeedChanged(percent));
                            }
                        }
                    }
                }
                // Only run event processing when the actual event deadline was reached
                if event_reached {
                    for (_id, node) in nodes_map.iter_mut() {
                        let now = Instant::now();
                        for cad_waiting_item in node.cad_waiting_list.iter_mut() {
                            if cad_waiting_item.end_time < now {
                                let mut activity = false;
                                for packet in &node.airtime_waiting_packets {
                                    if packet.start_time < cad_waiting_item.end_time && packet.start_time + packet.airtime > cad_waiting_item.start_time {
                                        activity = true;
                                        break;
                                    }
                                }

                                let _ = node.node_input_queue_sender.as_ref().unwrap().try_send(NodeInputMessage::CADResponse(activity));
                            }
                        }

                        //delete all outdated CAD items
                        node.cad_waiting_list.retain(|item| item.end_time >= now);

                        //clean outdated items from airtime_waiting_packets
                        //get the earliest start time of unprocessed items
                        let mut earliest_start_time = Instant::now();
                        for packet in &node.airtime_waiting_packets {
                            if !packet.processed {
                                if packet.start_time < earliest_start_time {
                                    earliest_start_time = packet.start_time;
                                }
                            }
                        }

                        // Delete all processed packets that end before the earliest start of any
                        // remaining unprocessed packets. This bounds the window of interest and
                        // reduces per-iteration work without changing outcomes.
                        node.airtime_waiting_packets
                            .retain(|packet| !packet.processed || packet.start_time + packet.airtime >= earliest_start_time);

                        //get the index, start_time,end_time of the next packet to process
                        let mut packet_to_process_index: Option<usize> = None;
                        let mut packet_to_process_start_time = Instant::now();
                        let mut packet_to_process_end_time = Instant::now();
                        let mut packet_to_process_rssi = 0.0;

                        for (i, packet) in node.airtime_waiting_packets.iter().enumerate() {
                            if !packet.processed {
                                packet_to_process_index = Some(i);
                                packet_to_process_start_time = packet.start_time;
                                packet_to_process_end_time = packet.start_time + packet.airtime;
                                packet_to_process_rssi = packet.rssi;
                                break;
                            }
                        }

                        let mut destructive_collision = false;
                        // Calculate total noise as: thermal noise floor + overlapping RSSI (in mW).
                        // Determine destructive collisions:
                        // - If an overlapping packet started before and is above the SNR limit, it
                        //   destroys the current one (preamble/header lock lost).
                        // - If an overlapping packet starts after current start and is stronger by
                        //   CAPTURE_THRESHOLD, it captures the receiver and destroys the current.
                        if let Some(packet_to_process_index) = packet_to_process_index {
                            node.airtime_waiting_packets[packet_to_process_index].processed = true;
                            let mut sum_noise = dbm_to_mw(scene.path_loss_parameters.noise_floor);
                            let mut collision = false;
                            let snr_limit = calculate_snr_limit(&scene.lora_parameters);
                            for (i, packet) in node.airtime_waiting_packets.iter().enumerate() {
                                if i != packet_to_process_index {
                                    if packet.start_time < packet_to_process_end_time && packet.start_time + packet.airtime > packet_to_process_start_time {
                                        if packet.start_time < packet_to_process_start_time && packet.rssi > snr_limit {
                                            destructive_collision = true;
                                        }
                                        if packet.start_time >= packet_to_process_start_time && packet_to_process_rssi - packet.rssi > CAPTURE_THRESHOLD {
                                            destructive_collision = true;
                                        }
                                        sum_noise += dbm_to_mw(packet.rssi);
                                        collision = true;
                                    }
                                }
                            }

                            let total_noise = mw_to_dbm(sum_noise);

                            // Since rssi/total_noise are in dBm, subtracting yields SINR in dB.
                            let sinr = node.airtime_waiting_packets[packet_to_process_index].rssi - total_noise;

                            let link_quality =
                                moonblokz_radio_lib::calculate_link_quality(node.airtime_waiting_packets[packet_to_process_index].rssi as i16, sinr as i16);

                            if sinr >= snr_limit && !destructive_collision {
                                if let Some(sender) = &node.node_input_queue_sender {
                                    let _ = sender
                                        .send(NodeInputMessage::RadioTransfer(moonblokz_radio_lib::ReceivedPacket {
                                            packet: node.airtime_waiting_packets[packet_to_process_index].packet.clone(),
                                            link_quality,
                                        }))
                                        .await;
                                } else {
                                    log::warn!("Node {} does not have an input queue sender", node.node_id);
                                }
                                total_received_packets += 1;

                                node.push_message(NodeMessage {
                                    timestamp: Instant::now(),
                                    message_type: node.airtime_waiting_packets[packet_to_process_index].packet.message_type(),
                                    sender_node: node.airtime_waiting_packets[packet_to_process_index].sender_node_id,
                                    packet_size: node.airtime_waiting_packets[packet_to_process_index].packet.length,
                                    packet_index: node.airtime_waiting_packets[packet_to_process_index].packet.packet_index(),
                                    packet_count: node.airtime_waiting_packets[packet_to_process_index].packet.total_packet_count(),
                                    collision: false,
                                    link_quality,
                                });

                                ui_refresh_tx
                                    .try_send(UIRefreshState::RadioMessagesCountUpdated(
                                        total_sent_packets,
                                        total_received_packets,
                                        total_collision,
                                    ))
                                    .ok();
                            } else if collision {
                                total_collision += 1;
                                node.push_message(NodeMessage {
                                    timestamp: Instant::now(),
                                    message_type: node.airtime_waiting_packets[packet_to_process_index].packet.message_type(),
                                    sender_node: node.airtime_waiting_packets[packet_to_process_index].sender_node_id,
                                    packet_size: node.airtime_waiting_packets[packet_to_process_index].packet.length,
                                    packet_index: node.airtime_waiting_packets[packet_to_process_index].packet.packet_index(),
                                    packet_count: node.airtime_waiting_packets[packet_to_process_index].packet.total_packet_count(),
                                    collision: true,
                                    link_quality,
                                });
                                ui_refresh_tx
                                    .try_send(UIRefreshState::RadioMessagesCountUpdated(
                                        total_sent_packets,
                                        total_received_packets,
                                        total_collision,
                                    ))
                                    .ok();
                            }
                        }
                    }
                } // event_reached
            }
        }
    }
}
