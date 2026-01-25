//! Scene loading, parsing, and validation logic.
//!
//! Contains all data structures for scene configuration and provides
//! functions for loading and validating scenes for both simulation
//! and analyzer modes.

use anyhow::Context;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;

/// Scene loading mode determines which fields are required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneMode {
    /// Simulation mode requires physics parameters.
    Simulation,
    /// Analyzer mode requires effective_distance per node.
    Analyzer,
}

/// Error type for scene loading failures.
#[derive(Debug)]
pub enum SceneLoadError {
    FileReadError(String),
    ParseError(String),
    ValidationError(String),
}

impl std::fmt::Display for SceneLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SceneLoadError::FileReadError(msg) => write!(f, "Failed to read file: {}", msg),
            SceneLoadError::ParseError(msg) => write!(f, "Failed to parse JSON: {}", msg),
            SceneLoadError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
        }
    }
}

impl std::error::Error for SceneLoadError {}

/// Parameters defining the radio channel propagation model.
#[derive(Deserialize, Clone)]
pub struct PathLossParameters {
    /// Path loss exponent (n). 2.0 for free space, 2.7-3.5 for urban.
    pub path_loss_exponent: f32,
    /// Standard deviation for log-normal shadowing (σ) in dB.
    pub shadowing_sigma: f32,
    /// Path loss at the reference distance d₀ (typically 1 meter) in dB.
    pub path_loss_at_reference_distance: f32,
    /// The thermal noise floor of the receiver in dBm.
    pub noise_floor: f32,
}

/// LoRa modulation parameters.
#[derive(Deserialize, Clone)]
pub struct LoraParameters {
    pub bandwidth: u32,
    pub spreading_factor: u8,
    pub coding_rate: u32,
    pub preamble_symbols: f32,
    pub crc_enabled: bool,
    pub low_data_rate_optimization: bool,
}

/// Radio module configuration for the simulated radio manager.
#[derive(Deserialize, Clone)]
pub struct RadioModuleConfig {
    /// Inter-packet gap inside a single message (ms).
    pub delay_between_tx_packets: u16,
    /// Delay between separate messages (ms).
    pub delay_between_tx_messages: u8,
    /// Minimum spacing between echo requests (minutes).
    pub echo_request_minimal_interval: u16,
    /// Target interval (ms) for echo messages.
    pub echo_messages_target_interval: u8,
    /// Timeout (ms) for collecting echo messages.
    pub echo_gathering_timeout: u8,
    /// Artificial delay (ms) before relaying position reports.
    pub relay_position_delay: u8,
    /// Encoded scoring matrix thresholds.
    pub scoring_matrix: [u8; 5],
    /// Interval (ms) between retries for missing packets.
    pub retry_interval_for_missing_packets: u8,
    /// Maximum random delay in milliseconds added to transmission timing.
    pub tx_maximum_random_delay: u16,
}

/// Simple 2D point.
#[derive(Debug, Deserialize, Clone)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Rectangle position with two corners.
#[derive(Debug, Deserialize, Clone)]
pub struct RectPos {
    #[serde(rename = "top-left-position")]
    pub top_left: Point,
    #[serde(rename = "bottom-right-position")]
    pub bottom_right: Point,
}

/// Circle position defined by its center.
#[derive(Debug, Deserialize, Clone)]
pub struct CirclePos {
    #[serde(rename = "center_position")]
    pub center: Point,
    pub radius: f64,
}

/// Obstacles represented as tagged enum.
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum Obstacle {
    #[serde(rename = "rectangle")]
    Rectangle {
        #[serde(flatten)]
        position: RectPos,
    },
    #[serde(rename = "circle")]
    Circle {
        #[serde(flatten)]
        position: CirclePos,
    },
}

