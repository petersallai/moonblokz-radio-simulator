//! # Central Map Visualization
//!
//! This module renders the main 2D map view showing:
//! - A grid representing the dynamic world coordinate system
//! - Obstacles (circles and rectangles) that block radio signals
//! - Nodes as colored circles with optional ID labels
//! - Selected node with a semi-transparent radio range indicator
//! - Animated radio transmission pulses expanding from transmitting nodes
//!
//! ## Coordinate Mapping
//!
//! The simulation uses world coordinates defined by top_left and bottom_right bounds.
//! These are linearly mapped to screen pixels using `egui::lerp`, maintaining
//! aspect ratio (from width/height in meters) by fitting the map centered in the available space.
//!
//! ## Radio Transmission Animation
//!
//! When a node transmits, an animated indicator shows a colored circle expanding
//! from the node to its effective radio range over 1 second, fading from fully
//! opaque to transparent. The color indicates the message type.
//!
//! ## Node Selection
//!
//! Clicking on the map selects the nearest node (using squared distance for
//! efficiency). Selecting a node triggers a `RequestNodeInfo` command to populate
//! the right panel inspector with that node's message history.

use crate::simulation::Obstacle;
use crate::ui::app_state::{NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT, color_for_message_type};
use crate::ui::{AppState, UICommand};
use eframe::egui;
use egui::Color32;
use embassy_time::{Duration, Instant};

/// Render the central map panel showing the simulation world.
///
/// This is the main rendering function for the map. It:
/// 1. Reserves a drawing area with proper aspect ratio, centered in the available space
/// 2. Draws the background and coordinate grid
/// 3. Renders obstacles, then nodes, then selection indicators
/// 4. Handles mouse clicks for node selection
///
/// # Parameters
///
/// * `ctx` - egui context for rendering
/// * `state` - Mutable application state for updating selection
pub fn render(ctx: &egui::Context, state: &mut AppState) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("Map");
        ui.separator();

        // Calculate aspect ratio from world dimensions (width/height in meters)
        let aspect_ratio = if state.height > 0.0 {
            (state.width / state.height) as f32
        } else {
            1.0 // Fallback to square if height is invalid
        };

        // Reserve a drawing area with proper aspect ratio, centered in available space
        let avail_rect = ui.available_rect_before_wrap();
        let avail_width = avail_rect.width();
        let avail_height = avail_rect.height();

        // Calculate best fit dimensions maintaining aspect ratio
        let (map_width, map_height) = if avail_width / avail_height > aspect_ratio {
            // Container is wider than map aspect ratio - constrain by height
            let height = avail_height;
            let width = height * aspect_ratio;
            (width, height)
        } else {
            // Container is taller than map aspect ratio - constrain by width
            let width = avail_width;
            let height = width / aspect_ratio;
            (width, height)
        };

        // Center the map in the available space
        let x = avail_rect.center().x - map_width / 2.0;
        let y = avail_rect.center().y - map_height / 2.0;
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(map_width, map_height));
        let response = ui.interact(rect, egui::Id::new("map_canvas"), egui::Sense::click());
        let painter = ui.painter_at(rect);

        // Draw background
        painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);

        // Draw background image if loaded
        if let Some(ref texture) = state.background_image_texture {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(texture.id(), rect, uv, Color32::WHITE);
        }

        // Draw grid: dark blue lines every 1000 world units
        draw_grid(&painter, rect, state);

        // Draw obstacles before nodes so nodes appear on top
        draw_obstacles(&painter, rect, state);

        // Draw nodes scaled into rect
        draw_nodes(&painter, rect, state, ui);

        // Draw selected node's radio range
        if let Some(selected) = state.selected {
            draw_radio_range(&painter, rect, &state.nodes[selected], state);
        }

        // Handle selection by nearest node (squared-distance comparison)
        handle_node_selection(&response, rect, state);
    });
}

