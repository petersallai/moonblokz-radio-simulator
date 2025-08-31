use anyhow::Context;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, Timer};
use moonblokz_radio_lib::{
    MAX_NODE_COUNT,
    MessageType, // <-- Add this import
    RadioCommunicationManager,
    RadioMessage,
    RadioPacket,
    ReceivedPacket,
    ScoringMatrix,
    radio_device_simulator::{RadioInputQueue, RadioOutputQueue},
};
use serde::{Deserialize, de};
use std::{collections::HashMap, time::Instant as StdInstant};
use std::{collections::HashSet, fs};

use crate::{
    NodeInfo, NodeUIState, UICommand, UICommandChannelReceiver, UIRefreshChannelSender, UIRefreshState,
    signal_calculations::{
        LoraParameters, PathLossParameters, calculate_air_time, calculate_effective_distance, calculate_path_loss, calculate_receiving_limit_with_basic_noise,
        calculate_rssi, calculate_snr_limit, calculate_tx_power_from_effective_distance, dbm_to_mw, get_cad_time, get_preamble_time, mw_to_dbm,
    },
};

const CAPTURE_THRESHOLD: f32 = 6.0;
const MEASUREMENT_HEADER: [u8; 4] = [0x01, 0x02, 0x03, 0x04];
const NODE_INPUT_QUEUE_SIZE: usize = 10;
type NodeInputQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;
type NodeInputQueueReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;
type NodeInputQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;

const NODES_OUTPUT_BUFFER_CAPACITY: usize = 10;
type NodesOutputQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;
type NodesOutputQueueReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;
type NodesOutputQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;

/// Root structure representing the entire scene
#[derive(Deserialize)]
struct Scene {
    path_loss_parameters: PathLossParameters,
    lora_parameters: LoraParameters,
    nodes: Vec<Node>,
    obstacles: Vec<Obstacle>,
}

