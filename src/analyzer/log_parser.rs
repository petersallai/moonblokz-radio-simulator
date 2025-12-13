//! Parse individual log lines and extract structured `LogEvent` data.
//!
//! Supports the following log line formats:
//! - *TM1*: Packet transmitted
//! - *TM2*: Packet received
//! - *TM3*: Start measurement
//! - *TM4*: Full message received

use super::types::LogEvent;
use chrono::{DateTime, Utc};

/// Parse a log line and extract timestamp and event.
///
/// # Parameters
///
/// * `line` - A single log line to parse
///
/// # Returns
///
/// `Some((timestamp, event))` if parsing succeeds, `None` for unparseable lines.
///
/// # Log Line Formats
///
/// ```text
/// *TM1* (Send Packet):
/// 2025-10-23T18:00:01Z moonblokz_radio_lib::radio_devices::rp_lora_sx1262: [3094] *TM1* Packet transmitted: type: 6, sequence: 30940779, length: 215, packet: 1/10
///
/// *TM2* (Receive Packet):
/// 2025-10-23T18:00:01Z moonblokz_radio_lib::radio_devices::rp_lora_sx1262:[3094] *TM2* Packet received: sender: 3093, type: 6, sequence: 30940779, length: 215, packet: 1/10, link quality: 26
///
/// *TM3* (Start Measurement):
/// 2025-10-23T18:00:00Z [3094] *TM3* Start measurement: sequence: 321312
///
/// *TM4* (Received Full Message):
/// 2025-10-23T18:00:05Z [3094] *TM4* Routing message to incoming queue: sender: 3093, type: 6, length: 2000, sequence: 321312
/// ```
pub fn parse_log_line(line: &str) -> Option<(DateTime<Utc>, LogEvent)> {
    // Extract timestamp from the start of the line
    let timestamp = parse_timestamp(line)?;
    // Find the node ID pattern [xxxx]
    let node_id = extract_node_id(line)?;

    // Determine event type and parse accordingly
    if line.contains("*TM1*") {
        parse_tm1(line, node_id).map(|event| (timestamp, event))
    } else if line.contains("*TM2*") {
        parse_tm2(line, node_id).map(|event| (timestamp, event))
    } else if line.contains("*TM3*") {
        parse_tm3(line, node_id).map(|event| (timestamp, event))
    } else if line.contains("*TM4*") {
        parse_tm4(line, node_id).map(|event| (timestamp, event))
    } else {
        None
    }
}

/// Extract timestamp from the beginning of a log line.
fn parse_timestamp(line: &str) -> Option<DateTime<Utc>> {
    // Timestamp format: 2025-10-23T18:00:00Z (20 chars)
    if line.len() < 20 {
        return None;
    }

    let timestamp_str = &line[..20];
    DateTime::parse_from_rfc3339(timestamp_str).ok().map(|dt| dt.with_timezone(&Utc))
}

/// Extract node ID from the last [xxxx] pattern in the line.
fn extract_node_id(line: &str) -> Option<u32> {
    let start = line.rfind('[')?;
    let end = line[start..].find(']')? + start;
    let id_str = &line[start + 1..end];
    id_str.parse().ok()
}

/// Parse *TM1* - Packet transmitted.
fn parse_tm1(line: &str, node_id: u32) -> Option<LogEvent> {
    let message_type = extract_field_u8(line, "type:")?;
    let sequence = extract_field_u32(line, "sequence:");
    let length = extract_field_usize(line, "length:").unwrap_or(0);
    let (packet_index, packet_count) = extract_packet_info(line).unwrap_or((1, 1));

    Some(LogEvent::SendPacket {
        node_id,
        message_type,
        sequence,
        packet_index,
        packet_count,
        length,
    })
}

/// Parse *TM2* - Packet received.
fn parse_tm2(line: &str, node_id: u32) -> Option<LogEvent> {
    let sender_id = extract_field_u32(line, "sender:")?;
    let message_type = extract_field_u8(line, "type:")?;
    let sequence = extract_field_u32(line, "sequence:");
    let length = extract_field_usize(line, "length:").unwrap_or(0);
    let (packet_index, packet_count) = extract_packet_info(line).unwrap_or((1, 1));
    let link_quality = extract_field_u8(line, "link quality:").unwrap_or(0);

    Some(LogEvent::ReceivePacket {
        node_id,
        sender_id,
        message_type,
        sequence,
        packet_index,
        packet_count,
        length,
        link_quality,
    })
}

/// Parse *TM3* - Start measurement.
fn parse_tm3(line: &str, node_id: u32) -> Option<LogEvent> {
    let sequence = extract_field_u32(line, "sequence:")?;

    Some(LogEvent::StartMeasurement { node_id, sequence })
}

/// Parse *TM4* - Full message received.
fn parse_tm4(line: &str, node_id: u32) -> Option<LogEvent> {
    let sender_id = extract_field_u32(line, "sender:")?;
    let message_type = extract_field_u8(line, "type:")?;
    let sequence = extract_field_u32(line, "sequence:")?;
    let length = extract_field_usize(line, "length:").unwrap_or(0);

    Some(LogEvent::ReceivedFullMessage {
        node_id,
        sender_id,
        message_type,
        sequence,
        length,
    })
}

