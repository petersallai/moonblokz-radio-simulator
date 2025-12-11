# MoonBlokz Radio Simulator - Analyzer Feature Detailed Specification

## Document Information

- **Based on**: `analyzer_draft_spec.md`
- **Date**: December 11, 2025
- **Scope**: Implementation specification for Real-time Tracking and Log Visualization modes

---

## Summary: Architectural Changes

### Overview

This specification adds two new operational modes to the MoonBlokz Radio Simulator:

1. **Real-time Tracking Mode**: Connects to a live log stream (file being appended by an external process) and visualizes network activity as it happens on real hardware.
2. **Log Visualization Mode**: Replays a previously saved log file with time-synchronized playback.

Both modes share a common log parsing and event dispatching infrastructure, but differ in:
- File reading strategy (tail-follow vs. start-from-beginning)
- Time synchronization behavior (real-time pacing vs. speed-controllable playback)
- UI controls (speed controls hidden in real-time mode, measurement button hidden in log visualization)

### Key Architectural Decisions

1. **Module Separation**: Create a new `analyzer` module parallel to `simulation` that handles log parsing, time synchronization, and event dispatching. Both modules communicate with the UI using the same channel infrastructure.

2. **Shared Scene Loading**: Extract scene loading, parsing, and validation from `simulation/network.rs` into a new `common/scene.rs` module, allowing both simulation and analyzer modes to reuse this logic.

3. **Embassy Async Task for Analyzer**: The analyzer runs its own async task (`analyzer_task`) on the Embassy executor thread, similar to `network_task`, enabling non-blocking log file reading and proper time synchronization.

4. **Extended Scene Format**: The `Scene` struct gains an optional `effective_distance` field per node (mandatory for analyzer modes, optional for simulation mode) while making `path_loss_parameters`, `lora_parameters`, and `radio_module_config` optional (mandatory only for simulation mode).

5. **Enhanced Mode Selector UI**: The mode selector screen is redesigned with two buttons per non-simulation column ("Select scene" + "Connect to stream" or "Open log file") with file selection state tracking.

6. **New UIRefreshState Variants**: Add variants for analyzer-specific events (e.g., `AnalyzerDelay`, `VisualizationEnded`).

7. **Conditional UI Rendering**: Top panel controls and right panel measurement button adapt based on the active mode.

---

## File-by-File Changes

### New Files

#### 1. `src/analyzer/mod.rs`
**Purpose**: Analyzer module root, exports public types and the main task.

```rust
//! Analyzer module for log parsing and visualization.
//!
//! Provides functionality for:
//! - Real-time tracking of live log streams
//! - Log file visualization with time-synchronized playback
//!
//! The analyzer communicates with the UI using the same channels as the simulation module.

pub mod log_loader;
pub mod log_parser;
pub mod task;
pub mod types;

pub use task::analyzer_task;
pub use types::{AnalyzerMode, LogEvent, NodePacketRecord};
```

#### 2. `src/analyzer/types.rs`
**Purpose**: Type definitions specific to the analyzer module.

**Contents**:
- `AnalyzerMode` enum: `RealtimeTracking` | `LogVisualization`
- `LogEvent` enum: Parsed log line variants
  - `SendPacket { node_id, message_type, sequence, packet_index, packet_count }`
  - `ReceivePacket { node_id, sender_id, message_type, sequence, packet_index, packet_count, link_quality }`
  - `StartMeasurement { node_id, sequence }`
  - `ReceivedFullMessage { node_id, sender_id, message_type, sequence }`
  - `Position { node_id, x, y }` (for potential future use)
- `NodePacketRecord` struct: Stores packet history for `RequestNodeInfo` responses
- `AnalyzerState` struct: Runtime state including
  - `reference_timestamp: Option<chrono::DateTime<Utc>>`
  - `reference_instant: Option<std::time::Instant>`
  - `active_measurement_id: Option<u32>`
  - `node_packet_histories: HashMap<u32, VecDeque<NodePacketRecord>>`
  - `last_processed_timestamp: Option<chrono::DateTime<Utc>>`

