// Central map: Display nodes, obstacles, and radio indicators

use eframe::egui;
use egui::Color32;
use std::time::Duration;
use crate::simulation::Obstacle;
use crate::ui::{AppState, UICommand};
use crate::ui::app_state::{color_for_message_type, NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT};

pub fn render(ctx: &egui::Context, state: &mut AppState) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("Map");
        ui.separator();

        // Reserve a square drawing area using the smaller of available width/height, centered both horizontally and vertically
        let avail_rect = ui.available_rect_before_wrap();
        let side = avail_rect.width().min(avail_rect.height());
        let x = avail_rect.center().x - side / 2.0;
        let y = avail_rect.center().y - side / 2.0;
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(side, side));
        let response = ui.interact(rect, egui::Id::new("map_canvas"), egui::Sense::click());
        let painter = ui.painter_at(rect);

        // Draw background
        painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);

        // Draw grid: dark blue lines every 1000 world units (0..=10000)
        draw_grid(&painter, rect, ui);

        // Draw obstacles before nodes so nodes appear on top
        draw_obstacles(&painter, rect, &state.obstacles);

        // Draw nodes scaled into rect
        draw_nodes(&painter, rect, state, ui);

        // Draw selected node's radio range
        if let Some(selected) = state.selected {
            draw_radio_range(&painter, rect, &state.nodes[selected]);
        }

        // Handle selection by nearest node (squared-distance comparison)
        handle_node_selection(&response, rect, state);
    });
}

fn draw_grid(painter: &egui::Painter, rect: egui::Rect, _ui: &egui::Ui) {
    let grid_color = Color32::from_rgb(0, 0, 100);
    let grid_stroke = egui::Stroke::new(1.0, grid_color);
    for i in 0..=10 {
        let t = i as f32 / 10.0; // 0.0 ..= 1.0 in 0.1 steps
        // Vertical line at x = i * 1000
        let x = egui::lerp(rect.left()..=rect.right(), t);
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], grid_stroke);
        // Horizontal line at y = i * 1000
        let y = egui::lerp(rect.top()..=rect.bottom(), t);
        painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], grid_stroke);
    }
}

fn draw_obstacles(painter: &egui::Painter, rect: egui::Rect, obstacles: &[Obstacle]) {
    let obstacle_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 255);
    let obstacle_stroke = egui::Stroke::new(1.5, Color32::from_rgb(255, 255, 255));
    
    for obs in obstacles {
        match obs {
            Obstacle::Rectangle { position, .. } => {
                // Compute bounds from corners in world units
                let l = position.top_left.x.min(position.bottom_right.x);
                let r = position.top_left.x.max(position.bottom_right.x);
                let t = position.top_left.y.min(position.bottom_right.y);
                let b = position.top_left.y.max(position.bottom_right.y);

                // Map world 0..10000 to rect coordinates
                let left = egui::lerp(rect.left()..=rect.right(), l as f32 / 10000.0);
                let right = egui::lerp(rect.left()..=rect.right(), r as f32 / 10000.0);
                let top = egui::lerp(rect.top()..=rect.bottom(), t as f32 / 10000.0);
                let bottom = egui::lerp(rect.top()..=rect.bottom(), b as f32 / 10000.0);
                let rect_px = egui::Rect::from_min_max(egui::pos2(left.min(right), top.min(bottom)), egui::pos2(left.max(right), top.max(bottom)));
                painter.rect_filled(rect_px, 0.0, obstacle_fill);
                painter.rect_stroke(rect_px, 0.0, obstacle_stroke);
            }
            Obstacle::Circle { position, .. } => {
                let cx = egui::lerp(rect.left()..=rect.right(), position.center.x as f32 / 10000.0);
                let cy = egui::lerp(rect.top()..=rect.bottom(), position.center.y as f32 / 10000.0);
                // Uniform scale for radius: take min scale to keep circle round in non-square rects
                let scale_x = rect.width() / 10000.0;
                let scale_y = rect.height() / 10000.0;
                let units_to_pixels = scale_x.min(scale_y);
                let r = position.radius as f32 * units_to_pixels;
                let center_px = egui::pos2(cx, cy);
                painter.circle_filled(center_px, r, obstacle_fill);
                painter.circle_stroke(center_px, r, obstacle_stroke);
            }
        }
    }
}

fn draw_nodes(painter: &egui::Painter, rect: egui::Rect, state: &mut AppState, ui: &egui::Ui) {
    let radius = 4.0;
    
    // Collect expired indicators first to avoid borrowing issues
    let expired_indicators: Vec<u32> = state.node_radio_transfer_indicators
        .iter()
        .filter(|(_, (expiry, _, _))| *expiry <= std::time::Instant::now())
        .map(|(id, _)| *id)
        .collect();
    
    // Remove expired indicators
    for id in expired_indicators {
        state.node_radio_transfer_indicators.remove(&id);
    }
    
    for p in state.nodes.iter() {
        let pos = egui::pos2(
            egui::lerp(rect.left()..=rect.right(), p.position.x as f32 / 10000f32),
            egui::lerp(rect.top()..=rect.bottom(), p.position.y as f32 / 10000f32),
        );

        let mut color = ui.visuals().widgets.inactive.fg_stroke.color;

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
                ui.visuals().text_color(),
            );
        }

        // Draw radio transfer indicator
        draw_radio_indicator(painter, rect, state, &pos, p.node_id);
    }
}

fn draw_radio_indicator(painter: &egui::Painter, rect: egui::Rect, state: &AppState, pos: &egui::Pos2, node_id: u32) {
    if let Some((expiry, message_type, distance)) = state.node_radio_transfer_indicators.get(&node_id) {
        let remaining = *expiry - std::time::Instant::now();
        if remaining > Duration::from_millis(0) {
            let alpha = (remaining.as_millis() as f32 / NODE_RADIO_TRANSFER_INDICATOR_TIMEOUT as f32).clamp(0.0, 1.0);
            // Convert world distance to pixels like we do for coordinates (range 0..10000)
            let scale_x = rect.width() / 10000.0;
            let scale_y = rect.height() / 10000.0;
            let units_to_pixels = scale_x.min(scale_y);
            let radius = (*distance as f32 * units_to_pixels) * (1.0 - alpha);
            let color = color_for_message_type(*message_type, alpha);
            painter.circle_stroke(*pos, radius, egui::Stroke::new(1.0, color));
        }
    }
}

fn draw_radio_range(painter: &egui::Painter, rect: egui::Rect, selected_node: &crate::ui::NodeUIState) {
    let pos = egui::pos2(
        egui::lerp(rect.left()..=rect.right(), selected_node.position.x as f32 / 10000f32),
        egui::lerp(rect.top()..=rect.bottom(), selected_node.position.y as f32 / 10000f32),
    );

    let scale_x = rect.width() / 10000.0;
    let scale_y = rect.height() / 10000.0;
    let units_to_pixels = scale_x.min(scale_y);
    let radius = selected_node.radio_strength as f32 * units_to_pixels;
    painter.circle_filled(pos, radius, Color32::from_rgba_unmultiplied(0, 128, 255, 50));
}

fn handle_node_selection(response: &egui::Response, rect: egui::Rect, state: &mut AppState) {
    if response.clicked() {
        if let Some(click_pos) = response.interact_pointer_pos() {
            let mut best: Option<(usize, f32)> = None;
            for (i, p) in state.nodes.iter().enumerate() {
                let pos = egui::pos2(
                    egui::lerp(rect.left()..=rect.right(), p.position.x as f32 / 10000f32),
                    egui::lerp(rect.top()..=rect.bottom(), p.position.y as f32 / 10000f32),
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