/// Node structure with position and radio strength.
#[derive(Deserialize, Clone)]
pub struct Node {
    pub node_id: u32,
    pub position: Point,
    /// Radio strength in dBm (used for simulation).
    #[serde(default)]
    pub radio_strength: f32,
    /// Pre-calculated effective radio range in meters (required for analyzer).
    #[serde(default)]
    pub effective_distance: Option<u32>,
}

/// Root structure representing the entire scene.
#[derive(Deserialize)]
pub struct Scene {
    /// Path loss model parameters (required for simulation, optional for analyzer).
    #[serde(default)]
    pub path_loss_parameters: Option<PathLossParameters>,
    /// LoRa parameters (required for simulation, optional for analyzer).
    #[serde(default)]
    pub lora_parameters: Option<LoraParameters>,
    /// Module-level configuration (required for simulation, optional for analyzer).
    #[serde(default)]
    pub radio_module_config: Option<RadioModuleConfig>,
    /// All nodes present in the scene.
    pub nodes: Vec<Node>,
    /// Static obstacles for line-of-sight checks.
    #[serde(default)]
    pub obstacles: Vec<Obstacle>,
    /// Top-left corner of the world coordinate system.
    #[serde(rename = "world_top_left")]
    pub world_top_left: Point,
    /// Bottom-right corner of the world coordinate system.
    #[serde(rename = "world_bottom_right")]
    pub world_bottom_right: Point,
    /// Width of the world in meters.
    pub width: f64,
    /// Height of the world in meters.
    pub height: f64,
    /// Pre-calculated X-axis scaling factor.
    #[serde(skip)]
    pub scale_x: f64,
    /// Pre-calculated Y-axis scaling factor.
    #[serde(skip)]
    pub scale_y: f64,
    /// Optional path to background image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_image: Option<String>,
    /// Optional link quality threshold for "poor" (real-time tracking).
    #[serde(default)]
    pub link_quality_weak_threshold: Option<u8>,
    /// Optional link quality threshold for "excellent" (real-time tracking).
    #[serde(default)]
    pub link_quality_excellent_threshold: Option<u8>,
}

/// Load and parse a scene from a file.
///
/// # Parameters
///
/// * `path` - Path to the scene JSON file
/// * `mode` - Scene mode determining validation rules
///
/// # Returns
///
/// Parsed and validated Scene or an error.
pub fn load_scene(path: &str, mode: SceneMode) -> Result<Scene, SceneLoadError> {
    let data = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path))
        .map_err(|e| SceneLoadError::FileReadError(e.to_string()))?;

    let mut scene: Scene = serde_json::from_str(&data)
        .context("Invalid JSON format")
        .map_err(|e| SceneLoadError::ParseError(e.to_string()))?;

    // If background_image is specified, prepend the scene file's directory
    if let Some(ref bg_image) = scene.background_image {
        use std::path::Path;
        if let Some(parent_dir) = Path::new(path).parent() {
            let full_path = parent_dir.join(bg_image);
            scene.background_image = Some(full_path.to_string_lossy().to_string());
        }
    }

    // Pre-calculate scaling factors
    let world_width = scene.world_bottom_right.x - scene.world_top_left.x;
    let world_height = scene.world_bottom_right.y - scene.world_top_left.y;
    scene.scale_x = scene.width / world_width;
    scene.scale_y = scene.height / world_height;

    // Validate the scene
    validate_scene(&scene, mode).map_err(SceneLoadError::ValidationError)?;

    Ok(scene)
}

