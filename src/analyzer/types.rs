//! Type definitions specific to the analyzer module.

use chrono::{DateTime, Utc};
use std::collections::{HashMap, VecDeque};

/// Analyzer operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyzerMode {
    /// Connect to a live log stream (file being appended).
    RealtimeTracking,
    /// Replay a previously saved log file.
    LogVisualization,
}

/// Parsed log line variants.
#[derive(Debug, Clone)]
pub enum LogEvent {
    /// *TM1* - Packet transmitted by a node.
    SendPacket {
        node_id: u32,
        message_type: u8,
        sequence: Option<u32>,
        packet_index: u8,
        packet_count: u8,
        length: usize,
    },
    /// *TM2* - Packet received by a node.
    ReceivePacket {
        node_id: u32,
        sender_id: u32,
        message_type: u8,
        sequence: Option<u32>,
        packet_index: u8,
        packet_count: u8,
        length: usize,
        link_quality: u8,
    },
    /// *TM3* - Measurement started.
    StartMeasurement {
        node_id: u32,
        sequence: u32,
    },
    /// *TM4* - Full message received and routed.
    ReceivedFullMessage {
        node_id: u32,
        sender_id: u32,
        message_type: u8,
        sequence: u32,
        length: usize,
    },
    /// Position update for a node (for potential future use).
    Position {
        x: f64,
        y: f64,
    },
}

/// Record of a packet for history tracking.
#[derive(Debug, Clone)]
pub struct NodePacketRecord {
    pub timestamp: DateTime<Utc>,
    pub event: LogEvent,
}

/// Runtime state for the analyzer task.
#[derive(Debug)]
pub struct AnalyzerState {
    /// First timestamp encountered, used as reference for time sync.
    pub reference_timestamp: Option<DateTime<Utc>>,
    /// Real-time instant when reference_timestamp was set.
    pub reference_instant: Option<std::time::Instant>,
    /// Currently active measurement ID (from *TM3*).
    pub active_measurement_id: Option<u32>,
    /// Per-node packet history for RequestNodeInfo responses.
    pub node_packet_histories: HashMap<u32, VecDeque<NodePacketRecord>>,
    /// Last processed log timestamp for delay calculation.
    pub last_processed_timestamp: Option<DateTime<Utc>>,
}

impl AnalyzerState {
    pub fn new() -> Self {
        Self {
            reference_timestamp: None,
            reference_instant: None,
            active_measurement_id: None,
            node_packet_histories: HashMap::new(),
            last_processed_timestamp: None,
        }
    }

    /// Add a packet record to a node's history.
    pub fn add_packet_record(&mut self, node_id: u32, record: NodePacketRecord) {
        const MAX_HISTORY: usize = 1000;
        let history = self.node_packet_histories.entry(node_id).or_insert_with(VecDeque::new);
        if history.len() >= MAX_HISTORY {
            history.pop_front();
        }
        history.push_back(record);
    }
}

impl Default for AnalyzerState {
    fn default() -> Self {
        Self::new()
    }
}
