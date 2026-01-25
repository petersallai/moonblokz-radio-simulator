# Connection Matrix Visualization – Detailed Specification

## 1) Goal and Scope
Add connection matrix visualization to both the Simulator and Analyzer in:
- **Simulation mode** (local simulation)
- **Real-Time tracking mode** (tailing live logs)

The feature includes:
- Requesting a node’s connection matrix on demand.
- Parsing encoded connection matrix logs.
- Presenting a tabular list with link quality color coding.
- Drawing matrix-derived links on the map when the Connection Matrix tab is active.

This spec is limited to UI, data flow, parsing, and visualization. It assumes the radio library already supports connection matrix logging (feature: `connection-matrix-logging`).

## 2) Connection Matrix Logging Format (Source)
The radio library emits log records when `MessageProcessingResult::RequestConnectionMatrixIntoLog` is reported. The log format is:

1) Start line:
```
[<requester_node_id>] *TM9* Logging Connection Matrix, node_count: <N>
```

2) One or more row lines per row (row may be chunked):
```
[<requester_node_id>] *TM9* connection_matrix_row: <row_node_id>, values:<encoded_chunk>
```
Where `<encoded_chunk>` is a sequence of characters from the encoder table (A–Z, a–z, 0–9, '-', '_'), each encoding a 6-bit quality value (0–63).

3) End line:
```
[<requester_node_id>] *TM9* Logging Connection Matrix ended
```

Example:
```
[7338] *TM9* Logging Connection Matrix, node_count: 4
[7338] *TM9* connection_matrix_row: 7338, values:_A73
[7338] *TM9* connection_matrix_row: 2754, values:A_AA
[7338] *TM9* connection_matrix_row: 3094, values:7A_3
[7338] *TM9* connection_matrix_row: 1792, values:2A3_
[7338] *TM9* Logging Connection Matrix ended
```

## 3) Data Model
### 3.1 Connection Matrix Structure
A connection matrix is a square `N x N` matrix describing link qualities between the `N` nodes involved.

The data model should include:
- `requester_node_id: u32`
- `node_count: usize`
- `row_node_ids: Vec<u32>` — ordered row identifiers, in the exact order emitted by the log
- `values: Vec<Vec<u8>>` — decoded link quality values (0–63)
- `timestamp: DateTime` — the time the matrix was completed (end line observed)

### 3.2 Parsing State
A stateful parser is required because rows may arrive across multiple log lines and in multiple chunks.

Parser state should include:
- `active_requester: Option<u32>`
- `active_node_count: Option<usize>`
- `active_rows: HashMap<u32, Vec<u8>>` — row buffers accumulated in order received
- `row_order: Vec<u32>` — tracks row insertion order for display consistency
- `is_complete: bool`

Completion is detected when the end line is parsed for the same `requester_node_id`.

### 3.3 Encoder/Decoder
The decoder is the inverse of the encoder table:
```
A–Z => 0–25
a–z => 26–51
0–9 => 52–61
-   => 62
_   => 63
```
All decoded values are 6-bit quality values (0–63).

## 4) Request Flow
### 4.1 Triggering a Request
- In Simulator and Analyzer, the UI includes a **Query Connection Matrix** button.
- On click, the system must:
  - Clear any currently stored connection matrix for the selected node (discard previous).
  - Send a request to the node using the existing command mechanism:
    - `run_command` command with parameter `"/CM"`.
  - Set an “awaiting matrix” state (used for the activity indicator).

### 4.2 Real-Time Tracking Mode
- The query sends the `\CM` command to the selected node (Telemetry Hub / Control module).
- Matrix data is parsed from the live log stream.

### 4.3 Simulation Mode
- The query triggers `MessageProcessingResult::RequestConnectionMatrixIntoLog` for the selected simulated node using existing simulator control pathways.
- Matrix data is parsed from the simulation log stream.

## 5) UI Requirements
### 5.1 Right Panel Tab
- Add a **fourth** tab to the right panel: **Connection Matrix**.
- This tab appears in:
  - Simulation mode
  - Real-Time tracking mode
- It should not be visible in log analyzer mode

### 5.2 Tab Contents
- Button: **Query Connection Matrix**
  - Remains active while waiting for data.
  - When pressed, it triggers a request and clears any previous matrix.
