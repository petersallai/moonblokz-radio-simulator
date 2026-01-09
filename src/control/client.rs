//! HTTP client for communicating with the Telemetry Hub.

use super::{ControlCommand, ControlConfig};
use reqwest::blocking::Client;
use std::time::Duration;

/// Client for sending commands to the Telemetry Hub.
pub struct TelemetryClient {
    client: Client,
    config: ControlConfig,
}

impl TelemetryClient {
    /// Create a new TelemetryClient with the given configuration.
    pub fn new(config: ControlConfig) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self { client, config })
    }

    /// Send a command to the Telemetry Hub.
    ///
    /// # Returns
    /// * `Ok(())` if the command was accepted (HTTP 200)
    /// * `Err(String)` with error details otherwise
    pub fn send_command(&self, command: &ControlCommand) -> Result<(), String> {
        let url = format!("{}/command", self.config.hub_url);
        let payload = command.to_payload();

        log::info!("Sending command to {}: {:?}", url, payload);

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Api-Key", &self.config.api_key)
            .json(&payload)
            .send()
            .map_err(|e| format!("Network error: {}", e))?;

        let status = response.status();
        if status.is_success() {
            log::info!("Command sent successfully");
            Ok(())
        } else if status.is_client_error() {
            let body = response.text().unwrap_or_default();
            if status.as_u16() == 401 {
                Err("Authentication failed. Check API key in config.toml".to_string())
            } else {
                Err(format!("Invalid command ({}): {}", status.as_u16(), body))
            }
        } else {
            let body = response.text().unwrap_or_default();
            Err(format!("Server error ({}): {}", status.as_u16(), body))
        }
    }
}