#[derive(Debug, Clone)]
pub struct NodeMessage {
    pub timestamp: StdInstant,
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
pub(crate) struct Point {
    pub(crate) x: u32,
    pub(crate) y: u32,
}

/// Rectangle position with two corners
#[derive(Debug, Deserialize)]
struct RectPos {
    #[serde(rename = "top-left-position")]
    top_left: Point,
    #[serde(rename = "bottom-right-position")]
    bottom_right: Point,
}

/// Circle position defined by its center
#[derive(Debug, Deserialize)]
struct CirclePos {
    #[serde(rename = "center_position")]
    center: Point,
    radius: u32,
}

/// Obstacles represented as tagged enum
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Obstacle {
    #[serde(rename = "rectangle")]
    Rectangle {
        #[serde(flatten)]
        position: RectPos,
        reduction: u32,
    },
    #[serde(rename = "circle")]
    Circle {
        #[serde(flatten)]
        position: CirclePos,
        reduction: u32,
    },
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

#[embassy_executor::task(pool_size = MAX_NODE_COUNT)]
async fn node_task(spawner: Spawner, node_id: u32, out_tx: NodesOutputQueueSender, in_rx: NodeInputQueueReceiver) {
    let radio_output_queue: &'static mut RadioOutputQueue = Box::leak(Box::new(RadioOutputQueue::new()));
    let radio_input_queue: &'static mut RadioInputQueue = Box::leak(Box::new(RadioInputQueue::new()));

    let radio_output_queue_receiver = radio_output_queue.receiver();
    let radio_input_queue_sender = radio_input_queue.sender();
    let radio_device = moonblokz_radio_lib::radio_device_simulator::RadioDevice::new(radio_output_queue.sender(), radio_input_queue.receiver());
    let mut manager = RadioCommunicationManager::new();
    /*let radio_config = moonblokz_radio_lib::RadioConfiguration {
        delay_between_tx_packets: 1,
        delay_between_tx_messages: 10,
        echo_request_minimal_interval: 500,
        echo_messages_target_interval: 1,
        echo_gathering_timeout: 1,
        relay_position_delay: 1,
        scoring_matrix: ScoringMatrix::new_from_encoded(&[255u8, 243u8, 65u8, 82u8, 143u8]),
    };*/

    let radio_config = moonblokz_radio_lib::RadioConfiguration {
        delay_between_tx_packets: 1,
        delay_between_tx_messages: 20,
        echo_request_minimal_interval: 10000,
        echo_messages_target_interval: 255,
        echo_gathering_timeout: 10,
        relay_position_delay: 1,
        scoring_matrix: ScoringMatrix::new_from_encoded(&[255u8, 243u8, 65u8, 82u8, 143u8]),
    };

    let _ = manager.initialize(radio_config, spawner, radio_device, node_id, node_id as u64);

    let mut arrived_sequences: HashSet<u32> = HashSet::new();

    loop {
        match select3(manager.receive_message(), in_rx.receive(), radio_output_queue_receiver.receive()).await {
            Either3::First(res) => {
                if let Ok(msg) = res {
                    if msg.message_type() == MessageType::AddBlock as u8 {
                        let sequence = u32::from_le_bytes([msg.payload[5], msg.payload[6], msg.payload[7], msg.payload[8]]);
                        let measurement_header = u32::from_le_bytes([msg.payload[13], msg.payload[14], msg.payload[15], msg.payload[16]]);
                        if arrived_sequences.contains(&sequence) {
                            //duplicate, ignore
                            continue;
                        } else {
                            arrived_sequences.insert(sequence);
                            if measurement_header == u32::from_le_bytes(MEASUREMENT_HEADER) {
                                out_tx
                                    .send(NodeOutputMessage {
                                        node_id,
                                        payload: NodeOutputPayload::NodeReachedInMeasurement(sequence),
                                    })
                                    .await;
                            }

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

    let mut scene = match result {
        Ok(scene) => scene,
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error parsing config file: {}", err))).await;
            return;
        }
    };

    for node in scene.nodes.iter_mut() {
        node.radio_strength = node.radio_strength * 1.5;
    }

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

    let nodes_output_channel = Box::leak(Box::new(NodesOutputQueue::new()));

    let mut nodes_map: HashMap<u32, Node> = HashMap::new();

    // Build nodes and spawn a task per node that owns its manager
    for node in scene.nodes {
        let node_input_channel = Box::leak(Box::new(NodeInputQueue::new()));
        let _ = spawner.spawn(node_task(spawner, node.node_id, nodes_output_channel.sender(), node_input_channel.receiver()));
        let mut new_node = node.clone();
        new_node.node_input_queue_sender = Some(node_input_channel.sender());
        nodes_map.insert(new_node.node_id, new_node);
    }
    let mut delay_warning_issued = false;
    let cad_time = get_cad_time(&scene.lora_parameters);
    let preamble_time = get_preamble_time(&scene.lora_parameters);

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

        // Await forwarded messages from any node or UI commands
        match select3(
            nodes_output_channel.receiver().receive(),
            ui_command_rx.receive(),
            Timer::at(next_airtime_event),
        )
        .await
        {
            Either3::First(NodeOutputMessage { node_id, payload }) => match payload {
                NodeOutputPayload::RadioTransfer(packet) => {
                    let node_position;
                    let node_radio_strength;
                    if let Some(node) = nodes_map.get_mut(&node_id) {
                        node.node_messages.push(NodeMessage {
                            timestamp: StdInstant::now(),
                            message_type: packet.message_type(),
                            sender_node: node_id,
                            packet_size: packet.length,
                            packet_index: packet.packet_index(),
                            link_quality: 63,
                            packet_count: packet.total_packet_count(),
                            collision: false,
                        });

                        //add to our queue to handle tx,rx collisions
                        node.airtime_waiting_packets.push(AirtimeWaitingPacket {
                            packet: packet.clone(),
                            sender_node_id: node_id,
                            start_time: Instant::now(),
                            airtime: Duration::from_millis((calculate_air_time(scene.lora_parameters.clone(), packet.length) * 1000.0) as u64),
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
                                target_node_ids.push(other_node.node_id);
                            }
                        }

                        for target_node_id in target_node_ids {
                            if let Some(target_node) = nodes_map.get_mut(&target_node_id) {
                                let distance = distance(&node_position, &target_node.position);
                                if distance < calculate_effective_distance(node_radio_strength as f32, &scene.lora_parameters, &scene.path_loss_parameters) {
                                    target_node.airtime_waiting_packets.push(AirtimeWaitingPacket {
                                        packet: packet.clone(),
                                        sender_node_id: node_id,
                                        start_time: Instant::now(),
                                        airtime: Duration::from_millis((calculate_air_time(scene.lora_parameters.clone(), packet.length) * 1000.0) as u64),
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
                            let mut message_body: [u8; 2000] = [0; 2000];
                            message_body[0..4].copy_from_slice(&MEASUREMENT_HEADER);
                            let message = RadioMessage::new_add_block(node_id, measurement_identifier, &message_body);
                            let _ = sender.send(NodeInputMessage::SendMessage(message)).await;
                        }
                    }
                }
            },
            Either3::Third(_) => {
                //check time delay
                let time_delay = Instant::now().duration_since(next_airtime_event);
                if time_delay > Duration::from_millis(10) {
                    let delay_ms = time_delay.as_millis() as u32;
                    if !delay_warning_issued {
                        delay_warning_issued = true;
                        let _ = ui_refresh_tx.try_send(UIRefreshState::SimulationDelayWarningChanged(delay_ms));
                    }
                } else {
                    if delay_warning_issued {
                        delay_warning_issued = false;
                        let _ = ui_refresh_tx.try_send(UIRefreshState::SimulationDelayWarningChanged(0));
                    }
                }

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
                                timestamp: StdInstant::now(),
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
                                timestamp: StdInstant::now(),
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
            }
        }
    }
}
