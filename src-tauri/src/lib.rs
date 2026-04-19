mod commands;
mod logging;
mod monitor;

use anyhow::Result;
use shared::MonitorSnapshot;

pub fn run() {
    logging::init_tracing();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::list_monitors,
            commands::set_monitor_feature,
            commands::transition_monitor_feature,
            commands::apply_color_scene,
            commands::quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running WarmLite");
}

pub fn list_monitors_blocking() -> Result<Vec<MonitorSnapshot>> {
    monitor::list_monitors()
}

pub fn set_monitor_feature_blocking(
    monitor_id: &str,
    code: &str,
    value: u16,
) -> Result<MonitorSnapshot> {
    monitor::set_monitor_feature(monitor_id, code, value)
}

pub fn transition_monitor_feature_blocking(
    monitor_id: &str,
    code: &str,
    value: u16,
    step_delay_ms: u64,
) -> Result<MonitorSnapshot> {
    monitor::transition_monitor_feature(monitor_id, code, value, step_delay_ms)
}

pub fn apply_color_scene_blocking(monitor_id: &str, scene_id: &str) -> Result<MonitorSnapshot> {
    monitor::apply_color_scene(monitor_id, scene_id)
}
