//! # Right Panel - Node Inspector and Message Stream
//!
//! This module renders the fixed-width right panel displaying detailed information
//! about the currently selected node, including:
//! - Node metadata (ID, position, radio strength)
//! - Message statistics (sent/received counts)
//! - Complete message history in a scrollable, virtualized table
//! - Measurement control button (Start/Reset)
//!
//! ## Message Table
//!
//! The message table uses `egui_extras::TableBuilder` for efficient virtualized rendering.
//! Only visible rows are rendered, allowing smooth scrolling through thousands of messages.
//! Messages are displayed newest-first, with color coding:
//! - Yellow rows: Messages sent by this node
//! - Green rows: Messages received from other nodes
//! - Red rows: Collision detected (packet lost)
//!
//! ## Link Quality Visualization
//!
//! Link quality values are color-coded based on the scoring matrix thresholds:
//! - Red: Poor quality (≤ poor_limit)
//! - Yellow: Medium quality
//! - Green: Excellent quality (≥ excellent_limit)

use crate::simulation::types::LogLevel;
use crate::ui::app_state::InspectorTab;
use crate::ui::{AppState, OperatingMode, UICommand, color_for_message_type};
use chrono::{Local, TimeZone};
use eframe::egui;
use egui::Color32;
use std::cmp::max;

