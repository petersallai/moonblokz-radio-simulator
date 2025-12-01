//! # Top Panel - System Metrics and Controls
//!
//! This module renders the fixed-height top panel displaying:
//! - Column 1: Core system metrics (sim time, node count, throughput, collision rate)
//! - Column 2: Measurement data (distribution percentage, milestone times, packets/node ratio)
//! - Column 3: Simulation controls (speed slider, auto-speed checkbox, node ID display)
//!
//! The panel uses a 3-column layout to organize information clearly and provides
//! real-time feedback on simulation performance and network behavior.

use eframe::egui;
use crate::ui::{AppState, UICommand};

/// Render the top panel with metrics and controls.
///
/// Creates a fixed-height (150px) top panel with three columns:
/// 1. System metrics showing simulation time, node count, and packet statistics
/// 2. Measurement data showing distribution progress and milestone times
/// 3. Control widgets for adjusting simulation speed and display options
///
/// # Parameters
///
/// * `ctx` - egui context
/// * `state` - Mutable application state for reading metrics and updating controls
pub fn render(ctx: &egui::Context, state: &mut AppState) {
    egui::TopBottomPanel::top("top_metrics").exact_height(150.0).show(ctx, |ui| {
        let throughput_tx = if state.start_time.elapsed().as_secs() > 0 {
            ((state.total_sent_packets as f64 / state.start_time.elapsed().as_secs() as f64) * 60.0) as u64
        } else {
            0
        };

        let throughput_rx = if state.start_time.elapsed().as_secs() > 0 {
            ((state.total_received_packets as f64 / state.start_time.elapsed().as_secs() as f64) * 60.0) as u64
        } else {
            0
        };

        let collision_rate = if state.total_received_packets > 0 {
            (state.total_collision as f64 / (state.total_received_packets as f64 + state.total_collision as f64)) * 100.0
        } else {
            0.0
        };

        ui.columns(3, |cols| {
            // Column 1: Title + core metrics
            cols[0].vertical(|ui| {
                ui.heading("System Metrics");
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Sim time:");
                    // Fixed-width, monospace time so following labels don't shift horizontally
                    // Use embassy_time Instant for simulation time (scaled by driver)
                    let sim_secs = embassy_time::Instant::now().as_secs();
                    let sim_secs_with_s = format!("{}s", sim_secs);
                    let sim_time_str = format!("{:<6}", sim_secs_with_s); // fixed 6 chars, left-aligned (e.g., "42    ")
                    ui.label(egui::RichText::new(sim_time_str).monospace().strong());
                    ui.label("Total TX: ");
                    ui.label(egui::RichText::new(state.total_sent_packets.to_string()).strong());
                });

                let nodes_count_str = format!("{:<7}", state.nodes.len()); // fixed 7 chars, left-aligned (e.g., "42    ")

                ui.horizontal(|ui| {
                    ui.label("Nodes:");
                    ui.label(egui::RichText::new(nodes_count_str).monospace().strong());
                    ui.label("  Echo results: ");
                    ui.label(egui::RichText::new(state.echo_result_count.to_string()).strong());
                });
                ui.horizontal(|ui| {
                    ui.label("Throughput(TX):");
                    ui.label(egui::RichText::new(format!("{}", throughput_tx)).strong());
                    ui.label("packets/minutes");
                });
                ui.horizontal(|ui| {
                    ui.label("Throughput(RX):");
                    ui.label(egui::RichText::new(format!("{}", throughput_rx)).strong());
                    ui.label("packets/minutes");
                });

                ui.horizontal(|ui| {
                    ui.label("Collision rate:");
                    ui.label(egui::RichText::new(format!("{:.2}", collision_rate)).strong());
                    ui.label("%");
                });
            });

            // Column 2: Measured distribution
            cols[1].vertical(|ui| {
                render_measured_data(ui, state);
            });

            // Column 3: Controls
            cols[2].vertical(|ui| {
                render_controls(ui, state);
            });
        });
    });
}

