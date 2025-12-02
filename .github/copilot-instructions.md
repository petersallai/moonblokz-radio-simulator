# MoonBlokz Radio Simulator - AI Coding Agent Instructions

## Project Overview

This is a **desktop simulator** for the MoonBlokz mesh radio network. It runs the **exact same embedded codebase** (from `moonblokz-radio-lib`) used on real microcontroller nodes, enabling large-scale testing (300+ nodes) in a single process without hardware. The simulator validates mesh protocol behavior using LoRa physics models and real-time visualization.

**Key Architectural Insight**: This is a hybrid embedded/desktop app. Embassy async runtime (normally for embedded) runs on a background thread, while egui (immediate-mode GUI) renders on the main thread. Bounded channels bridge the two worlds.

##Background Reading
- https://medium.com/moonblokz/moonblokz-series-part-vii-1-bootstrapping-lora-on-the-rp2040-14c6be9904d4
- https://medium.com/moonblokz/moonblokz-series-part-vii-2-mesh-radio-algorithm-3650af3711f3
- https://medium.com/moonblokz/moonblokz-series-part-vii-3-inside-the-radio-module-d92545624d2b
- https://medium.com/moonblokz/moonblokz-series-part-vii-4-radio-network-simulation-5cc86a721e8c

## Critical Architecture Rules

### 1. Lock Ordering (DEADLOCK PREVENTION)

**ALWAYS acquire locks in this order**: `CLOCK` → `SCHED` (never reversed)

```rust
// ✅ CORRECT: Acquire CLOCK first, extract data, drop, then SCHED
let data = { let c = clock().lock().unwrap(); extract(&c) };
let mut s = sched().lock().unwrap();

// ❌ WRONG: SCHED before CLOCK = DEADLOCK RISK
let s = sched().lock().unwrap();
let c = clock().lock().unwrap();  // DEADLOCK!
```

See `src/time_driver.rs` module-level docs for full rationale. This is enforced throughout the codebase.

### 2. Thread Architecture

- **Main thread**: egui/eframe UI loop (required for macOS AppKit)
- **Embassy executor thread**: 192MB stack, runs `network_task` + all `node_task`s
- **Communication**: `UIRefreshChannel` (500 capacity, network→UI) and `UICommandChannel` (100 capacity, UI→network)

Both channels are `Box::leak`ed to satisfy `'static` lifetime requirements—intentional, safe, cleaned up on process exit.

### 3. Time Scaling System

Virtual time can scale 1-1000% of real-time with **continuity preservation**:
- Speed changes rebase `origin_real` but keep `origin_virtual_ticks` fixed
- Prevents scheduled timers from "bursting" into the past
- Uses Q32.32 fixed-point math (`ONE_Q32 = 1<<32`) to avoid float drift

API: `time_driver::set_simulation_speed_percent(u32)` / `get_simulation_speed_percent() -> u32`

## Critical Module Boundaries

### `src/simulation/network.rs`
Central event loop:
1. Computes next deadline (CAD/airtime end + 10ms UI pulse)
2. `select3` waits for: node events, UI commands, or deadline
3. On deadlines: evaluates CAD windows, processes one queued packet per node, computes SINR/collisions, delivers RX

**Scene validation**: `validate_scene()` checks bounds before simulation start:
- Max 10000 nodes
- Positions in 0-10000 world units
- Radio strength -50 to +50 dBm (realistic LoRa range)
- SF 5-12, positive bandwidth, coding rate 1-4
- Valid obstacle geometry (rectangles: TL < BR, circles: radius > 0)

### `src/simulation/signal_calculations.rs`
Physics layer using log-distance path loss:
```
PL(d) = PL(d₀) + 10×n×log₁₀(d/d₀) + X_σ
RSSI = tx_power - PL(d)
SNR = RSSI - 10×log₁₀(sum_mW(noise_floor + interferers))
```

**Critical constants**:
- `MAX_LORA_PAYLOAD = 255` (enforced with recursive clamping + log::warn)
- Capture threshold: 6 dB (stronger signal destroys weaker in collisions)

### `src/ui/app_state.rs`
Immediate-mode state rebuilt every frame:
- **HashMap cleanup**: `node_radio_transfer_indicators.retain()` every frame (50 FPS) removes expired (>1000ms) entries
- **Update pattern**: Drain `ui_refresh_rx` until `TryRecvError::Empty`, apply all updates, then render

### `src/simulation/geometry.rs`
**Hot-path optimization**: Use `distance2()` (squared distance) for range checks to avoid `sqrt()`. Only call `distance_from_d2()` when actual distance needed for RSSI calculation.

## Development Workflows

### Build & Run
```bash
cargo build --release    # Standard Rust build
cargo run -- scenes/example.json  # Pass scene file directly (optional)
```

The app starts with a file picker if no args. Select from `scenes/*.json`.

### Testing
```bash
cargo test              # Runs 11 unit tests
cargo test -- --nocapture  # Show test output
```

Tests cover: time driver mapping, geometry intersections, radio timing/airtime, RSSI calculations.

### Scene File Structure
JSON with 4 sections:
1. `path_loss_parameters`: `path_loss_exponent` (2.0=free space, 3-4=urban), `shadowing_sigma`, `noise_floor`
2. `lora_parameters`: `bandwidth`, `spreading_factor` (5-12), `coding_rate` (1-4 = 4/5 to 4/8), `preamble_symbols`
3. `radio_module_config`: Protocol timings (tx delays, echo intervals, scoring matrix)
4. `nodes`: array of `{node_id, position: {x,y}, radio_strength}` (coordinates 0-10000)
5. `obstacles`: `{type: "rectangle"|"circle", ...}` (blocks line-of-sight)