- Activity indicator (spinner or animated icon) next to the button while awaiting data.
  - Stops once a full matrix is received (end line parsed).
- Below the button, a list table with columns:
  - **Sender node**
  - **Target node**
  - **Link quality**

### 5.3 Link Quality Color Coding
- If thresholds are available:
  - **Red** = weak
  - **Yellow** = fair
  - **Green** = excellent
- **Threshold sources**:
  - Simulation mode: use existing thresholds already available.
  - Real-Time tracking: add thresholds to scene definition; if missing, display without color.

### 5.4 Display Ordering
List entries are generated from the matrix in row-major order as emitted:
- For each row in `row_order` (row node id), enumerate all columns in the same index order as `row_order`.
- Each entry corresponds to `values[row_index][col_index]`.

## 6) Map Visualization
### 6.1 When to Draw
- Only draw connection matrix lines when:
  - **Connection Matrix tab is selected**, and
  - A **valid, active matrix** is present.

### 6.2 Line Drawing Rules
Draw lines in two passes (directional emphasis):

1) **Forward pass**
   - For all pairs where `sender_node_id < receiver_node_id`.
   - Line width: **3 px**.
   - Color by link quality.

2) **Backward pass**
   - For all pairs where `sender_node_id > receiver_node_id`.
   - Line width: **1 px**.
   - Color by link quality.

This yields visually distinct bidirectional links, where each direction is shown with different thickness.

### 6.3 Missing Nodes
If a matrix row/column references a node not present in the scene or map:
- Skip drawing that line.
- Still show the row in the list with node IDs.

## 7) Log Parsing Details
### 7.1 Start Line Handling
On start line:
- Capture `requester_node_id`.
- Capture `node_count`.
- Clear any previous in-progress parser state.
- Mark awaiting matrix as **true** for the requester.

### 7.2 Row Line Handling
On row line:
- Parse `row_node_id`.
- Decode `values:<encoded_chunk>`.
- Append decoded values to the row buffer for `row_node_id`.
- Preserve row insertion order on first sight.

### 7.3 End Line Handling
On end line:
- Build the final matrix using:
  - `row_order` as the row/column index mapping.
  - Each row buffer must have length equal to `node_count`.
- If any row is missing or length mismatches `node_count`, mark as invalid and discard.
- On success:
  - Store matrix in UI state.
  - Clear awaiting indicator.

### 7.4 Partial/Interrupted Logs
If a new start line arrives for the same requester before the previous matrix completes:
- Discard previous in-progress data and restart.

## 8) Scene Definition Changes (Real-Time Tracking)
Add optional fields to the scene definition (or configuration for tracking) for link quality thresholds:
- `link_quality_weak_threshold` (u8)
- `link_quality_excellent_threshold` (u8)

If these are missing, disable color coding in real-time tracking.

## 9) UI State and Behavior
### 9.1 State per Mode
Maintain state per mode and per selected node:
- `active_matrix: Option<ConnectionMatrix>`
- `awaiting_matrix: bool`
- `selected_node_id: u32`

### 9.2 Reset Rules
- On **Query Connection Matrix**:
  - `active_matrix = None`
  - `awaiting_matrix = true`
- On **matrix completion**:
  - `active_matrix = Some(matrix)`
  - `awaiting_matrix = false`
- On **mode change** or **scene change**:
  - `active_matrix = None`
  - `awaiting_matrix = false`

## 10) Error Handling and Edge Cases
- If log lines are malformed, ignore them.
- If matrix is incomplete at end line, discard and keep awaiting state cleared (do not keep stale data).
- If thresholds are inverted or invalid, treat as missing and disable color.
- If link quality value is out of range (should not happen), clamp to 0–63.

## 11) Testing Notes
Recommended test coverage:
- Decode encoder table mapping.
- Parsing a full matrix with single-line rows.
- Parsing row chunking with multiple `values:` lines per row.
- Handling out-of-order row lines (preserve order of first appearance).
- Handling incomplete/missing rows.
- Map visualization logic for forward/backward line passes.

## 12) Non-Goals
- No changes to the radio library logging format.
- No persistence of matrices across sessions.
- No automatic periodic querying (manual only).