#### 3. `src/analyzer/log_parser.rs`
**Purpose**: Parse individual log lines and extract structured `LogEvent` data.

**Contents**:
- `parse_log_line(line: &str) -> Option<(chrono::DateTime<Utc>, LogEvent)>`
  - Extracts timestamp from line start (format: `2025-10-23T18:00:00Z`)
  - Identifies log type by marker (`*TM1*`, `*TM2*`, `*TM3*`, `*TM4*`)
  - Extracts node ID from `[xxxx]` pattern
  - Parses relevant fields (message type, sequence, sender, link quality, etc.)
  - Returns `None` for unparseable lines (graceful degradation)

**Log Line Formats**:
```
*TM1* (Send Packet):
moonblokz_radio_lib::radio_devices::rp_lora_sx1262: [3094] *TM1* Packet transmitted: type: 6, sequence: 30940779, length: 215, packet: 1/10

*TM2* (Receive Packet):
moonblokz_radio_lib::radio_devices::rp_lora_sx1262:[3094] *TM2* Packet received: sender: 3093, type: 6, sequence: 30940779, length: 215, packet: 1/10, link quality: 26

*TM3* (Start Measurement):
[3094] *TM3* Start measurement: sequence: 321312

*TM4* (Received Full Message):
[3094] *TM4* Routing message to incoming queue: sender: 3093, type: 6, length: 2000, sequence: 321312
```

#### 4. `src/analyzer/log_loader.rs`
**Purpose**: File I/O abstraction for both real-time and historical log reading.

**Contents**:
- `LogLoader` struct with methods:
  - `new(path: &str, mode: AnalyzerMode) -> Result<Self, std::io::Error>`
  - `async fn next_line(&mut self) -> Option<String>`
    - In `RealtimeTracking`: Uses tail-follow semantics (seeks to end initially, polls for new lines)
    - In `LogVisualization`: Reads sequentially from start
  - `fn is_eof(&self) -> bool` (only meaningful in LogVisualization mode)

**Implementation Notes**:
- Uses `tokio::fs` or `embassy_time::Timer` for async polling in real-time mode
- Buffer size: 8KB (typical log lines are 100-500 bytes)
- Poll interval for real-time mode: 50ms

#### 5. `src/analyzer/task.rs`
**Purpose**: Main analyzer async task that runs on the Embassy executor.

**Function Signature**:
```rust
#[embassy_executor::task]
pub async fn analyzer_task(
    mode: AnalyzerMode,
    scene_path: String,
    log_path: String,
    ui_refresh_tx: UIRefreshQueueSender,
    ui_command_rx: UICommandQueueReceiver,
)
```

**Responsibilities**:
1. Load and validate scene file using `common::scene::load_scene()`
2. Initialize UI with node positions and effective distances
3. Open log file with appropriate mode
4. Main loop:
   - Read next log line
   - Parse timestamp and event
   - Synchronize timing:
     - Set first timestamp as reference
     - Calculate delay between log timestamp and reference + elapsed
     - Wait if log is ahead; process immediately if behind (reset reference)
   - Dispatch events to UI via `UIRefreshQueue`
   - Handle `UICommand` messages (e.g., `RequestNodeInfo`, `SeekAnalyzer`)
   - In LogVisualization: Show "Visualization ended" alert on EOF
5. Track packet history per node for `RequestNodeInfo` responses
6. Update `last_processed_timestamp` for delay calculation in real-time mode

#### 6. `src/common/mod.rs`
**Purpose**: Common module root for shared functionality between simulation and analyzer.

```rust
//! Common utilities shared between simulation and analyzer modules.

pub mod scene;
```

#### 7. `src/common/scene.rs`
**Purpose**: Scene loading, parsing, and validation logic extracted from simulation.

