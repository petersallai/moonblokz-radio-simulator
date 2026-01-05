# Right Panel Tabbed View Specification

## Overview

This specification describes the addition of a tabbed interface to the right inspector panel. The current title "Radio stream for #<node_id>" will be replaced with a tab bar containing three tabs: **Radio Stream**, **Message Stream**, and **Log Stream**.

## Current State Analysis

### Existing Components
- [src/ui/right_panel.rs](src/ui/right_panel.rs): Renders the fixed-width right panel with node details and message table
- [src/ui/app_state.rs](src/ui/app_state.rs): Central application state with `NodeInfo` containing message history
- [src/ui/mod.rs](src/ui/mod.rs): Defines `NodeInfo` struct with `messages: Vec<NodeMessage>`
- [src/simulation/types.rs](src/simulation/types.rs): Defines `NodeMessage` struct for radio packets
- [src/simulation/node_task.rs](src/simulation/node_task.rs): Per-node task handling message receipt and transmission
- [src/analyzer/log_parser.rs](src/analyzer/log_parser.rs): Parses log lines with TM1-TM4 patterns

### Current Data Flow
1. Simulation mode: `node_task` → `NodeOutputMessage` → `network.rs` → `UIRefreshState::NodeInfo` → UI
2. Analyzer mode: `log_parser` → `LogEvent` → `analyzer_task` → `UIRefreshState::NodeInfo` → UI

---

## Tab 1: Radio Stream (Existing Functionality)

### Description
The existing radio packet stream table, renamed from the current implementation.

### Columns (unchanged)
| Column | Description |
|--------|-------------|
| Time | Virtual time (simulation) or HH:MM:SS (analyzer modes) |
| From | "Sent msg" for outgoing, "#ID" for incoming |
| Type | Message type name (Add block, Echo, etc.) |
| Sequence | Sequence number for AddBlock/RequestBlockPart, "-" otherwise |
| Packet | "index/total" packet sequence |
| Size | Packet size in bytes |
| LQ | Link quality (0-63), color-coded |

### Color Coding (unchanged)
- **Yellow**: Messages sent by this node
- **Green**: Messages received from other nodes
- **Red background**: Collision detected (packet lost)

---

## Tab 2: Message Stream (New)

### Description
Shows only fully assembled messages (complete AddBlock messages for now, extensible to other message types later). This differs from Radio Stream which shows individual radio packets.

### Log Patterns to Parse
```
*TM6* Received AddBlock message: sender: {sender_id}, sequence: {sequence}, length: {length}
*TM7* Sending AddBlock: sender: {sender_id}, sequence: {sequence}, length: {length}
```

### Columns
| Column | Width | Description |
|--------|-------|-------------|
| Time | 60px | Virtual time (simulation) or HH:MM:SS (analyzer modes) |
| From | 70px | "Sent" for outgoing (*TM7*), "#{sender_id}" for incoming (*TM6*) |
| Type | remainder | Message type name ("AddBlock" initially) |
| Sequence | 80px | Sequence number from the log pattern |

### Color Coding
- **Yellow**: Messages sent by this node (*TM7*)
- **Green**: Messages received from other nodes (*TM6*)

### Data Structure

#### New struct: `FullMessage`
Location: `src/simulation/types.rs`

```rust
#[derive(Debug, Clone)]
pub struct FullMessage {
    /// Timestamp when the message was fully received/sent.
    pub timestamp: embassy_time::Instant,
    /// Message type (e.g., MessageType::AddBlock = 6).
    pub message_type: u8,
    /// Sender node ID. If equals this node's ID, the message was sent by self.
    pub sender_node: u32,
    /// Sequence number of the message.
    pub sequence: u32,
    /// Total message payload length.
    pub length: usize,
    /// Whether this is an outgoing (sent) or incoming (received) message.
    pub is_outgoing: bool,
}
```

#### NodeInfo Modifications
Location: `src/ui/mod.rs`

