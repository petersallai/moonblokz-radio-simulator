# Control Functions Specification

This document specifies the control functionality to be added to the MoonBlokz Radio Simulator's real-time analyzer mode. The control features enable operators to send commands to remote nodes via the Telemetry Hub, following the same protocol used by the Telemetry CLI.

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Configuration](#configuration)
4. [Control Module](#control-module)
5. [UI Components](#ui-components)
6. [Command Specifications](#command-specifications)
7. [Implementation Details](#implementation-details)
8. [Error Handling](#error-handling)

---

## Overview

### Purpose

Add remote control capabilities to the real-time analyzer mode, allowing operators to:
- Set network-wide update intervals with active/inactive scheduling
- Adjust log levels and filters for all nodes or specific nodes
- Send arbitrary commands to nodes
- Initiate network-wide measurements

### Scope

- **Applies to**: Real-time tracking mode (`OperatingMode::RealtimeTracking`) only
- **Does NOT apply to**: Simulation mode or Log Visualization mode (UI elements hidden/disabled)
- **Protocol**: Uses the same Telemetry CLI protocol as defined in `moonblokz_test_infrastructure_full_spec.md`

---

## Architecture

### Component Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                     UI Layer                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │  Top Panel   │  │ Right Panel  │  │   Modal Dialogs  │  │
│  │  (Network    │  │ (Per-Node    │  │                  │  │
│  │   Commands)  │  │  Commands)   │  │                  │  │
│  └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘  │
│         │                 │                    │            │
│         └─────────────────┼────────────────────┘            │
│                           │                                  │
│                    ┌──────▼───────┐                         │
│                    │  AppState    │                         │
│                    │  (Modal      │                         │
│                    │   State)     │                         │
│                    └──────┬───────┘                         │
└───────────────────────────┼─────────────────────────────────┘
                            │ UICommand::SendControlCommand
                            ▼
┌───────────────────────────────────────────────────────────────┐
│                    Control Module                              │
│  ┌────────────────┐  ┌─────────────────┐  ┌────────────────┐ │
│  │ ControlConfig  │  │ ControlCommand  │  │ TelemetryClient│ │
│  │ (config.toml)  │  │ (Command Types) │  │ (HTTP Client)  │ │
│  └────────────────┘  └─────────────────┘  └────────────────┘ │
└───────────────────────────────────────────────────────────────┘
                            │
                            │ HTTPS POST /command
                            ▼
                 ┌──────────────────────┐
                 │   Telemetry Hub      │
                 │   (External Service) │
                 └──────────────────────┘
```

### Data Flow

1. User clicks a control button in the UI
2. Modal dialog opens for parameter input
3. User fills in parameters and clicks "Send"
4. UI validates input and constructs a `ControlCommand`
5. `UICommand::SendControlCommand(ControlCommand)` is sent via the command channel
6. Analyzer task receives the command and delegates to the control module
7. Control module sends HTTPS POST to the Telemetry Hub `/command` endpoint
8. Response is logged; errors are displayed as alerts

---

## Configuration

### Config File Location

The control module configuration is read from a `config.toml` file located in the **same directory as the scene file**.

Example: If the scene file is `/path/to/scenes/analyzer/diosd.json`, the config file should be `/path/to/scenes/analyzer/config.toml`.

### Config File Format

```toml
# Telemetry Hub connection settings for control commands

# The API key for authenticating with the Telemetry Hub's /command endpoint
# This should match the cli_api_key configured on the hub
api_key = "your-secret-api-key-here"

# The base URL of the Telemetry Hub (without the /command suffix)
hub_url = "https://your-telemetry-hub.example.com"
```

### Configuration Struct

```rust
/// Configuration for connecting to the Telemetry Hub.
#[derive(Debug, Clone, Deserialize)]
pub struct ControlConfig {
    /// API key for the hub's /command endpoint
    pub api_key: String,
    /// Base URL of the hub (without /command suffix)
    pub hub_url: String,
}
```

### Loading Behavior

1. When the analyzer task starts with a scene file, derive the config path by replacing the scene filename with `config.toml`
2. Attempt to load and parse the config file
3. If the file is missing or invalid:
   - Log a warning
   - Control buttons remain visible but disabled
   - Show tooltip explaining the configuration requirement
4. If successfully loaded:
   - Store in `AnalyzerState`
   - Enable control buttons

---

## Control Module

### Module Structure

Create a new folder `src/control/` with the following files:

```
src/control/
├── mod.rs           # Module exports and re-exports
├── config.rs        # Configuration loading and parsing
├── command.rs       # Command type definitions
└── client.rs        # HTTP client for Telemetry Hub communication
```

### File: `src/control/mod.rs`

```rust
//! Control module for sending commands to nodes via the Telemetry Hub.
//!
//! This module implements the same protocol as the Telemetry CLI, allowing
//! the simulator's real-time analyzer to send commands to remote nodes.

pub mod config;
pub mod command;
pub mod client;

pub use config::ControlConfig;
pub use command::{ControlCommand, LogLevel};
pub use client::TelemetryClient;
```

### File: `src/control/config.rs`

```rust
//! Configuration loading for the control module.

use serde::Deserialize;
use std::path::Path;

/// Configuration for connecting to the Telemetry Hub.
#[derive(Debug, Clone, Deserialize)]
pub struct ControlConfig {
    /// API key for the hub's /command endpoint
    pub api_key: String,
    /// Base URL of the hub (without /command suffix)
    pub hub_url: String,
}

impl ControlConfig {
    /// Load configuration from a TOML file.
    ///
    /// # Arguments
    /// * `config_path` - Path to the config.toml file
    ///
    /// # Returns
    /// * `Ok(ControlConfig)` if the file was successfully loaded and parsed
    /// * `Err(String)` with a descriptive error message otherwise
    pub fn load(config_path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;
        
        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse config file: {}", e))
    }

    /// Derive the config path from a scene file path.
    ///
    /// Replaces the scene filename with "config.toml" in the same directory.
    pub fn config_path_from_scene(scene_path: &str) -> std::path::PathBuf {
        let scene = Path::new(scene_path);
        scene.parent()
            .unwrap_or(Path::new("."))
            .join("config.toml")
    }
}
```

### File: `src/control/command.rs`

```rust
//! Command type definitions for the control module.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Log level values matching the Telemetry CLI specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "TRACE"),
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

/// Commands that can be sent to the Telemetry Hub.
#[derive(Debug, Clone)]
pub enum ControlCommand {
    /// Set the update interval with active/inactive scheduling.
    /// Targets all nodes if node_id is None.
    SetUpdateInterval {
        node_id: Option<u32>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        active_period: u32,
        inactive_period: u32,
    },

    /// Set the log level for a node or all nodes.
    SetLogLevel {
        node_id: Option<u32>,
        log_level: LogLevel,
    },

    /// Set the log filter for a node or all nodes.
    SetLogFilter {
        node_id: Option<u32>,
        log_filter: String,
    },

    /// Send an arbitrary command to a node or all nodes.
    RunCommand {
        node_id: Option<u32>,
        command: String,
    },

    /// Start a measurement on a specific node.
    StartMeasurement {
        node_id: u32,
        sequence: u32,
    },
}

/// JSON payload structure for the Telemetry Hub /command endpoint.
#[derive(Debug, Serialize)]
pub struct CommandPayload {
    pub command: String,
    pub parameters: serde_json::Value,
}

impl ControlCommand {
    /// Convert the command to a JSON payload for the Telemetry Hub.
    pub fn to_payload(&self) -> CommandPayload {
        match self {
            ControlCommand::SetUpdateInterval {
                node_id,
                start_time,
                end_time,
                active_period,
                inactive_period,
            } => {
                let mut params = serde_json::json!({
                    "start_time": start_time.to_rfc3339(),
                    "end_time": end_time.to_rfc3339(),
                    "active_period": active_period,
                    "inactive_period": inactive_period,
                });
                if let Some(id) = node_id {
                    params["node_id"] = serde_json::json!(id);
                }
                CommandPayload {
                    command: "set_update_interval".to_string(),
                    parameters: params,
                }
            }

            ControlCommand::SetLogLevel { node_id, log_level } => {
                let mut params = serde_json::json!({
                    "log_level": log_level.to_string(),
                });
                if let Some(id) = node_id {
                    params["node_id"] = serde_json::json!(id);
                }
                CommandPayload {
                    command: "set_log_level".to_string(),
                    parameters: params,
                }
            }

            ControlCommand::SetLogFilter { node_id, log_filter } => {
                let mut params = serde_json::json!({
                    "log_filter": log_filter,
                });
                if let Some(id) = node_id {
                    params["node_id"] = serde_json::json!(id);
                }
                CommandPayload {
                    command: "set_log_filter".to_string(),
                    parameters: params,
                }
            }

            ControlCommand::RunCommand { node_id, command } => {
                let mut params = serde_json::json!({
                    "command": command,
                });
                if let Some(id) = node_id {
                    params["node_id"] = serde_json::json!(id);
                }
                CommandPayload {
                    command: "command".to_string(),
                    parameters: params,
                }
            }

            ControlCommand::StartMeasurement { node_id, sequence } => {
                CommandPayload {
                    command: "start_measurement".to_string(),
                    parameters: serde_json::json!({
                        "node_id": node_id,
                        "sequence": sequence,
                    }),
                }
            }
        }
    }
}
```

### File: `src/control/client.rs`

```rust
//! HTTP client for communicating with the Telemetry Hub.

use super::{ControlConfig, ControlCommand};
use reqwest::blocking::Client;
use std::time::Duration;

/// Client for sending commands to the Telemetry Hub.
pub struct TelemetryClient {
    client: Client,
    config: ControlConfig,
}

impl TelemetryClient {
    /// Create a new TelemetryClient with the given configuration.
    pub fn new(config: ControlConfig) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self { client, config })
    }

    /// Send a command to the Telemetry Hub.
    ///
    /// # Returns
    /// * `Ok(())` if the command was accepted (HTTP 200)
    /// * `Err(String)` with error details otherwise
    pub fn send_command(&self, command: &ControlCommand) -> Result<(), String> {
        let url = format!("{}/command", self.config.hub_url);
        let payload = command.to_payload();

        log::debug!("Sending command to {}: {:?}", url, payload);

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Api-Key", &self.config.api_key)
            .json(&payload)
            .send()
            .map_err(|e| format!("Network error: {}", e))?;

        let status = response.status();
        if status.is_success() {
            log::info!("Command sent successfully");
            Ok(())
        } else if status.is_client_error() {
            let body = response.text().unwrap_or_default();
            Err(format!("Command error ({}): {}", status.as_u16(), body))
        } else {
            let body = response.text().unwrap_or_default();
            Err(format!("Server error ({}): {}", status.as_u16(), body))
        }
    }
}
```

---

## UI Components

### Modal Dialog State

Add the following to `AppState`:

```rust
/// Modal dialog state for control commands.
#[derive(Debug, Clone)]
pub struct ControlModalState {
    /// Which modal is currently open, if any.
    pub active_modal: Option<ControlModalType>,

    // Set Update Interval modal fields
    pub update_interval_active: String,      // e.g., "30"
    pub update_interval_inactive: String,    // e.g., "300"
    pub update_interval_start_date: chrono::NaiveDate,  // Date selection via egui_extras
    pub update_interval_start_time: String,             // Time as "HH:MM:SS"
    pub update_interval_end_date: chrono::NaiveDate,    // Date selection via egui_extras
    pub update_interval_end_time: String,               // Time as "HH:MM:SS"

    // Set Log Level modal fields
    pub log_level: LogLevel,
    pub log_filter: String,

    // Send Command modal field
    pub command_text: String,

    // Target node for per-node modals (None = all nodes)
    pub target_node_id: Option<u32>,
    
    // Validation error message, if any
    pub validation_error: Option<String>,
}

impl Default for ControlModalState {
    fn default() -> Self {
        let today = chrono::Local::now().date_naive();
        Self {
            active_modal: None,
            update_interval_active: String::new(),
            update_interval_inactive: String::new(),
            update_interval_start_date: today,
            update_interval_start_time: "00:00:00".to_string(),
            update_interval_end_date: today,
            update_interval_end_time: "23:59:59".to_string(),
            log_level: LogLevel::Info,
            log_filter: String::new(),
            command_text: String::new(),
            target_node_id: None,
            validation_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlModalType {
    SetUpdateInterval,
    SetLogLevel,
    SendCommand,
}
```

### New UICommand Variants

Add to the `UICommand` enum:

```rust
pub enum UICommand {
    // ... existing variants ...
    
    /// Send a control command to the Telemetry Hub.
    SendControlCommand(ControlCommand),
}
```

### Top Panel Modifications

Add the following buttons to the top panel's control column (only visible in `RealtimeTracking` mode):

#### Layout

```
┌─────────────────────────────────────────────────────────┐
│ Controls                                                 │
│ ┌─────────────────┐                                     │
│ │ Speed: [slider] │                                     │
│ └─────────────────┘                                     │
│ ☐ Auto-speed                                            │
│                                                          │
│ ─────── Network Commands ───────                        │
│ [Set Update Interval]                                   │
│ [Set Log Level      ]                                   │
│ [Send Command       ]                                   │
└─────────────────────────────────────────────────────────┘
```

#### Button Specifications

| Button | Label | Tooltip | Modal Title |
|--------|-------|---------|-------------|
| 1 | "Set Update Interval" | "Configure active/inactive update intervals for all nodes" | "Set Network Update Interval" |
| 2 | "Set Log Level" | "Set log level and filter for all nodes" | "Set Log Level for All Nodes" |
| 3 | "Send Command" | "Send a custom command to all nodes" | "Send Command to All Nodes" |

### Right Panel Modifications

Add two buttons below the existing "Start Measurement" button (only visible in `RealtimeTracking` mode):

#### Layout

```
┌─────────────────────────────────────────────────────────┐
│ Inspector: Node #1792                                    │
│ ...                                                      │
│                                                          │
│ [Message Table]                                          │
│                                                          │
│ ──────────────────────────────────────────────────────  │
│ [    Set Log Level     ]                                │
│ [    Send Command      ]                                │
│ [   Start Measurement  ]                                │
└─────────────────────────────────────────────────────────┘
```

#### Button Specifications

| Button | Label | Tooltip | Modal Title |
|--------|-------|---------|-------------|
| 1 | "Set Log Level" | "Set log level and filter for this node" | "Set Log Level for #{node_id}" |
| 2 | "Send Command" | "Send a custom command to this node" | "Send Command to #{node_id}" |
| 3 | "Start Measurement" | (existing) | N/A |

---

## Command Specifications

### 1. Set Update Interval

#### Modal: "Set Network Update Interval"

| Field | Type | Validation | Default |
|-------|------|------------|---------|
| Update Interval (active) | Text input (seconds) | Must be a positive integer | Empty |
| Update Interval (inactive) | Text input (seconds) | Must be a positive integer | Empty |
| Active time start (date) | egui_extras DateChooser | Valid date | Today |
| Active time start (time) | Text input (HH:MM:SS) | Valid 24-hour time | 00:00:00 |
| Active time end (date) | egui_extras DateChooser | Valid date, >= start date | Today |
| Active time end (time) | Text input (HH:MM:SS) | Valid 24-hour time | 23:59:59 |

#### Command Generation

When "Send" is clicked:
```
set_update_interval(start_time=2026-01-04T00:00:00Z, end_time=2026-01-09T00:00:00Z, active_period=30, inactive_period=300)
```

#### JSON Payload

```json
{
  "command": "set_update_interval",
  "parameters": {
    "start_time": "2026-01-04T00:00:00Z",
    "end_time": "2026-01-09T00:00:00Z",
    "active_period": 30,
    "inactive_period": 300
  }
}
```

### 2. Set Log Level

#### Modal: "Set Log Level for All Nodes" / "Set Log Level for #{node_id}"

| Field | Type | Validation | Default |
|-------|------|------------|---------|
| Log level | Dropdown | One of: Trace, Debug, Info, Warn, Error | Info |
| Log filter | Text input | Any string (may be empty) | Empty |

#### Command Generation

When "Send" is clicked, send TWO commands:

1. Log level command:
   - All nodes: `set_log_level(log_level=TRACE)`
   - Specific node: `set_log_level(node_id=1792, log_level=TRACE)`

2. Log filter command:
   - All nodes: `set_log_filter(log_filter=TM8)` or `set_log_filter(log_filter=)` for empty
   - Specific node: `set_log_filter(node_id=1792, log_filter=TM8)` or `set_log_filter(node_id=1792, log_filter=)`

#### JSON Payloads

**Log Level (all nodes):**
```json
{
  "command": "set_log_level",
  "parameters": {
    "log_level": "TRACE"
  }
}
```

**Log Level (specific node):**
```json
{
  "command": "set_log_level",
  "parameters": {
    "node_id": 1792,
    "log_level": "TRACE"
  }
}
```

**Log Filter (all nodes):**
```json
{
  "command": "set_log_filter",
  "parameters": {
    "log_filter": "TM8"
  }
}
```

**Log Filter (specific node):**
```json
{
  "command": "set_log_filter",
  "parameters": {
    "node_id": 1792,
    "log_filter": "TM8"
  }
}
```

### 3. Send Command

#### Modal: "Send Command to All Nodes" / "Send Command to #{node_id}"

| Field | Type | Validation | Default |
|-------|------|------------|---------|
| Command | Text input | Non-empty string | Empty |

#### Command Generation

- All nodes: `command(command=YOUR_COMMAND)`
- Specific node: `command(node_id=1792, command=YOUR_COMMAND)`

#### JSON Payloads

**All nodes:**
```json
{
  "command": "command",
  "parameters": {
    "command": "YOUR_COMMAND"
  }
}
```

**Specific node:**
```json
{
  "command": "command",
  "parameters": {
    "node_id": 1792,
    "command": "YOUR_COMMAND"
  }
}
```

### 4. Start Measurement

#### Behavior (Real-time Mode)

When "Start Measurement" is clicked in real-time mode:

1. Generate a new measurement identifier: `rand::random::<u32>() % 100001` (range 0-100000)
2. Set `measurement_start_time` to current time
3. Clear `reached_nodes` and insert the selected node
4. Send `start_measurement` command to the Telemetry Hub

#### JSON Payload

```json
{
  "command": "start_measurement",
  "parameters": {
    "node_id": 21,
    "sequence": 1
  }
}
```

**Note:** The `sequence` field should be set to the selected node's ID to match the simulation mode behavior.

---

## Implementation Details

### Analyzer Task Modifications

The analyzer task needs to:

1. Load `config.toml` from the scene directory on startup
2. Create a `TelemetryClient` if the config is valid
3. Handle `UICommand::SendControlCommand` in the main loop
4. Send commands via the `TelemetryClient`
5. Report errors back to the UI via `UIRefreshState::Alert`

```rust
// In analyzer_task:

// After loading scene, load control config
let control_config = ControlConfig::config_path_from_scene(&scene_path);
let telemetry_client = match ControlConfig::load(&control_config) {
    Ok(config) => {
        log::info!("Loaded control config from {:?}", control_config);
        Some(TelemetryClient::new(config).ok()?)
    }
    Err(e) => {
        log::warn!("Control config not available: {}", e);
        None
    }
};

// Notify UI about control availability
let _ = ui_refresh_tx.send(UIRefreshState::ControlAvailable(telemetry_client.is_some())).await;

// In the main loop, handle control commands:
UICommand::SendControlCommand(cmd) => {
    if let Some(ref client) = telemetry_client {
        if let Err(e) = client.send_command(&cmd) {
            let _ = ui_refresh_tx.send(UIRefreshState::Alert(format!("Command failed: {}", e))).await;
        }
    } else {
        let _ = ui_refresh_tx.send(UIRefreshState::Alert(
            "Control not configured. Add config.toml to the scene directory.".to_string()
        )).await;
    }
}
```

### New UIRefreshState Variant

```rust
pub enum UIRefreshState {
    // ... existing variants ...
    
    /// Indicates whether control commands are available.
    ControlAvailable(bool),
}
```

### DateTime Picker Implementation

Use `egui_extras::DateChooserButton` for date selection combined with a time text input:

```rust
use egui_extras::DateChooserButton;
use chrono::NaiveDate;

/// Render a datetime picker with egui_extras DateChooser for date and text input for time.
fn render_datetime_picker(
    ui: &mut egui::Ui,
    label: &str,
    date: &mut NaiveDate,
    time: &mut String,
) -> bool {
    let mut changed = false;
    
    ui.horizontal(|ui| {
        ui.label(label);
    });
    
    ui.horizontal(|ui| {
        // Date picker using egui_extras
        if ui.add(DateChooserButton::new(date).id_source(label)).changed() {
            changed = true;
        }
        
        // Time input (HH:MM:SS format)
        ui.label("Time:");
        let time_response = ui.add(
            egui::TextEdit::singleline(time)
                .desired_width(80.0)
                .hint_text("HH:MM:SS")
        );
        if time_response.changed() {
            changed = true;
        }
    });
    
    changed
}

/// Validate time string format (HH:MM:SS)
fn validate_time_string(time: &str) -> bool {
    chrono::NaiveTime::parse_from_str(time, "%H:%M:%S").is_ok()
}

/// Combine date and time into a UTC DateTime
fn combine_datetime(
    date: chrono::NaiveDate,
    time: &str,
) -> Result<chrono::DateTime<chrono::Utc>, String> {
    let time = chrono::NaiveTime::parse_from_str(time, "%H:%M:%S")
        .map_err(|e| format!("Invalid time format: {}", e))?;
    let naive_dt = date.and_time(time);
    Ok(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive_dt, chrono::Utc))
}
```

**Note**: Add `egui_extras` with the `datepicker` feature to `Cargo.toml`:

```toml
egui_extras = { version = "0.27", features = ["datepicker"] }
```

### Modal Rendering

Modals should be rendered using `egui::Window` with:
- `collapsible(false)` - No collapse button
- `resizable(false)` - Fixed size
- `anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])` - Centered on screen

```rust
use egui_extras::DateChooserButton;

fn render_control_modal(ctx: &egui::Context, state: &mut AppState) {
    if let Some(modal_type) = &state.control_modal.active_modal.clone() {
        let title = match modal_type {
            ControlModalType::SetUpdateInterval => "Set Network Update Interval".to_string(),
            ControlModalType::SetLogLevel => {
                if let Some(node_id) = state.control_modal.target_node_id {
                    format!("Set Log Level for #{}", node_id)
                } else {
                    "Set Log Level for All Nodes".to_string()
                }
            }
            ControlModalType::SendCommand => {
                if let Some(node_id) = state.control_modal.target_node_id {
                    format!("Send Command to #{}", node_id)
                } else {
                    "Send Command to All Nodes".to_string()
                }
            }
        };

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                match modal_type {
                    ControlModalType::SetUpdateInterval => {
                        render_set_update_interval_fields(ui, &mut state.control_modal);
                    }
                    // ... other modal types
                    _ => {}
                }

                // Validation error display
                if let Some(error) = &state.control_modal.validation_error {
                    ui.colored_label(egui::Color32::RED, error);
                }

                // Button row
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        state.control_modal.active_modal = None;
                        state.control_modal.validation_error = None;
                    }
                    if ui.button("Send").clicked() {
                        // Validate and send
                    }
                });
            });
    }
}