**Example scenes** in `scenes/`:
- `basic.json`: 5 nodes, minimal relay test
- `labyrinth.json`: 300 nodes, maze topology (demonstrates depth-first propagation)
- `100_random_nodes_with_wall.json`: Real-world conditions with obstacle

## Project-Specific Conventions

### Memory Management
- Embassy executor stack: 192MB (via `thread::Builder`)
- Per-node message history: ring buffer (1000 entries, `NODE_MESSAGES_CAPACITY`)
- Channel capacities: UIRefresh=500, UICommand=100, NodeInput=50, NodeOutput=100
- Leaked allocations: Only `Executor`, `UIRefreshQueue`, `UICommandQueue` (all `'static` lifetime, process-scoped)

### Error Handling
- Scene loading: Errors sent as `UIRefreshState::Alert(String)` for modal display
- Validation failures: Return `Result<(), String>` with human-readable messages
- Radio payload overflow: `log::warn!` + recursive clamping to `MAX_LORA_PAYLOAD`

### Units & Coordinates
- **Power**: dBm (convert via `dbm_to_mw()` / `mw_to_dbm()` for linear math)
- **Time**: `embassy_time::Duration` for API, `f32` seconds for formulas
- **Distance**: meters (world units 1:1 with meters), squared distance for comparisons
- **World space**: 0-10000 on both axes (enforced by validation)

### UI Rendering Patterns
- **Immediate mode**: Entire UI rebuilt every frame, no widget state retention
- **Virtualized tables**: `egui_extras::TableBuilder` with `row()` calls only for visible rows
- **Color coding**: Link quality thresholds (green >20, yellow >10, orange >5, red ≤5)
- **Animation**: Radio pulses expand from 0 to `effective_distance` over 1s with fade

## Common Tasks

### Adding a New Scene Parameter
1. Update `Scene` struct in `src/simulation/types.rs`
2. Add validation in `validate_scene()` with descriptive error messages
3. Deserialize from JSON (serde auto-handles)
4. Pass to physics calculations or node tasks

### Modifying Physics Model
1. Edit formulas in `src/simulation/signal_calculations.rs`
2. Update corresponding unit tests (11 existing tests provide coverage baseline)
3. Verify with example scenes (especially `labyrinth.json` for convergence behavior)

### Debugging Lock Issues
1. Check lock acquisition order in stack traces
2. Use `RUST_LOG=debug` to see time driver epoch bumps
3. Verify scheduler waits are sliced (≤25ms per `time_driver.rs` L304-L316)

### Performance Profiling
- Auto-speed mode tracks simulation delay (real time ahead of virtual time)
- Warnings appear in logs when node queues fill (e.g., `outgoing_message_queue full`)
- CPU profiling: Use `cargo flamegraph --release` on simulation loop

## Dependencies & External Context

**Key external dependency**: `moonblokz-radio-lib` (local path in `Cargo.toml`)
- Provides: `RadioMessage`, `RadioPacket`, `MessageType`, node task interface
- Memory config: `memory-config-large` feature (larger queues for desktop simulation)
- Radio device: `radio-device-simulator` feature (replaces hardware HAL with simulated channels)

**Embassy async stack**:
- `embassy-executor`: Provides cooperative task scheduler
- `embassy-sync`: Bounded channels with `CriticalSectionRawMutex`
- `embassy-time`: Virtual time layer (our custom driver in `time_driver.rs`)

**UI stack**:
- `egui` 0.27: Immediate-mode GUI framework
- `eframe`: Native windowing (macOS/Windows/Linux), 50 FPS refresh via `request_repaint_after`
- `egui_extras`: Virtualized table rendering

## Known Issues & Technical Debt

See `TODO.md`:
- Log replay mode not yet implemented (planned for field testing visualization)
- Processing delay handling from UI (stub exists in TODO)
- Dependency on local `moonblokz-radio-lib` path (should move to GitHub release)

Single TODO comment in code (`src/simulation/network.rs:864`): "handle message receipt UI/state if needed"—placeholder for future UI enhancement, not a bug.

## Testing Philosophy

**Unit tests** cover math-heavy modules:
- `geometry.rs`: Line-circle/rectangle intersections, collinear segments
- `signal_calculations.rs`: RSSI, airtime, CAD duration, SNR limits
- `time_driver.rs`: Q32.32 conversions, continuity on speed changes

**No integration tests**: Simulator is the integration test—load scenes, observe convergence visually or via logs.

## References

- [MoonBlokz article series](https://medium.com/@peter.sallai/moonblokz-series-part-i-building-a-hyper-local-blockchain-2f385b763c65)
- [Radio simulation deep dive](https://medium.com/moonblokz/moonblokz-series-part-vii-4-radio-network-simulation-5cc86a721e8c)
- Embassy docs: https://embassy.dev/
- egui docs: https://docs.rs/egui/

---

**Quick Start for AI Agents**: Read the mentioned Medium articles, Read `README.md` first for high-level architecture, then `src/main.rs` module docs for threading model, then `src/simulation/network.rs` for event loop mechanics. Always check `time_driver.rs` lock ordering rules before modifying timing code.
