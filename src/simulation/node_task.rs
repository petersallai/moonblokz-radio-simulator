//! Per-node asynchronous task logic.
//!
//! Each node runs an independent task that:
//! - Manages the radio device and RadioCommunicationManager
//! - Forwards outgoing radio events to the network task
//! - Accepts incoming control messages (packets, sends, CAD results)
//! - Handles message deduplication and block part requests

use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use moonblokz_radio_lib::{
    IncomingMessageItem, MAX_NODE_COUNT, MessageType, RADIO_MAX_PACKET_COUNT,
    RadioCommunicationManager,
    radio_devices::simulator::{
        RadioInputQueue, RadioInputQueueSender, RadioOutputQueue, RadioOutputQueueReceiver,
    },
};
use std::collections::HashMap;

use super::types::{
    NodeInputMessage, NodeInputQueueReceiver, NodeOutputMessage, NodeOutputPayload,
    NodesOutputQueueSender, RadioModuleConfig,
};

/// Context for managing node state and communication channels.
struct NodeContext {
    node_id: u32,
    manager: RadioCommunicationManager,
    arrived_messages: HashMap<u32, moonblokz_radio_lib::RadioMessage>,
    out_tx: NodesOutputQueueSender,
    radio_input_queue_sender: RadioInputQueueSender,
}

impl NodeContext {
    /// Creates radio queues and initializes the radio device and manager.
    fn initialize(
        spawner: Spawner,
        radio_module_config: RadioModuleConfig,
        node_id: u32,
        out_tx: NodesOutputQueueSender,
    ) -> (Self, RadioOutputQueueReceiver) {
        // INTENTIONAL LEAK: Box::leak provides 'static lifetimes for Embassy radio device channels.
        // This allows the embedded moonblokz-radio-lib code to run unmodified in the simulator.
        // These queues live for the entire simulation lifetime and are cleaned up on process exit.
        let radio_output_queue: &'static mut RadioOutputQueue =
            Box::leak(Box::new(RadioOutputQueue::new()));
        let radio_input_queue: &'static mut RadioInputQueue =
            Box::leak(Box::new(RadioInputQueue::new()));

        let radio_output_queue_receiver = radio_output_queue.receiver();
        let radio_input_queue_sender = radio_input_queue.sender();

        let radio_device = moonblokz_radio_lib::radio_devices::simulator::RadioDevice::with(
            radio_output_queue.sender(),
            radio_input_queue.receiver(),
        );

        let mut manager = RadioCommunicationManager::new();

        let radio_config = moonblokz_radio_lib::RadioConfiguration {
            delay_between_tx_packets: radio_module_config.delay_between_tx_packets,
            delay_between_tx_messages: radio_module_config.delay_between_tx_messages,
            echo_request_minimal_interval: radio_module_config.echo_request_minimal_interval,
            echo_messages_target_interval: radio_module_config.echo_messages_target_interval,
            echo_gathering_timeout: radio_module_config.echo_gathering_timeout,
            relay_position_delay: radio_module_config.relay_position_delay,
            scoring_matrix: moonblokz_radio_lib::ScoringMatrix::new_from_encoded(
                &radio_module_config.scoring_matrix,
            ),
            retry_interval_for_missing_packets: radio_module_config
                .retry_interval_for_missing_packets,
            tx_maximum_random_delay: radio_module_config.tx_maximum_random_delay,
        };

        let _ = manager.initialize(radio_config, spawner, radio_device, node_id, node_id as u64);

        let context = Self {
            node_id,
            manager,
            arrived_messages: HashMap::new(),
            out_tx,
            radio_input_queue_sender,
        };

        (context, radio_output_queue_receiver)
    }

    /// Extracts the sequence number from an AddBlock message payload.
    fn extract_sequence_from_payload(payload: &[u8]) -> u32 {
        u32::from_le_bytes([payload[5], payload[6], payload[7], payload[8]])
    }

    /// Handles a new AddBlock message: deduplicates and reports to manager.
    async fn handle_add_block_message(&mut self, msg: &moonblokz_radio_lib::RadioMessage) -> bool {
        let sequence = Self::extract_sequence_from_payload(msg.payload());

        if self.arrived_messages.contains_key(&sequence) {
            // Duplicate, ignore
            return false;
        }

        self.arrived_messages.insert(sequence, msg.clone());

        let _ = self
            .out_tx
            .send(NodeOutputMessage {
                node_id: self.node_id,
                payload: NodeOutputPayload::NodeReachedInMeasurement(sequence),
            })
            .await;

        // Notify about received full message for Message Stream tab
        let _ = self
            .out_tx
            .send(NodeOutputMessage {
                node_id: self.node_id,
                payload: NodeOutputPayload::FullMessageReceived {
                    message_type: MessageType::AddBlock as u8,
                    sender_node: msg.sender_node_id(),
                    sequence,
                    length: msg.payload().len(),
                },
            })
            .await;

        let _ = self.manager.report_message_processing_status(
            moonblokz_radio_lib::MessageProcessingResult::NewBlockAdded(msg.clone()),
        );

        true
    }

    /// Handles a RequestBlockPart message: finds the block and responds with requested parts.
    fn handle_request_block_part(&mut self, msg: &moonblokz_radio_lib::RadioMessage) {
        let Some(sequence) = msg.sequence() else {
            return;
        };

        let Some(stored_message) = self.arrived_messages.get(&sequence) else {
            return;
        };

        let Some(request_blockpart_iterator) = msg.get_request_block_part_iterator() else {
            return;
        };

        let mut block_parts: [bool; RADIO_MAX_PACKET_COUNT] = [false; RADIO_MAX_PACKET_COUNT];
        for part in request_blockpart_iterator {
            block_parts[part.packet_index as usize] = true;
        }

        let mut response_message = stored_message.clone();
        let _ = response_message.add_packet_list(block_parts);
        let _ = self.manager.report_message_processing_status(
            moonblokz_radio_lib::MessageProcessingResult::RequestedBlockPartsFound(
                response_message,
                msg.sender_node_id(),
            ),
        );
    }