/// Render the Set Update Interval modal fields with DateChooser.
fn render_set_update_interval_fields(ui: &mut egui::Ui, modal: &mut ControlModalState) {
    ui.heading("Interval Settings");
    
    ui.horizontal(|ui| {
        ui.label("Active interval (seconds):");
        ui.text_edit_singleline(&mut modal.update_interval_active);
    });
    
    ui.horizontal(|ui| {
        ui.label("Inactive interval (seconds):");
        ui.text_edit_singleline(&mut modal.update_interval_inactive);
    });
    
    ui.add_space(10.0);
    ui.heading("Schedule");
    
    // Start date/time using egui_extras DateChooserButton
    ui.horizontal(|ui| {
        ui.label("Start:");
        ui.add(DateChooserButton::new(&mut modal.update_interval_start_date)
            .id_source("start_date"));
        ui.label("Time:");
        ui.add(egui::TextEdit::singleline(&mut modal.update_interval_start_time)
            .desired_width(70.0)
            .hint_text("HH:MM:SS"));
    });
    
    // End date/time using egui_extras DateChooserButton
    ui.horizontal(|ui| {
        ui.label("End:  ");
        ui.add(DateChooserButton::new(&mut modal.update_interval_end_date)
            .id_source("end_date"));
        ui.label("Time:");
        ui.add(egui::TextEdit::singleline(&mut modal.update_interval_end_time)
            .desired_width(70.0)
            .hint_text("HH:MM:SS"));
    });
}
```

---

## Error Handling

### Configuration Errors

| Error | Behavior |
|-------|----------|
| `config.toml` not found | Log warning, disable control buttons, show tooltip explaining requirement |
| `config.toml` parse error | Log warning with details, disable control buttons, show alert on first attempt |
| Missing `api_key` field | Parse error - config invalid |
| Missing `hub_url` field | Parse error - config invalid |

### Validation Errors

| Field | Error Condition | Message |
|-------|-----------------|---------|
| Active interval | Empty or non-numeric | "Active interval must be a positive number" |
| Active interval | Zero or negative | "Active interval must be a positive number" |
| Inactive interval | Empty or non-numeric | "Inactive interval must be a positive number" |
| Inactive interval | Zero or negative | "Inactive interval must be a positive number" |
| Start time | Invalid format | "Invalid start time format. Use HH:MM:SS" |
| End time | Invalid format | "Invalid end time format. Use HH:MM:SS" |
| End datetime | Before start datetime | "End date/time must be after start date/time" |
| Command | Empty | "Command cannot be empty" |

### Network Errors

| Error | Behavior |
|-------|----------|
| Connection timeout | Show alert: "Network error: Connection timed out" |
| DNS failure | Show alert: "Network error: DNS lookup failed" |
| TLS error | Show alert: "Network error: TLS handshake failed" |
| HTTP 401 | Show alert: "Authentication failed. Check API key in config.toml" |
| HTTP 400 | Show alert: "Invalid command: {server message}" |
| HTTP 5xx | Show alert: "Server error ({code}): {message}" |

---

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# ... existing dependencies ...
toml = "0.8"
reqwest = { version = "0.11", features = ["json", "blocking"] }
egui_extras = { version = "0.27", features = ["datepicker"] }
```

