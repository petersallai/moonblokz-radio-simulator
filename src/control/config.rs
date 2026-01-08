//! Configuration loading for the control module.

use serde::Deserialize;
use std::path::Path;

/// Configuration for connecting to the Telemetry Hub.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
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
        let content = std::fs::read_to_string(config_path).map_err(|e| format!("Failed to read config file: {}", e))?;

        toml::from_str(&content).map_err(|e| format!("Failed to parse config file: {}", e))
    }

    /// Derive the config path from a scene file path.
    ///
    /// Replaces the scene filename with "config.toml" in the same directory.
    pub fn config_path_from_scene(scene_path: &str) -> std::path::PathBuf {
        let scene = Path::new(scene_path);
        scene.parent().unwrap_or(Path::new(".")).join("config.toml")
    }
}
