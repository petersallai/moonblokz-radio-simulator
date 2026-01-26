# Code Review Findings

Date: 2026-01-25
Scope: Rust simulator/analyzer UI, simulation core, analyzer parsing, control client, common scene loading.

## Findings (ordered by severity)

### Critical
1) Collision/capture logic uses inconsistent units and reversed comparison, which likely breaks collision modeling.
   - In `process_packet_reception`, preamble lock loss compares `other_packet.rssi` (dBm) directly to `snr_limit` (dB), which are not comparable; this makes destructive collisions almost never trigger or trigger incorrectly depending on values. Capture effect check also compares `packet_rssi - other_packet.rssi > CAPTURE_THRESHOLD`, which is the wrong direction for “later stronger packet captures.” This prevents later-stronger packets from capturing earlier ones and flips the condition. This will materially skew collision statistics and RX delivery. 
   - Location: `src/simulation/network.rs:799-806`.
   - Suggested fix: Compare SNR (or RSSI relative to noise floor) to `snr_limit`, and reverse the capture comparison to `other_packet.rssi - packet_rssi > CAPTURE_THRESHOLD` for later-starting packets.

### High
2) Scene scaling can divide by zero (or NaN) when world bounds are degenerate, which then cascades into mapping errors and potential panics.
   - Both `common::scene::load_scene` and `simulation::network::load_scene` compute `scale_x/scale_y` using `(world_bottom_right - world_top_left)`. There is no validation that these deltas are non‑zero, and `validate_scene` doesn’t enforce width/height or world span positivity. Degenerate or inverted bounds will cause division by zero and break coordinate transforms in the map and physics.
   - Locations: `src/common/scene.rs:213-218`, `src/simulation/network.rs:266-271`, `src/common/scene.rs:235-340`.
   - Suggested fix: Validate `world_bottom_right.x != world_top_left.x`, `world_bottom_right.y != world_top_left.y`, and `width/height > 0` in scene validation (both common and simulation paths) before computing scales.

3) Mode selector icons are loaded under a shared texture name (`"icon"`), which can cause all icons to render as the same image (the last loaded) and/or be reuploaded every frame.
   - `render_icon` always uses `load_texture("icon", ...)` so each call overwrites the texture backing all icons. In practice, the three panels can display the same icon or flicker depending on update order.
   - Location: `src/ui/mode_selector.rs:270-279`.
   - Suggested fix: Use distinct texture IDs per icon (e.g., `icon_simulation`, `icon_realtime`, `icon_log`) and cache the `TextureHandle`s in `ModeSelector` instead of reloading each frame.

### Medium
4) Simulation time displayed in the top panel is not based on `start_time`, so it reports an absolute (virtual) time instead of “time since simulation start.”
   - In Simulation mode, top panel uses `embassy_time::Instant::now().as_secs()` rather than `Instant::now() - start_time` or `start_time.elapsed()`. This makes the displayed “Sim time” unrelated to the current run and inconsistent with other UI elements.
   - Location: `src/ui/top_panel.rs:62-67`.
   - Suggested fix: `let sim_secs = state.start_time.elapsed().as_secs();` and reset `start_time` when a scene/mode starts.

5) UI polling for `RequestNodeInfo` is tied to virtual time, so the request cadence changes with simulation speed.
   - `last_node_info_update` uses `embassy_time::Instant`, and `elapsed()` scales with the time driver. At higher sim speeds, the UI will spam node-info requests; at lower speeds, it will stall.
   - Location: `src/ui/app_state.rs:166-170`, `src/ui/app_state.rs:1054-1060`.
   - Suggested fix: Use `std::time::Instant` for UI‑local throttling (real time), or explicitly document that requests are scaled with simulation speed.

### Low
6) Debug logging in `parse_tm3` runs at `info` level for every *TM3* line, which is noisy and can materially slow log visualization on large logs.
   - Location: `src/analyzer/log_parser.rs:262-264`.
   - Suggested fix: remove these logs or drop to `trace` behind a feature flag.

## Tests / Coverage Gaps
- No tests cover collision/capture modeling logic in `process_packet_reception` (preamble lock and capture effect). Adding targeted tests would prevent regressions for the collision model.
- Scene validation lacks tests for zero/negative world extents; a unit test for degenerate bounds would catch divide-by-zero issues early.
- UI: no test coverage for mode selector icon loading; a minimal egui integration test (or manual check) should confirm unique textures per icon.

## Notes / Assumptions
- This review assumes `RadioPacket.data` is always at least 9 bytes (fixed-size buffer), so indexing offsets 5–8 is safe; if that is not guaranteed, those reads should be length-checked.

