# Repository Guidelines

## Project Structure & Module Organization
- `src/` houses the Rust desktop app. Core areas: `src/main.rs` (app entry + UI wiring), `src/simulation/` (multi-node simulator + propagation/collision logic), `src/analyzer/` (log parsing + real-time and replay visualization), `src/common/` (scene loading/validation), `src/control/` (Telemetry HUB command client), `src/ui/` (egui panels/state), and `src/time_driver.rs` (virtual time).
- `scenes/` contains JSON scene files (node positions/power, obstacles, radio + path-loss parameters). Newer analyzer/field-test scenes may also include floating-point coordinates, explicit world bounds, real-world meter dimensions, and optional background map images.
- `docs/` and `docs/images/` contain design notes and screenshots. Spec files in the repo root (e.g., `analyzer_detailed_spec.md`, `control_functions*.md`) describe planned behavior and command semantics.
- `icons/` stores app icon assets. `target/` is local build output.

## Build, Test, and Development Commands
- `cargo run` builds and launches the desktop simulator.
- `cargo build` compiles without running.
- `cargo test` runs unit tests (geometry, time driver, analyzer parsing, radio math, and relay/simulation helpers).
- `cargo fmt` formats code; `cargo clippy` runs lint checks.

## Architecture Overview
- The simulator runs many nodes in one process. Each node has a dedicated task driving shared embedded radio logic via queues, while a central network task computes propagation, interference, collisions, and delivery (mpsc: many node senders, one network receiver, one receiver per node).
- The embedded radio library expects `'static` queues; simulation uses `Box::leak()` on `std` targets so the same radio code can run unchanged in multi-node desktop mode.
- Radio-module behavior follows the Part VII algorithm design: best-effort blockchain-state propagation (not guaranteed per-message delivery), adaptive relaying with calculated wait times, and strict airtime/duty-cycle pacing.
- Neighbor discovery is echo-based (`request_echo` / `echo` / `echo_result`), with a bounded connection matrix per node. Link quality is stored on a 0-63 scale and aged out if stale; matrix capacity eviction favors stronger links.
- Message sizing rule: only `add_block` and `add_transaction` are fragmented/reassembled; control/echo/request messages should remain single-packet and small.
- TX scheduling is intentionally conservative: pacing between message transmissions, pause/resume behavior around competing multi-packet traffic, and duty-cycle compliance through delay configuration.
- Analyzer modes reuse the same UI flow as simulation:
  - Real-time Tracking tails the live central log and supports interactive network control.
  - Log Visualization replays from the beginning for post-mortem analysis.
- Telemetry control path is out-of-band versus LoRa traffic: field stations use LoRa for mesh data and WiFi for telemetry/commands via Telemetry HUB -> Log Collector -> Analyzer.
- Scene/analysis workflow assumptions:
  - 2D simulation is intentional for speed/clarity, while algorithm logic remains compatible with 3D deployment assumptions.
  - `effective_distance` is used as a fast receive prefilter.
  - Obstacles currently block signals fully (no diffraction/reflection model).
- Control commands (Telemetry HUB) load `config.toml` next to the selected scene file. Typical command set includes update interval, log-level/filter changes, arbitrary node commands, update/reboot actions, and measurement starts.
- Connection-matrix dumps for deployed nodes are obtained via log-based export (`/CM`) and reconstructed by analyzer tooling.

## Coding Style & Naming Conventions
- Use rustfmt defaults (4-space indentation).
- Filenames in `snake_case`; types in `UpperCamelCase`; functions/vars in `snake_case`.
- Prefer smaller modules under `src/simulation/` and `src/ui/` to keep logic localized.

## Testing Guidelines
- Use Rust’s built-in test harness (`#[test]`) colocated with the code.
- Keep tests descriptive (e.g., `test_path_loss_at_distance`), and prefer coverage for relay timing, message fragmentation edge cases, and analyzer parsing of telemetry-tagged logs.
- Run `cargo test` before PRs; run targeted scenario checks with representative scene files when changing simulation physics or analyzer behavior.

## Commit & Pull Request Guidelines
- Follow Conventional Commit style seen in history: `feat(ui): ...`, `fix(simulation): ...`.
- PRs should include a concise summary, linked issues, and screenshots/GIFs for UI changes.

## Further References
- https://medium.com/moonblokz/moonblokz-series-part-vii-2-mesh-radio-algorithm-3650af3711f3
- https://medium.com/moonblokz/moonblokz-series-part-vii-3-inside-the-radio-module-d92545624d2b
- https://medium.com/moonblokz/moonblokz-series-part-vii-4-radio-network-simulation-5cc86a721e8c
- https://medium.com/moonblokz/moonblokz-series-part-vii-5-field-testing-infrastructure-6be10e18796c
