use shared::MonitorSnapshot;

#[tauri::command]
pub async fn list_monitors() -> Result<Vec<MonitorSnapshot>, String> {
    tauri::async_runtime::spawn_blocking(crate::monitor::list_monitors)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_monitor_feature(
    monitor_id: String,
    code: String,
    value: u16,
) -> Result<MonitorSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::monitor::set_monitor_feature(&monitor_id, &code, value)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn transition_monitor_feature(
    monitor_id: String,
    code: String,
    value: u16,
    step_delay_ms: u64,
) -> Result<MonitorSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::monitor::transition_monitor_feature(&monitor_id, &code, value, step_delay_ms)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn apply_color_scene(
    monitor_id: String,
    scene_id: String,
) -> Result<MonitorSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::monitor::apply_color_scene(&monitor_id, &scene_id)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}