/// Render the right inspector panel.
///
/// If a node is selected, displays:
/// 1. Node metadata at the top (ID, position, radio strength)
/// 2. Statistics (sent/received message counts)
/// 3. Scrollable message table (virtualized for performance)
/// 4. Measurement control button at the bottom
///
/// If no node is selected, displays a centered prompt to select a node.
///
/// # Parameters
///
/// * `ctx` - egui context
/// * `state` - Mutable application state
pub fn render(ctx: &egui::Context, state: &mut AppState) {
    let panel = egui::SidePanel::right("inspector_right")
        .min_width(400.0)
        .default_width(state.right_panel_width)
        .resizable(true);
    let response = panel.show(ctx, |ui| {
        // Top content (default top-down, left-aligned)
        ui.heading("Inspector");
        ui.separator();
        if let Some(i) = state.selected {
            let p = &state.nodes[i];
            ui.horizontal(|ui| {
                ui.label("Selected Node:");
                let node_id_text = format!("#{}", p.node_id);
                let font_id = egui::FontId::default();
                let bg_color = Color32::from_rgb(0, 255, 0); // Green
                let text_color = Color32::BLACK;
                let galley = ui.painter().layout_no_wrap(node_id_text.clone(), font_id.clone(), text_color);
                let text_size = galley.size();
                let padding = egui::vec2(4.0, 2.0);
                let (rect, _response) = ui.allocate_exact_size(egui::vec2(text_size.x + padding.x * 2.0, text_size.y + padding.y * 2.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, bg_color);
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, node_id_text, font_id, text_color);
            });
            // Display version info if available (from TM8 messages in analyzer modes only)
            if state.operating_mode != OperatingMode::Simulation {
                if let Some(node_info) = &state.node_info {
                    ui.horizontal(|ui| {
                        ui.label("Node version:");
                        let node_ver_str = node_info.node_version.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
                        ui.label(egui::RichText::new(node_ver_str).strong());
                        ui.add_space(10.0);
                        ui.label("Probe version:");
                        let probe_ver_str = node_info.probe_version.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
                        ui.label(egui::RichText::new(probe_ver_str).strong());
                    });
                }
            }
            ui.horizontal(|ui| {
                ui.label("Position: (");
                ui.label(egui::RichText::new(format!("{:.5}", p.position.x)).strong());
                ui.label(",");
                ui.label(egui::RichText::new(format!("{:.5}", p.position.y)).strong());
                ui.label(")");
                ui.add_space(10.0);
                ui.label("Radio strength:");
                ui.label(egui::RichText::new(format!("{}", p.radio_strength)).strong());
            });

            let mut sent_messages_count = 0;
            let mut received_messages_count = 0;

            if let Some(node_info) = &state.node_info {
                for msg in &node_info.radio_packets {
                    if msg.sender_node == p.node_id {
                        sent_messages_count += 1;
                    } else {
                        received_messages_count += 1;
                    }
                }
            }

            ui.horizontal(|ui| {
                ui.label("Sent packets:");
                ui.label(egui::RichText::new(format!("{}", sent_messages_count)).strong());
                ui.add_space(10.0);
                ui.label("Received packets:");
                ui.label(egui::RichText::new(format!("{}", received_messages_count)).strong());
            });

            // Tab bar header (outside of bottom-up so it doesn't steal table space)
            if let Some(_node_info) = &state.node_info {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut state.inspector_tab, InspectorTab::RadioStream, "Radio Stream");
                    ui.selectable_value(&mut state.inspector_tab, InspectorTab::MessageStream, "Message Stream");
                    ui.selectable_value(&mut state.inspector_tab, InspectorTab::LogStream, "Log Stream");
                    if state.operating_mode != OperatingMode::LogVisualization {
                        ui.selectable_value(&mut state.inspector_tab, InspectorTab::ConnectionMatrix, "Connection Matrix");
                    }
                });
                ui.add_space(4.0);
            }

            // Remaining area: use bottom-up layout so buttons stick to the bottom
            let avail_w = ui.available_width();
            let avail_h = ui.available_height();
            let button_h = ui.spacing().interact_size.y;
            let node_id = p.node_id; // Capture node_id before moving into closure
            let show_measurement_button = state.operating_mode != OperatingMode::LogVisualization;
            let show_control_buttons = state.operating_mode == OperatingMode::RealtimeTracking;
            let control_available = state.control_available;

            ui.allocate_ui_with_layout(
                egui::vec2(avail_w, avail_h),
                egui::Layout::bottom_up(egui::Align::Center),
                |ui| {
                    // In bottom_up layout, first items appear at the bottom

                    // Bottom padding
                    ui.add_space(8.0);

                    // Button area at bottom
                    if let Some(_i) = state.selected {
                        let button_w = (avail_w * 0.8).max(0.0);
                        let half_button_w = ((button_w - 6.0) / 2.0).max(0.0);

                        // Control buttons (only in RealtimeTracking mode)
                        if show_control_buttons {
                            let button_tooltip = if control_available {
                                ""
                            } else {
                                "Control not available. Add config.toml to the scene directory."
                            };

                            // Row 2 (bottom row): Start Measurement, Auto AddBlock
                            ui.horizontal(|ui| {
                                let pad = (ui.available_width() - button_w).max(0.0) / 2.0;
                                ui.add_space(pad);

                                let measurement_button_title: String = if state.measurement_identifier > 0 {
                                    "Reset Measurement".into()
                                } else {
                                    "Start Measurement".into()
                                };
                                if ui.add_sized([half_button_w, button_h], egui::Button::new(measurement_button_title)).clicked() {
                                    if state.measurement_identifier == 0 {
                                        state.measurement_identifier = max(rand::random::<u32>() % 100000, 1);
                                        state.measurement_start_time = embassy_time::Instant::now();
                                        let _ = state.ui_command_tx.try_send(UICommand::StartMeasurement(node_id, state.measurement_identifier));
                                        log::info!("Started measurement {} on node {}", state.measurement_identifier, node_id);
                                        state.reached_nodes.clear();
                                        state.reached_nodes.insert(node_id);
                                    } else {
                                        state.reached_nodes.clear();
                                        state.measurement_identifier = 0;
                                        state.measurement_50_time = 0;
                                        state.measurement_90_time = 0;
                                        state.measurement_100_time = 0;
                                        state.measurement_total_time = 0;
                                        state.measurement_total_message_count = 0;
                                        state.measurement_50_message_count = 0;
                                        state.measurement_90_message_count = 0;
                                        state.measurement_100_message_count = 0;
                                    }
                                }

                                ui.add_space(6.0);

                                ui.add_enabled_ui(control_available, |ui| {
                                    if ui
                                        .add_sized([half_button_w, button_h], egui::Button::new("Auto AddBlock"))
                                        .on_disabled_hover_text(button_tooltip)
                                        .on_hover_text("Configure automatic AddBlock sending interval for this node")
                                        .clicked()
                                    {
                                        state.open_auto_addblock_modal(Some(node_id));
                                    }
                                });
                            });

                            ui.add_space(6.0);

                            // Row 1 (top row): Set Log Level, Node Command
                            ui.horizontal(|ui| {
                                let pad = (ui.available_width() - button_w).max(0.0) / 2.0;
                                ui.add_space(pad);

                                ui.add_enabled_ui(control_available, |ui| {
                                    if ui
                                        .add_sized([half_button_w, button_h], egui::Button::new("Set Log Level"))
                                        .on_disabled_hover_text(button_tooltip)
                                        .on_hover_text("Set log level and filter for this node")
                                        .clicked()
                                    {
                                        state.open_set_log_level_modal(Some(node_id));
                                    }
                                });

                                ui.add_space(6.0);

                                ui.add_enabled_ui(control_available, |ui| {
                                    if ui
                                        .add_sized([half_button_w, button_h], egui::Button::new("Node Command"))
                                        .on_disabled_hover_text(button_tooltip)
                                        .on_hover_text("Send a custom command to this node")
                                        .clicked()
                                    {
                                        state.open_send_command_modal(Some(node_id));
                                    }
                                });
                            });
                        } else if show_measurement_button {
                            // For Simulation mode (no control buttons), just show measurement button
                            ui.horizontal(|ui| {
                                let pad = (ui.available_width() - button_w).max(0.0) / 2.0;
                                ui.add_space(pad);
                                let measurement_button_title: String = if state.measurement_identifier > 0 {
                                    "Reset Measurement".into()
                                } else {
                                    "Start Measurement".into()
                                };
                                if ui.add_sized([button_w, button_h], egui::Button::new(measurement_button_title)).clicked() {
                                    if state.measurement_identifier == 0 {
                                        state.measurement_identifier = max(rand::random::<u32>() % 100000, 1);
                                        state.measurement_start_time = embassy_time::Instant::now();
                                        let _ = state.ui_command_tx.try_send(UICommand::StartMeasurement(node_id, state.measurement_identifier));
                                        log::info!("Started measurement {} on node {}", state.measurement_identifier, node_id);
                                        state.reached_nodes.clear();
                                        state.reached_nodes.insert(node_id);
                                    } else {
                                        state.reached_nodes.clear();
                                        state.measurement_identifier = 0;
                                        state.measurement_50_time = 0;
                                        state.measurement_90_time = 0;
                                        state.measurement_100_time = 0;
                                        state.measurement_total_time = 0;
                                        state.measurement_total_message_count = 0;
                                        state.measurement_50_message_count = 0;
                                        state.measurement_90_message_count = 0;
                                        state.measurement_100_message_count = 0;
                                    }
                                }
                            });
                        }

                        // Spacing between buttons and table
                        ui.add_space(8.0);
                    }

                    // Table area fills remaining space above buttons
                    let table_h = ui.available_height().max(0.0);
                    if table_h > 0.0 {
                        let has_matching_node_info = state.node_info.as_ref().map_or(false, |ni| ni.node_id == node_id);
                        let current_tab = state.inspector_tab;

                        ui.allocate_ui_with_layout(
                            egui::vec2(avail_w, table_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                match current_tab {
                                    InspectorTab::ConnectionMatrix => {
                                        render_connection_matrix(ui, state, node_id);
                                    }
                                    _ => {
                                        if has_matching_node_info {
                                            match current_tab {
                                                InspectorTab::RadioStream => {
                                                    if let Some(node_info) = &state.node_info {
                                                        render_radio_stream_table(ui, state, node_info);
                                                    }
                                                }
                                                InspectorTab::MessageStream => {
                                                    if let Some(node_info) = &state.node_info {
                                                        render_message_stream_table(ui, state, node_info);
                                                    }
                                                }
                                                InspectorTab::LogStream => {
                                                    let log_lines = state.node_info.as_ref().map(|ni| ni.log_lines.clone());
                                                    if let Some(log_lines) = log_lines {
                                                        render_log_stream(ui, state, &log_lines);
                                                    }
                                                }
                                                InspectorTab::ConnectionMatrix => {}
                                            }
                                        }
                                    }
                                }
                            },
                        );
                    }
                },
            );
        } else {
            // Center the info label both horizontally and vertically within the remaining panel space
            ui.centered_and_justified(|ui| {
                ui.label("No node selected. Click on a node on the map to select it.");
            });
        }
    });
    // Update stored width when panel is resized
    state.right_panel_width = response.response.rect.width();
}

