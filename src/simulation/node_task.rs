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
    IncomingMessageItem, MAX_NODE_COUNT, MessageType, RADIO_MAX_PACKET_COUNT, RadioCommunicationManager,
    radio_devices::simulator::{RadioInputQueue, RadioOutputQueue},
};
use std::collections::HashMap;

use super::types::{
    NodeInputMessage, NodeOutputMessage, NodeOutputPayload, NodesOutputQueueSender,
    RadioModuleConfig, NodeInputQueueReceiver,
};

/// Per-node asynchronous task bridging the simulated radio device, the radio
/// manager from `moonblokz_radio_lib`, and the network task.
///
/// Responsibilities:
/// - Initialize the per-node radio manager and device queues.
/// - Forward outgoing radio events to the network task via `out_tx`.
/// - Accept incoming control messages (packets to deliver, sends, CAD results).
#[embassy_executor::task(pool_size = MAX_NODE_COUNT)]
pub async fn node_task(spawner: Spawner, radio_module_config: RadioModuleConfig, node_id: u32, out_tx: NodesOutputQueueSender, in_rx: NodeInputQueueReceiver) {
    let radio_output_queue: &'static mut RadioOutputQueue = Box::leak(Box::new(RadioOutputQueue::new()));
    let radio_input_queue: &'static mut RadioInputQueue = Box::leak(Box::new(RadioInputQueue::new()));

    let radio_output_queue_receiver = radio_output_queue.receiver();
    let radio_input_queue_sender = radio_input_queue.sender();
    let radio_device = moonblokz_radio_lib::radio_devices::simulator::RadioDevice::with(radio_output_queue.sender(), radio_input_queue.receiver());
    let mut manager = RadioCommunicationManager::new();

    let radio_config = moonblokz_radio_lib::RadioConfiguration {
        delay_between_tx_packets: radio_module_config.delay_between_tx_packets,
        delay_between_tx_messages: radio_module_config.delay_between_tx_messages,
        echo_request_minimal_interval: radio_module_config.echo_request_minimal_interval,
        echo_messages_target_interval: radio_module_config.echo_messages_target_interval,
        echo_gathering_timeout: radio_module_config.echo_gathering_timeout,
        relay_position_delay: radio_module_config.relay_position_delay,
        scoring_matrix: moonblokz_radio_lib::ScoringMatrix::new_from_encoded(&radio_module_config.scoring_matrix),
        retry_interval_for_missing_packets: radio_module_config.retry_interval_for_missing_packets,
        tx_maximum_random_delay: radio_module_config.tx_maximum_random_delay,
    };

    let _ = manager.initialize(radio_config, spawner, radio_device, node_id, node_id as u64);

    let mut arrived_messages: HashMap<u32, moonblokz_radio_lib::RadioMessage> = HashMap::new();

    loop {
        match select3(manager.receive_message(), in_rx.receive(), radio_output_queue_receiver.receive()).await {
            Either3::First(res) => {
                if let Ok(IncomingMessageItem::NewMessage(msg)) = res {
                    if msg.message_type() == MessageType::AddBlock as u8 {
                        let sequence = u32::from_le_bytes([msg.payload()[5], msg.payload()[6], msg.payload()[7], msg.payload()[8]]);
                        if arrived_messages.contains_key(&sequence) {
                            //duplicate, ignore
                            continue;
                        } else {
                            arrived_messages.insert(sequence, msg.clone());
                            out_tx
                                .send(NodeOutputMessage {
                                    node_id,
                                    payload: NodeOutputPayload::NodeReachedInMeasurement(sequence),
                                })
                                .await;

                            let _ = manager.report_message_processing_status(moonblokz_radio_lib::MessageProcessingResult::NewBlockAdded(msg.clone()));
                        }
                    }

                    if msg.message_type() == MessageType::RequestBlockPart as u8 {
                        let Some(sequence) = msg.sequence() else {
                            continue;
                        };
                        if let Some(message) = arrived_messages.get(&sequence) {
                            if let Some(request_blockpart_iterator) = msg.get_request_block_part_iterator() {
                                let mut block_parts: [bool; RADIO_MAX_PACKET_COUNT] = [false; RADIO_MAX_PACKET_COUNT];
                                for part in request_blockpart_iterator {
                                    block_parts[part.packet_index as usize] = true;
                                }
                                let mut response_message = message.clone();
                                let _ = response_message.add_packet_list(block_parts);
                                let _ = manager.report_message_processing_status(moonblokz_radio_lib::MessageProcessingResult::RequestedBlockPartsFound(
                                    response_message,
                                    msg.sender_node_id(),
                                ));
                            }
                        }
                    }

                    let _ = out_tx
                        .send(NodeOutputMessage {
                            node_id,
                            payload: NodeOutputPayload::MessageReceived(msg),
                        })
                        .await;
                } else if let Ok(IncomingMessageItem::CheckIfAlreadyHaveMessage(message_type, sequence, payload_checksum)) = res {
                    if arrived_messages.contains_key(&sequence) {
                        let _ = manager.report_message_processing_status(moonblokz_radio_lib::MessageProcessingResult::AlreadyHaveMessage(
                            message_type,
                            sequence,
                            payload_checksum,
                        ));
                    }
                }
            }
            Either3::Second(cmd) => match cmd {
                NodeInputMessage::SendMessage(msg) => {
                    if msg.message_type() == MessageType::AddBlock as u8 {
                        let sequence = u32::from_le_bytes([msg.payload()[5], msg.payload()[6], msg.payload()[7], msg.payload()[8]]);
                        arrived_messages.insert(sequence, msg.clone());
                    }
                    let _ = manager.send_message(msg);
                }
                NodeInputMessage::RadioTransfer(received_packet) => {
                    radio_input_queue_sender
                        .send(moonblokz_radio_lib::radio_devices::simulator::RadioInputMessage::ReceivePacket(received_packet))
                        .await;
                }
                NodeInputMessage::CADResponse(success) => {
                    let _ = radio_input_queue_sender
                        .send(moonblokz_radio_lib::radio_devices::simulator::RadioInputMessage::CADResponse(success))
                        .await;
                }
            },
            Either3::Third(packet) => match packet {
                moonblokz_radio_lib::radio_devices::simulator::RadioOutputMessage::SendPacket(packet) => {
                    out_tx
                        .send(NodeOutputMessage {
                            node_id,
                            payload: NodeOutputPayload::RadioTransfer(packet),
                        })
                        .await;
                }
                moonblokz_radio_lib::radio_devices::simulator::RadioOutputMessage::RequestCAD => {
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