**Contents** (moved from `simulation/network.rs` and `simulation/types.rs`):
- `Scene` struct (with modified field optionality)
- `Node` struct with new optional `effective_distance` field
- `Point`, `RectPos`, `CirclePos`, `Obstacle` types
- `PathLossParameters`, `LoraParameters`, `RadioModuleConfig` (moved from `signal_calculations.rs`)
- `load_scene(path: &str, mode: SceneMode) -> Result<Scene, SceneLoadError>`
- `validate_scene(scene: &Scene, mode: SceneMode) -> Result<(), String>`
- `SceneMode` enum: `Simulation` | `Analyzer`

**Validation Rules by Mode**:

| Field | Simulation | Analyzer |
|-------|-----------|----------|
| `path_loss_parameters` | Required | Optional |
| `lora_parameters` | Required | Optional |
| `radio_module_config` | Required | Optional |
| `node.effective_distance` | Optional (calculated) | Required |
| `nodes` | Required (≥1) | Required (≥1) |
| `obstacles` | Optional | Optional |

---

### Modified Files

#### 1. `src/main.rs`
**Changes**:
- Add `mod analyzer;` declaration
- Add `mod common;` declaration
- Modify Embassy thread spawning to support mode selection:
  - Instead of directly spawning `network_task`, spawn a dispatcher that waits for mode selection
  - Based on mode, spawn either `simulation::network_task` or `analyzer::analyzer_task`

**New Code Pattern**:
```rust
// In embassy_init, spawn a mode-aware dispatcher instead of network_task directly
let _ = spawner.spawn(mode_dispatcher_task(spawner, ui_refresh_tx, ui_command_rx));
```

**New Task**:
```rust
#[embassy_executor::task]
async fn mode_dispatcher_task(
    spawner: Spawner,
    ui_refresh_tx: UIRefreshQueueSender,
    ui_command_rx: UICommandQueueReceiver,
) {
    // Wait for UICommand::StartMode(mode, scene_path, Option<log_path>)
    // Spawn appropriate task based on mode
}
```

#### 2. `src/ui/mod.rs`
**Changes**:
- Add new `UIRefreshState` variants:
  ```rust
  /// Delay between real clock and last processed log timestamp (real-time tracking only)
  AnalyzerDelay(u64), // milliseconds
  
  /// Log visualization has reached end of file
  VisualizationEnded,
  
  /// Current mode (for UI adaptation)
  ModeChanged(OperatingMode),
  ```
- Add new `UICommand` variants:
  ```rust
  /// Start the application in a specific mode with file paths
  StartMode {
      mode: OperatingMode,
      scene_path: String,
      log_path: Option<String>, // None for Simulation mode
  },
  
  /// Seek to a specific time in log visualization (future enhancement)
  SeekAnalyzer(u64), // timestamp in seconds
  ```
- Add `OperatingMode` enum:
  ```rust
  pub enum OperatingMode {
      Simulation,
      RealtimeTracking,
      LogVisualization,
  }
  ```

#### 3. `src/ui/mode_selector.rs`
**Changes**:
- Redesign column layouts:
  - **Simulation**: Single "Select scene" button (unchanged)
  - **Real-time Tracking**: Two stacked buttons:
    - "Select scene" (first, same vertical position as simulation button)
    - "Connect to stream" (below)
  - **Log Visualization**: Two stacked buttons:
    - "Select scene"
    - "Open log file"
- Add state tracking for file selection:
  ```rust
  pub struct ModeSelector {
      // ... existing fields ...
      realtime_scene_path: Option<String>,
      realtime_log_path: Option<String>,
      logvis_scene_path: Option<String>,
      logvis_log_path: Option<String>,
  }
  ```
- Button behavior:
  - Clicking a button opens file picker
  - After selection, show filename + checkmark on button
  - When both files selected for a mode, auto-proceed to main screen
- Return value changes to include file paths:
  ```rust
  pub enum ModeSelection {
      Simulation { scene_path: String },
      RealtimeTracking { scene_path: String, log_path: String },
      LogVisualization { scene_path: String, log_path: String },
  }
  ```

