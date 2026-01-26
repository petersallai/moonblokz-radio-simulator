//! Connection matrix parsing utilities shared between simulation and analyzer.

use embassy_time::Instant;
use std::collections::HashMap;

/// Decoded connection matrix for a requester node.
#[derive(Debug, Clone)]
pub struct ConnectionMatrix {
    pub requester_node_id: u32,
    pub node_count: usize,
    /// Timestamp from the log line that ended the matrix.
    pub timestamp: Instant,
    /// Node IDs in row/column order.
    pub node_ids: Vec<u32>,
    /// Link quality values [row][col], 0-63.
    pub values: Vec<Vec<u8>>,
}

/// Stateful parser for *TM9* connection matrix logs.
#[derive(Debug, Default)]
pub struct ConnectionMatrixParser {
    active_requester: Option<u32>,
    node_count: Option<usize>,
    row_buffers: HashMap<u32, Vec<u8>>,
    row_order: Vec<u32>,
}

impl ConnectionMatrixParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.active_requester = None;
        self.node_count = None;
        self.row_buffers.clear();
        self.row_order.clear();
    }

    /// Handle a single log content line (after the [node_id] prefix).
    /// Returns a completed ConnectionMatrix when an end line is parsed.
    pub fn handle_line(
        &mut self,
        requester_node_id: u32,
        timestamp: Instant,
        content: &str,
    ) -> Option<ConnectionMatrix> {
        if !content.contains("*TM9*") {
            return None;
        }

        if let Some(node_count) = parse_start_line(content) {
            self.reset();
            self.active_requester = Some(requester_node_id);
            self.node_count = Some(node_count);
            return None;
        }

        if let Some((row_node_id, values_chunk)) = parse_row_line(content) {
            if self.active_requester != Some(requester_node_id) {
                return None;
            }
            if self.node_count.is_none() {
                return None;
            }

            let decoded = decode_chunk(values_chunk)?;
            let entry = self.row_buffers.entry(row_node_id).or_insert_with(|| {
                self.row_order.push(row_node_id);
                Vec::new()
            });
            entry.extend(decoded);
            return None;
        }

        if is_end_line(content) {
            if self.active_requester != Some(requester_node_id) {
                self.reset();
                return None;
            }
            let node_count = self.node_count.unwrap_or(0);
            if node_count == 0 {
                self.reset();
                return None;
            }

            let mut values: Vec<Vec<u8>> = Vec::with_capacity(self.row_order.len());
            for row_id in &self.row_order {
                let row = match self.row_buffers.get(row_id) {
                    Some(r) => r,
                    None => {
                        self.reset();
                        return None;
                    }
                };
                if row.len() != node_count {
                    self.reset();
                    return None;
                }
                values.push(row.clone());
            }

            let matrix = ConnectionMatrix {
                requester_node_id,
                node_count,
                timestamp,
                node_ids: self.row_order.clone(),
                values,
            };
            self.reset();
            return Some(matrix);
        }

        None
    }
}

fn parse_start_line(content: &str) -> Option<usize> {
    let marker = "Logging Connection Matrix, node_count:";
    let idx = content.find(marker)?;
    let tail = &content[idx + marker.len()..];
    tail.trim().parse::<usize>().ok()
}

fn parse_row_line(content: &str) -> Option<(u32, &str)> {
    let marker = "connection_matrix_row:";
    let idx = content.find(marker)?;
    let tail = &content[idx + marker.len()..];
    let mut parts = tail.splitn(2, ", values:");
    let row_str = parts.next()?.trim();
    let values = parts.next()?.trim();
    let row_id = row_str.parse::<u32>().ok()?;
    Some((row_id, values))
}

fn is_end_line(content: &str) -> bool {
    content.contains("Logging Connection Matrix ended")
}

fn decode_chunk(values: &str) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(values.len());
    for c in values.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => 26 + (c - b'a'),
            b'0'..=b'9' => 52 + (c - b'0'),
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        decoded.push(v);
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_values() {
        let v = decode_chunk("Aa0-_").unwrap();
        assert_eq!(v, vec![0, 26, 52, 62, 63]);
    }
}