/// Extract a u8 field value from the line.
fn extract_field_u8(line: &str, field_name: &str) -> Option<u8> {
    let pos = line.find(field_name)?;
    let start = pos + field_name.len();
    let remaining = &line[start..].trim_start();

    // Find the end of the number (comma, space, or end of line)
    let end = remaining
        .find(|c: char| c == ',' || c == ' ' || c == '\n' || c == '\r')
        .unwrap_or(remaining.len());

    remaining[..end].parse().ok()
}

/// Extract a u32 field value from the line.
fn extract_field_u32(line: &str, field_name: &str) -> Option<u32> {
    let pos = line.find(field_name)?;
    let start = pos + field_name.len();
    let remaining = &line[start..].trim_start();

    let end = remaining
        .find(|c: char| c == ',' || c == ' ' || c == '\n' || c == '\r')
        .unwrap_or(remaining.len());

    remaining[..end].parse().ok()
}

/// Extract a usize field value from the line.
fn extract_field_usize(line: &str, field_name: &str) -> Option<usize> {
    let pos = line.find(field_name)?;
    let start = pos + field_name.len();
    let remaining = &line[start..].trim_start();

    let end = remaining
        .find(|c: char| c == ',' || c == ' ' || c == '\n' || c == '\r')
        .unwrap_or(remaining.len());

    remaining[..end].parse().ok()
}

/// Extract packet index and count from "packet: X/Y" format.
fn extract_packet_info(line: &str) -> Option<(u8, u8)> {
    let pos = line.find("packet:")?;
    let start = pos + "packet:".len();
    let remaining = &line[start..].trim_start();

    let slash_pos = remaining.find('/')?;
    let end = remaining
        .find(|c: char| c == ',' || c == ' ' || c == '\n' || c == '\r')
        .unwrap_or(remaining.len());

    let index: u8 = remaining[..slash_pos].parse().ok()?;
    let count: u8 = remaining[slash_pos + 1..end].parse().ok()?;

    Some((index, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_parse_tm1() {
        let line = "2025-10-23T18:00:01Z moonblokz_radio_lib::radio_devices::rp_lora_sx1262: [3094] *TM1* Packet transmitted: type: 6, sequence: 30940779, length: 215, packet: 1/10";
        let result = parse_log_line(line);
        assert!(result.is_some());

        let (timestamp, event) = result.unwrap();
        assert_eq!(timestamp.year(), 2025);

        if let LogEvent::SendPacket {
            node_id,
            message_type,
            sequence,
            packet_index,
            packet_count,
            length,
        } = event
        {
            assert_eq!(node_id, 3094);
            assert_eq!(message_type, 6);
            assert_eq!(sequence, Some(30940779));
            assert_eq!(packet_index, 1);
            assert_eq!(packet_count, 10);
            assert_eq!(length, 215);
        } else {
            panic!("Expected SendPacket event");
        }
    }

    #[test]
    fn test_parse_tm2() {
        let line = "2025-10-23T18:00:01Z moonblokz_radio_lib::radio_devices::rp_lora_sx1262:[3094] *TM2* Packet received: sender: 3093, type: 6, sequence: 30940779, length: 215, packet: 1/10, link quality: 26";
        let result = parse_log_line(line);
        assert!(result.is_some());

        let (_, event) = result.unwrap();
        if let LogEvent::ReceivePacket {
            node_id,
            sender_id,
            message_type,
            link_quality,
            ..
        } = event
        {
            assert_eq!(node_id, 3094);
            assert_eq!(sender_id, 3093);
            assert_eq!(message_type, 6);
            assert_eq!(link_quality, 26);
        } else {
            panic!("Expected ReceivePacket event");
        }
    }

    #[test]
    fn test_parse_tm3() {
        let line = "2025-10-23T18:00:00Z [3094] *TM3* Start measurement: sequence: 321312";
        let result = parse_log_line(line);
        assert!(result.is_some());

        let (_, event) = result.unwrap();
        if let LogEvent::StartMeasurement { node_id, sequence } = event {
            assert_eq!(node_id, 3094);
            assert_eq!(sequence, 321312);
        } else {
            panic!("Expected StartMeasurement event");
        }
    }

    #[test]
    fn test_parse_tm4() {
        let line = "2025-10-23T18:00:05Z [3094] *TM4* Routing message to incoming queue: sender: 3093, type: 6, length: 2000, sequence: 321312";
        let result = parse_log_line(line);
        assert!(result.is_some());

        let (_, event) = result.unwrap();
        if let LogEvent::ReceivedFullMessage {
            node_id,
            sender_id,
            message_type,
            sequence,
            length,
        } = event
        {
            assert_eq!(node_id, 3094);
            assert_eq!(sender_id, 3093);
            assert_eq!(message_type, 6);
            assert_eq!(sequence, 321312);
            assert_eq!(length, 2000);
        } else {
            panic!("Expected ReceivedFullMessage event");
        }
    }

    #[test]
    fn test_parse_unparseable_line() {
        let line = "This is not a valid log line";
        let result = parse_log_line(line);
        assert!(result.is_none());
    }
}
