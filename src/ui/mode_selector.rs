//! # Mode Selector Screen
//!
//! This module provides the initial mode selection interface allowing users to choose
//! between three operational modes:
//!
//! ## Simulation Mode
//! Start a simulated network based on a pre-defined scene file. This mode runs the
//! full radio network simulation with configurable nodes, obstacles, and propagation
//! parameters. Ideal for protocol testing and topology experiments.
//!
//! ## Real-time Tracking Mode
//! Connect to a live log stream from the log_collector. Visualizes network activity
//! as it happens on real hardware. Requires both a log file and a scene.json file
//! defining node positions.
//!
//! ## Log Visualization Mode
//! Open and replay a previously saved log file. Useful for analyzing historical
//! network behavior and creating reproducible test cases. Also requires a scene.json
//! file for node positions.
//!
//! The mode selector displays three panels with icons, descriptions, and action buttons.
//! After selection, the application proceeds to file picker dialogs for the required files.

use eframe::egui;
use egui::Color32;
use std::sync::Arc;

/// Mode selector UI component managing the initial mode selection screen.
///
/// Loads and displays icons for each mode and handles user interaction.
pub struct ModeSelector {
    simulation_icon: Option<Arc<egui::ColorImage>>,
    realtime_icon: Option<Arc<egui::ColorImage>>,
    log_icon: Option<Arc<egui::ColorImage>>,
    /// Scene path for real-time tracking mode.
    pub realtime_scene_path: Option<String>,
    /// Log path for real-time tracking mode.
    pub realtime_log_path: Option<String>,
    /// Scene path for log visualization mode.
    pub logvis_scene_path: Option<String>,
    /// Log path for log visualization mode.
    pub logvis_log_path: Option<String>,
}

impl ModeSelector {
    /// Create a new mode selector and load embedded icons.
    ///
    /// Icons are embedded in the binary and decoded at startup.
    /// If icon loading fails, placeholder emojis are used instead.
    pub fn new() -> Self {
        // Load icons
        let simulation_icon = Self::load_image(include_bytes!("../../icons/simulation_icon.png"));
        let realtime_icon = Self::load_image(include_bytes!("../../icons/realtime_icon.png"));
        let log_icon = Self::load_image(include_bytes!("../../icons/log_icon.png"));

        Self {
            simulation_icon,
            realtime_icon,
            log_icon,
            realtime_scene_path: None,
            realtime_log_path: None,
            logvis_scene_path: None,
            logvis_log_path: None,
        }
    }

