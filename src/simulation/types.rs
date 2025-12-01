//! Type definitions for the simulation.
//!
//! Contains all data structures used across the simulation including:
//! - Scene configuration (nodes, obstacles, parameters)
//! - Message and packet structures
//! - Node state and runtime data
//! - Communication channels and queues

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant};
use moonblokz_radio_lib::{RadioMessage, RadioPacket};
use serde::Deserialize;
use std::collections::VecDeque;

use super::signal_calculations::{LoraParameters, PathLossParameters};

/// Minimum RSSI dominance (dB) for the capture effect to destroy a later
/// overlapping packet. If the in-progress packet is stronger by this margin,
/// the overlapping (later-starting) one is treated as destructively colliding.
///
/// Note: This is a simplification; real capture behavior depends on timing,
/// coding/interleaving, and receiver implementation.
pub const CAPTURE_THRESHOLD: f32 = 6.0;

/// Depth of the per-node control channel (UI→node manager inputs).
/// Small to avoid unbounded buffering while allowing modest burstiness.
pub const NODE_INPUT_QUEUE_SIZE: usize = 10;
/// Bounded channel used to send control messages to a node's manager.
pub type NodeInputQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;
/// Receiver side of the node input channel.
pub type NodeInputQueueReceiver = embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;
/// Sender side of the node input channel.
pub type NodeInputQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, NodeInputMessage, NODE_INPUT_QUEUE_SIZE>;

/// Depth of the global output channel (nodes→network task). Also intentionally
/// small; higher volumes are aggregated and handled by the network loop.
pub const NODES_OUTPUT_BUFFER_CAPACITY: usize = 10;
/// Bounded channel used by node tasks to publish events for the network task.
pub type NodesOutputQueue = embassy_sync::channel::Channel<CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;
/// Sender side of the nodes output channel.
pub type NodesOutputQueueSender = embassy_sync::channel::Sender<'static, CriticalSectionRawMutex, NodeOutputMessage, NODES_OUTPUT_BUFFER_CAPACITY>;

/// Root structure representing the entire scene
#[derive(Deserialize)]
pub struct Scene {
    /// Path loss model parameters for the physical layer.
    pub path_loss_parameters: PathLossParameters,
    /// LoRa-like parameters for airtime/SNR limit and symbol timings.
    pub lora_parameters: LoraParameters,
    /// Module-level configuration for the simulated radio manager.
    pub radio_module_config: RadioModuleConfig,
    /// All nodes present in the scene (positions and radios).
    pub nodes: Vec<Node>,
    /// Static obstacles used to determine line-of-sight.
    pub obstacles: Vec<Obstacle>,
}

#[derive(Debug, Clone)]
pub struct NodeMessage {
    /// Virtual timestamp when the event was recorded.
    pub timestamp: embassy_time::Instant,
    /// Encoded message type (matches `moonblokz_radio_lib::MessageType` values).
    pub message_type: u8,
    /// Total packet payload size (bytes).
    pub packet_size: usize,
    /// Total number of packets in the message.
    pub packet_count: u8,
    /// Zero-based packet index within the message sequence.
    pub packet_index: u8,
    /// Sender node ID. If equals this node's ID, the message was sent by self.
    pub sender_node: u32,
    /// Computed link quality for received packets; '-' (ignored) for self.
    pub link_quality: u8,
    /// Whether this event represents a detected collision.
    pub collision: bool,
}

#[derive(Clone)]
pub struct AirtimeWaitingPacket {
    /// The packet payload to be delivered upon successful reception.
    pub packet: RadioPacket,
    /// Sender node ID of the packet.
    pub sender_node_id: u32,
    /// The time transmission started at the sender.
    pub start_time: Instant,
    /// Simulated on-air time of this packet.
    pub airtime: Duration,
    /// Whether this packet has already been evaluated/processed in the event loop.
    pub processed: bool,
    /// Received signal strength (dBm) at the receiver location (includes path loss).
    pub rssi: f32,
}

#[derive(Clone)]
pub struct CadItem {
    /// CAD start time at this receiver.
    pub start_time: Instant,
    /// CAD end time at this receiver.
    pub end_time: Instant,
}