/// Render the virtualized radio stream table for the selected node.
///
/// Uses `egui_extras::TableBuilder` to efficiently render only visible rows.
/// Messages are shown newest-first (reversed order) with columns for:
/// - Time: Virtual simulation time in seconds
/// - From: "Sent msg" for outgoing, "#ID" for incoming
/// - Type: Human-readable message type name
/// - Packet: "index/total" showing packet sequence
/// - Size: Packet size in bytes
/// - LQ: Link quality (0-63), color-coded by threshold
///
/// Collision rows are highlighted in red with white text.
///
/// # Parameters
///
/// * `ui` - egui UI context
/// * `state` - Application state (for thresholds)
/// * `node_info` - The selected node's detailed information
fn render_radio_stream_table(ui: &mut egui::Ui, state: &AppState, node_info: &crate::ui::NodeInfo) {
    use egui_extras::{Column, TableBuilder};

    let row_height = ui.text_style_height(&egui::TextStyle::Body) * 1.3;
    TableBuilder::new(ui)
        .striped(true)
        .vscroll(true)
        .min_scrolled_height(100.0)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(60.0)) // Timestamp
        .column(Column::exact(55.0)) // From
        .column(Column::remainder().clip(true)) // Type - clips to prevent blocking resize
        .column(Column::exact(60.0)) // Sequence
        .column(Column::exact(45.0)) // Packet
        .column(Column::exact(45.0)) // Size
        .column(Column::exact(35.0)) // Link Quality
        .header(row_height, |mut header| {
            header.col(|ui| {
                ui.strong("Time");
            });
            header.col(|ui| {
                ui.strong("From");
            });
            header.col(|ui| {
                ui.strong("Type");
            });
            header.col(|ui| {
                ui.strong("Sequence");
            });
            header.col(|ui| {
                ui.strong("Packet");
            });
            header.col(|ui| {
                ui.strong("Size");
            });
            header.col(|ui| {
                ui.strong("LQ");
            });
        })
        .body(|body| {
            // Virtualized rows: only build visible rows; keep newest-first order
            let row_count = node_info.radio_packets.len();
            body.rows(row_height, row_count, |mut row| {
                // Map visible row index to reversed (newest-first) index
                let row_index = row.index();
                let msg_idx = row_count - 1 - row_index;
                let msg = &node_info.radio_packets[msg_idx];

                // Color rows red if from this node, else green
                let is_self = node_info.node_id == msg.sender_node;
                let mut row_color = if is_self {
                    Color32::YELLOW
                } else {
                    Color32::LIGHT_GREEN
                };
                let mut collision_fill: Option<Color32> = None;
                if msg.collision {
                    // Paint whole row red for collisions and use white text for contrast
                    collision_fill = Some(Color32::from_rgb(255, 0, 0));
                    row_color = Color32::WHITE;
                }
                let type_string = match msg.message_type {
                    1 => "Req echo",
                    2 => "Echo",
                    3 => "Echo result",
                    4 => "Req block",
                    5 => "Req blk prt",
                    6 => "Add block",
                    7 => "Add trans",
                    8 => "Req mempool",
                    9 => "Support",
                    255 => "Packet CRC Error",
                    _ => "Unknown",
                };
                let from_string = if msg.message_type == 255 {
                    // For CRC errors, sender is unknown
                    "?".to_string()
                } else if is_self {
                    "Sent msg".to_string()
                } else {
                    format!("#{}", msg.sender_node)
                };

                // Format time based on operating mode
                let time_string = match state.operating_mode {
                    OperatingMode::Simulation => {
                        // In simulation mode, show seconds since start
                        let secs = msg.timestamp.duration_since(state.start_time).as_secs();
                        format!("{} s", secs)
                    }
                    OperatingMode::RealtimeTracking | OperatingMode::LogVisualization => {
                        // In analyzer modes, timestamp is Unix epoch milliseconds
                        // Convert to local timezone with DST handling
                        let timestamp_secs = msg.timestamp.as_secs() as i64;
                        let local_time = Local.timestamp_opt(timestamp_secs, 0).single();
                        match local_time {
                            Some(dt) => dt.format("%H:%M:%S").to_string(),
                            None => "--:--:--".to_string(),
                        }
                    }
                };

                let message_type_color = color_for_message_type(msg.message_type, 1.0);
                let link_quality_string = if is_self {
                    "-".to_string()
                } else {
                    format!("{}", msg.link_quality)
                };

                row.col(|ui| {
                    if let Some(fill) = collision_fill {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }
                    ui.colored_label(row_color, time_string.clone());
                });
                row.col(|ui| {
                    if let Some(fill) = collision_fill {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }
                    ui.colored_label(row_color, from_string);
                });
                row.col(|ui| {
                    if let Some(fill) = collision_fill {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }
                    ui.colored_label(message_type_color, type_string);
                });
                row.col(|ui| {
                    if let Some(fill) = collision_fill {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }
                    let sequence_string = match msg.sequence {
                        Some(seq) => format!("#{}", seq),
                        None => "-".to_string(),
                    };
                    ui.colored_label(row_color, sequence_string);
                });
                row.col(|ui| {
                    if let Some(fill) = collision_fill {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }
                    ui.colored_label(
                        row_color,
                        format!("{}/{}", msg.packet_index, msg.packet_count),
                    );
                });
                row.col(|ui| {
                    if let Some(fill) = collision_fill {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }
                    ui.colored_label(row_color, format!("{} B", msg.packet_size));
                });
                row.col(|ui| {
                    if let Some(fill) = collision_fill {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }

                    let link_quality_color;

                    if state.poor_limit > 0 && state.excellent_limit > 0 {
                        if msg.link_quality <= state.poor_limit {
                            link_quality_color = Color32::RED;
                        } else if msg.link_quality >= state.excellent_limit {
                            link_quality_color = Color32::GREEN;
                        } else {
                            link_quality_color = Color32::YELLOW;
                        }
                    } else {
                        link_quality_color = Color32::WHITE;
                    }

                    ui.colored_label(link_quality_color, link_quality_string);
                });
            });
        });
}