#### 4. `src/ui/app_state.rs`
**Changes**:
- Add new state fields:
  ```rust
  pub operating_mode: OperatingMode,
  pub analyzer_delay: u64, // milliseconds, for real-time tracking
  pub visualization_ended: bool,
  ```
- Handle new `UIRefreshState` variants in `update()` method
- Modify file picker flow to support two-file selection for analyzer modes
- Add mode-aware state initialization

#### 5. `src/ui/top_panel.rs`
**Changes**:
- Modify `render()` to check `state.operating_mode`:
  - **Simulation**: Show all existing controls (speed slider, auto-speed checkbox)
  - **Real-time Tracking**:
    - Hide speed slider and auto-speed checkbox
    - Show "Delay: Xms" indicator next to sim time
  - **Log Visualization**:
    - Show speed slider (range: 1% - 1000%)
    - Hide auto-speed checkbox
- Modify sim time display to show log timestamp in analyzer modes

**UI Layout Change for Real-time Tracking**:
```
Sim time: 142s (Delay: 230ms)  | Total TX: 45023
```

#### 6. `src/ui/right_panel.rs`
**Changes**:
- Modify `render()` to check `state.operating_mode`:
  - **Simulation**: Show "Start Measurement" / "Reset Measurement" button
  - **Real-time Tracking**: Show measurement button (measurements come from log)
  - **Log Visualization**: Hide measurement button entirely
- Handle "Visualization ended" popup when `state.visualization_ended` is true

#### 7. `src/simulation/mod.rs`
**Changes**:
- Remove re-exports that move to `common`:
  - `Point`, `Obstacle` now come from `common::scene`
- Update imports in module

#### 8. `src/simulation/types.rs`
**Changes**:
- Move `Scene`, `Node`, `Point`, `RectPos`, `CirclePos`, `Obstacle` to `common/scene.rs`
- Keep simulation-specific types:
  - `NodeMessage`, `AirtimeWaitingPacket`, `CadItem`
  - `NodeInputMessage`, `NodeOutputMessage`, `NodeOutputPayload`
  - Channel types and constants
- Update imports to reference `crate::common::scene::*`

#### 9. `src/simulation/network.rs`
**Changes**:
- Remove `load_scene()` function (moved to `common/scene.rs`)
- Remove `validate_scene()` function (moved to `common/scene.rs`)
- Update imports to use `crate::common::scene::{Scene, Node, Point, Obstacle, ...}`
- Modify `network_task` to:
  - Wait for `UICommand::StartMode` instead of `UICommand::LoadFile`
  - Verify mode is `Simulation` before proceeding
  - Call `common::scene::load_scene()` with `SceneMode::Simulation`

#### 10. `src/simulation/signal_calculations.rs`
**Changes**:
- Move `PathLossParameters` and `LoraParameters` structs to `common/scene.rs`
- Keep calculation functions, but update imports
- Keep `RadioModuleConfig` in simulation/types.rs (only needed for simulation)

---

## Data Flow Diagrams

### Mode Selection Flow

```
┌─────────────────┐     ┌───────────────────┐     ┌─────────────────┐
│   ModeSelector  │────▶│    File Picker    │────▶│ UICommand::     │
│   (UI Thread)   │     │    Dialog(s)      │     │ StartMode       │
└─────────────────┘     └───────────────────┘     └────────┬────────┘
                                                           │
                        ┌──────────────────────────────────┼──────────────────────────────────┐
                        │                                  │                                  │
                        ▼                                  ▼                                  ▼
              ┌─────────────────┐              ┌─────────────────┐              ┌─────────────────┐
              │  network_task   │              │ analyzer_task   │              │ analyzer_task   │
              │  (Simulation)   │              │ (RealtimeTrack) │              │ (LogVisualize)  │
              └─────────────────┘              └─────────────────┘              └─────────────────┘
```

