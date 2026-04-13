use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    pub id: String,
    pub backend: String,
    pub device_path: Option<String>,
    pub connector_name: Option<String>,
    pub manufacturer_id: Option<String>,
    pub model_name: Option<String>,
    pub serial_number: Option<String>,
    pub controls: Vec<MonitorControl>,
    pub error: Option<String>,
}

impl MonitorSnapshot {
    pub fn label(&self) -> String {
        self.model_name
            .clone()
            .or_else(|| self.manufacturer_id.clone())
            .unwrap_or_else(|| String::from("Unknown display"))
    }

    pub fn supports_controls(&self) -> bool {
        self.controls.iter().any(|control| control.supported)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MonitorControl {
    pub code: String,
    pub label: String,
    pub control_type: MonitorControlType,
    pub current_value: Option<u16>,
    pub max_value: Option<u16>,
    pub options: Vec<ControlOption>,
    pub supported: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MonitorControlType {
    Range,
    Choice,
    Toggle,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlOption {
    pub value: u16,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugLogEntry {
    pub timestamp: String,
    pub scope: String,
    pub message: String,
}
