# Control Functions V2 Specification: Auto AddBlock

This document specifies the addition of an "Auto AddBlock" control functionality to the MoonBlokz Radio Simulator's real-time analyzer mode. This feature allows operators to configure automatic AddBlock sending intervals on nodes via the Telemetry Hub.

---

## Table of Contents

1. [Overview](#overview)
2. [UI Components](#ui-components)
3. [Modal Dialog Specification](#modal-dialog-specification)
4. [Command Specification](#command-specification)
5. [Implementation Details](#implementation-details)
6. [Validation Rules](#validation-rules)

---

## Overview

### Purpose

Add an "Auto AddBlock" control feature that allows operators to:
- Configure automatic AddBlock sending intervals for all nodes (network-wide)
- Configure automatic AddBlock sending intervals for a specific node

### Scope

- **Applies to**: Real-time tracking mode (`OperatingMode::RealtimeTracking`) only
- **Does NOT apply to**: Simulation mode or Log Visualization mode (UI elements hidden/disabled)
- **Command**: Uses the `run_command` protocol with a specially formatted command string

### Command Format

The Auto AddBlock feature uses the existing `run_command` infrastructure with the following command format:

```
/S_{interval}_
```

Where `{interval}` is the send interval in seconds (0 or positive integer).

**Examples:**
- `/S_30_` - Send AddBlock every 30 seconds
- `/S_300_` - Send AddBlock every 300 seconds (5 minutes)
- `/S_0_` - Disable automatic AddBlock sending

---

## UI Components

### Top Panel Modifications

Add a new button to the top panel's control column, placed at the end after the existing control buttons.

#### Updated Layout

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
│ [Auto AddBlock      ]  <-- NEW                          │
└─────────────────────────────────────────────────────────┘
```

#### Button Specification

| Button | Label | Tooltip | Modal Title |
|--------|-------|---------|-------------|
| 4 | "Auto AddBlock" | "Configure automatic AddBlock sending interval for all nodes" | "Auto AddBlock Send" |

### Right Panel Modifications

Add a new button to the right panel. The existing three buttons plus the new button should use a **2x2 grid layout**.

#### Updated Layout

```
┌─────────────────────────────────────────────────────────┐
│ Inspector: Node #1792                                    │
│ ...                                                      │
│                                                          │
│ [Message Table]                                          │
│                                                          │
│ ──────────────────────────────────────────────────────  │
│ ┌──────────────────┬──────────────────┐                 │
│ │  Set Log Level   │  Send Command    │                 │
│ ├──────────────────┼──────────────────┤                 │
│ │Start Measurement │  Auto AddBlock   │  <-- NEW        │
│ └──────────────────┴──────────────────┘                 │
└─────────────────────────────────────────────────────────┘
```

#### Button Specification

| Button | Label | Tooltip | Modal Title |
|--------|-------|---------|-------------|
| 4 | "Auto AddBlock" | "Configure automatic AddBlock sending interval for this node" | "Auto AddBlock Send for #{node_id}" |

---

## Modal Dialog Specification

### Modal: "Auto AddBlock Send" (Network-wide)

**Title:** `Auto AddBlock Send`

**Margin:** Use the same margin as other modal dialogs in the application.

#### Fields

| Field | Type | Label | Validation | Default Value |
|-------|------|-------|------------|---------------|
| Send Interval | Text input | "Send Interval (in seconds)" | Must be 0 or a positive integer | "300" |

#### Buttons

| Button | Label | Action |
|--------|-------|--------|
| Cancel | "Cancel" | Close the modal without sending |
| Send | "Send" | Validate input, send command if valid, close modal |

#### Layout

```
┌─────────────────────────────────────────────────────────┐
│ Auto AddBlock Send                                  [X] │
├─────────────────────────────────────────────────────────┤
│                                                         │
│ Send Interval (in seconds):                             │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ 300                                                 │ │
│ └─────────────────────────────────────────────────────┘ │
│                                                         │
│ [Validation error message, if any - in red]            │
│                                                         │
│                            [ Cancel ]  [ Send ]         │
└─────────────────────────────────────────────────────────┘
```

### Modal: "Auto AddBlock Send for #{node_id}" (Per-node)

**Title:** `Auto AddBlock Send for {node_id}` (e.g., "Auto AddBlock Send for 1792")

**Margin:** Use the same margin as other modal dialogs in the application.

#### Fields

| Field | Type | Label | Validation | Default Value |
|-------|------|-------|------------|---------------|
| Send Interval | Text input | "Send Interval (in seconds)" | Must be 0 or a positive integer | "300" |

#### Buttons

| Button | Label | Action |
|--------|-------|--------|
| Cancel | "Cancel" | Close the modal without sending |
| Send | "Send" | Validate input, send command if valid, close modal |

#### Layout

```
┌─────────────────────────────────────────────────────────┐
│ Auto AddBlock Send for 1792                         [X] │
├─────────────────────────────────────────────────────────┤
│                                                         │
│ Send Interval (in seconds):                             │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ 300                                                 │ │
│ └─────────────────────────────────────────────────────┘ │
│                                                         │
│ [Validation error message, if any - in red]            │
│                                                         │
│                            [ Cancel ]  [ Send ]         │
└─────────────────────────────────────────────────────────┘
```

---

## Command Specification

### Network-wide Auto AddBlock

When "Send" is clicked in the network-wide modal:

**Command Generation:**
```
run_command(command="/S_{interval}_")
```

**Example with 30-second interval:**
```
run_command(command="/S_30_")
```

**JSON Payload:**
```json
{
  "command": "run_command",
  "parameters": {
    "command": "/S_30_"
  }
}
```

### Per-node Auto AddBlock

When "Send" is clicked in the per-node modal:

**Command Generation:**
```
run_command(node_id={node_id}, command="/S_{interval}_")
```

**Example for node 1792 with 30-second interval:**
```
run_command(node_id=1792, command="/S_30_")
```

**JSON Payload:**
```json
{
  "command": "run_command",
  "parameters": {
    "node_id": 1792,
    "command": "/S_30_"
  }
}
```

---

## Implementation Details

### Modal State Additions

Add the following fields to `ControlModalState`:

```rust
/// Modal dialog state for control commands.
#[derive(Debug, Clone)]
pub struct ControlModalState {
    // ... existing fields ...

    // Auto AddBlock modal fields
    pub auto_addblock_interval: String,  // e.g., "300"
}

impl Default for ControlModalState {
    fn default() -> Self {
        // ... existing defaults ...
        Self {
            // ... existing fields ...
            auto_addblock_interval: "300".to_string(),
        }
    }
}
```

### Modal Type Addition

Add a new variant to `ControlModalType`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlModalType {
    SetUpdateInterval,
    SetLogLevel,
    SendCommand,
    AutoAddBlock,  // NEW
}
```

### Command Construction

The command is constructed using the existing `ControlCommand::RunCommand` variant:

```rust
// Network-wide Auto AddBlock
let command = ControlCommand::RunCommand {
    node_id: None,
    command: format!("/S_{}_", interval),
};

// Per-node Auto AddBlock
let command = ControlCommand::RunCommand {
    node_id: Some(node_id),
    command: format!("/S_{}_", interval),
};
```

### Top Panel Button Implementation

```rust
// In top_panel.rs, within the controls section (RealtimeTracking mode only):

if ui.button("Auto AddBlock")
    .on_hover_text("Configure automatic AddBlock sending interval for all nodes")
    .clicked()
{
    // Reset modal state with defaults
    app_state.control_modal.auto_addblock_interval = "300".to_string();
    app_state.control_modal.target_node_id = None;
    app_state.control_modal.validation_error = None;
    app_state.control_modal.active_modal = Some(ControlModalType::AutoAddBlock);
}
```

### Right Panel Button Implementation

```rust
// In right_panel.rs, change from vertical layout to 2x2 grid:

egui::Grid::new("node_control_buttons")
    .num_columns(2)
    .spacing([8.0, 8.0])
    .show(ui, |ui| {
        // Row 1
        if ui.button("Set Log Level")
            .on_hover_text("Set log level and filter for this node")
            .clicked()
        {
            // ... existing implementation ...
        }
        
        if ui.button("Send Command")
            .on_hover_text("Send a custom command to this node")
            .clicked()
        {
            // ... existing implementation ...
        }
        ui.end_row();
        
        // Row 2
        if ui.button("Start Measurement")
            .on_hover_text("Start a measurement from this node")
            .clicked()
        {
            // ... existing implementation ...
        }
        
        if ui.button("Auto AddBlock")
            .on_hover_text("Configure automatic AddBlock sending interval for this node")
            .clicked()
        {
            app_state.control_modal.auto_addblock_interval = "300".to_string();
            app_state.control_modal.target_node_id = Some(selected_node_id);
            app_state.control_modal.validation_error = None;
            app_state.control_modal.active_modal = Some(ControlModalType::AutoAddBlock);
        }
        ui.end_row();
    });
```

### Modal Rendering Implementation

```rust
// In the modal rendering section (e.g., app_state.rs or a dedicated modal.rs):

fn render_auto_addblock_modal(
    ctx: &egui::Context,
    control_modal: &mut ControlModalState,
    ui_command_tx: &Sender<UICommand>,
) {
    let title = match control_modal.target_node_id {
        Some(node_id) => format!("Auto AddBlock Send for {}", node_id),
        None => "Auto AddBlock Send".to_string(),
    };

    egui::Window::new(&title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.add_space(8.0);
            
            ui.horizontal(|ui| {
                ui.label("Send Interval (in seconds):");
            });
            
            ui.add_space(4.0);
            
            ui.add(
                egui::TextEdit::singleline(&mut control_modal.auto_addblock_interval)
                    .desired_width(f32::INFINITY)
            );
            
            ui.add_space(8.0);
            
            // Show validation error if any
            if let Some(ref error) = control_modal.validation_error {
                ui.colored_label(egui::Color32::RED, error);
                ui.add_space(4.0);
            }
            
            ui.add_space(8.0);
            
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Send button
                    if ui.button("Send").clicked() {
                        match validate_auto_addblock_interval(&control_modal.auto_addblock_interval) {
                            Ok(interval) => {
                                let command = ControlCommand::RunCommand {
                                    node_id: control_modal.target_node_id,
                                    command: format!("/S_{}_", interval),
                                };
                                let _ = ui_command_tx.try_send(UICommand::SendControlCommand(command));
                                control_modal.active_modal = None;
                                control_modal.validation_error = None;
                            }
                            Err(error) => {
                                control_modal.validation_error = Some(error);
                            }
                        }
                    }
                    
                    // Cancel button
                    if ui.button("Cancel").clicked() {
                        control_modal.active_modal = None;
                        control_modal.validation_error = None;
                    }
                });
            });
        });
}
```

---

## Validation Rules

### Send Interval Validation

The send interval field must satisfy the following rules:

1. **Non-empty**: The field cannot be empty
2. **Numeric**: The value must be a valid integer
3. **Non-negative**: The value must be 0 or positive (≥ 0)

### Validation Function

```rust
/// Validate the auto addblock interval input.
///
/// # Arguments
/// * `input` - The raw string input from the text field
///
/// # Returns
/// * `Ok(u32)` - The validated interval value
/// * `Err(String)` - A human-readable error message
fn validate_auto_addblock_interval(input: &str) -> Result<u32, String> {
    let trimmed = input.trim();
    
    if trimmed.is_empty() {
        return Err("Send interval is required".to_string());
    }
    
    match trimmed.parse::<i64>() {
        Ok(value) if value >= 0 => Ok(value as u32),
        Ok(_) => Err("Send interval must be 0 or a positive number".to_string()),
        Err(_) => Err("Send interval must be a valid number".to_string()),
    }
}
```

### Error Messages

| Condition | Error Message |
|-----------|---------------|
| Empty input | "Send interval is required" |
| Non-numeric input | "Send interval must be a valid number" |
| Negative number | "Send interval must be 0 or a positive number" |

---

## Testing Checklist

### Functional Tests

- [ ] Top panel "Auto AddBlock" button is visible only in RealtimeTracking mode
- [ ] Top panel button opens modal with title "Auto AddBlock Send"
- [ ] Right panel "Auto AddBlock" button is visible only in RealtimeTracking mode
- [ ] Right panel button opens modal with title "Auto AddBlock Send for {node_id}"
- [ ] Right panel uses 2x2 grid layout for all four buttons
- [ ] Modal default value is "300"
- [ ] Clicking Cancel closes modal without sending command
- [ ] Validation error appears for empty input
- [ ] Validation error appears for non-numeric input
- [ ] Validation error appears for negative numbers
- [ ] Valid input of "0" is accepted
- [ ] Valid input of positive integers is accepted
- [ ] Network-wide command sends correct payload without node_id
- [ ] Per-node command sends correct payload with node_id
- [ ] Modal closes after successful send

### Edge Cases

- [ ] Leading/trailing whitespace in input is handled
- [ ] Very large numbers are handled (within u32 range)
- [ ] Modal state resets properly between opens
- [ ] Multiple rapid clicks don't cause issues

---

## Summary

This specification adds the "Auto AddBlock" control feature with:

1. **Top Panel**: New button for network-wide Auto AddBlock configuration
2. **Right Panel**: New button in a 2x2 grid layout for per-node Auto AddBlock configuration
3. **Modal Dialog**: Consistent styling with other modals, single input field for send interval
4. **Validation**: Input must be 0 or a positive integer
5. **Command**: Uses existing `run_command` infrastructure with `/S_{interval}_` format