### Analyzer Event Flow

```
┌────────────────┐     ┌───────────────┐     ┌───────────────┐     ┌─────────────────┐
│   Log File     │────▶│  LogLoader    │────▶│  LogParser    │────▶│  AnalyzerTask   │
│                │     │  (async I/O)  │     │  (line parse) │     │  (time sync)    │
└────────────────┘     └───────────────┘     └───────────────┘     └────────┬────────┘
                                                                            │
                                             ┌──────────────────────────────┘
                                             ▼
                                   ┌─────────────────┐
                                   │ UIRefreshQueue  │
                                   │ (to UI Thread)  │
                                   └────────┬────────┘
                                            │
              ┌─────────────────────────────┼─────────────────────────────┐
              ▼                             ▼                             ▼
    ┌─────────────────┐          ┌─────────────────┐          ┌─────────────────┐
    │ NodeSentRadio-  │          │ NodeReachedIn-  │          │ AnalyzerDelay   │
    │ Message         │          │ Measurement     │          │ (realtime only) │
    └─────────────────┘          └─────────────────┘          └─────────────────┘
```

### Time Synchronization Algorithm

```
For each log line:
  1. Parse timestamp T_log
  2. If reference not set:
       reference_timestamp = T_log
       reference_instant = Instant::now()
       Process line immediately
  3. Else:
       elapsed = Instant::now() - reference_instant
       expected_timestamp = reference_timestamp + elapsed
       
       If T_log > expected_timestamp:
           wait_duration = T_log - expected_timestamp
           Timer::after(wait_duration).await
           Process line
       Else:
           // Log is behind real-time
           Process line immediately
           reference_timestamp = T_log  // Reset reference
           reference_instant = Instant::now()
```

---

## Scene File Format Changes

### New Node Field

```json
{
  "nodes": [
    {
      "node_id": 3094,
      "position": { "x": 100.5, "y": 200.3 },
      "radio_strength": 14,
      "effective_distance": 850
    }
  ]
}
```

- `effective_distance`: Pre-calculated effective radio range in meters
  - **Simulation mode**: Optional (calculated from `radio_strength` + path loss parameters)
  - **Analyzer modes**: Required (no physics simulation)

### Optional Sections for Analyzer Modes

```json
{
  "path_loss_parameters": { ... },     // Optional for analyzer
  "lora_parameters": { ... },          // Optional for analyzer
  "radio_module_config": { ... },      // Optional for analyzer
  "nodes": [ ... ],                    // Required for all
  "obstacles": [ ... ],                // Optional for all
  "world_top_left": { ... },           // Required for all
  "world_bottom_right": { ... },       // Required for all
  "width": 1000,                       // Required for all
  "height": 1000                       // Required for all
}
```

---

## UI Changes Summary

### Mode Selector Screen

| Column | Current | New |
|--------|---------|-----|
| Simulation | "Select scene" button | No change |
| Real-time | "Connect to stream" button | "Select scene" + "Connect to stream" (stacked) |
| Log Viz | "Open log file" button | "Select scene" + "Open log file" (stacked) |

### Top Panel (Mode-Dependent)

| Element | Simulation | Real-time Tracking | Log Visualization |
|---------|-----------|-------------------|-------------------|
| Sim time | Virtual time | Log timestamp | Log timestamp |
| Delay indicator | Warning only | Always shown | Hidden |
| Speed slider | Shown | Hidden | Shown (no auto) |
| Auto-speed checkbox | Shown | Hidden | Hidden |
| Reset button | Shown | Hidden | Shown |

### Right Panel (Mode-Dependent)

| Element | Simulation | Real-time Tracking | Log Visualization |
|---------|-----------|-------------------|-------------------|
| Measurement button | Shown | Shown | Hidden |
| Message stream | Shown | Shown | Shown |
| EOF popup | N/A | N/A | Shown at end |

---

## Error Handling