/// Validate scene configuration based on the mode.
///
/// # Parameters
///
/// * `scene` - The parsed scene to validate
/// * `mode` - Determines which fields are required
///
/// # Returns
///
/// `Ok(())` if validation passes, `Err(String)` with error description otherwise.
pub fn validate_scene(scene: &Scene, mode: SceneMode) -> Result<(), String> {
    const MAX_WORLD_COORD: f64 = 10000.0;
    const MAX_NODES: usize = 10000;
    const MIN_RADIO_STRENGTH: f32 = -50.0;
    const MAX_RADIO_STRENGTH: f32 = 50.0;

    // Check node count
    if scene.nodes.is_empty() {
        return Err("Scene must contain at least one node".to_string());
    }
    if scene.nodes.len() > MAX_NODES {
        return Err(format!(
            "Node count {} exceeds maximum of {}",
            scene.nodes.len(),
            MAX_NODES
        ));
    }

    // Check for duplicate node IDs
    let mut node_ids = HashSet::new();
    for node in &scene.nodes {
        if !node_ids.insert(node.node_id) {
            return Err(format!("Duplicate node_id found: {}", node.node_id));
        }
    }

    // Validate each node
    for node in &scene.nodes {
        // Check position bounds
        if node.position.x > MAX_WORLD_COORD || node.position.y > MAX_WORLD_COORD {
            return Err(format!(
                "Node {} position ({}, {}) exceeds world bounds (0-{})",
                node.node_id, node.position.x, node.position.y, MAX_WORLD_COORD
            ));
        }

        // Mode-specific validation
        match mode {
            SceneMode::Simulation => {
                // Check radio strength is realistic
                if node.radio_strength < MIN_RADIO_STRENGTH
                    || node.radio_strength > MAX_RADIO_STRENGTH
                {
                    return Err(format!(
                        "Node {} radio_strength {} dBm outside realistic range ({} to {} dBm)",
                        node.node_id, node.radio_strength, MIN_RADIO_STRENGTH, MAX_RADIO_STRENGTH
                    ));
                }
            }
            SceneMode::Analyzer => {
                // Analyzer mode requires effective_distance
                if node.effective_distance.is_none() {
                    return Err(format!(
                        "Node {} is missing required 'effective_distance' field for analyzer mode",
                        node.node_id
                    ));
                }
            }
        }
    }

    // Simulation mode requires physics parameters
    if mode == SceneMode::Simulation {
        if scene.path_loss_parameters.is_none() {
            return Err("Simulation mode requires 'path_loss_parameters'".to_string());
        }
        if scene.lora_parameters.is_none() {
            return Err("Simulation mode requires 'lora_parameters'".to_string());
        }
        if scene.radio_module_config.is_none() {
            return Err("Simulation mode requires 'radio_module_config'".to_string());
        }

        let lora = scene.lora_parameters.as_ref().unwrap();
        if lora.spreading_factor < 5 || lora.spreading_factor > 12 {
            return Err(format!(
                "Invalid spreading_factor {}, must be 5-12",
                lora.spreading_factor
            ));
        }
        if lora.bandwidth == 0 {
            return Err("Invalid bandwidth, must be positive".to_string());
        }
        if lora.coding_rate < 1 || lora.coding_rate > 4 {
            return Err(format!(
                "Invalid coding_rate {}, must be 1-4 (representing 4/5 to 4/8)",
                lora.coding_rate
            ));
        }
        if lora.preamble_symbols < 0.0 {
            return Err("Invalid preamble_symbols, must be non-negative".to_string());
        }

        let path_loss = scene.path_loss_parameters.as_ref().unwrap();
        if path_loss.path_loss_exponent <= 0.0 {
            return Err("Invalid path_loss_exponent, must be positive".to_string());
        }
        if path_loss.shadowing_sigma < 0.0 {
            return Err("Invalid shadowing_sigma, must be non-negative".to_string());
        }
    }

    // Optional link quality thresholds validation
    if let (Some(weak), Some(excellent)) = (
        scene.link_quality_weak_threshold,
        scene.link_quality_excellent_threshold,
    ) {
        if weak >= excellent {
            return Err(format!(
                "Invalid link quality thresholds: weak {} must be less than excellent {}",
                weak, excellent
            ));
        }
        if weak > 63 || excellent > 63 {
            return Err("Link quality thresholds must be within 0-63".to_string());
        }
    }

    // Validate obstacles
    for (idx, obstacle) in scene.obstacles.iter().enumerate() {
        match obstacle {
            Obstacle::Rectangle { position } => {
                if position.top_left.x > MAX_WORLD_COORD
                    || position.top_left.y > MAX_WORLD_COORD
                    || position.bottom_right.x > MAX_WORLD_COORD
                    || position.bottom_right.y > MAX_WORLD_COORD
                {
                    return Err(format!(
                        "Obstacle {} (rectangle) has coordinates exceeding world bounds (0-{})",
                        idx, MAX_WORLD_COORD
                    ));
                }
                if position.top_left.x >= position.bottom_right.x
                    || position.top_left.y >= position.bottom_right.y
                {
                    return Err(format!(
                        "Obstacle {} (rectangle) has invalid geometry: top-left ({}, {}) must be strictly less than bottom-right ({}, {})",
                        idx,
                        position.top_left.x,
                        position.top_left.y,
                        position.bottom_right.x,
                        position.bottom_right.y
                    ));
                }
            }
            Obstacle::Circle { position } => {
                if position.center.x > MAX_WORLD_COORD || position.center.y > MAX_WORLD_COORD {
                    return Err(format!(
                        "Obstacle {} (circle) center ({}, {}) exceeds world bounds (0-{})",
                        idx, position.center.x, position.center.y, MAX_WORLD_COORD
                    ));
                }
                if position.radius == 0.0 {
                    return Err(format!("Obstacle {} (circle) has zero radius", idx));
                }
                let max_extent_x = position.center.x + position.radius;
                let max_extent_y = position.center.y + position.radius;
                if max_extent_x > MAX_WORLD_COORD || max_extent_y > MAX_WORLD_COORD {
                    return Err(format!(
                        "Obstacle {} (circle) extends beyond world bounds (0-{})",
                        idx, MAX_WORLD_COORD
                    ));
                }
            }
        }
    }

    Ok(())
}