```rust
pub struct NodeInfo {
    pub node_id: u32,
    /// Radio packet history (renamed from 'messages').
    pub radio_packets: Vec<NodeMessage>,
    /// Full message history (AddBlock messages, etc.).
    pub messages: Vec<FullMessage>,
    /// Log lines for this node (last 1000 entries).
    pub log_lines: VecDeque<LogLine>,
}
```

#### Node struct Modifications
Location: `src/simulation/types.rs`

```rust
pub struct Node {
    // ... existing fields ...
    #[serde(skip)]
    pub node_messages: VecDeque<NodeMessage>,  // Rename to node_radio_packets
    #[serde(skip)]
    pub full_messages: VecDeque<FullMessage>,  // New field for complete messages
    #[serde(skip)]
    pub log_lines: VecDeque<LogLine>,          // New field for log stream
}
```

### Simulation Mode Integration

#### Location 1: Received AddBlock (node_task.rs ~line 83)
In `handle_add_block_message()`, after successful deduplication:

```rust
// After: self.arrived_messages.insert(sequence, msg.clone());
// Add notification for received full message
let _ = self.out_tx.send(NodeOutputMessage {
    node_id: self.node_id,
    payload: NodeOutputPayload::FullMessageReceived {
        message_type: MessageType::AddBlock as u8,
        sender_node: msg.sender_node_id(),
        sequence,
        length: msg.payload().len(),
    },
}).await;
```

#### Location 2: Sending AddBlock (node_task.rs ~line 182)
In `handle_input_command()` when sending AddBlock:

```rust
NodeInputMessage::SendMessage(msg) => {
    if msg.message_type() == MessageType::AddBlock as u8 {
        let sequence = Self::extract_sequence_from_payload(msg.payload());
        self.arrived_messages.insert(sequence, msg.clone());
        
        // Add notification for sent full message
        let _ = self.out_tx.send(NodeOutputMessage {
            node_id: self.node_id,
            payload: NodeOutputPayload::FullMessageSent {
                message_type: MessageType::AddBlock as u8,
                sender_node: self.node_id,
                sequence,
                length: msg.payload().len(),
            },
        }).await;
    }
    let _ = self.manager.send_message(msg);
}
```

#### New NodeOutputPayload variants
Location: `src/simulation/types.rs`

```rust
pub enum NodeOutputPayload {
    // ... existing variants ...
    FullMessageReceived {
        message_type: u8,
        sender_node: u32,
        sequence: u32,
        length: usize,
    },
    FullMessageSent {
        message_type: u8,
        sender_node: u32,
        sequence: u32,
        length: usize,
    },
}
```

### Analyzer Mode Integration

#### New Log Patterns
Add to `src/analyzer/log_parser.rs`:

```rust
// *TM6* - AddBlock received (full message)
// *TM7* - AddBlock sent (full message)

pub fn parse_log_line(line: &str) -> Option<(DateTime<Utc>, LogEvent)> {
    // ... existing code ...
    } else if line.contains("*TM6*") {
        parse_tm6(line, node_id).map(|event| (timestamp, event))
    } else if line.contains("*TM7*") {
        parse_tm7(line, node_id).map(|event| (timestamp, event))
    } else {
        None
    }
}

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
```

#### New LogEvent Variants
Location: `src/analyzer/types.rs`

```rust
pub enum LogEvent {
    // ... existing variants ...
    /// *TM6* - AddBlock fully received.
    AddBlockReceived {
        node_id: u32,
        sender_id: u32,
        sequence: u32,
        length: usize,
    },
    /// *TM7* - AddBlock sent.
    AddBlockSent {
        node_id: u32,
        sender_id: u32,
        sequence: u32,
        length: usize,
    },
}
```

---

## Tab 3: Log Stream (New)

### Description
Displays the last 1000 log lines for the selected node, filtered by `[node_id]` pattern in the log line.

### Data Structure

#### New struct: `LogLine`
Location: `src/simulation/types.rs` or `src/ui/mod.rs`

```rust
#[derive(Debug, Clone)]
pub struct LogLine {
    /// Timestamp of the log entry.
    pub timestamp: embassy_time::Instant,
    /// The raw log line content (without timestamp prefix).
    pub content: String,
    /// Log level (for optional color coding).
    pub level: LogLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
```