/// Render the message stream table showing complete messages (e.g., AddBlock).
///
/// Shows full messages (not individual packets) with columns for:
/// - Time: Virtual simulation time or HH:MM:SS
/// - From: "Sent" for outgoing, "#ID" for incoming
/// - Type: Message type name
/// - Sequence: Message sequence number
///
/// # Parameters
///
/// * `ui` - egui UI context
/// * `state` - Application state
/// * `node_info` - The selected node's detailed information
fn render_message_stream_table(
    ui: &mut egui::Ui,
    state: &AppState,
    node_info: &crate::ui::NodeInfo,
) {
    use egui_extras::{Column, TableBuilder};

    let row_height = ui.text_style_height(&egui::TextStyle::Body) * 1.3;

    TableBuilder::new(ui)
        .striped(true)
        .vscroll(true)
        .min_scrolled_height(100.0)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(60.0)) // Time
        .column(Column::exact(70.0)) // From
        .column(Column::remainder().clip(true)) // Type - clips to prevent blocking resize
        .column(Column::exact(80.0)) // Sequence
        .header(row_height, |mut header| {
            header.col(|ui| {
                ui.strong("Time");
            });
            header.col(|ui| {
                ui.strong("From");
            });
            header.col(|ui| {
                ui.strong("Type");
            });
            header.col(|ui| {
                ui.strong("Sequence");
            });
        })
        .body(|body| {
            let row_count = node_info.messages.len();
            body.rows(row_height, row_count, |mut row| {
                let row_index = row.index();
                let msg_idx = row_count - 1 - row_index; // Newest first
                let msg = &node_info.messages[msg_idx];

                let is_outgoing = msg.is_outgoing;
                let row_color = if is_outgoing {
                    Color32::YELLOW
                } else {
                    Color32::LIGHT_GREEN
                };
                let from_string = if is_outgoing {
                    "Sent".to_string()
                } else {
                    format!("#{}", msg.sender_node)
                };
                let type_string = match msg.message_type {
                    6 => "AddBlock",
                    _ => "Unknown",
                };

                // Format time based on operating mode
                let time_string = match state.operating_mode {
                    OperatingMode::Simulation => {
                        let secs = msg.timestamp.duration_since(state.start_time).as_secs();
                        format!("{} s", secs)
                    }
                    OperatingMode::RealtimeTracking | OperatingMode::LogVisualization => {
                        let timestamp_secs = msg.timestamp.as_secs() as i64;
                        let local_time = Local.timestamp_opt(timestamp_secs, 0).single();
                        match local_time {
                            Some(dt) => dt.format("%H:%M:%S").to_string(),
                            None => "--:--:--".to_string(),
                        }
                    }
                };

                row.col(|ui| {
                    ui.colored_label(row_color, &time_string);
                });
                row.col(|ui| {
                    ui.colored_label(row_color, &from_string);
                });
                row.col(|ui| {
                    ui.colored_label(row_color, type_string);
                });
                row.col(|ui| {
                    ui.colored_label(row_color, format!("#{}", msg.sequence));
                });
            });
        });
}