/// Draw the coordinate grid with square cells.
///
/// The longer dimension (width or height) is divided into 10 cells, and that spacing
/// is used for both axes to create square grid cells. Renders dark blue lines.
///
/// # Parameters
///
/// * `painter` - egui painter for drawing primitives
/// * `rect` - The screen-space rectangle representing the map area
/// * `state` - Application state for world bounds
fn draw_grid(painter: &egui::Painter, rect: egui::Rect, state: &AppState) {
    let grid_color = Color32::from_rgb(0, 0, 100);
    let grid_stroke = egui::Stroke::new(1.0, grid_color);

    let world_min_x = state.world_top_left.x;
    let world_max_x = state.world_bottom_right.x;
    let world_min_y = state.world_top_left.y;
    let world_max_y = state.world_bottom_right.y;
    let world_width = (world_max_x - world_min_x).abs();
    let world_height = (world_max_y - world_min_y).abs();

    // Divide the longer dimension (in meters) into 10 cells
    let spacing_meters = if state.width >= state.height {
        state.width / 10.0
    } else {
        state.height / 10.0
    };

    // Convert spacing from meters to world coordinates
    let scale_x = world_width / state.width;
    let scale_y = world_height / state.height;
    let grid_spacing_x = spacing_meters * scale_x;
    let grid_spacing_y = spacing_meters * scale_y;

    // Vertical lines (handle both normal and inverted X coordinates)
    let (x_start, x_end) = if world_min_x <= world_max_x {
        (world_min_x, world_max_x)
    } else {
        (world_max_x, world_min_x)
    };
    let start_x = (x_start / grid_spacing_x).ceil() * grid_spacing_x;
    let mut x = start_x;
    while x <= x_end {
        let t = ((x - world_min_x).abs() / world_width) as f32;
        let screen_x = egui::lerp(rect.left()..=rect.right(), t);
        painter.line_segment([egui::pos2(screen_x, rect.top()), egui::pos2(screen_x, rect.bottom())], grid_stroke);
        x += grid_spacing_x;
    }

    // Horizontal lines (handle both normal and inverted Y coordinates)
    let (y_start, y_end) = if world_min_y <= world_max_y {
        (world_min_y, world_max_y)
    } else {
        (world_max_y, world_min_y)
    };
    let start_y = (y_start / grid_spacing_y).ceil() * grid_spacing_y;
    let mut y = start_y;
    while y <= y_end {
        let t = ((y - world_min_y).abs() / world_height) as f32;
        let screen_y = egui::lerp(rect.top()..=rect.bottom(), t);
        painter.line_segment([egui::pos2(rect.left(), screen_y), egui::pos2(rect.right(), screen_y)], grid_stroke);
        y += grid_spacing_y;
    }
}
/// Draw all obstacles (rectangles and circles) on the map.
///
/// Obstacles are rendered as white filled shapes with white outlines.
/// They represent physical barriers that block line-of-sight radio propagation.
///
/// # Parameters
///
/// * `painter` - egui painter for drawing
/// * `rect` - The screen-space map rectangle
/// * `state` - Application state for world bounds and obstacles
fn draw_obstacles(painter: &egui::Painter, rect: egui::Rect, state: &AppState) {
    let obstacle_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 255);
    let obstacle_stroke = egui::Stroke::new(1.5, Color32::from_rgb(255, 255, 255));

    let world_min_x = state.world_top_left.x;
    let world_max_x = state.world_bottom_right.x;
    let world_min_y = state.world_top_left.y;
    let world_max_y = state.world_bottom_right.y;
    let world_width = world_max_x - world_min_x;
    let world_height = world_max_y - world_min_y;

    for obs in &state.obstacles {
        match obs {
            Obstacle::Rectangle { position, .. } => {
                // Compute bounds from corners in world units
                let l = position.top_left.x.min(position.bottom_right.x);
                let r = position.top_left.x.max(position.bottom_right.x);
                let t = position.top_left.y.min(position.bottom_right.y);
                let b = position.top_left.y.max(position.bottom_right.y);

                // Map world coordinates to rect coordinates
                let left = egui::lerp(rect.left()..=rect.right(), ((l - world_min_x) / world_width) as f32);
                let right = egui::lerp(rect.left()..=rect.right(), ((r - world_min_x) / world_width) as f32);
                let top = egui::lerp(rect.top()..=rect.bottom(), ((t - world_min_y) / world_height) as f32);
                let bottom = egui::lerp(rect.top()..=rect.bottom(), ((b - world_min_y) / world_height) as f32);
                let rect_px = egui::Rect::from_min_max(egui::pos2(left.min(right), top.min(bottom)), egui::pos2(left.max(right), top.max(bottom)));
                painter.rect_filled(rect_px, 0.0, obstacle_fill);
                painter.rect_stroke(rect_px, 0.0, obstacle_stroke);
            }
            Obstacle::Circle { position, .. } => {
                let cx = egui::lerp(rect.left()..=rect.right(), ((position.center.x - world_min_x) / world_width) as f32);
                let cy = egui::lerp(rect.top()..=rect.bottom(), ((position.center.y - world_min_y) / world_height) as f32);

                // Radius is in meters, convert to pixels:
                // meters_to_pixels = meters * (pixels / meters)
                // where pixels/meters = rect_pixels / world_meters
                let pixels_per_meter_x = rect.width() / state.width as f32;
                let pixels_per_meter_y = rect.height() / state.height as f32;
                let avg_pixels_per_meter = (pixels_per_meter_x + pixels_per_meter_y) / 2.0;

                let r = position.radius as f32 * avg_pixels_per_meter;

                let center_px = egui::pos2(cx, cy);
                painter.circle_filled(center_px, r, obstacle_fill);
                painter.circle_stroke(center_px, r, obstacle_stroke);
            }
        }
    }
}