### Constants
```rust
/// Maximum number of log lines to retain per node.
pub const NODE_LOG_LINES_CAPACITY: usize = 1000;
```

### Display Format
- Single column displaying the full log line content
- Color coding by log level:
  - **White**: INFO
  - **Yellow**: WARN  
  - **Red**: ERROR
  - **Gray**: DEBUG/TRACE

### Simulation Mode: Log Capture

#### Approach
Capture stdout from `moonblokz-radio-lib` by redirecting the log output.

#### Implementation Options

**Option A: Custom Log Subscriber (Recommended)**

Create a custom `tracing` or `log` subscriber that captures log events:

```rust
// src/simulation/log_capture.rs

use std::sync::mpsc;
use log::{Log, Metadata, Record};

pub struct LogCapture {
    sender: mpsc::Sender<CapturedLog>,
}

pub struct CapturedLog {
    pub node_id: Option<u32>,
    pub level: log::Level,
    pub message: String,
    pub timestamp: embassy_time::Instant,
}

impl Log for LogCapture {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.target().starts_with("moonblokz_radio_lib")
    }
    
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let message = format!("{}", record.args());
            // Extract node_id from [xxxx] pattern
            let node_id = extract_node_id(&message);
            let _ = self.sender.send(CapturedLog {
                node_id,
                level: record.level(),
                message,
                timestamp: embassy_time::Instant::now(),
            });
        }
    }
    
    fn flush(&self) {}
}
```

**Option B: Log Tee (Alternative)**

Install a custom logger that forwards to both the original logger and a capture buffer.

#### Integration with Network Task

In `network.rs`, add a receiver for captured logs:

```rust
// In the main event loop, add handling for captured logs
loop {
    match select4(
        node_output_rx.receive(),
        ui_command_rx.receive(),
        Timer::at(next_event),
        log_capture_rx.recv(),  // New: captured log messages
    ).await {
        // ... existing handlers ...
        Fourth(captured_log) => {
            if let Some(node_id) = captured_log.node_id {
                if let Some(node) = nodes_map.get_mut(&node_id) {
                    node.push_log_line(LogLine {
                        timestamp: captured_log.timestamp,
                        content: captured_log.message,
                        level: captured_log.level.into(),
                    });
                }
            }
        }
    }
}
```

### Analyzer Modes: Log Processing

For real-time tracking and log visualization modes, all log lines (not just TM* events) should be captured.

#### Modifications to log_loader.rs

Store raw log lines along with parsed events:

```rust
pub struct RawLogLine {
    pub timestamp: DateTime<Utc>,
    pub node_id: Option<u32>,
    pub content: String,
    pub level: LogLevel,
}

/// Extract all log lines for a node, not just parsed events.
pub fn extract_raw_log_line(line: &str) -> Option<RawLogLine> {
    let timestamp = parse_timestamp(line)?;
    let node_id = extract_node_id(line);
    let level = extract_log_level(line);
    let content = extract_content_after_timestamp(line);
    
    Some(RawLogLine {
        timestamp,
        node_id,
        content,
        level,
    })
}
```

#### Modifications to analyzer_task

Process all log lines and store them per-node:

```rust
// In analyzer_task, add storage for raw log lines
pub struct AnalyzerState {
    // ... existing fields ...
    pub node_log_lines: HashMap<u32, VecDeque<RawLogLine>>,
}

// When processing log lines:
if let Some(raw_line) = extract_raw_log_line(&line) {
    if let Some(node_id) = raw_line.node_id {
        let history = state.node_log_lines.entry(node_id).or_insert_with(VecDeque::new);
        if history.len() >= NODE_LOG_LINES_CAPACITY {
            history.pop_front();
        }
        history.push_back(raw_line);
    }
}
```

---

## UI Implementation

### Tab State
Location: `src/ui/app_state.rs`