/// Render the log stream showing raw log lines for the selected node.
///
/// Shows log lines with columns for:
/// - Time: Timestamp of the log entry
/// - Log: The log message content
///
/// Color coded by log level.
///
/// # Parameters
///
/// * `ui` - egui UI context
/// * `state` - Application state (mutable for filter input)
/// * `log_lines` - The log lines to display
fn render_log_stream(
    ui: &mut egui::Ui,
    state: &mut AppState,
    log_lines: &[crate::simulation::types::LogLine],
) {
    use egui_extras::{Column, TableBuilder};

    // Filter input field at the top
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.add(
            egui::TextEdit::singleline(&mut state.log_filter).hint_text("Type to filter logs..."),
        );
        if ui.button("x").clicked() {
            state.log_filter.clear();
        }
    });
    ui.add_space(4.0);

    // Collect filtered log indices (matching the filter string, case-insensitive)
    let filter_lower = state.log_filter.to_lowercase();
    let filtered_indices: Vec<usize> = if filter_lower.is_empty() {
        (0..log_lines.len()).collect()
    } else {
        log_lines
            .iter()
            .enumerate()
            .filter(|(_, log)| log.content.to_lowercase().contains(&filter_lower))
            .map(|(i, _)| i)
            .collect()
    };

    let row_height = ui.text_style_height(&egui::TextStyle::Body) * 1.3;

    TableBuilder::new(ui)
        .striped(true)
        .vscroll(true)
        .min_scrolled_height(100.0)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(60.0)) // Time
        .column(Column::exact(16.0)) // Level (single letter)
        .column(Column::remainder().clip(true)) // Log content - clips to prevent blocking resize
        .header(row_height, |mut header| {
            header.col(|ui| {
                ui.strong("Time");
            });
            header.col(|ui| {
                ui.strong("L");
            });
            header.col(|ui| {
                ui.strong("Log");
            });
        })
        .body(|body| {
            let row_count = filtered_indices.len();
            body.rows(row_height, row_count, |mut row| {
                let row_index = row.index();
                let log_idx = filtered_indices[row_count - 1 - row_index]; // Newest first
                let log_line = &log_lines[log_idx];

                let color = match log_line.level {
                    LogLevel::Error => Color32::RED,
                    LogLevel::Warn => Color32::YELLOW,
                    LogLevel::Info => Color32::WHITE,
                    LogLevel::Debug | LogLevel::Trace => Color32::GRAY,
                };

                let level_letter = match log_line.level {
                    LogLevel::Trace => "T",
                    LogLevel::Debug => "D",
                    LogLevel::Info => "I",
                    LogLevel::Warn => "W",
                    LogLevel::Error => "E",
                };

                // Format time based on operating mode
                let time_string = match state.operating_mode {
                    OperatingMode::Simulation => {
                        let secs = log_line
                            .timestamp
                            .duration_since(state.start_time)
                            .as_secs();
                        format!("{} s", secs)
                    }
                    OperatingMode::RealtimeTracking | OperatingMode::LogVisualization => {
                        let timestamp_secs = log_line.timestamp.as_secs() as i64;
                        let local_time = Local.timestamp_opt(timestamp_secs, 0).single();
                        match local_time {
                            Some(dt) => dt.format("%H:%M:%S").to_string(),
                            None => "--:--:--".to_string(),
                        }
                    }
                };

                row.col(|ui| {
                    ui.colored_label(color, &time_string);
                });
                row.col(|ui| {
                    ui.colored_label(color, level_letter);
                });
                row.col(|ui| {
                    ui.colored_label(color, &log_line.content);
                });
            });
        });
}

