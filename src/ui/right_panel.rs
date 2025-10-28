// Right panel: Node inspector and message stream

use eframe::egui;
use egui::Color32;
use std::cmp::max;
use crate::ui::{AppState, UICommand, color_for_message_type};

pub fn render(ctx: &egui::Context, state: &mut AppState) {
    egui::SidePanel::right("inspector_right").exact_width(400.0).show(ctx, |ui| {
        // Top content (default top-down, left-aligned)
        ui.heading("Inspector");
        ui.separator();
        if let Some(i) = state.selected {
            let p = &state.nodes[i];
            ui.horizontal(|ui| {
                ui.label("Selected point:");
                ui.label(egui::RichText::new(format!("#{}", p.node_id)).strong().color(Color32::from_rgb(0, 128, 255)));
            });
            ui.horizontal(|ui| {
                ui.label("Position: (");
                ui.label(egui::RichText::new(format!("{:.2}", p.position.x)).strong());
                ui.label(",");
                ui.label(egui::RichText::new(format!("{:.2}", p.position.y)).strong());
                ui.label(")");
            });
            ui.horizontal(|ui| {
                ui.label("Radio strength:");
                ui.label(egui::RichText::new(format!("{}", p.radio_strength)).strong());
            });
            ui.separator();

            let mut sent_messages_count = 0;
            let mut received_messages_count = 0;

            if let Some(node_info) = &state.node_info {
                for msg in &node_info.messages {
                    if msg.sender_node == p.node_id {
                        sent_messages_count += 1;
                    } else {
                        received_messages_count += 1;
                    }
                }
            }

            ui.horizontal(|ui| {
                ui.label("Sent messages:");
                ui.label(egui::RichText::new(format!("{}", sent_messages_count)).strong());
            });
            ui.horizontal(|ui| {
                ui.label("Received messages:");
                ui.label(egui::RichText::new(format!("{}", received_messages_count)).strong());
            });

            // Messages header (outside of bottom-up so it doesn't steal table space)
            if let Some(node_info) = &state.node_info {
                ui.separator();
                ui.heading(format!("Radio stream for #{}", node_info.node_id));
                ui.add_space(4.0);
            }

            // Remaining area: bottom-up so buttons stick to the bottom and table fills above
            let avail_w = ui.available_width();
            let button_h = ui.spacing().interact_size.y;
            let node_id = p.node_id; // Capture node_id before moving into closure
            ui.allocate_ui_with_layout(egui::vec2(avail_w, ui.available_height()), egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                // Bottom buttons, centered at 80% width
                if let Some(_i) = state.selected {
                    let button_w = (avail_w * 0.8).max(0.0);

                    // 20px padding at the very bottom under the last (bottom-most) button
                    ui.add_space(20.0);

                    ui.horizontal(|ui| {
                        let pad = (avail_w - button_w).max(0.0) / 2.0;
                        ui.add_space(pad);
                        let measurement_button_title: String = if state.measurement_identifier > 0 {
                            "Reset Measurement".into()
                        } else {
                            "Start Measurement".into()
                        };
                        if ui.add_sized([button_w, button_h], egui::Button::new(measurement_button_title)).clicked() {
                            if state.measurement_identifier == 0 {
                                state.measurement_identifier = max(rand::random::<u32>(), 1);
                                state.measurement_start_time = embassy_time::Instant::now();
                                let _ = state.ui_command_tx.try_send(UICommand::StartMeasurement(node_id, state.measurement_identifier));
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
                    ui.horizontal(|ui| {
                        let pad = (avail_w - button_w).max(0.0) / 2.0;
                        ui.add_space(pad);
                        if ui.add_sized([button_w, button_h], egui::Button::new("Send Message...")).clicked() {
                            log::debug!("Center on {}", node_id);
                        }
                    });

                    // 5px spacing above the first (top-most) button, separating it from the table
                    ui.add_space(5.0);
                }

                // Table area above buttons: fill whatever is left
                let table_h = ui.available_height().max(0.0);
                if table_h > 0.0 {
                    ui.allocate_ui_with_layout(egui::vec2(avail_w, table_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        if let Some(node_info) = &state.node_info {
                            if node_info.node_id == node_id {
                                render_message_table(ui, state, node_info, table_h);
                            }
                        }
                    });
                }
            });
        } else {
            // Center the info label both horizontally and vertically within the remaining panel space
            ui.centered_and_justified(|ui| {
                ui.label("No node selected. Click on a node on the map to select it.");
            });
        }
    });
}

fn render_message_table(ui: &mut egui::Ui, state: &AppState, node_info: &crate::ui::NodeInfo, table_h: f32) {
    use egui_extras::{Column, TableBuilder};

    let row_height = ui.text_style_height(&egui::TextStyle::Body) * 1.3;
    // Ensure total table height (header + body) fits in the allocated space,
    // otherwise the body would push into the buttons area by ~header height.
    let header_h = row_height;
    let body_min_h = (table_h - header_h).max(0.0);
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .vscroll(true)
        .min_scrolled_height(body_min_h)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(60.0).at_least(40.0)) // Timestamp
        .column(Column::initial(50.0).at_least(40.0)) // From
        .column(Column::remainder()) // Type
        .column(Column::initial(50.0).at_least(40.0)) // Packet
        .column(Column::initial(50.0).at_least(40.0)) // Size
        .column(Column::initial(50.0).at_least(40.0)) // Link Quality
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
            let row_count = node_info.messages.len();
            body.rows(row_height, row_count, |mut row| {
                // Map visible row index to reversed (newest-first) index
                let row_index = row.index();
                let msg_idx = row_count - 1 - row_index;
                let msg = &node_info.messages[msg_idx];

                // Color rows red if from this node, else green
                let is_self = node_info.node_id == msg.sender_node;
                let mut row_color = if is_self { Color32::YELLOW } else { Color32::LIGHT_GREEN };
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
                    _ => "Unknown",
                };
                let from_string = if is_self { "Sent msg".to_string() } else { format!("#{}", msg.sender_node) };
                let secs = msg.timestamp.duration_since(state.start_time).as_secs();
                let message_type_color = color_for_message_type(msg.message_type, 1.0);
                let link_quality_string = if is_self { "-".to_string() } else { format!("{}", msg.link_quality) };

                row.col(|ui| {
                    if let Some(fill) = collision_fill {
                        let rect = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(rect, 0.0, fill);
                    }
                    ui.colored_label(row_color, format!("{} s", secs));
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
                    ui.colored_label(row_color, format!("{}/{}", msg.packet_index + 1, msg.packet_count));
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
