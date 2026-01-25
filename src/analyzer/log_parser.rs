//! Parse individual log lines and extract structured `LogEvent` data.
//!
//! Supports the following log line formats:
//! - *TM1*: Packet transmitted
//! - *TM2*: Packet received
//! - *TM3*: Start measurement
//! - *TM4*: Full message received
//! - *TM5*: Packet CRC mismatch (corrupted packet)
//! - *TM6*: AddBlock message fully received
//! - *TM7*: AddBlock message sent
//! - *TM8*: Version information

use super::types::{LogEvent, RawLogLine};
use crate::simulation::types::LogLevel;
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
    } else if line.contains("*TM5*") || line.contains("TM5 CRC mismatch") {
        parse_tm5(line, node_id).map(|event| (timestamp, event))
    } else if line.contains("*TM6*") {
        parse_tm6(line, node_id).map(|event| (timestamp, event))
    } else if line.contains("*TM7*") {
        parse_tm7(line, node_id).map(|event| (timestamp, event))
    } else if line.contains("*TM8*") {
        parse_tm8(line, node_id).map(|event| (timestamp, event))
    } else {
        None
    }
}

/// Parse a raw log line for the Log Stream tab.
///
/// Extracts timestamp, node ID, log content, and log level from any log line
/// that has a `[node_id]` pattern. This captures all log lines, not just
/// structured *TM* events.
///
/// # Parameters
///
/// * `line` - A single log line to parse
///
/// # Returns
///
/// `Some((node_id, raw_log_line))` if the line contains a node ID, `None` otherwise.
pub fn parse_raw_log_line(line: &str) -> Option<(u32, RawLogLine)> {
    let timestamp = parse_timestamp(line)?;
    let (node_id, bracket_end) = extract_node_id_with_position(line)?;

    // Extract the content after the [node_id] bracket
    let content = line[bracket_end + 1..].trim().to_string();

    // Determine log level from content or default to Info
    let level = extract_log_level(line);

    Some((
        node_id,
        RawLogLine {
            timestamp,
            content,
            level,
        },
    ))
}

/// Extract node ID and its ending position from a [xxxx] pattern in the line.
///
/// Finds all bracket patterns and returns the first one that parses as a valid u32,
/// along with the position of the closing bracket.
fn extract_node_id_with_position(line: &str) -> Option<(u32, usize)> {
    let mut search_start = 0;
    while let Some(start) = line[search_start..].find('[') {
        let abs_start = search_start + start;
        if let Some(end_offset) = line[abs_start..].find(']') {
            let abs_end = abs_start + end_offset;
            let id_str = &line[abs_start + 1..abs_end];
            if let Ok(node_id) = id_str.parse::<u32>() {
                return Some((node_id, abs_end));
            }
        }
        search_start = abs_start + 1;
    }
    None
}

/// Extract log level from a log line.
///
/// Looks for standard log level indicators (ERROR, WARN, INFO, DEBUG, TRACE).
/// Handles multiple log formats:
/// - Simulator: `[timestamp LEVEL  module::path] [node_id] message`
/// - Real device: `timestamp:[LEVEL] module: [node_id] message`
fn extract_log_level(line: &str) -> LogLevel {
    // Check for log level markers in the line
    let upper = line.to_uppercase();

    // Look for level patterns in various formats:
    // - "Z LEVEL " (simulator format after timestamp)
    // - ":[LEVEL]" (real device format with brackets)
    // - " LEVEL " or "LEVEL:" (generic patterns)
    if upper.contains(" ERROR ")
        || upper.contains("Z ERROR ")
        || upper.contains(":[ERROR]")
        || upper.contains("ERROR:")
    {
        LogLevel::Error
    } else if upper.contains(" WARN ")
        || upper.contains("Z WARN ")
        || upper.contains(":[WARN]")
        || upper.contains("WARN:")
    {
        LogLevel::Warn
    } else if upper.contains(" DEBUG ")
        || upper.contains("Z DEBUG ")
        || upper.contains(":[DEBUG]")
        || upper.contains("DEBUG:")
    {
        LogLevel::Debug
    } else if upper.contains(" TRACE ")
        || upper.contains("Z TRACE ")
        || upper.contains(":[TRACE]")
        || upper.contains("TRACE:")
    {
        LogLevel::Trace
    } else if upper.contains(" INFO ")
        || upper.contains("Z INFO ")
        || upper.contains(":[INFO]")
        || upper.contains("INFO:")
    {
        LogLevel::Info
    } else {
        // Default to Info for lines without explicit level
        LogLevel::Info
    }
}