/// Draw all nodes as colored circles with optional ID labels.
///
/// Nodes that were reached during a measurement are rendered in green,
/// while others use the default theme color. Expired radio transmission
/// indicators are cleaned up during this pass.
///
/// # Parameters
///
/// * `painter` - egui painter
/// * `rect` - Screen-space map rectangle
/// * `state` - Application state (for indicators and node data)
/// * `ui` - UI context for text rendering
fn draw_nodes(painter: &egui::Painter, rect: egui::Rect, state: &mut AppState, ui: &egui::Ui) {
    let radius = 4.0;

    let world_min_x = state.world_top_left.x;
    let world_max_x = state.world_bottom_right.x;
    let world_min_y = state.world_top_left.y;
    let world_max_y = state.world_bottom_right.y;
    let world_width = world_max_x - world_min_x;
    let world_height = world_max_y - world_min_y;

    // Collect expired indicators first to avoid borrowing issues
    let expired_indicators: Vec<u32> = state
        .node_radio_transfer_indicators
        .iter()
        .filter(|(_, (expiry, _, _))| *expiry <= Instant::now())
        .map(|(id, _)| *id)
        .collect();

    // Remove expired indicators
    for id in expired_indicators {
        state.node_radio_transfer_indicators.remove(&id);
    }

    for p in state.nodes.iter() {
        let pos = egui::pos2(
            egui::lerp(rect.left()..=rect.right(), ((p.position.x - world_min_x) / world_width) as f32),
            egui::lerp(rect.top()..=rect.bottom(), ((p.position.y - world_min_y) / world_height) as f32),
        );

        let mut color = Color32::from_rgb(40, 200, 255);

        if state.measurement_identifier != 0 && state.reached_nodes.contains(&p.node_id) {
            color = Color32::from_rgb(0, 255, 0); // Green if reached in current measurement
        }

        painter.circle_filled(pos, radius, color);

        // Optional ID label next to each node
        if state.show_node_ids {
            let label_pos = egui::pos2(pos.x + 6.0, pos.y - 6.0);
            painter.text(
                label_pos,
                egui::Align2::LEFT_BOTTOM,
                format!("#{}", p.node_id),
                egui::FontId::monospace(12.0),
                Color32::from_rgb(40, 200, 255),
            );
        }

        // Draw radio transfer indicator
        draw_radio_indicator(painter, rect, state, &pos, p.node_id);
    }
}