**Note:** Using `blocking` feature for reqwest to integrate with the synchronous modal flow. Alternatively, commands can be queued and processed asynchronously in the analyzer task.

---

## Testing

### Manual Testing Checklist

1. **Configuration Loading**
   - [ ] App loads without `config.toml` - control buttons disabled
   - [ ] App loads with valid `config.toml` - control buttons enabled
   - [ ] App loads with malformed `config.toml` - warning logged, buttons disabled

2. **Modal Dialogs**
   - [ ] Each modal opens with correct title
   - [ ] Default values are populated correctly
   - [ ] Cancel button closes modal without sending
   - [ ] Validation errors display correctly
   - [ ] Send button triggers command when validation passes

3. **Command Sending**
   - [ ] Commands are sent to correct endpoint
   - [ ] API key is included in header
   - [ ] Success shows no error
   - [ ] Network errors show alert
   - [ ] HTTP errors show alert with status

4. **Per-Node Commands**
   - [ ] Modal shows correct node ID in title
   - [ ] Commands include node_id parameter
   - [ ] Works for Set Log Level
   - [ ] Works for Send Command
   - [ ] Start Measurement sends correct command

5. **Mode Visibility**
   - [ ] Control buttons visible in RealtimeTracking mode
   - [ ] Control buttons hidden in Simulation mode
   - [ ] Control buttons hidden in LogVisualization mode

---

## Future Enhancements

1. **Command History** - Store recently sent commands for quick resending
2. **Response Display** - Show command response in a status bar
3. **Bulk Operations** - Select multiple nodes and send commands to subset
4. **Preset Commands** - Save commonly used command configurations
5. **Async Command Sending** - Non-blocking command submission with progress indicator