    /// Processes a newly received message from the radio manager.
    async fn handle_new_message(&mut self, msg: moonblokz_radio_lib::RadioMessage) {
        let message_type = msg.message_type();

        if message_type == MessageType::AddBlock as u8 {
            if !self.handle_add_block_message(&msg).await {
                // Duplicate message, don't forward
                return;
            }
        }

        if message_type == MessageType::RequestBlockPart as u8 {
            self.handle_request_block_part(&msg);
        }

        let _ = self
            .out_tx
            .send(NodeOutputMessage {
                node_id: self.node_id,
                payload: NodeOutputPayload::MessageReceived(msg),
            })
            .await;
    }

    /// Handles a check for duplicate messages.
    fn handle_duplicate_check(&mut self, message_type: u8, sequence: u32, payload_checksum: u32) {
        if self.arrived_messages.contains_key(&sequence) {
            let _ = self.manager.report_message_processing_status(
                moonblokz_radio_lib::MessageProcessingResult::AlreadyHaveMessage(
                    message_type,
                    sequence,
                    payload_checksum,
                ),
            );
        }
    }

    /// Processes incoming messages from the radio communication manager.
    async fn handle_incoming_message_item(&mut self, item: IncomingMessageItem) {
        match item {
            IncomingMessageItem::NewMessage(msg) => {
                self.handle_new_message(msg).await;
            }
            IncomingMessageItem::CheckIfAlreadyHaveMessage(
                message_type,
                sequence,
                payload_checksum,
            ) => {
                self.handle_duplicate_check(message_type, sequence, payload_checksum);
            }
        }
    }

    /// Handles input commands from the network task.
    async fn handle_input_command(&mut self, cmd: NodeInputMessage) {
        match cmd {
            NodeInputMessage::SendMessage(msg) => {
                if msg.message_type() == MessageType::AddBlock as u8 {
                    let sequence = Self::extract_sequence_from_payload(msg.payload());
                    self.arrived_messages.insert(sequence, msg.clone());

                    // Notify about sent full message for Message Stream tab
                    let _ = self
                        .out_tx
                        .send(NodeOutputMessage {
                            node_id: self.node_id,
                            payload: NodeOutputPayload::FullMessageSent {
                                message_type: MessageType::AddBlock as u8,
                                sender_node: self.node_id,
                                sequence,
                                length: msg.payload().len(),
                            },
                        })
                        .await;
                }
                let _ = self.manager.send_message(msg);
            }
            NodeInputMessage::RadioTransfer(received_packet) => {
                let _ = self
                    .radio_input_queue_sender
                    .send(moonblokz_radio_lib::radio_devices::simulator::RadioInputMessage::ReceivePacket(received_packet))
                    .await;
            }
            NodeInputMessage::CADResponse(success) => {
                let _ = self
                    .radio_input_queue_sender
                    .send(moonblokz_radio_lib::radio_devices::simulator::RadioInputMessage::CADResponse(success))
                    .await;
            }
            NodeInputMessage::RequestConnectionMatrix => {
                let _ = self.manager.report_message_processing_status(
                    moonblokz_radio_lib::MessageProcessingResult::RequestConnectionMatrixIntoLog,
                );
            }
        }
    }

    /// Handles outgoing radio events from the device.
    async fn handle_radio_output(
        &mut self,
        output: moonblokz_radio_lib::radio_devices::simulator::RadioOutputMessage,
    ) {
        match output {
            moonblokz_radio_lib::radio_devices::simulator::RadioOutputMessage::SendPacket(
                packet,
            ) => {
                let _ = self
                    .out_tx
                    .send(NodeOutputMessage {
                        node_id: self.node_id,
                        payload: NodeOutputPayload::RadioTransfer(packet),
                    })
                    .await;
            }
            moonblokz_radio_lib::radio_devices::simulator::RadioOutputMessage::RequestCAD => {
                let _ = self
                    .out_tx
                    .send(NodeOutputMessage {
                        node_id: self.node_id,
                        payload: NodeOutputPayload::RequestCAD,
                    })
                    .await;
            }
        }
    }
}

/// Per-node asynchronous task bridging the simulated radio device, the radio
/// manager from `moonblokz_radio_lib`, and the network task.
///
/// Responsibilities:
/// - Initialize the per-node radio manager and device queues.
/// - Forward outgoing radio events to the network task via `out_tx`.
/// - Accept incoming control messages (packets to deliver, sends, CAD results).
#[embassy_executor::task(pool_size = MAX_NODE_COUNT)]
pub async fn node_task(
    spawner: Spawner,
    radio_module_config: RadioModuleConfig,
    node_id: u32,
    out_tx: NodesOutputQueueSender,
    in_rx: NodeInputQueueReceiver,
) {
    let (mut context, radio_output_queue_receiver) =
        NodeContext::initialize(spawner, radio_module_config, node_id, out_tx);

    loop {
        match select3(
            context.manager.receive_message(),
            in_rx.receive(),
            radio_output_queue_receiver.receive(),
        )
        .await
        {
            Either3::First(Ok(item)) => {
                context.handle_incoming_message_item(item).await;
            }
            Either3::Second(cmd) => {
                context.handle_input_command(cmd).await;
            }
            Either3::Third(output) => {
                context.handle_radio_output(output).await;
            }
            Either3::First(Err(_)) => {
                // Handle error if needed
            }
        }
    }
}
