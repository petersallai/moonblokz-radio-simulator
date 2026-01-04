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
use core::time;
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_time::{Duration, Instant, Timer};
use moonblokz_radio_lib::{MessageType, RadioMessage, RadioPacket, ScoringMatrix};
use std::collections::{HashMap, VecDeque};
use std::fs;

use crate::{
    UICommandQueueReceiver, UIRefreshQueueSender, time_driver,
    ui::{NodeInfo, NodeUIState, UICommand, UIRefreshState},
};

use super::geometry::{distance_from_d2, distance2, is_intersect};
use super::node_task::node_task;
use super::signal_calculations::{calculate_air_time, calculate_effective_distance, calculate_rssi, calculate_snr_limit, dbm_to_mw, get_cad_time, mw_to_dbm};
use super::types::{
    AirtimeWaitingPacket, CAPTURE_THRESHOLD, CadItem, NODE_MESSAGES_CAPACITY, Node, NodeInputMessage, NodeInputQueue, NodeMessage, NodeOutputMessage,
    NodeOutputPayload, NodesOutputQueue, Point, Scene,
};

/// Wait for a configuration file path from UI commands.
///
/// Blocks until the UI sends a `LoadFile` command containing the scene file path.
/// Other commands received during this wait are ignored.
///
/// # Parameters
///
/// * `ui_command_rx` - Receiver for UI commands
///
/// # Returns
///
/// The file path string for the scene configuration JSON file.
async fn wait_for_config_file(ui_command_rx: &UICommandQueueReceiver) -> String {
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
///
/// Reads the JSON file, parses it into a Scene struct, and reports errors to the UI
/// via alerts if file reading or JSON parsing fails.
///
/// # Parameters
///
/// * `config_file_path` - Path to the scene JSON file
/// * `ui_refresh_tx` - Channel for sending error alerts to the UI
///
/// # Returns
///
/// `Some(Scene)` if successful, `None` if file read or parse errors occurred.
/// Validate scene configuration to reject malformed inputs.
///
/// Checks for common issues that would cause runtime problems:
/// - Excessive node count (>10000 causes UI/performance issues)
/// - Node positions outside world bounds (0-10000)
/// - Unrealistic radio strength values (outside -30 to +30 dBm)
/// - Invalid LoRa parameters (SF must be 5-12, bandwidth must be positive)
/// - Invalid path loss parameters (exponent must be positive)
/// - Obstacle geometry issues (invalid rectangles, zero-radius circles)
/// - Duplicate node IDs
///
/// # Parameters
///
/// * `scene` - The parsed scene to validate
///
/// # Returns
///
/// `Ok(())` if validation passes, `Err(String)` with error description if validation fails.
fn validate_scene(scene: &Scene) -> Result<(), String> {
    // World coordinate bounds as per Obstacle documentation
    const MAX_WORLD_COORD: f64 = 10000.0;
    const MAX_NODES: usize = 10000;
    const MIN_RADIO_STRENGTH: f32 = -50.0;
    const MAX_RADIO_STRENGTH: f32 = 50.0;

    // Check node count
    if scene.nodes.is_empty() {
        return Err("Scene must contain at least one node".to_string());
    }
    if scene.nodes.len() > MAX_NODES {
        return Err(format!("Node count {} exceeds maximum of {}", scene.nodes.len(), MAX_NODES));
    }

    // Check for duplicate node IDs
    let mut node_ids = std::collections::HashSet::new();
    for node in &scene.nodes {
        if !node_ids.insert(node.node_id) {
            return Err(format!("Duplicate node_id found: {}", node.node_id));
        }
    }

    // Validate each node
    for node in &scene.nodes {
        // Check position bounds
        if node.position.x > MAX_WORLD_COORD || node.position.y > MAX_WORLD_COORD {
            return Err(format!(
                "Node {} position ({}, {}) exceeds world bounds (0-{})",
                node.node_id, node.position.x, node.position.y, MAX_WORLD_COORD
            ));
        }

        // Check radio strength is realistic
        if node.radio_strength < MIN_RADIO_STRENGTH || node.radio_strength > MAX_RADIO_STRENGTH {
            return Err(format!(
                "Node {} radio_strength {} dBm outside realistic range ({} to {} dBm)",
                node.node_id, node.radio_strength, MIN_RADIO_STRENGTH, MAX_RADIO_STRENGTH
            ));
        }
    }

    // Validate LoRa parameters
    if scene.lora_parameters.spreading_factor < 5 || scene.lora_parameters.spreading_factor > 12 {
        return Err(format!("Invalid spreading_factor {}, must be 5-12", scene.lora_parameters.spreading_factor));
    }
    if scene.lora_parameters.bandwidth == 0 {
        return Err("Invalid bandwidth, must be positive".to_string());
    }
    if scene.lora_parameters.coding_rate < 1 || scene.lora_parameters.coding_rate > 4 {
        return Err(format!(
            "Invalid coding_rate {}, must be 1-4 (representing 4/5 to 4/8)",
            scene.lora_parameters.coding_rate
        ));
    }
    if scene.lora_parameters.preamble_symbols < 0.0 {
        return Err("Invalid preamble_symbols, must be non-negative".to_string());
    }

    // Validate path loss parameters
    if scene.path_loss_parameters.path_loss_exponent <= 0.0 {
        return Err("Invalid path_loss_exponent, must be positive".to_string());
    }
    if scene.path_loss_parameters.shadowing_sigma < 0.0 {
        return Err("Invalid shadowing_sigma, must be non-negative".to_string());
    }

    // Validate obstacles
    for (idx, obstacle) in scene.obstacles.iter().enumerate() {
        match obstacle {
            super::types::Obstacle::Rectangle { position } => {
                // Check bounds
                if position.top_left.x > MAX_WORLD_COORD
                    || position.top_left.y > MAX_WORLD_COORD
                    || position.bottom_right.x > MAX_WORLD_COORD
                    || position.bottom_right.y > MAX_WORLD_COORD
                {
                    return Err(format!(
                        "Obstacle {} (rectangle) has coordinates exceeding world bounds (0-{})",
                        idx, MAX_WORLD_COORD
                    ));
                }
                // Check that top-left is actually top-left and bottom-right is bottom-right
                if position.top_left.x >= position.bottom_right.x || position.top_left.y >= position.bottom_right.y {
                    return Err(format!(
                        "Obstacle {} (rectangle) has invalid geometry: top-left ({}, {}) must be strictly less than bottom-right ({}, {})",
                        idx, position.top_left.x, position.top_left.y, position.bottom_right.x, position.bottom_right.y
                    ));
                }
            }
            super::types::Obstacle::Circle { position } => {
                // Check bounds
                if position.center.x > MAX_WORLD_COORD || position.center.y > MAX_WORLD_COORD {
                    return Err(format!(
                        "Obstacle {} (circle) center ({}, {}) exceeds world bounds (0-{})",
                        idx, position.center.x, position.center.y, MAX_WORLD_COORD
                    ));
                }
                // Check radius is non-zero and reasonable
                if position.radius == 0.0 {
                    return Err(format!("Obstacle {} (circle) has zero radius", idx));
                }
                // Check circle doesn't extend beyond world bounds
                let max_extent_x = position.center.x + position.radius;
                let max_extent_y = position.center.y + position.radius;
                if max_extent_x > MAX_WORLD_COORD || max_extent_y > MAX_WORLD_COORD {
                    return Err(format!("Obstacle {} (circle) extends beyond world bounds (0-{})", idx, MAX_WORLD_COORD));
                }
            }
        }
    }

    Ok(())
}

async fn load_scene(config_file_path: &str, ui_refresh_tx: &UIRefreshQueueSender) -> Option<Scene> {
    let file_result = fs::read_to_string(config_file_path).with_context(|| format!("Failed to read file: {config_file_path}"));

    let data = match file_result {
        Ok(data) => data,
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error reading config file: {}", err))).await;
            return None;
        }
    };

    let result = serde_json::from_str::<Scene>(&data).context("Invalid JSON format");

    let mut scene = match result {
        Ok(scene) => scene,
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error parsing config file: {}", err))).await;
            return None;
        }
    };

    // If background_image is specified, prepend the scene file's directory
    if let Some(ref bg_image) = scene.background_image {
        use std::path::Path;
        if let Some(parent_dir) = Path::new(config_file_path).parent() {
            let full_path = parent_dir.join(bg_image);
            scene.background_image = Some(full_path.to_string_lossy().to_string());
        }
    }

    // Pre-calculate scaling factors for distance calculations
    let world_width = scene.world_bottom_right.x - scene.world_top_left.x;
    let world_height = scene.world_bottom_right.y - scene.world_top_left.y;
    scene.scale_x = scene.width / world_width;
    scene.scale_y = scene.height / world_height;

    // Validate the parsed scene before returning
    if let Err(validation_error) = validate_scene(&scene) {
        ui_refresh_tx
            .send(UIRefreshState::Alert(format!("Invalid scene configuration: {}", validation_error)))
            .await;
        return None;
    }

    Some(scene)
}