```rust
/// Currently selected tab in the right panel inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorTab {
    #[default]
    RadioStream,
    MessageStream,
    LogStream,
}

pub struct AppState {
    // ... existing fields ...
    /// Currently selected inspector tab.
    pub inspector_tab: InspectorTab,
}
```

### Tab Bar Rendering
Location: `src/ui/right_panel.rs`

Replace the heading with a tab bar:

```rust
// Replace this:
// ui.heading(format!("Radio stream for #{}", node_info.node_id));

// With:
ui.horizontal(|ui| {
    ui.selectable_value(&mut state.inspector_tab, InspectorTab::RadioStream, "Radio Stream");
    ui.selectable_value(&mut state.inspector_tab, InspectorTab::MessageStream, "Message Stream");
    ui.selectable_value(&mut state.inspector_tab, InspectorTab::LogStream, "Log Stream");
});
ui.separator();

// Then render content based on selected tab:
match state.inspector_tab {
    InspectorTab::RadioStream => {
        render_radio_stream_table(ui, state, node_info, table_h);
    }
    InspectorTab::MessageStream => {
        render_message_stream_table(ui, state, node_info, table_h);
    }
    InspectorTab::LogStream => {
        render_log_stream(ui, state, node_info, table_h);
    }
}
```

### New Render Functions

#### render_message_stream_table

```rust
fn render_message_stream_table(ui: &mut egui::Ui, state: &AppState, node_info: &NodeInfo, table_h: f32) {
    use egui_extras::{Column, TableBuilder};
    
    let row_height = ui.text_style_height(&egui::TextStyle::Body) * 1.3;
    let header_h = row_height;
    let body_min_h = (table_h - header_h).max(0.0);
    
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .vscroll(true)
        .min_scrolled_height(body_min_h)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(60.0).at_least(40.0))   // Time
        .column(Column::initial(70.0).at_least(50.0))   // From
        .column(Column::remainder())                      // Type
        .column(Column::initial(80.0).at_least(60.0))   // Sequence
        .header(row_height, |mut header| {
            header.col(|ui| { ui.strong("Time"); });
            header.col(|ui| { ui.strong("From"); });
            header.col(|ui| { ui.strong("Type"); });
            header.col(|ui| { ui.strong("Sequence"); });
        })
        .body(|body| {
            let row_count = node_info.messages.len();
            body.rows(row_height, row_count, |mut row| {
                let row_index = row.index();
                let msg_idx = row_count - 1 - row_index;  // Newest first
                let msg = &node_info.messages[msg_idx];
                
                let is_outgoing = msg.is_outgoing;
                let row_color = if is_outgoing { Color32::YELLOW } else { Color32::LIGHT_GREEN };
                let from_string = if is_outgoing { 
                    "Sent".to_string() 
                } else { 
                    format!("#{}", msg.sender_node) 
                };
                let type_string = match msg.message_type {
                    6 => "AddBlock",
                    _ => "Unknown",
                };
                
                // Format time based on operating mode (same as radio stream)
                let time_string = format_timestamp(msg.timestamp, state);
                
                row.col(|ui| { ui.colored_label(row_color, &time_string); });
                row.col(|ui| { ui.colored_label(row_color, &from_string); });
                row.col(|ui| { ui.colored_label(row_color, type_string); });
                row.col(|ui| { ui.colored_label(row_color, format!("#{}", msg.sequence)); });
            });
        });
}
```

#### render_log_stream