// ============================================================================
// Conversion traits to simulation types
// ============================================================================

impl From<Point> for crate::simulation::types::Point {
    fn from(p: Point) -> Self {
        crate::simulation::types::Point { x: p.x, y: p.y }
    }
}

impl From<&Point> for crate::simulation::types::Point {
    fn from(p: &Point) -> Self {
        crate::simulation::types::Point { x: p.x, y: p.y }
    }
}

impl From<RectPos> for crate::simulation::types::RectPos {
    fn from(r: RectPos) -> Self {
        crate::simulation::types::RectPos {
            top_left: r.top_left.into(),
            bottom_right: r.bottom_right.into(),
        }
    }
}

impl From<CirclePos> for crate::simulation::types::CirclePos {
    fn from(c: CirclePos) -> Self {
        crate::simulation::types::CirclePos {
            center: c.center.into(),
            radius: c.radius,
        }
    }
}

impl From<Obstacle> for crate::simulation::types::Obstacle {
    fn from(o: Obstacle) -> Self {
        match o {
            Obstacle::Rectangle { position } => crate::simulation::types::Obstacle::Rectangle {
                position: position.into(),
            },
            Obstacle::Circle { position } => crate::simulation::types::Obstacle::Circle {
                position: position.into(),
            },
        }
    }
}

impl From<&Obstacle> for crate::simulation::types::Obstacle {
    fn from(o: &Obstacle) -> Self {
        match o {
            Obstacle::Rectangle { position } => crate::simulation::types::Obstacle::Rectangle {
                position: crate::simulation::types::RectPos {
                    top_left: (&position.top_left).into(),
                    bottom_right: (&position.bottom_right).into(),
                },
            },
            Obstacle::Circle { position } => crate::simulation::types::Obstacle::Circle {
                position: crate::simulation::types::CirclePos {
                    center: (&position.center).into(),
                    radius: position.radius,
                },
            },
        }
    }
}