/// Initialize and publish scene state to the UI.
///
/// Sends initial updates to the UI including:
/// - Link quality threshold values (poor/excellent limits)
/// - Complete node list with positions and pre-calculated effective radio ranges
/// - Obstacle list for map rendering
///
/// # Parameters
///
/// * `scene` - The loaded scene configuration
/// * `ui_refresh_tx` - Channel for sending UI updates
async fn initialize_scene_ui(scene: &Scene, ui_refresh_tx: &UIRefreshQueueSender) {
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

    // Publish scene dimensions to the UI
    {
        ui_refresh_tx
            .send(UIRefreshState::SceneDimensionsUpdated(
                scene.world_top_left.clone(),
                scene.world_bottom_right.clone(),
                scene.width,
                scene.height,
            ))
            .await;
    }

    if let Some(ref bg_image) = scene.background_image {
        log::info!("Background image specified: {:?}", bg_image);
        ui_refresh_tx.send(UIRefreshState::BackgroundImageUpdated(Some(bg_image.clone()))).await;
    }
}

/// Initialize nodes map and spawn node tasks.
///
/// For each node in the scene:
/// 1. Creates dedicated input/output queues for communication
/// 2. Spawns an async `node_task` to manage that node's radio stack
/// 3. Pre-calculates effective radio distance for range checks
/// 4. Initializes runtime-only fields (message history, event queues)
///
/// # Parameters
///
/// * `spawner` - Embassy spawner for creating async tasks
/// * `scene` - The loaded scene configuration
/// * `nodes_output_channel` - Shared output channel for all nodes
///
/// # Returns
///
/// HashMap mapping node IDs to their initialized Node structs.
fn initialize_nodes(spawner: &Spawner, scene: &Scene, nodes_output_channel: &'static NodesOutputQueue) -> HashMap<u32, Node> {
    let mut nodes_map: HashMap<u32, Node> = HashMap::new();

    for node in &scene.nodes {
        // INTENTIONAL LEAK: Box::leak provides 'static lifetime for Embassy channels.
        // Required to use the embedded moonblokz-radio-lib code in the simulator.
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
        new_node.cached_effective_distance = calculate_effective_distance(new_node.radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters);

        // Ensure runtime-only fields are initialized
        if new_node.node_messages.is_empty() {
            new_node.node_messages = VecDeque::with_capacity(NODE_MESSAGES_CAPACITY.min(64));
        }
        nodes_map.insert(new_node.node_id, new_node);
    }

    nodes_map
}