### Log Parsing Errors
- Unparseable lines are silently skipped (logged at DEBUG level)
- Missing optional fields use defaults (e.g., `sequence: 0`, `packet: 1/1`)
- Invalid timestamps skip the line

### File Errors
- Scene file not found / unreadable: Show alert, return to mode selector
- Log file not found / unreadable: Show alert, return to mode selector
- Log file becomes unavailable during real-time tracking: Show warning, continue polling

### Time Synchronization Issues
- Timestamps going backwards: Reset reference, log warning
- Extremely old timestamps (>1 hour behind): Reset reference, log warning
- Clock skew detection: Compare elapsed time vs. log time progression

---

## Testing Considerations

### Unit Tests (New)
- `analyzer/log_parser.rs`: Parse each log line format correctly
- `analyzer/log_parser.rs`: Handle malformed lines gracefully
- `common/scene.rs`: Validate scene with different modes
- `common/scene.rs`: Optional fields handled correctly per mode

### Integration Tests (Manual)
- Real-time tracking with actively written log file
- Log visualization with complete log file
- Mode switching (return to selector and choose different mode)
- Scene file without physics parameters (analyzer only)

---

## Implementation Order

1. **Phase 1: Common Infrastructure**
   - Create `src/common/mod.rs` and `src/common/scene.rs`
   - Move scene types and loading logic
   - Update imports in `simulation/` module
   - Verify simulation still works

2. **Phase 2: Analyzer Module Structure**
   - Create `src/analyzer/mod.rs`, `types.rs`, `log_parser.rs`
   - Implement log line parsing with tests
   - Create `log_loader.rs` with file reading logic

3. **Phase 3: Analyzer Task**
   - Implement `analyzer_task` with time synchronization
   - Connect to UI via existing channels
   - Test with static log file

4. **Phase 4: UI Integration**
   - Modify mode selector for two-button layout
   - Add new `UIRefreshState`/`UICommand` variants
   - Implement mode-dependent UI rendering
   - Add operating mode state tracking

5. **Phase 5: Main Integration**
   - Create mode dispatcher task
   - Connect all pieces
   - End-to-end testing

---

## Dependencies

### Existing Dependencies (No Changes Needed)
- `embassy-executor`, `embassy-sync`, `embassy-time`: Async runtime
- `serde`, `serde_json`: Scene parsing
- `egui`, `eframe`: UI framework
- `log`: Logging

### New Dependencies (May Be Needed)
- `chrono`: Timestamp parsing (format: `2025-10-23T18:00:00Z`)
  - Already commonly used, add if not present
- `regex`: Log line parsing (optional, can use string methods)
  - Consider for cleaner field extraction

---

## Appendix: Example Log Lines

### *TM1* - Packet Transmitted
```
2025-10-23T18:00:01Z moonblokz_radio_lib::radio_devices::rp_lora_sx1262: [3094] *TM1* Packet transmitted: type: 6, sequence: 30940779, length: 215, packet: 1/10
2025-10-23T18:00:02Z moonblokz_radio_lib::radio_devices::rp_lora_sx1262: [3094] *TM1* Packet transmitted: type: 1, length: 50
```

### *TM2* - Packet Received
```
2025-10-23T18:00:01Z moonblokz_radio_lib::radio_devices::rp_lora_sx1262:[3094] *TM2* Packet received: sender: 3093, type: 6, sequence: 30940779, length: 215, packet: 1/10, link quality: 26
2025-10-23T18:00:02Z moonblokz_radio_lib::radio_devices::rp_lora_sx1262:[3094] *TM2* Packet received: sender: 3095, type: 2, length: 100, link quality: 18
```

### *TM3* - Start Measurement
```
2025-10-23T18:00:00Z [3094] *TM3* Start measurement: sequence: 321312
```

### *TM4* - Full Message Received
```
2025-10-23T18:00:05Z [3094] *TM4* Routing message to incoming queue: sender: 3093, type: 6, length: 2000, sequence: 321312
```
