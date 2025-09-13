use anyhow::Context;
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, Timer};
use moonblokz_radio_lib::{
    MAX_NODE_COUNT, MessageType, RadioCommunicationManager, RadioMessage, RadioPacket, ReceivedPacket, ScoringMatrix,
    radio_device_simulator::{RadioInputQueue, RadioOutputQueue},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::{collections::HashSet, fs};

use crate::{
    NodeInfo, NodeUIState, UICommand, UICommandChannelReceiver, UIRefreshChannelSender, UIRefreshState,
    signal_calculations::{
        LoraParameters, PathLossParameters, calculate_air_time, calculate_effective_distance, calculate_rssi, calculate_snr_limit, dbm_to_mw, get_cad_time,
        get_preamble_time, mw_to_dbm,
    },
    time_driver,
};

const CAPTURE_THRESHOLD: f32 = 6.0;
const NODE_INPUT_QUEUE_SIZE: usize = 10;
type NodeInputQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;
type NodeInputQueueReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;
type NodeInputQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;

const NODES_OUTPUT_BUFFER_CAPACITY: usize = 10;
type NodesOutputQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;
type NodesOutputQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;

/// Root structure representing the entire scene
#[derive(Deserialize)]
struct Scene {
    path_loss_parameters: PathLossParameters,
    lora_parameters: LoraParameters,
    radio_module_config: RadioModuleConfig,
    nodes: Vec<Node>,
    obstacles: Vec<Obstacle>,
}

#[derive(Debug, Clone)]
pub struct NodeMessage {
    pub timestamp: embassy_time::Instant,
    pub message_type: u8,
    pub packet_size: usize,
    pub packet_count: u8,
    pub packet_index: u8,
    pub sender_node: u32,
    pub link_quality: u8,
    pub collision: bool,
}

#[derive(Clone)]
pub struct AirtimeWaitingPacket {
    packet: RadioPacket,
    sender_node_id: u32,
    start_time: Instant,
    airtime: Duration,
    processed: bool,
    rssi: f32,
}

#[derive(Clone)]
pub struct CadItem {
    start_time: Instant,
    end_time: Instant,
}

/// Node structure with position and radio strength
#[derive(Deserialize, Clone)]
struct Node {
    node_id: u32,
    position: Point,
    radio_strength: f32,
    #[serde(skip)]
    node_input_queue_sender: Option<NodeInputQueueSender>,
    #[serde(skip)]
    node_messages: Vec<NodeMessage>,
    #[serde(skip)]
    airtime_waiting_packets: Vec<AirtimeWaitingPacket>,
    #[serde(skip)]
    cad_waiting_list: Vec<CadItem>,
}

/// Simple 2D point
#[derive(Debug, Deserialize, Clone)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

/// Rectangle position with two corners
#[derive(Debug, Deserialize, Clone)]
pub struct RectPos {
    #[serde(rename = "top-left-position")]
    pub top_left: Point,
    #[serde(rename = "bottom-right-position")]
    pub bottom_right: Point,
}

/// Circle position defined by its center
#[derive(Debug, Deserialize, Clone)]
pub struct CirclePos {
    #[serde(rename = "center_position")]
    pub center: Point,
    pub radius: u32,
}

/// Obstacles represented as tagged enum
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub(crate) enum Obstacle {
    #[serde(rename = "rectangle")]
    Rectangle {
        #[serde(flatten)]
        position: RectPos,
    },
    #[serde(rename = "circle")]
    Circle {
        #[serde(flatten)]
        position: CirclePos,
    },
}
#[derive(Deserialize, Clone)]
struct RadioModuleConfig {
    delay_between_tx_packets: u8,
    delay_between_tx_messages: u8,
    echo_request_minimal_interval: u32,
    echo_messages_target_interval: u8,
    echo_gathering_timeout: u8,
    relay_position_delay: u8,
    scoring_matrix: [u8; 5],
}

enum NodeOutputPayload {
    RadioTransfer(RadioPacket),
    MessageReceived(RadioMessage),
    RequestCAD,
    NodeReachedInMeasurement(u32), // measurement ID
}

struct NodeOutputMessage {
    node_id: u32,
    payload: NodeOutputPayload,
}

enum NodeInputMessage {
    RadioTransfer(ReceivedPacket),
    SendMessage(RadioMessage),
    CADResponse(bool),
}

fn distance(a: &Point, b: &Point) -> f32 {
    let dx = a.x as f32 - b.x as f32;
    let dy = a.y as f32 - b.y as f32;
    (dx * dx + dy * dy).sqrt()
}

fn is_intersect(point1: &Point, point2: &Point, obstacles: &Vec<Obstacle>) -> bool {
    // Early out if degenerate segment
    if point1.x == point2.x && point1.y == point2.y {
        // Treat as a point: intersects if the point is inside any obstacle
        for obs in obstacles {
            match obs {
                Obstacle::Rectangle { position, .. } => {
                    if point_in_rect(point1, &position) {
                        return true;
                    }
                }
                Obstacle::Circle { position, .. } => {
                    if point_in_circle(point1, &position) {
                        return true;
                    }
                }
            }
        }
        return false;
    }

    for obs in obstacles {
        match obs {
            Obstacle::Rectangle { position, .. } => {
                if segment_intersects_rect(point1, point2, &position) {
                    return true;
                }
            }
            Obstacle::Circle { position, .. } => {
                if segment_intersects_circle(point1, point2, &position) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------- Geometry helpers ----------

fn rect_bounds(rect: &RectPos) -> (u32, u32, u32, u32) {
    let left = rect.top_left.x.min(rect.bottom_right.x);
    let right = rect.top_left.x.max(rect.bottom_right.x);
    let top = rect.top_left.y.min(rect.bottom_right.y);
    let bottom = rect.top_left.y.max(rect.bottom_right.y);
    (left, right, top, bottom)
}

fn point_in_rect(p: &Point, rect: &RectPos) -> bool {
    let (left, right, top, bottom) = rect_bounds(rect);
    p.x >= left && p.x <= right && p.y >= top && p.y <= bottom
}

fn point_in_circle(p: &Point, circle: &CirclePos) -> bool {
    let dx = p.x as i64 - circle.center.x as i64;
    let dy = p.y as i64 - circle.center.y as i64;
    let r2 = (circle.radius as i64) * (circle.radius as i64);
    dx * dx + dy * dy <= r2
}

fn segment_intersects_rect(p1: &Point, p2: &Point, rect: &RectPos) -> bool {
    // Inside check
    if point_in_rect(p1, rect) || point_in_rect(p2, rect) {
        return true;
    }

    let (left, right, top, bottom) = rect_bounds(rect);
    let lt = Point { x: left, y: top };
    let rt = Point { x: right, y: top };
    let rb = Point { x: right, y: bottom };
    let lb = Point { x: left, y: bottom };

    // Check segment against each rectangle edge
    segments_intersect(p1, p2, &lt, &rt) || segments_intersect(p1, p2, &rt, &rb) || segments_intersect(p1, p2, &rb, &lb) || segments_intersect(p1, p2, &lb, &lt)
}

fn segment_intersects_circle(p1: &Point, p2: &Point, circle: &CirclePos) -> bool {
    // Distance from circle center to segment <= radius
    let x1 = p1.x as f32;
    let y1 = p1.y as f32;
    let x2 = p2.x as f32;
    let y2 = p2.y as f32;
    let cx = circle.center.x as f32;
    let cy = circle.center.y as f32;
    let r = circle.radius as f32;

    let dx = x2 - x1;
    let dy = y2 - y1;
    if dx == 0.0 && dy == 0.0 {
        let ddx = x1 - cx;
        let ddy = y1 - cy;
        return ddx * ddx + ddy * ddy <= r * r;
    }
    let t = ((cx - x1) * dx + (cy - y1) * dy) / (dx * dx + dy * dy);
    let t_clamped = t.max(0.0).min(1.0);
    let closest_x = x1 + t_clamped * dx;
    let closest_y = y1 + t_clamped * dy;
    let ddx = closest_x - cx;
    let ddy = closest_y - cy;
    ddx * ddx + ddy * ddy <= r * r
}

fn orientation(a: &Point, b: &Point, c: &Point) -> i32 {
    let ax = a.x as i64;
    let ay = a.y as i64;
    let bx = b.x as i64;
    let by = b.y as i64;
    let cx = c.x as i64;
    let cy = c.y as i64;
    let val = (by - ay) * (cx - bx) - (bx - ax) * (cy - by);
    if val > 0 {
        1
    } else if val < 0 {
        -1
    } else {
        0
    }
}

fn on_segment(a: &Point, b: &Point, c: &Point) -> bool {
    // Is b on segment a-c (assuming collinear)
    let min_x = a.x.min(c.x);
    let max_x = a.x.max(c.x);
    let min_y = a.y.min(c.y);
    let max_y = a.y.max(c.y);
    b.x >= min_x && b.x <= max_x && b.y >= min_y && b.y <= max_y
}

fn segments_intersect(p1: &Point, q1: &Point, p2: &Point, q2: &Point) -> bool {
    let o1 = orientation(p1, q1, p2);
    let o2 = orientation(p1, q1, q2);
    let o3 = orientation(p2, q2, p1);
    let o4 = orientation(p2, q2, q1);

    if o1 != o2 && o3 != o4 {
        return true; // Proper intersection
    }
    // Special cases: collinear and overlapping endpoints
    (o1 == 0 && on_segment(p1, p2, q1)) || (o2 == 0 && on_segment(p1, q2, q1)) || (o3 == 0 && on_segment(p2, p1, q2)) || (o4 == 0 && on_segment(p2, q1, q2))
}

#[embassy_executor::task(pool_size = MAX_NODE_COUNT)]
async fn node_task(spawner: Spawner, radio_module_config: RadioModuleConfig, node_id: u32, out_tx: NodesOutputQueueSender, in_rx: NodeInputQueueReceiver) {
    let radio_output_queue: &'static mut RadioOutputQueue = Box::leak(Box::new(RadioOutputQueue::new()));
    let radio_input_queue: &'static mut RadioInputQueue = Box::leak(Box::new(RadioInputQueue::new()));

    let radio_output_queue_receiver = radio_output_queue.receiver();
    let radio_input_queue_sender = radio_input_queue.sender();
    let radio_device = moonblokz_radio_lib::radio_device_simulator::RadioDevice::new(radio_output_queue.sender(), radio_input_queue.receiver());
    let mut manager = RadioCommunicationManager::new();

    let radio_config = moonblokz_radio_lib::RadioConfiguration {
        delay_between_tx_packets: radio_module_config.delay_between_tx_packets,
        delay_between_tx_messages: radio_module_config.delay_between_tx_messages,
        echo_request_minimal_interval: radio_module_config.echo_request_minimal_interval,
        echo_messages_target_interval: radio_module_config.echo_messages_target_interval,
        echo_gathering_timeout: radio_module_config.echo_gathering_timeout,
        relay_position_delay: radio_module_config.relay_position_delay,
        scoring_matrix: ScoringMatrix::new_from_encoded(&radio_module_config.scoring_matrix),
    };

    let _ = manager.initialize(radio_config, spawner, radio_device, node_id, node_id as u64);

    let mut arrived_sequences: HashSet<u32> = HashSet::new();

    loop {
        match select3(manager.receive_message(), in_rx.receive(), radio_output_queue_receiver.receive()).await {
            Either3::First(res) => {
                if let Ok(msg) = res {
                    if msg.message_type() == MessageType::AddBlock as u8 {
                        let sequence = u32::from_le_bytes([msg.payload[5], msg.payload[6], msg.payload[7], msg.payload[8]]);
                        if arrived_sequences.contains(&sequence) {
                            //duplicate, ignore
                            continue;
                        } else {
                            arrived_sequences.insert(sequence);
                            out_tx
                                .send(NodeOutputMessage {
                                    node_id,
                                    payload: NodeOutputPayload::NodeReachedInMeasurement(sequence),
                                })
                                .await;

                            manager.report_message_processing_status(moonblokz_radio_lib::MessageProcessingResult::NewBlockAdded(msg.clone()), true);
                        }
                    }

                    let _ = out_tx
                        .send(NodeOutputMessage {
                            node_id,
                            payload: NodeOutputPayload::MessageReceived(msg),
                        })
                        .await;
                }
            }
            Either3::Second(cmd) => match cmd {
                NodeInputMessage::SendMessage(msg) => {
                    if msg.message_type() == MessageType::AddBlock as u8 {
                        let sequence = u32::from_le_bytes([msg.payload[5], msg.payload[6], msg.payload[7], msg.payload[8]]);
                        arrived_sequences.insert(sequence);
                    }
                    let _ = manager.send_message(msg);
                }
                NodeInputMessage::RadioTransfer(received_packet) => {
                    radio_input_queue_sender
                        .send(moonblokz_radio_lib::radio_device_simulator::RadioInputMessage::ReceivePacket(received_packet))
                        .await;
                }
                NodeInputMessage::CADResponse(success) => {
                    let _ = radio_input_queue_sender
                        .send(moonblokz_radio_lib::radio_device_simulator::RadioInputMessage::CADResponse(success))
                        .await;
                }
            },
            Either3::Third(packet) => match packet {
                moonblokz_radio_lib::radio_device_simulator::RadioOutputMessage::SendPacket(packet) => {
                    out_tx
                        .send(NodeOutputMessage {
                            node_id,
                            payload: NodeOutputPayload::RadioTransfer(packet),
                        })
                        .await;
                }
                moonblokz_radio_lib::radio_device_simulator::RadioOutputMessage::RequestCAD => {
                    out_tx
                        .send(NodeOutputMessage {
                            node_id,
                            payload: NodeOutputPayload::RequestCAD,
                        })
                        .await;
                }
            },
        }
    }
}

#[embassy_executor::task]
pub(crate) async fn network_task(spawner: Spawner, ui_refresh_tx: UIRefreshChannelSender, ui_command_rx: UICommandChannelReceiver) {
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

    // Keep node radio_strength as defined in the scene configuration.

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
        nodes_map.insert(new_node.node_id, new_node);
    }
    let mut delay_warning_issued = false;
    let cad_time = get_cad_time(&scene.lora_parameters);
    let _preamble_time = get_preamble_time(&scene.lora_parameters);

    let mut upcounter = 0;
    let mut auto_speed_enabled = false;
    // Auto-speed guardrails to avoid stalling the simulation
    let auto_speed_min_percent: u32 = 20; // don't go below 80%
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
                    if let Some(node) = nodes_map.get_mut(&node_id) {
                        node.node_messages.push(NodeMessage {
                            timestamp: Instant::now(),
                            message_type: packet.message_type(),
                            sender_node: node_id,
                            packet_size: packet.length,
                            packet_index: packet.packet_index(),
                            link_quality: 63,
                            packet_count: packet.total_packet_count(),
                            collision: false,
                        });

                        //add to our queue to handle tx,rx collisions
                        let airtime_ms = (calculate_air_time(scene.lora_parameters.clone(), packet.length) * 1000.0) as u64;
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
                    } else {
                        continue;
                    }

                    if let (Some(node_position), Some(node_radio_strength)) = (node_position, node_radio_strength) {
                        let mut target_node_ids: Vec<u32> = vec![];
                        _ = ui_refresh_tx.try_send(UIRefreshState::NodeSentRadioMessage(
                            node_id,
                            packet.message_type(),
                            calculate_effective_distance(node_radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters) as u32,
                        ));

                        for (_other_node_id, other_node) in nodes_map.iter().filter_map(|(id, node)| if *id != node_id { Some((id, node)) } else { None }) {
                            let distance = distance(&node_position, &other_node.position);
                            if distance < calculate_effective_distance(node_radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters) {
                                if !is_intersect(&node_position, &other_node.position, &scene.obstacles) {
                                    target_node_ids.push(other_node.node_id);
                                }
                            }
                        }

                        for target_node_id in target_node_ids {
                            if let Some(target_node) = nodes_map.get_mut(&target_node_id) {
                                let distance = distance(&node_position, &target_node.position);
                                if distance < calculate_effective_distance(node_radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters) {
                                    let airtime_ms = (calculate_air_time(scene.lora_parameters.clone(), packet.length) * 1000.0) as u64;
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
                            messages: node.node_messages.clone(),
                        }));
                    }
                }
                UICommand::StartMeasurement(node_id, measurement_identifier) => {
                    if let Some(node) = nodes_map.get(&node_id) {
                        if let Some(sender) = &node.node_input_queue_sender {
                            let message_body: [u8; 2000] = [22; 2000];
                            let message = RadioMessage::new_add_block(node_id, measurement_identifier, &message_body);
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

                        //delete all processed packets where end_time<earliest_start_time
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
                        //calculate total noise
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

                            let sinr = node.airtime_waiting_packets[packet_to_process_index].rssi - total_noise;

                            let link_quality =
                                moonblokz_radio_lib::calculate_link_quality(node.airtime_waiting_packets[packet_to_process_index].rssi as i16, sinr as i16);

                            if sinr >= snr_limit && !destructive_collision {
                                if let Some(sender) = &node.node_input_queue_sender {
                                    let _ = sender
                                        .send(NodeInputMessage::RadioTransfer(ReceivedPacket {
                                            packet: node.airtime_waiting_packets[packet_to_process_index].packet.clone(),
                                            link_quality,
                                        }))
                                        .await;
                                } else {
                                    log::warn!("Node {} does not have an input queue sender", node.node_id);
                                }
                                total_received_packets += 1;

                                node.node_messages.push(NodeMessage {
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
                                node.node_messages.push(NodeMessage {
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
