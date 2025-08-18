use anyhow::Context;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Duration;
use log::debug;
use moonblokz_radio_lib::{RadioCommunicationManager, RadioMessage, RadioPacket, ScoringMatrix};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

use crate::{NodeUIState, UICommand, UICommandChannelReceiver, UIRefreshChannelSender, UIRefreshState};

const NODE_INPUT_QUEUE_SIZE: usize = 10;
type NodeInputQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;
type NodeInputQueueReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;
type NodeInputQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;

const NODES_OUTPUT_BUFFER_CAPACITY: usize = 10;
type NodesOutputQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;
type NodesOutputQueueReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;
type NodesOutputQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;

/// Root structure representing the entire scene
#[derive(Debug, Deserialize)]
struct Scene {
    nodes: Vec<Node>,
    obstacles: Vec<Obstacle>,
}

/// Node structure with position and radio strength
#[derive(Debug, Deserialize)]
struct Node {
    node_id: u32,
    position: Point,
    radio_strength: u32,
}

/// Simple 2D point
#[derive(Debug, Deserialize)]
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
}

struct NodeOutputMessage {
    node_id: u32,
    payload: NodeOutputPayload,
}

enum NodeInputMessage {
    RadioTransfer(u8, RadioPacket),
    SendMessage(RadioMessage),
}

#[embassy_executor::task]
async fn node_task(spawner: Spawner, node_id: u32, out_tx: NodesOutputQueueSender, in_rx: NodeInputQueueReceiver) {
    let radio_device = moonblokz_radio_lib::radio_device_simulator::RadioDevice::new();
    let mut manager = RadioCommunicationManager::new();
    let radio_config = moonblokz_radio_lib::RadioConfiguration {
        delay_between_tx_packets: 1,
        delay_between_tx_messages: 10,
        echo_request_minimal_interval: 86400,
        echo_messages_target_interval: 100,
        echo_gathering_timeout: 10,
        relay_position_delay: 1,
        scoring_matrix: ScoringMatrix::new_from_encoded(&[255u8, 243u8, 65u8, 82u8, 143u8]),
    };
    let _ = manager.initialize(radio_config, spawner, radio_device, node_id, node_id as u64);

    loop {
        match select(manager.receive_message(), in_rx.receive()).await {
            Either::First(res) => {
                if let Ok(msg) = res {
                    let _ = out_tx
                        .send(NodeOutputMessage {
                            node_id,
                            payload: NodeOutputPayload::MessageReceived(msg),
                        })
                        .await;
                }
            }
            Either::Second(cmd) => match cmd {
                NodeInputMessage::SendMessage(msg) => {
                    let _ = manager.send_message(msg);
                }
                NodeInputMessage::RadioTransfer(_ch, _packet) => {
                    // TODO: feed incoming radio packet into the manager when API is available
                }
            },
        }
    }
}

#[embassy_executor::task]
async fn nodes_send_task(idx: usize, manager: RadioCommunicationManager) {
    loop {
        // Example: periodically initiate an echo request
        let _ = manager.send_message(RadioMessage::new_request_echo(1));
        embassy_time::Timer::after(Duration::from_millis(1000)).await;
    }
}

#[embassy_executor::task]
pub(crate) async fn network_task(spawner: Spawner, ui_refresh_tx: UIRefreshChannelSender, ui_command_rx: UICommandChannelReceiver) {
    let mut config_file_path_option = Option::None;

    while config_file_path_option.is_none() {
        match ui_command_rx.receive().await {
            UICommand::LoadFile(file_path) => {
                config_file_path_option = Some(file_path);
            }
        }
    }

    let config_file_path = config_file_path_option.as_ref().unwrap();

    log::info!("Loaded configuration file: {:?}", config_file_path);

    let file_result = fs::read_to_string(&config_file_path).with_context(|| format!("Failed to read file: {config_file_path}"));

    let data = match file_result {
        Ok(data) => data,
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error reading config file: {}", err)));
            return;
        }
    };

    let result = serde_json::from_str::<Scene>(&data).context("Invalid JSON format");

    let scene = match result {
        Ok(scene) => scene,
        Err(err) => {
            ui_refresh_tx.send(UIRefreshState::Alert(format!("Error parsing config file: {}", err)));
            return;
        }
    };

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
                    radio_strength: n.radio_strength,
                })
                .collect(),
        ))
        .await;

    ui_refresh_tx
        .send(UIRefreshState::Alert(format!("Configuration file loaded with {} nodes", scene.nodes.len())))
        .await;

    let nodes_output_channel = Box::leak(Box::new(NodesOutputQueue::new()));

    let mut node_input_senders: HashMap<u32, NodeInputQueueSender> = HashMap::new();

    // Build nodes and spawn a task per node that owns its manager
    for node in scene.nodes {
        let node_input_channel = Box::leak(Box::new(NodeInputQueue::new()));
        let _ = spawner.spawn(node_task(spawner, node.node_id, nodes_output_channel.sender(), node_input_channel.receiver()));
        node_input_senders.insert(node.node_id, node_input_channel.sender());
    }
    loop {
        // Await first forwarded message from any node
        let message = nodes_output_channel.receiver().receive().await;
        debug!("Processed message from node #{}", message.node_id);

        // Send periodic updates to the UI
        let _ = ui_refresh_tx.send(UIRefreshState::Alert(format!("Processed message from node #{}", message.node_id)));
        embassy_time::Timer::after(Duration::from_millis(10000)).await;
    }
}