/// Extract timestamp from the beginning of a log line.
///
/// Handles multiple timestamp formats:
/// - Short: `2025-10-23T18:00:00Z` (20 chars)
/// - Long:  `2026-01-06T09:14:34.900912254+00:00` (35 chars with nanoseconds)
/// - Short with separator: `2026-01-09T17:53:55Z:...` (Z followed by colon)
fn parse_timestamp(line: &str) -> Option<DateTime<Utc>> {
    if line.len() < 20 {
        return None;
    }

    // Try to find the end of the timestamp by looking for common delimiters
    // Handle format "2026-01-09T17:53:55Z:..." where Z is followed by colon
    // Also handle "timestamp:[node_id]" and "timestamp message" formats
    let z_colon = line.find("Z:").map(|p| p + 1);
    let space_pos = line.find(' ');
    let colon_bracket = line.find(":[");

    let timestamp_end = z_colon
        .or_else(|| match (space_pos, colon_bracket) {
            (Some(space), Some(cb)) if space < cb => Some(space),
            (Some(space), None) => Some(space),
            (None, Some(cb)) => Some(cb),
            (Some(space), Some(_)) => Some(space),
            (None, None) => None,
        })
        .unwrap_or(line.len().min(35));

    let timestamp_str = &line[..timestamp_end];
    DateTime::parse_from_rfc3339(timestamp_str)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Extract node ID from a [xxxx] pattern in the line.
///
/// Finds all bracket patterns and returns the first one that parses as a valid u32.
/// This handles cases where the log content itself may contain brackets.
fn extract_node_id(line: &str) -> Option<u32> {
    // Find all bracket patterns and try to parse each as a node ID
    let mut search_start = 0;
    while let Some(start) = line[search_start..].find('[') {
        let abs_start = search_start + start;
        if let Some(end_offset) = line[abs_start..].find(']') {
            let abs_end = abs_start + end_offset;
            let id_str = &line[abs_start + 1..abs_end];
            if let Ok(node_id) = id_str.parse::<u32>() {
                return Some(node_id);
            }
        }
        search_start = abs_start + 1;
    }
    None
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
    log::info!("Parsing TM3 line: {}", line); // --- IGNORE ---
    let sequence = extract_field_u32(line, "sequence:")?;
    log::info!("Extracted sequence: {}", sequence); // --- IGNORE ---
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

/// Parse *TM5* - Packet CRC mismatch.
///
/// Log format:
/// ```text
/// [{nodeid}] TM5 CRC mismatch: received=XXXX, calculated=XXXX. link quality: XX. Dropping packet.
/// ```
fn parse_tm5(line: &str, node_id: u32) -> Option<LogEvent> {
    let link_quality = extract_field_u8(line, "link quality:").unwrap_or(0);

    Some(LogEvent::PacketCrcError {
        node_id,
        link_quality,
    })
}

/// Parse *TM6* - AddBlock message fully received.
///
/// Log format:
/// ```text
/// *TM6* Received AddBlock message: sender: {sender_id}, sequence: {sequence}, length: {length}
/// ```
fn parse_tm6(line: &str, node_id: u32) -> Option<LogEvent> {
    let sender_id = extract_field_u32(line, "sender:")?;
    let sequence = extract_field_u32(line, "sequence:")?;
    let length = extract_field_usize(line, "length:").unwrap_or(0);

    Some(LogEvent::AddBlockReceived {
        node_id,
        sender_id,
        sequence,
        length,
    })
}

/// Parse *TM7* - AddBlock message sent.
///
/// Log format:
/// ```text
/// *TM7* Sending AddBlock: sender: {sender_id}, sequence: {sequence}, length: {length}
/// ```
fn parse_tm7(line: &str, node_id: u32) -> Option<LogEvent> {
    let sender_id = extract_field_u32(line, "sender:")?;
    let sequence = extract_field_u32(line, "sequence:")?;
    let length = extract_field_usize(line, "length:").unwrap_or(0);

    Some(LogEvent::AddBlockSent {
        node_id,
        sender_id,
        sequence,
        length,
    })
}

/// Extract a u8 field value from the line.
fn extract_field_u8(line: &str, field_name: &str) -> Option<u8> {
    let pos = line.find(field_name)?;
    let start = pos + field_name.len();
    let remaining = &line[start..].trim_start();

    // Find the end of the number (comma, space, period, or end of line)
    let end = remaining
        .find(|c: char| c == ',' || c == ' ' || c == '.' || c == '\n' || c == '\r')
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

/// Parse *TM8* - Version information.
///
/// Log format:
/// ```text
/// *TM8* probe_version: 27, node_version: 5
/// ```
fn parse_tm8(line: &str, node_id: u32) -> Option<LogEvent> {
    let probe_version = extract_field_u8(line, "probe_version:")?;
    let node_version = extract_field_u8(line, "node_version:")?;

    Some(LogEvent::VersionInfo {
        node_id,
        probe_version,
        node_version,
    })
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

    #[test]
    fn test_parse_tm5_crc_mismatch() {
        let line = "2025-10-23T18:00:01Z [3094] TM5 CRC mismatch: received=AB12, calculated=CD34. link quality: 15. Dropping packet.";
        let result = parse_log_line(line);
        assert!(result.is_some());

        let (_, event) = result.unwrap();
        if let LogEvent::PacketCrcError {
            node_id,
            link_quality,
        } = event
        {
            assert_eq!(node_id, 3094);
            assert_eq!(link_quality, 15);
        } else {
            panic!("Expected PacketCrcError event");
        }
    }
}