    /// Load an icon image from embedded PNG bytes.
    ///
    /// # Parameters
    ///
    /// * `bytes` - Embedded PNG image data
    ///
    /// # Returns
    ///
    /// `Some(Arc<ColorImage>)` if successful, `None` if decoding fails.
    fn load_image(bytes: &'static [u8]) -> Option<Arc<egui::ColorImage>> {
        match image::load_from_memory(bytes) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let pixels = rgba.as_flat_samples();
                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
                Some(Arc::new(color_image))
            }
            Err(e) => {
                eprintln!("Failed to decode embedded icon: {}", e);
                None
            }
        }
    }

    /// Render the mode selector screen
    /// Returns the selected mode if any button was clicked
    pub fn render(&mut self, ctx: &egui::Context) -> Option<ModeSelection> {
        let mut selection = None;
        const PANEL_HEIGHT: f32 = 500.0;
        let button_size = egui::vec2(160.0, 32.0);
        let button_height = button_size.y;
        let bottom_padding = 30.0;
        let min_button_gap = 20.0;
        let button_spacing = 10.0;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(30.0);

                // Title
                ui.heading(egui::RichText::new("MoonBlokz Radio Simulator & Analyzer").size(28.0));
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Select Mode").size(18.0));
                ui.add_space(50.0);
            });

            // Three panels horizontally with proper spacing
            // Optimized for 1000px × 800px minimum window size
            ui.horizontal(|ui| {
                let spacing = 15.0;
                let panel_width = 280.0; // Optimized width for 1000px window
                let total_width = panel_width * 3.0 + spacing * 2.0;
                let available_width = ui.available_width();

                let original_spacing = ui.spacing().item_spacing.x;
                ui.spacing_mut().item_spacing.x = spacing;

                let padding = (available_width - total_width).max(0.0) / 2.0-20.0;
                ui.add_space(padding);

                ui.group(|ui| {
                    ui.set_width(panel_width);
                    ui.set_min_height(PANEL_HEIGHT);
                    ui.vertical_centered(|ui| {
                        let start_y = ui.cursor().min.y;
                        ui.add_space(20.0);
                        self.render_icon(ui, &self.simulation_icon);
                        ui.add_space(15.0);
                        ui.heading(egui::RichText::new("Simulation").size(22.0).color(Color32::WHITE));
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("Start a simulated network based on pre-defined nodes and obstacles. This mode requires a scene definition file with node positions, obstacles & radio pathloss parameters.\n\nSee the documentation for file format definitions and examples.")
                                .size(16.0),
                        );
                        let used_height = ui.cursor().min.y - start_y;
                        let remaining = PANEL_HEIGHT - used_height - button_height - bottom_padding;
                        let gap = remaining.max(min_button_gap);
                        ui.add_space(gap);
                        let button = egui::Button::new(egui::RichText::new("Select scene").size(15.0).color(Color32::WHITE))
                            .min_size(button_size);
                        if ui.add(button).clicked() {
                            selection = Some(ModeSelection::Simulation);
                        }
                        ui.add_space(bottom_padding);
                    });
                });

                ui.group(|ui| {
                    ui.set_width(panel_width);
                    ui.set_min_height(PANEL_HEIGHT);
                    ui.vertical_centered(|ui| {
                        let start_y = ui.cursor().min.y;
                        ui.add_space(20.0);
                        self.render_icon(ui, &self.realtime_icon);
                        ui.add_space(15.0);
                        ui.heading(egui::RichText::new("Real-time Tracking").size(22.0).color(Color32::WHITE));
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("To begin real-time network log visualization, first select the scene file with node positions, then select the log file actively updated by the log_collector.\n\nSee the documentation for file format definitions and examples.")
                                .size(16.0),
                        );
                        let used_height = ui.cursor().min.y - start_y;
                        // Account for two buttons + spacing
                        let remaining = PANEL_HEIGHT - used_height - (button_height * 2.0) - button_spacing - bottom_padding;
                        let gap = remaining.max(min_button_gap);
                        ui.add_space(gap);

                        // First button: Select scene
                        let scene_label = if self.realtime_scene_path.is_some() {
                            "✅ Scene selected"
                        } else {
                            "Select scene"
                        };
                        let scene_button = egui::Button::new(egui::RichText::new(scene_label).size(15.0).color(Color32::WHITE))
                            .min_size(button_size);
                        if ui.add(scene_button).clicked() {
                            selection = Some(ModeSelection::RealtimeSelectScene);
                        }

                        ui.add_space(button_spacing);

                        // Second button: Connect to stream
                        let stream_label = if self.realtime_log_path.is_some() {
                            "✅ Stream selected"
                        } else {
                            "Connect to stream"
                        };
                        let stream_button = egui::Button::new(egui::RichText::new(stream_label).size(15.0).color(Color32::WHITE))
                            .min_size(button_size);
                        if ui.add(stream_button).clicked() {
                            selection = Some(ModeSelection::RealtimeSelectLog);
                        }

                        ui.add_space(bottom_padding);
                    });
                });

                ui.group(|ui| {
                    ui.set_width(panel_width);
                    ui.set_min_height(PANEL_HEIGHT);
                    ui.vertical_centered(|ui| {
                        let start_y = ui.cursor().min.y;
                        ui.add_space(20.0);
                        self.render_icon(ui, &self.log_icon);
                        ui.add_space(15.0);
                        ui.heading(egui::RichText::new("Log Visualization").size(22.0).color(Color32::WHITE));
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("To view a saved network log, first select the scene file with node positions, then open the log file that was previously created by the log_collector.\n\nSee the documentation for file format definitions and examples.")
                                .size(16.0),
                        );
                        let used_height = ui.cursor().min.y - start_y;
                        // Account for two buttons + spacing
                        let remaining = PANEL_HEIGHT - used_height - (button_height * 2.0) - button_spacing - bottom_padding;
                        let gap = remaining.max(min_button_gap);
                        ui.add_space(gap);

                        // First button: Select scene
                        let scene_label = if self.logvis_scene_path.is_some() {
                            "✅ Scene selected"
                        } else {
                            "Select scene"
                        };
                        let scene_button = egui::Button::new(egui::RichText::new(scene_label).size(15.0).color(Color32::WHITE))
                            .min_size(button_size);
                        if ui.add(scene_button).clicked() {
                            selection = Some(ModeSelection::LogVisSelectScene);
                        }

                        ui.add_space(button_spacing);

                        // Second button: Open log file
                        let log_label = if self.logvis_log_path.is_some() {
                            "✅ Log file selected"
                        } else {
                            "Open log file"
                        };
                        let log_button = egui::Button::new(egui::RichText::new(log_label).size(15.0).color(Color32::WHITE))
                            .min_size(button_size);
                        if ui.add(log_button).clicked() {
                            selection = Some(ModeSelection::LogVisSelectLog);
                        }

                        ui.add_space(bottom_padding);
                    });
                });

                ui.add_space(padding);
                ui.spacing_mut().item_spacing.x = original_spacing;
            });

        });

        selection
    }

    /// Render an icon image or placeholder if loading failed.
    ///
    /// # Parameters
    ///
    /// * `ui` - egui UI context
    /// * `icon` - Optional icon image to display
    fn render_icon(&self, ui: &mut egui::Ui, icon: &Option<Arc<egui::ColorImage>>) {
        if let Some(color_image) = icon {
            let texture =
                ui.ctx()
                    .load_texture("icon", color_image.as_ref().clone(), Default::default());
            ui.add(
                egui::Image::new(&texture)
                    .max_size(egui::vec2(128.0, 128.0))
                    .maintain_aspect_ratio(true),
            );
        } else {
            // Placeholder if image failed to load
            ui.label(egui::RichText::new("🖼").size(50.0));
        }
    }
}

/// The three operational modes available in the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeSelection {
    /// Simulation mode - single scene file selection.
    Simulation,
    /// Real-time tracking - scene file selection step.
    RealtimeSelectScene,
    /// Real-time tracking - log file selection step.
    RealtimeSelectLog,
    /// Real-time tracking - ready to start (both files selected).
    RealtimeTracking {
        scene_path: String,
        log_path: String,
    },
    /// Log visualization - scene file selection step.
    LogVisSelectScene,
    /// Log visualization - log file selection step.
    LogVisSelectLog,
    /// Log visualization - ready to start (both files selected).
    LogVisualization {
        scene_path: String,
        log_path: String,
    },
}
