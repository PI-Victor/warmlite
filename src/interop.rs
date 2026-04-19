use serde::Serialize;
use serde_wasm_bindgen::{from_value, to_value};
use shared::MonitorSnapshot;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    async fn invoke_with_args(command: &str, args: JsValue) -> JsValue;
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetFeatureArgs<'a> {
    monitor_id: &'a str,
    code: &'a str,
    value: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransitionFeatureArgs<'a> {
    monitor_id: &'a str,
    code: &'a str,
    value: u16,
    step_delay_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyColorSceneArgs<'a> {
    monitor_id: &'a str,
    scene_id: &'a str,
}

pub async fn list_monitors() -> Result<Vec<MonitorSnapshot>, String> {
    invoke("list_monitors", ()).await
}

pub async fn set_feature(
    monitor_id: &str,
    code: &str,
    value: u16,
) -> Result<MonitorSnapshot, String> {
    invoke(
        "set_monitor_feature",
        SetFeatureArgs {
            monitor_id,
            code,
            value,
        },
    )
    .await
}

pub async fn transition_feature(
    monitor_id: &str,
    code: &str,
    value: u16,
    step_delay_ms: u64,
) -> Result<MonitorSnapshot, String> {
    invoke(
        "transition_monitor_feature",
        TransitionFeatureArgs {
            monitor_id,
            code,
            value,
            step_delay_ms,
        },
    )
    .await
}

pub async fn apply_color_scene(
    monitor_id: &str,
    scene_id: &str,
) -> Result<MonitorSnapshot, String> {
    invoke(
        "apply_color_scene",
        ApplyColorSceneArgs {
            monitor_id,
            scene_id,
        },
    )
    .await
}

pub async fn quit_app() -> Result<(), String> {
    invoke::<(), _>("quit_app", ()).await
}

async fn invoke<T, A>(command: &str, args: A) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
    A: Serialize,
{
    let args =
        to_value(&args).map_err(|error| format!("Failed to serialize arguments: {error}"))?;
    let response = invoke_with_args(command, args).await;
    from_value(response).map_err(|error| format!("Failed to deserialize Tauri response: {error}"))
}