/// Render the measurement data column.
///
/// Displays current measurement progress including:
/// - Total measurement time and packet count
/// - Distribution percentage (what % of nodes have been reached)
/// - Milestone times (50%, 90%, 100% distribution reached)
/// - Packets per node ratio for each milestone
///
/// Automatically detects and records milestone times as they are reached.
///
/// # Parameters
///
/// * `ui` - egui UI context
/// * `state` - Mutable state for updating milestone times
fn render_measured_data(ui: &mut egui::Ui, state: &mut AppState) {
    let measurement_duration_string = if state.measurement_identifier > 0 {
        let measurement_total_time_with_s = format!("{}s", state.measurement_total_time);
        format!("{:<7}", measurement_total_time_with_s)
    } else {
        "-".into()
    };

    let distribution_percentage = if state.nodes.len() > 0 && state.measurement_identifier > 0 {
        (state.reached_nodes.len() as f64 / state.nodes.len() as f64) * 100.0
    } else {
        0.0
    };

    let distribution_percentage_string = if state.nodes.len() > 0 && state.measurement_identifier > 0 {
        format!("{:.0}", distribution_percentage)
    } else {
        "-".into()
    };

    if distribution_percentage >= 50.0 && state.measurement_50_time == 0 {
        state.measurement_50_time = state.measurement_start_time.elapsed().as_secs();
        state.measurement_50_message_count = state.measurement_total_message_count;
    }

    if distribution_percentage >= 90.0 && state.measurement_90_time == 0 {
        state.measurement_90_time = state.measurement_start_time.elapsed().as_secs();
        state.measurement_90_message_count = state.measurement_total_message_count;
    }

    if distribution_percentage >= 99.9 && state.measurement_100_time == 0 {
        state.measurement_100_time = state.measurement_start_time.elapsed().as_secs();
        state.measurement_100_message_count = state.measurement_total_message_count;
    }

    let measurement_50_time_string = if state.measurement_50_time > 0 {
        format!("{}s", state.measurement_50_time)
    } else {
        "-".into()
    };
    let measurement_90_time_string = if state.measurement_90_time > 0 {
        format!("{}s", state.measurement_90_time)
    } else {
        "-".into()
    };
    let measurement_100_time_string = if state.measurement_100_time > 0 {
        format!("{}s", state.measurement_100_time)
    } else {
        "-".into()
    };

    let p_per_n_string = if state.measurement_total_message_count > 0 && state.nodes.len() > 0 {
        format!("{}", (state.measurement_total_message_count * 100) / state.nodes.len() as u32)
    } else {
        "-".into()
    };

    let p_per_n_50_string = if state.measurement_50_message_count > 0 && state.nodes.len() > 0 {
        format!("{}", (state.measurement_50_message_count * 100) / state.nodes.len() as u32)
    } else {
        "-".into()
    };

    let p_per_n_90_string = if state.measurement_90_message_count > 0 && state.nodes.len() > 0 {
        format!("{}", (state.measurement_90_message_count * 100) / state.nodes.len() as u32)
    } else {
        "-".into()
    };

    let p_per_n_100_string = if state.measurement_100_message_count > 0 && state.nodes.len() > 0 {
        format!("{}", (state.measurement_100_message_count * 100) / state.nodes.len() as u32)
    } else {
        "-".into()
    };

    ui.heading("Measured data");
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Total time: ");
        ui.label(egui::RichText::new(measurement_duration_string).strong().monospace());
        ui.label("packets: ");
        ui.label(egui::RichText::new(format!("{}", state.measurement_total_message_count)).strong());
    });
    ui.horizontal(|ui| {
        ui.label("Distribution: ");
        ui.label(egui::RichText::new(format!("{}", distribution_percentage_string)).strong().monospace());
        ui.label("% ");
        ui.label("  P/N: ");
        ui.label(egui::RichText::new(p_per_n_string).strong());
        ui.label("%");
    });
    ui.horizontal(|ui| {
        ui.label("50% time: ");
        ui.label(egui::RichText::new(format!("{}", measurement_50_time_string)).strong());
        ui.label("   P/N:");
        ui.label(egui::RichText::new(p_per_n_50_string).strong());
        ui.label("%");
    });
    ui.horizontal(|ui| {
        ui.label("90% time: ");
        ui.label(egui::RichText::new(format!("{}", measurement_90_time_string)).strong());
        ui.label("   P/N:");
        ui.label(egui::RichText::new(p_per_n_90_string).strong());
        ui.label("%");
    });
    ui.horizontal(|ui| {
        ui.label("100% time: ");
        ui.label(egui::RichText::new(format!("{}", measurement_100_time_string)).strong());
        ui.label("   P/N:");
        ui.label(egui::RichText::new(p_per_n_100_string).strong());
        ui.label("%");
    });
}

/// Render the controls column.
///
/// Provides interactive widgets for:
/// - Speed slider (20% - 1000%): Adjust simulation time scaling
/// - Auto speed checkbox: Enable automatic speed adjustment based on CPU load
/// - Reset button: Return speed to 100% (real-time)
/// - Show node IDs checkbox: Toggle node ID labels on the map
/// - Delay warning: Display if simulation is running behind schedule
///
/// # Parameters
///
/// * `ui` - egui UI context
/// * `state` - Mutable state for updating control values
fn render_controls(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("Controls");
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Speed:");
        let mut speed = state.speed_percent as f64;
        // Keep UI slider in sync with autoscaler bounds
        if ui.add(egui::Slider::new(&mut speed, 20.0..=1000.0).suffix("%")).changed() {
            state.speed_percent = speed.round() as u32;
            crate::time_driver::set_simulation_speed_percent(state.speed_percent);
        }
    });
    ui.horizontal(|ui| {
        let mut auto = state.auto_speed_enabled;
        if ui.checkbox(&mut auto, "Auto speed").changed() {
            state.auto_speed_enabled = auto;
            let _ = state.ui_command_tx.try_send(UICommand::SetAutoSpeed(state.auto_speed_enabled));
        }
        if ui.button("Reset").clicked() {
            state.speed_percent = 100;
            crate::time_driver::set_simulation_speed_percent(state.speed_percent);
        }
    });
    ui.separator();
    let mut show_ids = state.show_node_ids;
    if ui.checkbox(&mut show_ids, "Show node IDs").changed() {
        state.show_node_ids = show_ids;
    }
    if state.simulation_delay > 10 {
        ui.separator();
        let warn_text = format!("Warning! Simulation delay is more than 10 milliseconds ({} ms)", state.simulation_delay);
        ui.add(egui::Label::new(egui::RichText::new(warn_text).color(egui::Color32::RED)).wrap(true));
    }
}
