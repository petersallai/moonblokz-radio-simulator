//! Command type definitions for the control module.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Log level values matching the Telemetry CLI specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "TRACE"),
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

/// Commands that can be sent to the Telemetry Hub.
#[derive(Debug, Clone)]
pub enum ControlCommand {
    /// Set the update interval with active/inactive scheduling.
    /// Targets all nodes if node_id is None.
    SetUpdateInterval {
        node_id: Option<u32>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        active_period: u32,
        inactive_period: u32,
    },

    /// Set the log level for a node or all nodes.
    SetLogLevel { node_id: Option<u32>, log_level: LogLevel },

    /// Set the log filter for a node or all nodes.
    SetLogFilter { node_id: Option<u32>, log_filter: String },

    /// Send an arbitrary command to a node or all nodes.
    RunCommand { node_id: Option<u32>, command: String },

    /// Start a measurement on a specific node.
    StartMeasurement { node_id: u32, sequence: u32 },
}

/// JSON payload structure for the Telemetry Hub /command endpoint.
#[derive(Debug, Serialize)]
pub struct CommandPayload {
    pub command: String,
    pub parameters: serde_json::Value,
}

impl ControlCommand {
    /// Convert the command to a JSON payload for the Telemetry Hub.
    pub fn to_payload(&self) -> CommandPayload {
        match self {
            ControlCommand::SetUpdateInterval {
                node_id,
                start_time,
                end_time,
                active_period,
                inactive_period,
            } => {
                let mut params = serde_json::json!({
                    "start_time": start_time.to_rfc3339(),
                    "end_time": end_time.to_rfc3339(),
                    "active_period": active_period,
                    "inactive_period": inactive_period,
                });
                if let Some(id) = node_id {
                    params["node_id"] = serde_json::json!(id);
                }
                CommandPayload {
                    command: "set_update_interval".to_string(),
                    parameters: params,
                }
            }

            ControlCommand::SetLogLevel { node_id, log_level } => {
                let mut params = serde_json::json!({
                    "log_level": log_level.to_string(),
                });
                if let Some(id) = node_id {
                    params["node_id"] = serde_json::json!(id);
                }
                CommandPayload {
                    command: "set_log_level".to_string(),
                    parameters: params,
                }
            }

            ControlCommand::SetLogFilter { node_id, log_filter } => {
                let mut params = serde_json::json!({
                    "log_filter": log_filter,
                });
                if let Some(id) = node_id {
                    params["node_id"] = serde_json::json!(id);
                }
                CommandPayload {
                    command: "set_log_filter".to_string(),
                    parameters: params,
                }
            }

            ControlCommand::RunCommand { node_id, command } => {
                let mut params = serde_json::json!({
                    "command": command,
                });
                if let Some(id) = node_id {
                    params["node_id"] = serde_json::json!(id);
                }
                CommandPayload {
                    command: "command".to_string(),
                    parameters: params,
                }
            }

            ControlCommand::StartMeasurement { node_id, sequence } => CommandPayload {
                command: "start_measurement".to_string(),
                parameters: serde_json::json!({
                    "node_id": node_id,
                    "sequence": sequence,
                }),
            },
        }
    }
}