/// Render the connection matrix tab with a query button and table.
fn render_connection_matrix(ui: &mut egui::Ui, state: &mut AppState, node_id: u32) {
    let can_query = state.operating_mode == OperatingMode::Simulation
        || (state.operating_mode == OperatingMode::RealtimeTracking && state.control_available);

    let has_matrix = state.connection_matrices.contains_key(&node_id);
    let is_pending = state.connection_matrix_pending.contains(&node_id);

    if can_query && !has_matrix && !is_pending {
        state.connection_matrix_pending.insert(node_id);
        let _ = state
            .ui_command_tx
            .try_send(UICommand::RequestConnectionMatrix(node_id));
    }

    ui.horizontal(|ui| {
        ui.add_enabled_ui(can_query, |ui| {
            if ui.button("Query Connection Matrix").clicked() {
                state.connection_matrices.remove(&node_id);
                state.connection_matrix_pending.insert(node_id);
                let _ = state
                    .ui_command_tx
                    .try_send(UICommand::RequestConnectionMatrix(node_id));
            }
        });

        if state.connection_matrix_pending.contains(&node_id) {
            ui.add(egui::Spinner::new());
        }
    });

    ui.add_space(8.0);

    let matrix = state.connection_matrices.get(&node_id);

    if let Some(matrix) = matrix {
        render_connection_matrix_table(ui, state, matrix);
    } else {
        ui.label("No connection matrix available.");
    }
}