```rust
fn render_log_stream(ui: &mut egui::Ui, state: &AppState, node_info: &NodeInfo, table_h: f32) {
    use egui_extras::{Column, TableBuilder};
    
    let row_height = ui.text_style_height(&egui::TextStyle::Body) * 1.3;
    let header_h = row_height;
    let body_min_h = (table_h - header_h).max(0.0);
    
    TableBuilder::new(ui)
        .striped(true)
        .vscroll(true)
        .min_scrolled_height(body_min_h)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(60.0).at_least(40.0))   // Time
        .column(Column::remainder())                      // Log content
        .header(row_height, |mut header| {
            header.col(|ui| { ui.strong("Time"); });
            header.col(|ui| { ui.strong("Log"); });
        })
        .body(|body| {
            let row_count = node_info.log_lines.len();
            body.rows(row_height, row_count, |mut row| {
                let row_index = row.index();
                let log_idx = row_count - 1 - row_index;  // Newest first
                let log_line = &node_info.log_lines[log_idx];
                
                let color = match log_line.level {
                    LogLevel::Error => Color32::RED,
                    LogLevel::Warn => Color32::YELLOW,
                    LogLevel::Info => Color32::WHITE,
                    LogLevel::Debug | LogLevel::Trace => Color32::GRAY,
                };
                
                let time_string = format_timestamp(log_line.timestamp, state);
                
                row.col(|ui| { ui.colored_label(color, &time_string); });
                row.col(|ui| { ui.colored_label(color, &log_line.content); });
            });
        });
}
```

---

## Rename Refactoring Summary

### Files to Modify

| File | Change |
|------|--------|
| `src/ui/mod.rs` | Rename `NodeInfo.messages` → `NodeInfo.radio_packets`, add `messages: Vec<FullMessage>`, add `log_lines: VecDeque<LogLine>` |
| `src/simulation/types.rs` | Rename `Node.node_messages` → `Node.node_radio_packets`, add `full_messages`, add `log_lines` |
| `src/simulation/network.rs` | Update all references to `node_messages` → `node_radio_packets` |
| `src/ui/right_panel.rs` | Update references to `messages` → `radio_packets` in Radio Stream table |
| `src/analyzer/task.rs` | Update `NodeInfo` construction |

### Search Pattern for Rename
```bash
grep -rn "node_messages\|\.messages" src/
```

---

## UIRefreshState Extensions

Location: `src/ui/mod.rs`

```rust
pub enum UIRefreshState {
    // ... existing variants ...
    
    /// A full message was received by a node (e.g., complete AddBlock).
    FullMessageReceived {
        node_id: u32,
        message_type: u8,
        sender_node: u32,
        sequence: u32,
        length: usize,
    },
    
    /// A full message was sent by a node.
    FullMessageSent {
        node_id: u32,
        message_type: u8,
        sender_node: u32,
        sequence: u32,
        length: usize,
    },
    
    /// A log line for a specific node.
    NodeLogLine {
        node_id: u32,
        line: LogLine,
    },
}
```

---

## Implementation Order

1. **Phase 1: Data Structure Changes**
   - Add `FullMessage` and `LogLine` structs
   - Rename `messages` → `radio_packets` in `NodeInfo`
   - Add new fields to `Node` struct
   - Update all references

2. **Phase 2: UI Tab Framework**
   - Add `InspectorTab` enum to `AppState`
   - Replace heading with tab bar in `right_panel.rs`
   - Rename existing table function to `render_radio_stream_table`

3. **Phase 3: Message Stream (Tab 2)**
   - Add `NodeOutputPayload` variants for full messages
   - Implement message capture in `node_task.rs`
   - Handle new payloads in `network.rs`
   - Implement `render_message_stream_table`

4. **Phase 4: Log Stream - Analyzer Modes (Tab 3)**
   - Add TM6/TM7 parsing to `log_parser.rs`
   - Add raw log line storage in analyzer
   - Implement `render_log_stream`

5. **Phase 5: Log Stream - Simulation Mode**
   - Implement log capture mechanism
   - Integrate with network task
   - Test end-to-end

---

## Testing Checklist

- [ ] Radio Stream tab displays correctly (regression test)
- [ ] Tab switching works smoothly
- [ ] Message Stream shows AddBlock messages in simulation mode
- [ ] Message Stream shows AddBlock messages in analyzer modes
- [ ] Color coding (yellow/green) works for both tabs
- [ ] Log Stream displays in simulation mode
- [ ] Log Stream displays in real-time tracking mode
- [ ] Log Stream displays in log visualization mode
- [ ] Log lines are correctly filtered by node ID
- [ ] Log lines capacity limit (1000) is enforced
- [ ] Performance with 300+ nodes remains acceptable