/// Node structure with position and radio strength
///
/// Runtime-only fields are skipped from serde and initialized at scene load:
/// - `node_messages`: bounded ring buffer to avoid unbounded memory use.
/// - `airtime_waiting_packets` and `cad_waiting_list`: event queues processed
///   by `network_task`.
/// - `cached_effective_distance`: per-node cache to avoid recomputing the same
///   range value for each candidate receiver.
#[derive(Deserialize, Clone)]
pub struct Node {
    pub node_id: u32,
    pub position: Point,
    pub radio_strength: f32,
    #[serde(skip)]
    pub node_input_queue_sender: Option<NodeInputQueueSender>,
    #[serde(skip)]
    pub node_messages: VecDeque<NodeMessage>,
    #[serde(skip)]
    pub airtime_waiting_packets: Vec<AirtimeWaitingPacket>,
    #[serde(skip)]
    pub cad_waiting_list: Vec<CadItem>,
    #[serde(skip)]
    pub cached_effective_distance: f32,
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
/// Obstacles expressed in world coordinates (0..=10000 for both axes). Rectangles
/// are defined by two corners; circles by center and radius. Intersection checks
/// are conservative with degenerate segment handling.
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum Obstacle {
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
pub struct RadioModuleConfig {
    /// Inter-packet gap inside a single message (ms) used by the TX scheduler.
    pub delay_between_tx_packets: u16,
    /// Delay between separate messages initiated by the manager (ms).
    pub delay_between_tx_messages: u8,
    /// Minimum spacing between echo requests (minutes).
    pub echo_request_minimal_interval: u16,
    /// Target interval (ms) for echo messages.
    pub echo_messages_target_interval: u8,
    /// Timeout (ms) for collecting echo messages.
    pub echo_gathering_timeout: u8,
    /// Artificial delay (ms) before relaying position reports.
    pub relay_position_delay: u8,
    /// Encoded scoring matrix thresholds (see `ScoringMatrix::new_from_encoded`).
    pub scoring_matrix: [u8; 5],
    /// Interval (ms) between retries for missing packets in a multi-packet message.
    pub retry_interval_for_missing_packets: u8,
    /// Maximum random delay in milliseconds added to transmission timing
    pub tx_maximum_random_delay: u16,
}

pub enum NodeOutputPayload {
    /// Node emitted a packet over the simulated radio.
    RadioTransfer(RadioPacket),
    /// Node received a high-level `RadioMessage` from the stack.
    MessageReceived(RadioMessage),
    /// Node requests a channel activity detection operation window.
    RequestCAD,
    /// A node reached during a measurement (by sequence/measurement ID).
    NodeReachedInMeasurement(u32), // measurement ID
}

/// Envelope for events emitted by node tasks into the network loop.
pub struct NodeOutputMessage {
    pub node_id: u32,
    pub payload: NodeOutputPayload,
}

pub enum NodeInputMessage {
    /// Deliver a low-level radio packet to a node's RX path.
    RadioTransfer(moonblokz_radio_lib::ReceivedPacket),
    /// Ask a node to send a higher-level message (encodes into packets).
    SendMessage(RadioMessage),
    /// Respond to a CAD request indicating whether any activity was present.
    CADResponse(bool),
}

/// Maximum message history per node (ring buffer). Bounded to keep UI/memory predictable.
pub const NODE_MESSAGES_CAPACITY: usize = 1000;

/// Maximum number of airtime waiting packets per node before overflow warnings.
/// This prevents unbounded growth under extreme collision or high-load scenarios.
/// Typical values: 50-100 packets for normal operation, >200 indicates potential issues.
pub const MAX_AIRTIME_WAITING_PACKETS: usize = 500;

/// Warning threshold - log warnings when airtime packets exceed this percentage.
const AIRTIME_CAPACITY_WARNING_THRESHOLD: f32 = 0.8; // 80%

impl Node {
    /// Push a message into this node's bounded history, popping the oldest if
    /// at capacity.
    pub fn push_message(&mut self, msg: NodeMessage) {
        if self.node_messages.len() >= NODE_MESSAGES_CAPACITY {
            self.node_messages.pop_front();
        }
        self.node_messages.push_back(msg);
    }

    /// Push an airtime waiting packet with capacity checking and overflow warning.
    /// Returns true if the packet was added, false if capacity was exceeded.
    pub fn push_airtime_packet(&mut self, packet: AirtimeWaitingPacket) -> bool {
        if self.airtime_waiting_packets.len() >= MAX_AIRTIME_WAITING_PACKETS {
            log::error!(
                "Node {} airtime packet queue overflow! Capacity: {}/{}, dropping oldest packets",
                self.node_id,
                self.airtime_waiting_packets.len(),
                MAX_AIRTIME_WAITING_PACKETS
            );
            // Emergency cleanup: remove oldest processed packets
            self.airtime_waiting_packets.retain(|p| !p.processed);

            // If still at capacity, remove oldest unprocessed (should rarely happen)
            if self.airtime_waiting_packets.len() >= MAX_AIRTIME_WAITING_PACKETS {
                self.airtime_waiting_packets.remove(0);
            }
        } else if self.airtime_waiting_packets.len() as f32 >= (MAX_AIRTIME_WAITING_PACKETS as f32 * AIRTIME_CAPACITY_WARNING_THRESHOLD) {
            log::warn!(
                "Node {} airtime packet queue approaching capacity: {}/{}",
                self.node_id,
                self.airtime_waiting_packets.len(),
                MAX_AIRTIME_WAITING_PACKETS
            );
        }

        self.airtime_waiting_packets.push(packet);
        true
    }
}
