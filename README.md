# MoonBlokz Radio Simulator

GUI-based, interactive radio network simulator that drives and visualizes a multi-node system using moonblokz-radio-lib. It renders a 2D map with nodes and obstacles, simulates radio propagation and collisions, and lets you observe throughput, link quality, and distribution metrics in real time. A scalable virtual-time driver enables accelerated or slowed simulations without timer bursts or stalls.

## What you get

- Scalable virtual time (20–1000%) with smooth, continuous clock mapping
- Optional auto-speed controller to keep the simulation responsive under load
- Map view with obstacles, nodes, radio pulses, and optional node ID labels
- Inspector with a live per-node radio stream (RX/TX, size, packet index, link quality, collisions)
- Measurement mode to track message distribution across the network (50/90/100% reach times and packets/node)
- Live system metrics (totals, throughput, collision rate, node count)

## Quick start

Prerequisites: a recent Rust toolchain (stable) on macOS/Linux/Windows.

1) Build and run the simulator
	- The app starts with a file picker—select a scene JSON from the `scenes/` folder (e.g., `scenes/example.json`) or your own (see “Scene file format”).
2) Use the top “Controls” panel to adjust simulation speed or enable Auto speed.
3) Click nodes on the map to open the Inspector and view their radio stream.

## UI at a glance

- Top bar
	- System Metrics: simulation time, total TX/RX, collision rate, node count
	- Measured data: distribution %, time-to-50/90/100%, packets-per-node
	- Controls: Speed slider, Auto speed toggle, Reset speed, Show node IDs
- Right Inspector
	- Selected node details and a virtualized table of recent radio events
	- Collisions are highlighted; link quality is color-coded by thresholds
- Central Map
	- 0..10000 world-units square with grid, obstacles, nodes, and radio pulses
	- Click to select nearest node; selected node shows effective range overlay

## Scene file format (JSON)

The simulator loads a “Scene” JSON describing the environment and nodes.

- path_loss_parameters
	- path_loss_exponent (f32)
	- shadowing_sigma (f32)
	- path_loss_at_reference_distance (f32)
	- noise_floor (f32)
- lora_parameters
	- bandwidth (u32)
	- spreading_factor (u8)
	- coding_rate (u32) — 1..4 meaning 4/5..4/8
	- preamble_symbols (f32)
	- crc_enabled (bool)
	- low_data_rate_optimization (bool)
- radio_module_config
	- delay_between_tx_packets (u8)
	- delay_between_tx_messages (u8)
	- echo_request_minimal_interval (u32)
	- echo_messages_target_interval (u8)
	- echo_gathering_timeout (u8)
	- relay_position_delay (u8)
	- scoring_matrix ([u8; 5]) — passed to moonblokz-radio-lib
- nodes: array of
	- node_id (u32)
	- position { x: u32, y: u32 } in 0..10000 world units
	- radio_strength (f32) — TX power in dBm used by the path loss model
- obstacles: array of tagged enums
	- Rectangle: { "type": "rectangle", "top-left-position": {x,y}, "bottom-right-position": {x,y} }
	- Circle: { "type": "circle", "center_position": {x,y}, "radius": u32 }

Minimal example:

```json
{
	"path_loss_parameters": {
		"path_loss_exponent": 2.0,
		"shadowing_sigma": 0.0,
		"path_loss_at_reference_distance": 40.0,
		"noise_floor": -120.0
	},
	"lora_parameters": {
		"bandwidth": 125000,
		"spreading_factor": 7,
		"coding_rate": 1,
		"preamble_symbols": 8.0,
		"crc_enabled": true,
		"low_data_rate_optimization": false
	},
	"radio_module_config": {
		"delay_between_tx_packets": 10,
		"delay_between_tx_messages": 10,
		"echo_request_minimal_interval": 1000,
		"echo_messages_target_interval": 50,
		"echo_gathering_timeout": 10,
		"relay_position_delay": 0,
		"scoring_matrix": [10, 20, 30, 40, 50]
	},
	"nodes": [
		{ "node_id": 1, "position": { "x": 1000, "y": 1000 }, "radio_strength": 14.0 },
		{ "node_id": 2, "position": { "x": 3000, "y": 2000 }, "radio_strength": 14.0 }
	],
	"obstacles": [
		{ "type": "rectangle", "top-left-position": {"x": 1500, "y": 1500}, "bottom-right-position": {"x": 2500, "y": 2500} },
		{ "type": "circle", "center_position": {"x": 5000, "y": 5000}, "radius": 400 }
	]
}
```

## Architecture overview

The simulator is composed of four core modules:

- src/main.rs (GUI and app runtime)
	- Builds the egui interface and owns the UI state.
	- Spawns an Embassy executor on a background thread and bridges UI↔network via bounded channels.
- src/network.rs (simulation core)
	- Loads the scene, spawns one async node task per node, and runs the central event loop.
	- Maintains per-node message ring buffers and processes CAD/airtime windows.
	- Selects receivers by range and line-of-sight; computes SINR/collisions and delivers RX.
- src/signal_calculations.rs (radio/geometry math)
	- Path loss with log-normal shadowing, RSSI, SNR thresholds, airtime, preamble and CAD durations.
	- Deterministic “effective distance” used for fast range checks and UI overlays.
- src/time_driver.rs (virtual time)
	- Global, scaled embassy-time driver. Preserves virtual-time continuity on speed changes.
	- Slices waits (≤25 ms) and bumps an epoch on updates to keep timers responsive.

Threads and channels:

- UI thread: egui/eframe render loop.
- Embassy executor thread: runs the network task and node tasks.
- UIRefreshChannel (network→UI): alerts, nodes/obstacles updates, counters, pulses, speed updates, node info.
- UICommandChannel (UI→network): load scene, request node info, start measurement, toggle auto-speed.
- Per-node channels: NodeInputQueue (network→node) and NodesOutputQueue (node→network).

## Radio and collision model (simplified)

1) Transmission
	 - When a node emits a packet, the simulator enqueues a TX airtime window for the sender and for each in-range, unobstructed receiver.
	 - Range is based on a deterministic effective distance from the link budget; line-of-sight checks obstacle intersections.
2) Reception and SINR
	 - At the end of a receiver’s window, the simulator computes SINR in dB:
		 RSSI(dBm) − 10·log10(sum_mW(noise_floor + overlapping RSSIs)).
	 - If SINR ≥ SNR limit and not captured, the packet is delivered with a link quality via moonblokz-radio-lib.
3) Collisions and capture
	 - Overlaps are tracked; a simple capture rule is applied: a later-starting packet is destroyed if an earlier, stronger one exceeds a threshold (6 dB), and vice versa for strong later packets.

Notes:
- Geometry uses squared-distance comparisons in hot paths to avoid sqrt.
- Per-node history is a ring buffer (default 1000 entries) to keep memory bounded.

## Performance and limits

- Scaled time driver avoids timer “bursts” after speed changes and reduces stalls via short wait slices.
- Bounded channels and history buffers to prevent unbounded memory growth under heavy load.
- Large executor stack is used to support many simulated nodes on desktop targets.

## Development

- Build and test
	- The crate includes unit tests for time mapping, geometry, and radio timing/math.
- Key conventions
	- World coordinates: 0..10000 on both axes; UI maps these to the current viewport.
	- Power units: dBm/mW; time units: embassy::Duration or seconds (math only).

## License

This project is licensed under the terms of the LICENSE file in this repository.
