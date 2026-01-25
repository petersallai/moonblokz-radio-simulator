# Repository Guidelines

## Project Structure & Module Organization
- `src/` houses the Rust app. Core areas: `src/main.rs` (app entry and UI wiring), `src/simulation/` (multi-node simulator and radio physics), `src/analyzer/` (log parsing + real-time/log visualization modes), `src/common/` (shared scene loading/validation), `src/control/` (Telemetry Hub commands), `src/ui/` (egui panels/state), and `src/time_driver.rs` (virtual time).
- `scenes/` contains JSON scene files (nodes, obstacles, radio parameters).
- `docs/` and `docs/images/` contain design notes and screenshots. Spec files in the repo root (e.g., `analyzer_detailed_spec.md`, `control_functions*.md`) describe planned behavior.
- `icons/` stores app icon assets. `target/` is local build output.

## Build, Test, and Development Commands
- `cargo run` builds and launches the desktop simulator.
- `cargo build` compiles without running.
- `cargo test` runs unit tests (geometry, time driver, analyzer parsing, radio math).
- `cargo fmt` formats code; `cargo clippy` runs lint checks.

## Architecture Overview
- The simulator runs many nodes in one process. Each node has a dedicated task that drives the shared embedded radio logic through input/output queues, while a central network task computes propagation, collisions, and delivery. Queues follow an mpsc pattern: many node senders, one network receiver, and one receiver per node.
- The embedded radio library expects `'static` queues; the simulator uses heap allocation with `Box::leak()` for per-node queues to keep the embedded code unchanged. A simulated radio device bridges these queues so packetization, scheduling, and relay logic match real hardware behavior.
- Analyzer modes reuse the same UI queues as simulation: Real-time Tracking tails a live log file; Log Visualization replays from start.
- Scene JSONs define nodes, obstacles, and radio settings; analyzer modes expect per-node `effective_distance`. Control commands (Telemetry Hub) load `config.toml` placed next to the selected scene file.

## Coding Style & Naming Conventions
- Use rustfmt defaults (4-space indentation).
- Filenames in `snake_case`; types in `UpperCamelCase`; functions/vars in `snake_case`.
- Prefer smaller modules under `src/simulation/` and `src/ui/` to keep logic localized.

## Testing Guidelines
- Use Rust’s built-in test harness (`#[test]`) colocated with the code.
- Keep tests descriptive (e.g., `test_path_loss_at_distance`) and run `cargo test` before PRs.

## Commit & Pull Request Guidelines
- Follow Conventional Commit style seen in history: `feat(ui): ...`, `fix(simulation): ...`.
- PRs should include a concise summary, linked issues, and screenshots/GIFs for UI changes.
