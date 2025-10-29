// Mode selector screen for the MoonBlokz Radio Simulator
//
// This screen allows users to choose between three modes:
// 1. Simulation: Start a simulated network
// 2. Real-time Tracking: Visualize live network logs
// 3. Log Visualization: Open previously saved log files

use eframe::egui;
use egui::Color32;
use std::sync::Arc;

pub struct ModeSelector {
    simulation_icon: Option<Arc<egui::ColorImage>>,
    realtime_icon: Option<Arc<egui::ColorImage>>,
    log_icon: Option<Arc<egui::ColorImage>>,
}

impl ModeSelector {
    pub fn new() -> Self {
        // Load icons
        let simulation_icon = Self::load_image(include_bytes!("../../icons/simulation_icon.png"));
        let realtime_icon = Self::load_image(include_bytes!("../../icons/realtime_icon.png"));
        let log_icon = Self::load_image(include_bytes!("../../icons/log_icon.png"));
        
        Self {
            simulation_icon,
            realtime_icon,
            log_icon,
        }
    }

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
                            egui::RichText::new("To begin real-time network log visualization, first select the log file actively updated by the log_collector. Ensure a scene.json file containing node position definitions is present in the same directory.\n\nSee the documentation for file format definitions and examples.")
                                .size(16.0),
                        );
                        let used_height = ui.cursor().min.y - start_y;
                        let remaining = PANEL_HEIGHT - used_height - button_height - bottom_padding;
                        let gap = remaining.max(min_button_gap);
                        ui.add_space(gap);
                        let button = egui::Button::new(egui::RichText::new("Connect to stream").size(15.0).color(Color32::WHITE))
                            .min_size(button_size);
                        if ui.add(button).clicked() {
                            selection = Some(ModeSelection::RealtimeTracking);
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
                            egui::RichText::new("To view a saved network log, you must provide a log file that was previously created by the log_collector. This mode also requires a scene.json file, which defines the network node positions, to be present in the same directory.\n\nSee the documentation for file format definitions and examples.")
                                .size(16.0),
                        );
                        let used_height = ui.cursor().min.y - start_y;
                        let remaining = PANEL_HEIGHT - used_height - button_height - bottom_padding;
                        let gap = remaining.max(min_button_gap);
                        ui.add_space(gap);
                        let button = egui::Button::new(egui::RichText::new("Open log file").size(15.0).color(Color32::WHITE))
                            .min_size(button_size);
                        if ui.add(button).clicked() {
                            selection = Some(ModeSelection::LogVisualization);
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
    
    fn render_icon(&self, ui: &mut egui::Ui, icon: &Option<Arc<egui::ColorImage>>) {
        if let Some(color_image) = icon {
            let texture = ui.ctx().load_texture(
                "icon",
                color_image.as_ref().clone(),
                Default::default(),
            );
            ui.add(egui::Image::new(&texture)
                .max_size(egui::vec2(128.0, 128.0))
                .maintain_aspect_ratio(true));
        } else {
            // Placeholder if image failed to load
            ui.label(egui::RichText::new("🖼").size(50.0));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeSelection {
    Simulation,
    RealtimeTracking,
    LogVisualization,
}