fn render_connection_matrix_table(
    ui: &mut egui::Ui,
    state: &AppState,
    matrix: &crate::common::connection_matrix::ConnectionMatrix,
) {
    use egui_extras::{Column, TableBuilder};

    let node_count = matrix.node_ids.len();
    if node_count == 0 {
        ui.label("Empty connection matrix.");
        return;
    }

    let mut entries: Vec<(usize, usize, u8)> =
        Vec::with_capacity(node_count.saturating_sub(1) * node_count);
    for r in 0..node_count {
        for c in 0..node_count {
            if r == c {
                continue;
            }
            let lq = matrix
                .values
                .get(r)
                .and_then(|row| row.get(c))
                .copied()
                .unwrap_or(0);
            if lq == 0 {
                continue;
            }
            entries.push((r, c, lq));
        }
    }
    let total_rows = entries.len();
    let row_height = 18.0;

    TableBuilder::new(ui)
        .striped(true)
        .vscroll(true)
        .min_scrolled_height(100.0)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().resizable(true))
        .column(Column::auto().resizable(true))
        .column(Column::remainder().clip(true))
        .header(20.0, |mut header| {
            header.col(|ui| {
                ui.label(egui::RichText::new("Sender node").strong());
            });
            header.col(|ui| {
                ui.label(egui::RichText::new("Target node").strong());
            });
            header.col(|ui| {
                ui.label(egui::RichText::new("Link quality").strong());
            });
        })
        .body(|body| {
            body.rows(row_height, total_rows, |mut row| {
                let row_index = row.index();
                let (r, c, link_quality) = entries[row_index];
                let sender_id = matrix.node_ids[r];
                let target_id = matrix.node_ids[c];

                row.col(|ui| {
                    ui.label(format!("#{}", sender_id));
                });
                row.col(|ui| {
                    ui.label(format!("#{}", target_id));
                });
                row.col(|ui| {
                    let color =
                        link_quality_color(link_quality, state.poor_limit, state.excellent_limit);
                    let mut text = egui::RichText::new(format!("{}", link_quality));
                    if let Some(color) = color {
                        text = text.color(color);
                    }
                    ui.label(text);
                });
            });
        });
}

fn link_quality_color(value: u8, poor: u8, excellent: u8) -> Option<Color32> {
    if poor == 0 || excellent == 0 || poor >= excellent {
        return None;
    }
    if value <= poor {
        Some(Color32::RED)
    } else if value >= excellent {
        Some(Color32::GREEN)
    } else {
        Some(Color32::YELLOW)
    }
}