/// Draw an animated radio transmission indicator for a node.
///
/// The indicator shows as an expanding, fading circle representing the RF transmission.
/// The animation lasts 1 second, growing from the node to its effective distance while
/// fading from full opacity to transparent. Color indicates message type.
///
/// # Parameters
///
/// * `painter` - egui painter
/// * `rect` - Screen-space map rectangle
/// * `state` - Application state (for indicator data)
/// * `pos` - Screen position of the transmitting node
/// * `node_id` - ID of the node to check for active indicators
fn draw_radio_indicator(painter: &egui::Painter, rect: egui::Rect, state: &AppState, pos: &egui::Pos2, node_id: u32) {
    if let Some((expiry, message_type, distance)) = state.node_radio_transfer_indicators.get(&node_id) {
        let remaining = *expiry - Instant::now();
        if remaining > Duration::from_millis(0) {
            let alpha = (remaining.as_millis() as f32 / NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT as f32).clamp(0.0, 1.0);
            // Distance is in meters, convert to pixels
            let pixels_per_meter_x = rect.width() / state.width as f32;
            let pixels_per_meter_y = rect.height() / state.height as f32;
            let avg_pixels_per_meter = (pixels_per_meter_x + pixels_per_meter_y) / 2.0;
            let radius = (*distance as f32 * avg_pixels_per_meter) * (1.0 - alpha);
            let color = color_for_message_type(*message_type, alpha);
            painter.circle_stroke(*pos, radius, egui::Stroke::new(1.0, color));
        }
    }
}

/// Draw the effective radio range for the selected node.
///
/// Renders a semi-transparent blue circle showing the maximum distance
/// at which this node can communicate under ideal conditions.
///
/// # Parameters
///
/// * `painter` - egui painter
/// * `rect` - Screen-space map rectangle
/// * `selected_node` - The currently selected node
fn draw_radio_range(painter: &egui::Painter, rect: egui::Rect, selected_node: &crate::ui::NodeUIState, state: &AppState) {
    let world_min_x = state.world_top_left.x;
    let world_max_x = state.world_bottom_right.x;
    let world_min_y = state.world_top_left.y;
    let world_max_y = state.world_bottom_right.y;
    let world_width = world_max_x - world_min_x;
    let world_height = world_max_y - world_min_y;

    let pos = egui::pos2(
        egui::lerp(rect.left()..=rect.right(), ((selected_node.position.x - world_min_x) / world_width) as f32),
        egui::lerp(rect.top()..=rect.bottom(), ((selected_node.position.y - world_min_y) / world_height) as f32),
    );

    // Radio strength is in meters, convert to pixels
    let pixels_per_meter_x = rect.width() / state.width as f32;
    let pixels_per_meter_y = rect.height() / state.height as f32;
    let avg_pixels_per_meter = (pixels_per_meter_x + pixels_per_meter_y) / 2.0;
    let radius = selected_node.radio_strength as f32 * avg_pixels_per_meter;
    painter.circle_filled(pos, radius, Color32::from_rgba_unmultiplied(0, 128, 255, 50));
}

/// Handle mouse clicks on the map for node selection.
///
/// Finds the nearest node to the click position using squared distance (to avoid sqrt).
/// If a node is clicked again, it is deselected. Selecting a node sends a
/// `RequestNodeInfo` command to populate the inspector panel.
///
/// # Parameters
///
/// * `response` - egui response from the map interaction area
/// * `rect` - Screen-space map rectangle
/// * `state` - Mutable application state for updating selection
fn handle_node_selection(response: &egui::Response, rect: egui::Rect, state: &mut AppState) {
    if response.clicked() {
        if let Some(click_pos) = response.interact_pointer_pos() {
            let world_min_x = state.world_top_left.x;
            let world_max_x = state.world_bottom_right.x;
            let world_min_y = state.world_top_left.y;
            let world_max_y = state.world_bottom_right.y;
            let world_width = world_max_x - world_min_x;
            let world_height = world_max_y - world_min_y;

            let mut best: Option<(usize, f32)> = None;
            for (i, p) in state.nodes.iter().enumerate() {
                let pos = egui::pos2(
                    egui::lerp(rect.left()..=rect.right(), ((p.position.x - world_min_x) / world_width) as f32),
                    egui::lerp(rect.top()..=rect.bottom(), ((p.position.y - world_min_y) / world_height) as f32),
                );
                let d2 = pos.distance_sq(click_pos);
                if best.map_or(true, |(_, bd)| d2 < bd) {
                    best = Some((i, d2));
                }
            }
            let new_selected = best.map(|(i, _)| i);
            if new_selected != state.selected {
                state.selected = new_selected;
                if let Some(new_selected) = new_selected {
                    let node_id = &state.nodes[new_selected].node_id;
                    state.ui_command_tx.try_send(UICommand::RequestNodeInfo(node_id.clone())).ok();
                }
            } else {
                state.selected = None;
            }
        }
    }
}