/// Calculate the next interesting event time (earliest CAD or airtime completion).
///
/// Scans all nodes to find the earliest timestamp when something needs processing:
/// - Airtime windows ending (packet reception evaluation)
/// - CAD (Channel Activity Detection) windows ending
///
/// Returns a time far in the future if no events are pending.
///
/// # Parameters
///
/// * `nodes_map` - Map of all nodes with their pending events
///
/// # Returns
///
/// The `Instant` of the next event requiring processing.
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
///
/// Processing steps:
/// 1. Record the transmission in the sender's message history
/// 2. Queue the sender's own airtime window (for self-interference modeling)
/// 3. Update global packet counters and notify UI
/// 4. Find all target nodes within radio range and not blocked by obstacles
/// 5. Distribute the packet to each target by queueing their airtime windows
///
/// Special handling for AddBlock messages: notifies UI to track measurement progress.
///
/// # Parameters
///
/// * `node_id` - ID of the transmitting node
/// * `packet` - The radio packet being transmitted
/// * `nodes_map` - Mutable map of all nodes
/// * `scene` - Scene configuration (for propagation parameters)
/// * `ui_refresh_tx` - Channel for UI updates
/// * `total_sent_packets` - Mutable counter for total packets sent
/// * `total_received_packets` - Current count of received packets (for UI update)
/// * `total_collision` - Current collision count (for UI update)
async fn handle_radio_transfer(
    node_id: u32,
    packet: RadioPacket,
    nodes_map: &mut HashMap<u32, Node>,
    scene: &Scene,
    ui_refresh_tx: &UIRefreshQueueSender,
    total_sent_packets: &mut u64,
    total_received_packets: u64,
    total_collision: u64,
) {
    // Handle special message types for UI
    let sequence: Option<u32> = if packet.message_type() == MessageType::AddBlock as u8 {
        let seq = u32::from_le_bytes([packet.data[5], packet.data[6], packet.data[7], packet.data[8]]);
        _ = ui_refresh_tx.try_send(UIRefreshState::SendMessageInSimulation(seq)).ok();
        Some(seq)
    } else if packet.message_type() == MessageType::RequestBlockPart as u8 {
        // For RequestBlockPart, sequence is at the same offset as AddBlock
        Some(u32::from_le_bytes([packet.data[5], packet.data[6], packet.data[7], packet.data[8]]))
    } else {
        None
    };

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
            sequence,
        });

        // Enqueue the transmitter's own airtime window for collision modeling.
        let airtime_ms = (calculate_air_time(&scene.lora_parameters, packet.length) * 1000.0) as u64;
        node.push_airtime_packet(AirtimeWaitingPacket {
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
    let target_node_ids = find_target_nodes(node_id, &node_position, node_effective_distance, nodes_map, scene);

    // Queue packet reception for each target
    distribute_packet_to_targets(&packet, node_id, &node_position, node_radio_strength, &target_node_ids, nodes_map, scene);
}

/// Find all target nodes within radio range and not blocked by obstacles.
///
/// Uses squared distance for efficiency (avoiding sqrt in the hot path).
/// Only nodes within the sender's effective distance AND with clear line-of-sight
/// (no obstacle intersection) are included.
///
/// # Parameters
///
/// * `sender_id` - ID of the transmitting node (excluded from targets)
/// * `sender_position` - 2D position of the sender
/// * `sender_effective_distance` - Pre-calculated maximum range
/// * `nodes_map` - Map of all nodes
/// * `scene` - Scene configuration (for obstacle checks)
///
/// # Returns
///
/// Vector of node IDs that can receive the transmission.
fn find_target_nodes(sender_id: u32, sender_position: &Point, sender_effective_distance: f32, nodes_map: &HashMap<u32, Node>, scene: &Scene) -> Vec<u32> {
    let eff2 = (sender_effective_distance as f64).powi(2);
    let mut target_ids = Vec::new();

    for (&other_id, other_node) in nodes_map.iter() {
        if other_id == sender_id {
            continue;
        }

        let d2 = distance2(sender_position, &other_node.position, scene);
        if d2 < eff2 {
            if !is_intersect(sender_position, &other_node.position, &scene.obstacles) {
                target_ids.push(other_id);
            }
        }
    }

    target_ids
}

/// Distribute a packet to all target nodes, computing RSSI and airtime.
///
/// For each target node:
/// 1. Calculate the distance from sender to receiver
/// 2. Compute received signal strength (RSSI) including path loss and shadowing
/// 3. Calculate packet airtime based on LoRa parameters
/// 4. Queue an `AirtimeWaitingPacket` for later collision evaluation
///
/// The queued packets are processed by the main event loop when their airtime expires.
///
/// # Parameters
///
/// * `packet` - The radio packet to distribute
/// * `sender_id` - ID of the sender (for logging)
/// * `sender_position` - Sender's 2D position (for distance calculation)
/// * `sender_radio_strength` - Sender's TX power in dBm
/// * `target_node_ids` - List of nodes within range
/// * `nodes_map` - Mutable map of all nodes
/// * `scene` - Scene configuration (for propagation model)
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

        let d2 = distance2(sender_position, &target_node.position, scene);
        let distance = distance_from_d2(d2);
        target_node.push_airtime_packet(AirtimeWaitingPacket {
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
///
/// CAD is used by nodes to sense if the channel is busy before transmitting.
/// For each pending CAD request:
/// 1. Check if any airtime windows overlap with the CAD window
/// 2. Send CAD response (true if activity detected, false otherwise)
/// 3. Remove completed CAD items from the waiting list
///
/// This models the LoRa CAD feature which detects preambles without full decoding.
///
/// # Parameters
///
/// * `nodes_map` - Mutable map of all nodes with pending CAD requests
fn process_cad_requests(nodes_map: &mut HashMap<u32, Node>) {
    let now = Instant::now();

    for node in nodes_map.values_mut() {
        for cad_item in node.cad_waiting_list.iter() {
            if cad_item.end_time < now {
                let activity = node
                    .airtime_waiting_packets
                    .iter()
                    .any(|packet| packet.start_time < cad_item.end_time && packet.start_time + packet.airtime > cad_item.start_time);

                let _ = node.node_input_queue_sender.as_ref().unwrap().try_send(NodeInputMessage::CADResponse(activity));
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
    node.airtime_waiting_packets
        .retain(|packet| !packet.processed || packet.start_time + packet.airtime >= earliest_start_time);

    // Find the first unprocessed packet
    node.airtime_waiting_packets
        .iter()
        .enumerate()
        .find(|(_, p)| !p.processed)
        .map(|(i, p)| (i, p.start_time, p.start_time + p.airtime, p.rssi))
}

/// Process packet reception, including collision detection and SINR calculation.
///
/// Determines whether a packet is successfully received by:
/// 1. Computing total noise (baseline noise floor + interfering signals)
/// 2. Calculating SINR (Signal to Interference plus Noise Ratio)
/// 3. Checking for destructive collisions (capture effect)
/// 4. Comparing SINR against the required SNR limit
///
/// ## Collision Detection
///
/// - **Preamble lock loss**: Earlier packet above SNR destroys later packet
/// - **Capture effect**: Later stronger packet (>6dB) captures the receiver
/// - **Interference**: Overlapping signals add to noise floor
///
/// Successful packets are delivered to the node's input queue with link quality.
/// Collisions are logged to the message history but not delivered.
///
/// # Parameters
///
/// * `node` - Mutable reference to the receiving node
/// * `packet_index` - Index of packet in the airtime waiting list
/// * `packet_start` - When packet transmission started
/// * `packet_end` - When packet transmission ends
/// * `packet_rssi` - Received signal strength in dBm
/// * `scene` - Scene configuration (for SNR limit)
/// * `total_received_packets` - Counter for successful receptions
/// * `total_collision` - Counter for detected collisions
/// * `ui_refresh_tx` - Channel for UI updates
/// * `total_sent_packets` - Current sent count (for UI)
async fn process_packet_reception(
    node: &mut Node,
    packet_index: usize,
    packet_start: Instant,
    packet_end: Instant,
    packet_rssi: f32,
    scene: &Scene,
    total_received_packets: &mut u64,
    total_collision: &mut u64,
    ui_refresh_tx: &UIRefreshQueueSender,
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

    // Extract sequence for AddBlock and RequestBlockPart messages
    let sequence: Option<u32> = if packet.packet.message_type() == MessageType::AddBlock as u8 {
        Some(u32::from_le_bytes([
            packet.packet.data[5],
            packet.packet.data[6],
            packet.packet.data[7],
            packet.packet.data[8],
        ]))
    } else if packet.packet.message_type() == MessageType::RequestBlockPart as u8 {
        // For RequestBlockPart, sequence is at the same offset as AddBlock
        Some(u32::from_le_bytes([
            packet.packet.data[5],
            packet.packet.data[6],
            packet.packet.data[7],
            packet.packet.data[8],
        ]))
    } else {
        None
    };

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
            sequence,
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
            sequence,
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
    ui_refresh_tx: &UIRefreshQueueSender,
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
    ui_refresh_tx: &UIRefreshQueueSender,
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
pub async fn network_task(spawner: Spawner, ui_refresh_tx: UIRefreshQueueSender, ui_command_rx: UICommandQueueReceiver, scene_path: Option<String>) {
    // Global counters for the UI
    let mut total_collision = 0;
    let mut total_received_packets = 0;
    let mut total_sent_packets = 0;

    // Get configuration file path (either from parameter or wait for UI command)
    let config_file_path = match scene_path {
        Some(path) => {
            log::info!("Using provided scene path: {}", path);
            path
        }
        None => wait_for_config_file(&ui_command_rx).await,
    };

    // Load and parse scene
    let scene = match load_scene(&config_file_path, &ui_refresh_tx).await {
        Some(s) => s,
        None => return,
    };

    // Initialize UI with scene data
    initialize_scene_ui(&scene, &ui_refresh_tx).await;

    // Set up nodes and spawn tasks
    // INTENTIONAL LEAK: Box::leak provides 'static lifetime for Embassy channels.
    // Required to use the embedded moonblokz-radio-lib code in the simulator.
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
                UICommand::StartMode { .. } => {
                    // StartMode is handled by the mode selector before simulation starts
                    log::debug!("StartMode command ignored in running simulation");
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
                        if !delay_warning_issued {
                            delay_warning_issued = true;
                            let _ = ui_refresh_tx.try_send(UIRefreshState::SimulationDelayWarningChanged(time_delay));
                        }
                    } else if delay_warning_issued {
                        delay_warning_issued = false;
                        let _ = ui_refresh_tx.try_send(UIRefreshState::SimulationDelayWarningChanged(Duration::from_millis(0)));
                    }
                }

                if auto_speed_enabled {
                    if let Some(time_delay) = delay_for_autospeed {
                        adjust_auto_speed(time_delay, &mut upcounter, auto_speed_min_percent, auto_speed_max_percent, &ui_refresh_tx);
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
