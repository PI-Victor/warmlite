use leptos::ev::Event;
use leptos::prelude::*;
use shared::{ControlOption, MonitorControl, MonitorSnapshot};
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlDetailsElement, HtmlInputElement, window};

use crate::interop;

#[derive(Clone)]
struct RangeChangeContext {
    monitor_id: String,
    control_code: String,
    delay: u64,
    slider_value: RwSignal<u16>,
    pending_value: RwSignal<Option<u16>>,
    in_flight: RwSignal<bool>,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    local_error: RwSignal<String>,
    control_label: String,
}

#[derive(Clone, Copy)]
struct ColorSceneOption {
    id: &'static str,
    label: &'static str,
}

#[derive(Clone, Copy)]
struct SurfaceFoldState {
    manual_gains: RwSignal<bool>,
    warm_scene: RwSignal<bool>,
    input_source: RwSignal<bool>,
    display_mode: RwSignal<bool>,
    audio: RwSignal<bool>,
    osd: RwSignal<bool>,
    restore: RwSignal<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThemeMode {
    System,
    Dark,
    Light,
}

impl ThemeMode {
    fn next(self) -> Self {
        match self {
            Self::System => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::System,
        }
    }

    fn attr(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::System => "Theme: System",
            Self::Dark => "Theme: Dark",
            Self::Light => "Theme: Light",
        }
    }
}

const COLOR_SCENES: &[ColorSceneOption] = &[
    ColorSceneOption {
        id: "paper",
        label: "Paper",
    },
    ColorSceneOption {
        id: "sunset",
        label: "Sunset",
    },
    ColorSceneOption {
        id: "ember",
        label: "Ember",
    },
    ColorSceneOption {
        id: "incandescent",
        label: "Incandescent",
    },
    ColorSceneOption {
        id: "candle",
        label: "Candle",
    },
    ColorSceneOption {
        id: "nocturne",
        label: "Nocturne",
    },
];

#[component]
pub fn App() -> impl IntoView {
    let monitors = RwSignal::new(Vec::<MonitorSnapshot>::new());
    let selected_monitor_id = RwSignal::new(None::<String>);
    let status = RwSignal::new(String::from("Scanning displays..."));
    let is_loading = RwSignal::new(false);
    let glide_delay_ms = RwSignal::new(18_u16);
    let theme_mode = RwSignal::new(ThemeMode::System);
    let surface_folds = SurfaceFoldState {
        manual_gains: RwSignal::new(false),
        warm_scene: RwSignal::new(false),
        input_source: RwSignal::new(false),
        display_mode: RwSignal::new(false),
        audio: RwSignal::new(false),
        osd: RwSignal::new(false),
        restore: RwSignal::new(false),
    };

    let refresh = move || {
        is_loading.set(true);
        status.set(String::from("Refreshing monitor state..."));

        spawn_local(async move {
            match interop::list_monitors().await {
                Ok(next) => {
                    let count = next.len();
                    let current_selection = selected_monitor_id.get();
                    let next_selection = current_selection
                        .filter(|selected| next.iter().any(|monitor| &monitor.id == selected))
                        .or_else(|| next.first().map(|monitor| monitor.id.clone()));

                    monitors.set(next);
                    selected_monitor_id.set(next_selection);
                    status.set(match count {
                        0 => String::from("No displays responded to DDC."),
                        1 => String::from("1 display ready."),
                        _ => format!("{count} displays ready."),
                    });
                }
                Err(error) => status.set(error),
            }

            is_loading.set(false);
        });
    };

    let selected_monitor = move || {
        let selection = selected_monitor_id.get();
        monitors
            .get()
            .into_iter()
            .find(|monitor| Some(monitor.id.clone()) == selection)
    };

    Effect::new(move |_| {
        refresh();
    });

    Effect::new(move |_| {
        if let Some(window) = window()
            && let Some(document) = window.document()
            && let Some(root) = document.document_element()
        {
            let _ = root.set_attribute("data-theme", theme_mode.get().attr());
        }
    });

    view! {
        <main class="shell scene-shell">
            <header class="scene-topbar">
                <div class="scene-brand">
                    <div class="brand-copy">
                        <p class="eyebrow">"WarmLite"</p>
                        <strong>"Display Studio"</strong>
                    </div>
                </div>

                <div class="topbar-tools">
                    <button
                        class="button ghost toolbar-button"
                        type="button"
                        on:click=move |_| theme_mode.update(|mode| *mode = mode.next())
                    >
                        {move || theme_mode.get().label()}
                    </button>

                    <div class="glide-inline">
                        <span class="panel-label">"Glide"</span>
                        <input
                            class="slider slim toolbar-slider"
                            type="range"
                            min="0"
                            max="50"
                            step="2"
                            prop:value=move || glide_delay_ms.get().to_string()
                            style=move || format!(
                                "--slider-fill: {}%;",
                                ((glide_delay_ms.get() as f32 / 50.0) * 100.0).round() as u16
                            )
                            on:input=move |event: Event| {
                                let input = event_target::<HtmlInputElement>(&event);
                                if let Ok(parsed) = input.value().parse::<u16>() {
                                    glide_delay_ms.set(parsed);
                                }
                            }
                        />
                        <span class="glide-inline-value">
                            {move || glide_compact_label(glide_delay_ms.get())}
                        </span>
                    </div>

                    <button
                        class="button ghost toolbar-button"
                        on:click=move |_| refresh()
                        disabled=move || is_loading.get()
                    >
                        {move || if is_loading.get() { "Loading" } else { "Refresh" }}
                    </button>
                </div>
            </header>

            <div class=move || format!("status-pill {}", if status.get().starts_with("Failed") { "error" } else { "" })>
                {move || status.get()}
            </div>

            <section class="display-switcher-bar">
                <div class="display-switcher">
                    <For
                        each=move || monitors.get()
                        key=|monitor| monitor.id.clone()
                        children=move |monitor| {
                            let monitor_id = monitor.id.clone();
                            let monitor_id_for_class = monitor_id.clone();
                            let monitor_id_for_click = monitor_id.clone();
                            let monitor_name = monitor.label();
                            let meta = display_switch_meta(&monitor);

                            view! {
                                <button
                                    class=move || {
                                        if selected_monitor_id.get().as_deref() == Some(monitor_id_for_class.as_str()) {
                                            "display-switch active"
                                        } else {
                                            "display-switch"
                                        }
                                    }
                                    on:click=move |_| selected_monitor_id.set(Some(monitor_id_for_click.clone()))
                                >
                                    <span class="display-switch-name">{monitor_name}</span>
                                    <span class="display-switch-meta">{meta}</span>
                                </button>
                            }
                        }
                    />
                </div>
            </section>

            <section class="scene-stage">
                {move || {
                    if let Some(monitor) = selected_monitor() {
                        view! {
                            <MonitorStage
                                monitor
                                monitors
                                glide_delay_ms
                                surface_folds
                            />
                        }
                        .into_any()
                    } else {
                        view! {
                            <div class="empty-scene">
                                <div class="empty-display-search" aria-hidden="true">
                                    <div class="empty-display">
                                        <div class="empty-display-frame">
                                            <span class="empty-display-glow"></span>
                                            <span class="empty-display-scan"></span>
                                        </div>
                                        <div class="empty-display-neck"></div>
                                        <div class="empty-display-base"></div>
                                    </div>
                                </div>
                                <h2>"Waiting for a monitor"</h2>
                                <p>"Refresh after connecting a DDC-capable display."</p>
                            </div>
                        }
                        .into_any()
                    }
                }}
            </section>

        </main>
    }
}

#[component]
fn MonitorStage(
    monitor: MonitorSnapshot,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    glide_delay_ms: RwSignal<u16>,
    surface_folds: SurfaceFoldState,
) -> impl IntoView {
    let local_error = RwSignal::new(String::new());
    let is_busy = RwSignal::new(false);
    let identity_label = monitor_identity_label(&monitor);
    let monitor_error = monitor.error.clone();
    let monitor_error_when = monitor_error.clone();
    let title = monitor.label();
    let subtitle = monitor_subtitle(&monitor);
    let brightness_control = find_control(&monitor.controls, "10");
    let contrast_control = find_control(&monitor.controls, "12");
    let brightness_meter = brightness_control.clone();
    let contrast_meter = contrast_control.clone();
    let brightness_meter_when = brightness_control.clone();
    let contrast_meter_when = contrast_control.clone();
    let brightness_center = brightness_control.clone();
    let contrast_center = contrast_control.clone();
    let brightness_center_when = brightness_control.clone();
    let contrast_center_when = contrast_control.clone();
    let has_color_scenes = find_control(&monitor.controls, "16").is_some()
        && find_control(&monitor.controls, "18").is_some()
        && find_control(&monitor.controls, "1A").is_some();
    let mute_control = find_control(&monitor.controls, "8D");
    let power_control = find_control(&monitor.controls, "D6");
    let volume_control = find_control(&monitor.controls, "62");
    let input_source_control = find_control(&monitor.controls, "60");
    let display_mode_control = find_control(&monitor.controls, "DC");
    let osd_controls: Vec<_> = monitor
        .controls
        .iter()
        .filter(|control| matches!(control.code.as_str(), "02" | "CA" | "CC"))
        .cloned()
        .collect();
    let action_controls: Vec<_> = monitor
        .controls
        .iter()
        .filter(|control| matches!(control.code.as_str(), "04" | "05" | "08" | "B0"))
        .cloned()
        .collect();
    let mute_control_for_view = StoredValue::new(mute_control.clone());
    let power_control_for_view = StoredValue::new(power_control.clone());
    let volume_control_for_view = StoredValue::new(volume_control.clone());
    let input_source_control_for_view = StoredValue::new(input_source_control.clone());
    let display_mode_control_for_view = StoredValue::new(display_mode_control.clone());
    let osd_controls_for_view = StoredValue::new(osd_controls.clone());
    let action_controls_for_view = StoredValue::new(action_controls.clone());
    let gain_controls: Vec<_> = monitor
        .controls
        .iter()
        .filter(|control| matches!(control.code.as_str(), "16" | "18" | "1A"))
        .cloned()
        .collect();
    let gain_controls_for_each = StoredValue::new(gain_controls.clone());
    let preset_control = find_control(&monitor.controls, "14");
    let has_preset_control = preset_control.is_some();
    let has_audio_controls = mute_control.is_some() || volume_control.is_some();
    let supports_text = if monitor.supports_controls() {
        "ready"
    } else {
        "limited"
    };
    let monitor_id_center = monitor.id.clone();
    let monitor_id_center_contrast = monitor.id.clone();
    let monitor_id_audio = monitor.id.clone();
    let monitor_id_audio_for_view = StoredValue::new(monitor_id_audio.clone());
    let monitor_id_surface_controls = StoredValue::new(monitor.id.clone());
    let monitor_id_preset_row = monitor.id.clone();
    let monitor_id_warm_scene = monitor.id.clone();
    let monitor_id_power = monitor.id.clone();
    let monitor_id_power_for_view = StoredValue::new(monitor_id_power.clone());
    let monitor_vendor = monitor
        .manufacturer_id
        .clone()
        .unwrap_or_else(|| String::from("Unknown"));
    let readable_control_count = monitor
        .controls
        .iter()
        .filter(|control| control.supported)
        .count();

    view! {
        <div class="studio-plane">
            <header class="studio-header">
                <div class="studio-heading">
                    <p class="screen-route">{identity_label}</p>
                    <h2 class="screen-title">{title.clone()}</h2>
                    <p class="screen-subtitle">{subtitle.clone()}</p>
                </div>

                <div class="screen-badges">
                    <span class="badge">{supports_text}</span>
                    <span class="badge soft">{move || glide_label(glide_delay_ms.get())}</span>
                </div>
            </header>

            <section class="studio-layout">
                <aside class="inspector-column">
                    <div class="inspector-block compact side-read-panel">
                        <div class="side-read-row">
                            <span class="panel-label">"Backend"</span>
                            <strong class="side-read-value">{monitor.backend.clone()}</strong>
                        </div>
                        <div class="side-read-row">
                            <span class="panel-label">"Vendor"</span>
                            <strong class="side-read-value">{monitor_vendor}</strong>
                        </div>
                        <div class="side-read-row">
                            <span class="panel-label">"Access"</span>
                            <strong class="side-read-value">{supports_text}</strong>
                        </div>
                        <div class="side-read-row">
                            <span class="panel-label">"Controls"</span>
                            <strong class="side-read-value">{readable_control_count}</strong>
                        </div>
                    </div>
                </aside>

                <section class="display-plane">
                    <div class="display-frame">
                        <div class="frame-head">
                            <div class="frame-copy">
                                <p class="monitor-kicker">"Signal surface"</p>
                                <p class="monitor-meta">
                                    "Current source, preset, and live DDC state for the selected display."
                                </p>
                            </div>

                            <Show when=move || power_control_for_view.get_value().is_some()>
                                <PowerModeSlider
                                    monitor_id=monitor_id_power_for_view.get_value()
                                    monitors
                                    control=power_control_for_view.get_value().unwrap()
                                    is_busy
                                    local_error
                                />
                            </Show>
                        </div>

                        <div class="signal-rails">
                            <Show when=move || brightness_center_when.is_some()>
                                <CenterRangeControl
                                    monitor_id=monitor_id_center.clone()
                                    monitors
                                    control=brightness_center.clone().unwrap()
                                    glide_delay_ms
                                    local_error
                                    tone="warm"
                                />
                            </Show>
                            <Show when=move || contrast_center_when.is_some()>
                                <CenterRangeControl
                                    monitor_id=monitor_id_center_contrast.clone()
                                    monitors
                                    control=contrast_center.clone().unwrap()
                                    glide_delay_ms
                                    local_error
                                    tone="cool"
                                />
                            </Show>
                        </div>

                        <div class="surface-controls">
                            <Show when=move || has_preset_control || has_color_scenes>
                                <div class="surface-inline">
                                    <PresetSceneControl
                                        monitor_id=monitor_id_preset_row.clone()
                                        monitors
                                        preset_control=preset_control.clone()
                                        is_busy
                                        local_error
                                    />
                                </div>
                            </Show>

                            <Show when=move || has_color_scenes>
                                <div class="surface-inline">
                                    <WarmSceneControl
                                        monitor_id=monitor_id_warm_scene.clone()
                                        monitors
                                        is_busy
                                        local_error
                                        is_open=surface_folds.warm_scene
                                    />
                                </div>
                            </Show>

                            <Show when=move || !gain_controls.is_empty()>
                                <div class="surface-inline surface-gains-inline">
                                    <details
                                        class="surface-fold"
                                        prop:open=move || surface_folds.manual_gains.get()
                                        on:toggle=move |event: Event| {
                                            surface_folds
                                                .manual_gains
                                                .set(event_target::<HtmlDetailsElement>(&event).open());
                                        }
                                    >
                                        <summary class="surface-fold-summary">
                                            <div>
                                                <p class="panel-label">"Manual Gains"</p>
                                                <strong class="panel-value surface-fold-value">
                                                    "Red, green, and blue channel trims"
                                                </strong>
                                            </div>
                                        </summary>

                                        <div class="surface-fold-body surface-gain-grid">
                                            <For
                                                each=move || gain_controls_for_each.get_value()
                                                key=|control| control.code.clone()
                                                children=move |control| {
                                                    view! {
                                                        <RangeControl
                                                            monitor_id=monitor_id_surface_controls.get_value()
                                                            monitors
                                                            control
                                                            glide_delay_ms
                                                            is_busy
                                                            local_error
                                                            variant="inline"
                                                        />
                                                    }
                                                }
                                            />
                                        </div>
                                    </details>
                                </div>
                            </Show>

                            <Show
                                when=move || {
                                    input_source_control_for_view.get_value().is_some()
                                        || display_mode_control_for_view.get_value().is_some()
                                        || has_audio_controls
                                }
                            >
                                <div class="surface-inline surface-secondary-controls">
                                    <div class="surface-secondary-grid">
                                        <Show when=move || input_source_control_for_view.get_value().is_some()>
                                            <div class="surface-inline surface-inline-narrow">
                                                <ChoiceControl
                                                    monitor_id=monitor_id_surface_controls.get_value()
                                                    monitors
                                                    control=input_source_control_for_view.get_value().unwrap()
                                                    is_busy
                                                    local_error
                                                    variant="surface"
                                                    open_state=Some(surface_folds.input_source)
                                                />
                                            </div>
                                        </Show>

                                        <Show when=move || display_mode_control_for_view.get_value().is_some()>
                                            <div class="surface-inline surface-inline-narrow">
                                                <ChoiceControl
                                                    monitor_id=monitor_id_surface_controls.get_value()
                                                    monitors
                                                    control=display_mode_control_for_view.get_value().unwrap()
                                                    is_busy
                                                    local_error
                                                    variant="surface"
                                                    open_state=Some(surface_folds.display_mode)
                                                />
                                            </div>
                                        </Show>

                                        <Show when=move || has_audio_controls>
                                            <div class="surface-inline surface-inline-narrow">
                                                <AudioControlSection
                                                    monitor_id=monitor_id_audio_for_view.get_value()
                                                    monitors
                                                    mute_control=mute_control_for_view.get_value()
                                                    volume_control=volume_control_for_view.get_value()
                                                    glide_delay_ms
                                                    is_busy
                                                    local_error
                                                    is_open=surface_folds.audio
                                                />
                                            </div>
                                        </Show>
                                    </div>
                                </div>
                            </Show>

                            <Show when=move || !osd_controls_for_view.get_value().is_empty()>
                                <div class="surface-inline surface-secondary-controls">
                                    <OsdControlSection
                                        monitor_id=monitor_id_surface_controls.get_value()
                                        monitors
                                        controls=osd_controls_for_view.get_value()
                                        is_busy
                                        local_error
                                        is_open=surface_folds.osd
                                    />
                                </div>
                            </Show>

                            <Show when=move || !action_controls_for_view.get_value().is_empty()>
                                <div class="surface-inline surface-secondary-controls">
                                    <ActionControlSection
                                        monitor_id=monitor_id_surface_controls.get_value()
                                        monitors
                                        controls=action_controls_for_view.get_value()
                                        is_busy
                                        local_error
                                        is_open=surface_folds.restore
                                    />
                                </div>
                            </Show>
                        </div>

                    </div>
                </section>

                <aside class="meter-stack">
                    <Show
                        when=move || brightness_meter_when.is_some()
                        fallback=|| view! { <div class="meter-placeholder"></div> }
                    >
                        <PrimaryRangeControl
                            control=brightness_meter.clone().unwrap()
                        />
                    </Show>

                    <Show
                        when=move || contrast_meter_when.is_some()
                        fallback=|| view! { <div class="meter-placeholder"></div> }
                    >
                        <PrimaryRangeControl
                            control=contrast_meter.clone().unwrap()
                        />
                    </Show>
                </aside>
            </section>

            <footer class="screen-footer">
                <Show when=move || !local_error.get().is_empty()>
                    <p class="support-note warning">{move || local_error.get()}</p>
                </Show>

                <Show when=move || monitor_error_when.is_some()>
                    <p class="support-note warning">
                        {monitor_error.clone().unwrap_or_default()}
                    </p>
                </Show>
            </footer>
        </div>
    }
}

#[component]
fn PresetSceneControl(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    preset_control: Option<MonitorControl>,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
) -> impl IntoView {
    let monitor_id_for_presets = StoredValue::new(monitor_id.clone());
    let preset_selected_value = RwSignal::new(
        preset_control
            .as_ref()
            .and_then(|control| control.current_value)
            .unwrap_or_default(),
    );
    let preset_options = preset_control
        .as_ref()
        .map(|control| control.options.clone())
        .unwrap_or_default();
    let preset_options_for_value = StoredValue::new(preset_options.clone());
    let preset_options_for_each = StoredValue::new(preset_options.clone());
    let has_preset_control = preset_control.is_some();
    let preset_supported = preset_control
        .as_ref()
        .map(|control| control.supported)
        .unwrap_or(false);
    let preset_has_options = !preset_options.is_empty();
    let preset_error = preset_control
        .as_ref()
        .and_then(|control| control.error.clone());
    let preset_label = preset_control
        .as_ref()
        .map(|control| control.label.clone())
        .unwrap_or_else(|| String::from("Color Preset"));
    let preset_unavailable_message = preset_error
        .clone()
        .unwrap_or_else(|| format!("{preset_label} is unavailable."));
    let preset_no_values_message = format!("{preset_label} does not expose selectable values.");
    let preset_label_for_action = StoredValue::new(preset_label.clone());
    let preset_code = preset_control
        .as_ref()
        .map(|control| control.code.clone())
        .unwrap_or_else(|| String::from("14"));
    let preset_code_for_action = StoredValue::new(preset_code);
    let preset_unavailable_message_for_view = StoredValue::new(preset_unavailable_message);
    let preset_no_values_message_for_view = StoredValue::new(preset_no_values_message);

    view! {
        <section class="preset-stack">
            <Show when=move || has_preset_control>
                <div class="preset-section">
                    <div class="flat-section-head">
                        <div>
                            <p class="panel-label">"Color Preset"</p>
                            <strong class="panel-value">
                                {move || option_label(&preset_options_for_value.get_value(), preset_selected_value.get())}
                            </strong>
                        </div>
                    </div>

                    <div class="choice-strip preset-choice-strip" aria-label=preset_label.clone() role="group">
                        <For
                            each=move || preset_options_for_each.get_value()
                            key=|option| option.value
                            children=move |option| {
                                let option_value = option.value;
                                let option_label = option.label.clone();
                                let tone_class = preset_tone_class(&option_label).to_string();
                                let monitor_id = monitor_id_for_presets.get_value();
                                let control_code = preset_code_for_action.get_value();
                                let control_label = preset_label_for_action.get_value();

                                view! {
                                    <button
                                        class=move || {
                                            if preset_selected_value.get() == option_value {
                                                format!("choice-segment preset-segment {tone_class} active")
                                            } else {
                                                format!("choice-segment preset-segment {tone_class}")
                                            }
                                        }
                                        type="button"
                                        disabled=move || !preset_supported || is_busy.get() || !preset_has_options
                                        on:click=move |_| {
                                            if preset_selected_value.get_untracked() == option_value {
                                                return;
                                            }

                                            preset_selected_value.set(option_value);
                                            is_busy.set(true);
                                            local_error.set(String::new());

                                            let monitor_id = monitor_id.clone();
                                            let control_code = control_code.clone();
                                            let control_label = control_label.clone();

                                            spawn_local(async move {
                                                match interop::set_feature(&monitor_id, &control_code, option_value).await {
                                                    Ok(updated) => replace_monitor_snapshot(monitors, updated),
                                                    Err(error) => local_error.set(format!("{control_label}: {error}")),
                                                }

                                                is_busy.set(false);
                                            });
                                        }
                                    >
                                        <span class=format!("preset-swatch {tone_class}")></span>
                                        <span>{option_label}</span>
                                    </button>
                                }
                            }
                        />
                    </div>

                    <Show when=move || !preset_supported || !preset_has_options>
                        <p class="support-note warning">
                            {if !preset_supported {
                                preset_unavailable_message_for_view.get_value()
                            } else {
                                preset_no_values_message_for_view.get_value()
                            }}
                        </p>
                    </Show>
                </div>
            </Show>

        </section>
    }
}

#[component]
fn WarmSceneControl(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
    is_open: RwSignal<bool>,
) -> impl IntoView {
    let active_scene_id = RwSignal::new(None::<&'static str>);
    let monitor_id_for_scenes = StoredValue::new(monitor_id);

    view! {
        <details
            class="surface-fold"
            prop:open=move || is_open.get()
            on:toggle=move |event: Event| {
                is_open.set(event_target::<HtmlDetailsElement>(&event).open());
            }
        >
            <summary class="surface-fold-summary">
                <div>
                    <p class="panel-label">"Warm Scene"</p>
                    <strong class="panel-value surface-fold-value">
                        {move || {
                            active_scene_id
                                .get()
                                .and_then(color_scene_label)
                                .unwrap_or("Manual gains")
                        }}
                    </strong>
                </div>
            </summary>

            <div class="surface-fold-body">
                <div class="choice-strip preset-choice-strip" role="group" aria-label="Warm Scene">
                    <For
                        each=move || COLOR_SCENES.iter().copied()
                        key=|scene| scene.id
                        children=move |scene| {
                            let monitor_id = monitor_id_for_scenes.get_value();
                            let tone_class = warm_scene_tone_class(scene.id).to_string();

                            view! {
                                <button
                                    class=move || {
                                        if active_scene_id.get() == Some(scene.id) {
                                            format!("choice-segment preset-segment {tone_class} active")
                                        } else {
                                            format!("choice-segment preset-segment {tone_class}")
                                        }
                                    }
                                    type="button"
                                    disabled=move || is_busy.get()
                                    on:click=move |_| {
                                        is_busy.set(true);
                                        local_error.set(String::new());
                                        active_scene_id.set(Some(scene.id));

                                        let monitor_id = monitor_id.clone();
                                        spawn_local(async move {
                                            match interop::apply_color_scene(&monitor_id, scene.id).await {
                                                Ok(updated) => replace_monitor_snapshot(monitors, updated),
                                                Err(error) => local_error.set(format!("Custom Scene: {error}")),
                                            }

                                            is_busy.set(false);
                                        });
                                    }
                                >
                                    <span class=format!("preset-swatch {tone_class}")></span>
                                    <span>{scene.label}</span>
                                </button>
                            }
                        }
                    />
                </div>
            </div>
        </details>
    }
}

#[component]
fn AudioControlSection(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    mute_control: Option<MonitorControl>,
    volume_control: Option<MonitorControl>,
    glide_delay_ms: RwSignal<u16>,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
    is_open: RwSignal<bool>,
) -> impl IntoView {
    let has_mute_control = mute_control.is_some();
    let mute_options = mute_control
        .as_ref()
        .map(|control| control.options.clone())
        .unwrap_or_default();
    let mute_supported = mute_control
        .as_ref()
        .map(|control| control.supported && control.options.len() >= 2)
        .unwrap_or(false);
    let mute_selected_value = RwSignal::new(
        mute_control
            .as_ref()
            .and_then(|control| control.current_value)
            .unwrap_or_default(),
    );
    let mute_control_code = mute_control
        .as_ref()
        .map(|control| control.code.clone())
        .unwrap_or_else(|| String::from("8D"));
    let mute_control_label = mute_control
        .as_ref()
        .map(|control| control.label.clone())
        .unwrap_or_else(|| String::from("Audio Mute"));
    let mute_error = mute_control
        .as_ref()
        .and_then(|control| control.error.clone());
    let volume_summary = volume_control.as_ref().map(|control| {
        slider_display(control.current_value.unwrap_or_default(), control.max_value)
    });
    let mute_summary = mute_control
        .as_ref()
        .and_then(|control| control.current_value)
        .map(|value| option_label(&mute_options, value));
    let volume_control_for_view = StoredValue::new(volume_control.clone());
    let mute_options_for_each = StoredValue::new(mute_options.clone());
    let monitor_id_for_mute = StoredValue::new(monitor_id.clone());
    let mute_control_code_for_actions = StoredValue::new(mute_control_code.clone());
    let mute_control_label_for_actions = StoredValue::new(mute_control_label.clone());

    view! {
        <details
            class="surface-fold audio-section"
            prop:open=move || is_open.get()
            on:toggle=move |event: Event| {
                is_open.set(event_target::<HtmlDetailsElement>(&event).open());
            }
        >
            <summary class="surface-fold-summary">
                <div>
                    <p class="panel-label">"Audio"</p>
                    <strong class="panel-value surface-fold-value">
                        {move || {
                            if let Some(summary) = mute_summary.clone() {
                                summary
                            } else if let Some(summary) = volume_summary.clone() {
                                summary
                            } else {
                                String::from("Unavailable")
                            }
                        }}
                    </strong>
                </div>
            </summary>

            <div class="surface-fold-body">
                <Show when=move || !mute_options_for_each.get_value().is_empty()>
                    <div class="choice-strip preset-choice-strip" role="group" aria-label="Audio mute">
                        <For
                            each=move || mute_options_for_each.get_value()
                            key=|option| option.value
                            children=move |option| {
                                let option_value = option.value;
                                let option_label_text = option.label.clone();
                                let monitor_id = monitor_id_for_mute.get_value();
                                let control_code = mute_control_code_for_actions.get_value();
                                let control_label = mute_control_label_for_actions.get_value();

                                view! {
                                    <button
                                        class=move || {
                                            if mute_selected_value.get() == option_value {
                                                "choice-segment active"
                                            } else {
                                                "choice-segment"
                                            }
                                        }
                                        type="button"
                                        disabled=move || !mute_supported || is_busy.get()
                                        on:click=move |_| {
                                            if mute_selected_value.get_untracked() == option_value {
                                                return;
                                            }

                                            mute_selected_value.set(option_value);
                                            is_busy.set(true);
                                            local_error.set(String::new());

                                            let monitor_id = monitor_id.clone();
                                            let control_code = control_code.clone();
                                            let control_label = control_label.clone();

                                            spawn_local(async move {
                                                match interop::set_feature(&monitor_id, &control_code, option_value).await {
                                                    Ok(updated) => replace_monitor_snapshot(monitors, updated),
                                                    Err(error) => local_error.set(format!("{control_label}: {error}")),
                                                }

                                                is_busy.set(false);
                                            });
                                        }
                                    >
                                        {option_label_text}
                                    </button>
                                }
                            }
                        />
                    </div>
                </Show>

                <Show when=move || volume_control_for_view.get_value().is_some()>
                    <RangeControl
                        monitor_id=monitor_id.clone()
                        monitors
                        control=volume_control_for_view.get_value().unwrap()
                        glide_delay_ms
                        is_busy
                        local_error
                        variant="row"
                    />
                </Show>

                <Show when=move || has_mute_control && !mute_supported>
                    <p class="support-note warning">
                        {mute_error.clone().unwrap_or_else(|| format!("{mute_control_label} is unavailable."))}
                    </p>
                </Show>
            </div>
        </details>
    }
}

#[component]
fn OsdControlSection(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    controls: Vec<MonitorControl>,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
    is_open: RwSignal<bool>,
) -> impl IntoView {
    let controls_for_view = StoredValue::new(controls);

    view! {
        <details
            class="surface-fold"
            prop:open=move || is_open.get()
            on:toggle=move |event: Event| {
                is_open.set(event_target::<HtmlDetailsElement>(&event).open());
            }
        >
            <summary class="surface-fold-summary">
                <div>
                    <p class="panel-label">"OSD"</p>
                    <strong class="panel-value surface-fold-value">
                        "Buttons, language, and saved-state flags"
                    </strong>
                </div>
            </summary>

            <div class="surface-fold-body action-list surface-action-list">
                <For
                    each=move || controls_for_view.get_value()
                    key=|control| control.code.clone()
                    children=move |control| {
                        view! {
                            <SurfaceChoiceRow
                                monitor_id=monitor_id.clone()
                                monitors
                                control
                                is_busy
                                local_error
                            />
                        }
                    }
                />
            </div>
        </details>
    }
}

#[component]
fn SurfaceChoiceRow(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    control: MonitorControl,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
) -> impl IntoView {
    let selected_value = RwSignal::new(control.current_value.unwrap_or_default());
    let control_code = control.code.clone();
    let control_label = control.label.clone();
    let control_heading_label = control.label.clone();
    let control_supported = control.supported;
    let control_error = control.error.clone();
    let control_label_for_warning = control_label.clone();
    let options = control.options.clone();
    let options_for_label = StoredValue::new(options.clone());

    view! {
        <section class="action-row surface-choice-row">
            <div class="action-main">
                <div class="action-copy">
                    <p class="panel-label">{control_heading_label.clone()}</p>
                    <strong class="panel-value">
                        {move || {
                            if control_supported {
                                option_label(&options_for_label.get_value(), selected_value.get())
                            } else {
                                String::from("Unavailable")
                            }
                        }}
                    </strong>
                </div>
            </div>

            <div class="choice-strip preset-choice-strip" role="group" aria-label=control_heading_label.clone()>
                <For
                    each=move || options.clone()
                    key=|option| option.value
                    children=move |option| {
                        let option_value = option.value;
                        let option_label_text = option.label.clone();
                        let monitor_id = monitor_id.clone();
                        let control_code = control_code.clone();
                        let control_label = control_label.clone();

                        view! {
                            <button
                                class=move || {
                                    if selected_value.get() == option_value {
                                        "choice-segment active"
                                    } else {
                                        "choice-segment"
                                    }
                                }
                                type="button"
                                disabled=move || !control_supported || is_busy.get()
                                on:click=move |_| {
                                    if selected_value.get_untracked() == option_value {
                                        return;
                                    }

                                    selected_value.set(option_value);
                                    is_busy.set(true);
                                    local_error.set(String::new());

                                    let monitor_id = monitor_id.clone();
                                    let control_code = control_code.clone();
                                    let control_label = control_label.clone();

                                    spawn_local(async move {
                                        match interop::set_feature(&monitor_id, &control_code, option_value).await {
                                            Ok(updated) => replace_monitor_snapshot(monitors, updated),
                                            Err(error) => local_error.set(format!("{control_label}: {error}")),
                                        }

                                        is_busy.set(false);
                                    });
                                }
                            >
                                {option_label_text}
                            </button>
                        }
                    }
                />
            </div>

            <Show when=move || !control_supported>
                <p class="support-note warning">
                    {control_error
                        .clone()
                        .unwrap_or_else(|| format!("{control_label_for_warning} is unavailable."))}
                </p>
            </Show>
        </section>
    }
}

#[component]
fn ActionControlSection(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    controls: Vec<MonitorControl>,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
    is_open: RwSignal<bool>,
) -> impl IntoView {
    let primary_controls: Vec<_> = controls
        .iter()
        .filter(|control| matches!(control.code.as_str(), "04" | "05" | "08"))
        .cloned()
        .collect();
    let secondary_controls: Vec<_> = controls
        .iter()
        .filter(|control| !matches!(control.code.as_str(), "04" | "05" | "08"))
        .cloned()
        .collect();
    let primary_controls_for_view = StoredValue::new(primary_controls);
    let secondary_controls_for_view = StoredValue::new(secondary_controls);
    let monitor_id_for_primary = StoredValue::new(monitor_id.clone());
    let monitor_id_for_secondary = StoredValue::new(monitor_id);

    view! {
        <details
            class="surface-fold"
            prop:open=move || is_open.get()
            on:toggle=move |event: Event| {
                is_open.set(event_target::<HtmlDetailsElement>(&event).open());
            }
        >
            <summary class="surface-fold-summary">
                <div>
                    <p class="panel-label">"Restore & Store"</p>
                    <strong class="panel-value surface-fold-value">
                        "Reset defaults or store the current mode"
                    </strong>
                </div>
            </summary>

            <div class="surface-fold-body action-list surface-action-list">
                <Show when=move || !primary_controls_for_view.get_value().is_empty()>
                    <div class="action-row surface-action-strip">
                        <div class="choice-strip preset-choice-strip surface-action-inline" role="group" aria-label="Restore defaults">
                            <For
                                each=move || primary_controls_for_view.get_value()
                                key=|control| control.code.clone()
                                children=move |control| {
                                    view! {
                                        <SurfaceActionButton
                                            monitor_id=monitor_id_for_primary.get_value()
                                            monitors
                                            control
                                            is_busy
                                            local_error
                                        />
                                    }
                                }
                            />
                        </div>
                    </div>
                </Show>

                <For
                    each=move || secondary_controls_for_view.get_value()
                    key=|control| control.code.clone()
                    children=move |control| {
                        view! {
                            <SurfaceActionRow
                                monitor_id=monitor_id_for_secondary.get_value()
                                monitors
                                control
                                is_busy
                                local_error
                            />
                        }
                    }
                />
            </div>
        </details>
    }
}

#[component]
fn SurfaceActionButton(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    control: MonitorControl,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
) -> impl IntoView {
    let option = control.options.first().cloned();
    let label = control.label.clone();
    let label_for_warning = label.clone();
    let control_code = control.code.clone();
    let supported = control.supported && option.is_some();
    let error = control.error.clone();

    view! {
        <button
            class="choice-segment"
            type="button"
            disabled=move || !supported || is_busy.get()
            on:click=move |_| {
                let Some(option) = option.clone() else {
                    return;
                };

                is_busy.set(true);
                local_error.set(String::new());

                let monitor_id = monitor_id.clone();
                let control_code = control_code.clone();
                let label = label.clone();

                spawn_local(async move {
                    match interop::set_feature(&monitor_id, &control_code, option.value).await {
                        Ok(updated) => replace_monitor_snapshot(monitors, updated),
                        Err(error) => local_error.set(format!("{label}: {error}")),
                    }

                    is_busy.set(false);
                });
            }
        >
            {label.clone()}
        </button>

        <Show when=move || !supported>
            <p class="support-note warning">
                {error.clone().unwrap_or_else(|| format!("{label_for_warning} is unavailable."))}
            </p>
        </Show>
    }
}

#[component]
fn SurfaceActionRow(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    control: MonitorControl,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
) -> impl IntoView {
    let control_code = control.code.clone();
    let control_code_for_key = control.code.clone();
    let control_label = control.label.clone();
    let control_label_for_warning = control_label.clone();
    let options = control.options.clone();
    let supported = control.supported;
    let error = control.error.clone();

    view! {
        <section class="action-row surface-choice-row">
            <div class="action-main">
                <div class="action-copy">
                    <p class="panel-label">{control.label.clone()}</p>
                    <strong class="panel-value">
                        {if supported {
                            String::from("Available")
                        } else {
                            String::from("Unavailable")
                        }}
                    </strong>
                </div>
            </div>

            <div class="choice-strip preset-choice-strip" role="group" aria-label=control.label.clone()>
                <For
                    each=move || options.clone()
                    key=move |option| format!("{}-{}", control_code_for_key, option.value)
                    children=move |option| {
                        let option_value = option.value;
                        let option_label_text = option.label.clone();
                        let monitor_id = monitor_id.clone();
                        let control_code = control_code.clone();
                        let control_label = control_label.clone();

                        view! {
                            <button
                                class="choice-segment"
                                type="button"
                                disabled=move || !supported || is_busy.get()
                                on:click=move |_| {
                                    is_busy.set(true);
                                    local_error.set(String::new());

                                    let monitor_id = monitor_id.clone();
                                    let control_code = control_code.clone();
                                    let control_label = control_label.clone();

                                    spawn_local(async move {
                                        match interop::set_feature(&monitor_id, &control_code, option_value).await {
                                            Ok(updated) => replace_monitor_snapshot(monitors, updated),
                                            Err(error) => local_error.set(format!("{control_label}: {error}")),
                                        }

                                        is_busy.set(false);
                                    });
                                }
                            >
                                {option_label_text}
                            </button>
                        }
                    }
                />
            </div>

            <Show when=move || !supported>
                <p class="support-note warning">
                    {error
                        .clone()
                        .unwrap_or_else(|| format!("{control_label_for_warning} is unavailable."))}
                </p>
            </Show>
        </section>
    }
}

#[component]
fn PrimaryRangeControl(control: MonitorControl) -> impl IntoView {
    let max_value = control.max_value.unwrap_or(100);
    let current_value = control.current_value.unwrap_or_default();

    view! {
        <section class=move || {
            if control.supported {
                "meter-panel"
            } else {
                "meter-panel unsupported"
            }
        }>
            <div class="meter-head">
                <p class="meter-label">{control.label.clone()}</p>
                <strong class="meter-number">{current_value}</strong>
            </div>

            <div class="meter-legend">
                <span>
                    <em>"min"</em>
                    <strong>"0"</strong>
                </span>
                <span>
                    <em>"current"</em>
                    <strong>{current_value}</strong>
                </span>
                <span>
                    <em>"max"</em>
                    <strong>{max_value}</strong>
                </span>
            </div>

            <Show when=move || !control.supported>
                <p class="support-note warning">
                    {control.error.clone().unwrap_or_else(|| format!("{} is unavailable.", control.label))}
                </p>
            </Show>
        </section>
    }
}

#[component]
fn CenterRangeControl(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    control: MonitorControl,
    glide_delay_ms: RwSignal<u16>,
    local_error: RwSignal<String>,
    tone: &'static str,
) -> impl IntoView {
    let slider_value = RwSignal::new(control.current_value.unwrap_or_default());
    let range_change = RangeChangeContext {
        monitor_id: monitor_id.clone(),
        control_code: control.code.clone(),
        delay: glide_delay_ms.get_untracked() as u64,
        slider_value,
        pending_value: RwSignal::new(None::<u16>),
        in_flight: RwSignal::new(false),
        monitors,
        local_error,
        control_label: control.label.clone(),
    };
    let maximum = control.max_value.unwrap_or(100);

    let on_input = move |event: Event| {
        let input = event_target::<HtmlInputElement>(&event);
        if let Ok(parsed) = input.value().parse::<u16>() {
            slider_value.set(parsed);
        }
    };

    let on_change = move |event: Event| {
        let input = event_target::<HtmlInputElement>(&event);
        if let Ok(parsed) = input.value().parse::<u16>() {
            let mut next = range_change.clone();
            next.delay = glide_delay_ms.get() as u64;
            queue_range_change(next, parsed);
        }
    };

    view! {
        <div class="signal-rail-row interactive">
            <div class="signal-rail-head">
                <span>{control.label.clone()}</span>
                <strong>{move || slider_display(slider_value.get(), control.max_value)}</strong>
            </div>

            <div class=format!("signal-rail interactive {tone}")>
                <div
                    class=format!("signal-rail-fill {tone}")
                    style=move || format!(
                        "width: {}%;",
                        ((slider_value.get() as f32 / maximum.max(1) as f32) * 100.0).round() as u16
                    )
                ></div>
                <input
                    class="signal-slider"
                    type="range"
                    min="0"
                    max=maximum
                    prop:value=move || slider_value.get().to_string()
                    disabled=move || !control.supported
                    on:input=on_input
                    on:change=on_change
                />
            </div>

            <Show when=move || !control.supported>
                <p class="support-note warning">
                    {control.error.clone().unwrap_or_else(|| format!("{} is unavailable.", control.label))}
                </p>
            </Show>
        </div>
    }
}

#[component]
fn RangeControl(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    control: MonitorControl,
    glide_delay_ms: RwSignal<u16>,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
    variant: &'static str,
) -> impl IntoView {
    let _ = is_busy;
    let slider_value = RwSignal::new(control.current_value.unwrap_or_default());
    let range_change = RangeChangeContext {
        monitor_id: monitor_id.clone(),
        control_code: control.code.clone(),
        delay: glide_delay_ms.get_untracked() as u64,
        slider_value,
        pending_value: RwSignal::new(None::<u16>),
        in_flight: RwSignal::new(false),
        monitors,
        local_error,
        control_label: control.label.clone(),
    };

    let on_input = move |event: Event| {
        let input = event_target::<HtmlInputElement>(&event);
        if let Ok(parsed) = input.value().parse::<u16>() {
            slider_value.set(parsed);
        }
    };

    let on_change = move |event: Event| {
        let input = event_target::<HtmlInputElement>(&event);
        if let Ok(parsed) = input.value().parse::<u16>() {
            let mut next = range_change.clone();
            next.delay = glide_delay_ms.get() as u64;
            queue_range_change(next, parsed);
        }
    };

    if variant == "inline" {
        view! {
            <section class="gain-inline-card">
                <div class="gain-inline-head">
                    <p class="panel-label">{control.label.clone()}</p>
                    <span class="panel-tag">
                        {move || if glide_delay_ms.get() == 0 { "Instant" } else { "Ramped" }}
                    </span>
                </div>

                <strong class="panel-value gain-inline-value">
                    {move || slider_display(slider_value.get(), control.max_value)}
                </strong>

                <input
                    class="slider action-slider"
                    type="range"
                    min="0"
                    max=control.max_value.unwrap_or(100)
                    prop:value=move || slider_value.get().to_string()
                    style=move || format!(
                        "--slider-fill: {}%;",
                        control
                            .max_value
                            .map(|maximum| {
                                ((slider_value.get() as f32 / maximum.max(1) as f32) * 100.0).round()
                                    as u16
                            })
                            .unwrap_or(0)
                    )
                    disabled=move || !control.supported
                    on:input=on_input
                    on:change=on_change
                />

                <Show when=move || !control.supported>
                    <p class="support-note warning">
                        {control.error.clone().unwrap_or_else(|| format!("{} is unavailable.", control.label))}
                    </p>
                </Show>
            </section>
        }
            .into_any()
    } else if variant == "row" {
        view! {
            <section class="action-row range-row">
                <div class="action-main">
                    <div class="action-copy">
                        <p class="panel-label">{control.label.clone()}</p>
                        <strong class="panel-value">
                            {move || slider_display(slider_value.get(), control.max_value)}
                        </strong>
                    </div>
                    <span class="panel-tag">
                        {move || if glide_delay_ms.get() == 0 { "Instant" } else { "Ramped" }}
                    </span>
                </div>

                <input
                    class="slider action-slider"
                    type="range"
                    min="0"
                    max=control.max_value.unwrap_or(100)
                    prop:value=move || slider_value.get().to_string()
                    style=move || format!(
                        "--slider-fill: {}%;",
                        control
                            .max_value
                            .map(|maximum| {
                                ((slider_value.get() as f32 / maximum.max(1) as f32) * 100.0).round()
                                    as u16
                            })
                            .unwrap_or(0)
                    )
                    disabled=move || !control.supported
                    on:input=on_input
                    on:change=on_change
                />

                <Show when=move || !control.supported>
                    <p class="support-note warning">
                        {control.error.clone().unwrap_or_else(|| format!("{} is unavailable.", control.label))}
                    </p>
                </Show>
            </section>
        }
            .into_any()
    } else {
        view! {
            <section class="control-panel range-panel">
                <div class="control-header">
                    <div>
                        <p class="panel-label">{control.label.clone()}</p>
                        <strong class="panel-value">
                            {move || slider_display(slider_value.get(), control.max_value)}
                        </strong>
                    </div>
                    <span class="panel-tag">
                        {move || if glide_delay_ms.get() == 0 { "Instant" } else { "Ramped" }}
                    </span>
                </div>

                <input
                    class="slider"
                    type="range"
                    min="0"
                    max=control.max_value.unwrap_or(100)
                    prop:value=move || slider_value.get().to_string()
                    style=move || format!(
                        "--slider-fill: {}%;",
                        control
                            .max_value
                            .map(|maximum| {
                                ((slider_value.get() as f32 / maximum.max(1) as f32) * 100.0).round()
                                    as u16
                            })
                            .unwrap_or(0)
                    )
                    disabled=move || !control.supported
                    on:input=on_input
                    on:change=on_change
                />

                <Show when=move || !control.supported>
                    <p class="support-note warning">
                        {control.error.clone().unwrap_or_else(|| format!("{} is unavailable.", control.label))}
                    </p>
                </Show>
            </section>
        }
            .into_any()
    }
}

#[component]
fn ChoiceControl(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    control: MonitorControl,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
    variant: &'static str,
    open_state: Option<RwSignal<bool>>,
) -> impl IntoView {
    let selected_value = RwSignal::new(control.current_value.unwrap_or_default());
    let control_code = control.code.clone();
    let control_label = control.label.clone();
    let control_heading_label = control.label.clone();
    let control_aria_label = control.label.clone();
    let options = control.options.clone();
    let options_for_value = options.clone();
    let has_options = !options.is_empty();
    let control_supported = control.supported;
    let control_error = control.error.clone();
    let choice_body = move || {
        view! {
            <div class="choice-strip preset-choice-strip" aria-label=control_aria_label.clone() role="group">
                <For
                    each=move || options.clone()
                    key=|option| option.value
                    children=move |option| {
                        let option_value = option.value;
                        let option_label = option.label.clone();
                        let monitor_id = monitor_id.clone();
                        let control_code = control_code.clone();
                        let control_label = control_label.clone();

                        view! {
                            <button
                                class=move || {
                                    if selected_value.get() == option_value {
                                        "choice-segment active"
                                    } else {
                                        "choice-segment"
                                    }
                                }
                                type="button"
                                disabled=move || !control_supported || is_busy.get() || !has_options
                                on:click=move |_| {
                                    if selected_value.get_untracked() == option_value {
                                        return;
                                    }

                                    let monitor_id = monitor_id.clone();
                                    let control_code = control_code.clone();
                                    let control_label = control_label.clone();
                                    selected_value.set(option_value);
                                    is_busy.set(true);
                                    local_error.set(String::new());

                                    spawn_local(async move {
                                        match interop::set_feature(&monitor_id, &control_code, option_value).await {
                                            Ok(updated) => replace_monitor_snapshot(monitors, updated),
                                            Err(error) => local_error.set(format!("{control_label}: {error}")),
                                        }

                                        is_busy.set(false);
                                    });
                                }
                            >
                                {option_label}
                            </button>
                        }
                    }
                />
            </div>
        }
    };

    if variant == "surface" {
        let is_open = open_state.unwrap_or(RwSignal::new(false));
        view! {
            <details
                class="surface-fold"
                prop:open=move || is_open.get()
                on:toggle=move |event: Event| {
                    is_open.set(event_target::<HtmlDetailsElement>(&event).open());
                }
            >
                <summary class="surface-fold-summary">
                    <div>
                        <p class="panel-label">{control_heading_label.clone()}</p>
                        <strong class="panel-value surface-fold-value">
                            {move || option_label(&options_for_value, selected_value.get())}
                        </strong>
                    </div>
                </summary>

                <div class="surface-fold-body">
                    {choice_body()}

                    <Show when=move || !control_supported || !has_options>
                        <p class="support-note warning">
                            {if !control_supported {
                                control_error.clone().unwrap_or_else(|| format!("{} is unavailable.", control.label))
                            } else {
                                format!("{} does not expose selectable values.", control.label)
                            }}
                        </p>
                    </Show>
                </div>
            </details>
        }
            .into_any()
    } else {
        let card_variant = "control-panel compact choice-panel";
        view! {
            <section class=move || format!("{card_variant} {variant}")>
                <div class="control-header">
                    <div>
                        <p class="panel-label">{control.label.clone()}</p>
                        <strong class="panel-value">
                            {move || option_label(&options_for_value, selected_value.get())}
                        </strong>
                    </div>
                </div>

                {choice_body()}

                <Show when=move || !control_supported || !has_options>
                    <p class="support-note warning">
                        {if !control_supported {
                            control_error.clone().unwrap_or_else(|| format!("{} is unavailable.", control.label))
                        } else {
                            format!("{} does not expose selectable values.", control.label)
                        }}
                    </p>
                </Show>
            </section>
        }
            .into_any()
    }
}

#[component]
fn PowerModeSlider(
    monitor_id: String,
    monitors: RwSignal<Vec<MonitorSnapshot>>,
    control: MonitorControl,
    is_busy: RwSignal<bool>,
    local_error: RwSignal<String>,
) -> impl IntoView {
    let options = control.options.clone();
    let label = control.label.clone();
    let label_for_header = label.clone();
    let label_for_group = label.clone();
    let label_for_modal = label.clone();
    let label_for_warning = label.clone();
    let control_supported = control.supported && !options.is_empty();
    let control_error = control.error.clone();
    let monitor_id_apply = monitor_id.clone();
    let monitor_id_confirm = StoredValue::new(monitor_id.clone());
    let control_code = control.code.clone();
    let control_code_apply = control_code.clone();
    let control_code_confirm = StoredValue::new(control.code.clone());
    let power_label_apply = label.clone();
    let power_label_confirm = StoredValue::new(label.clone());
    let selected_value = RwSignal::new(control.current_value);
    let options_for_value = StoredValue::new(options.clone());
    let show_power_confirm = RwSignal::new(false);
    let pending_power_value = RwSignal::new(None::<u16>);

    view! {
        <section class="action-row toggle-row power-row">
            <div class="action-main">
                <div class="action-copy">
                    <p class="panel-label">{label_for_header.clone()}</p>
                    <strong class="panel-value">
                        {move || {
                            match selected_value.get() {
                                Some(value) => {
                                    if value == 0x01 {
                                        String::new()
                                    } else {
                                        option_label(&options_for_value.get_value(), value)
                                    }
                                }
                                None => String::new(),
                            }
                        }}
                    </strong>
                </div>
            </div>

            <div class="choice-strip preset-choice-strip" aria-label=label_for_group.clone() role="group">
                <For
                    each=move || options.clone()
                    key=|option| option.value
                    children=move |option| {
                        let option_value = option.value;
                        let option_label = option.label.clone();
                        let monitor_id = monitor_id_apply.clone();
                        let control_code = control_code_apply.clone();
                        let control_label = power_label_apply.clone();

                        view! {
                            <button
                                class=move || {
                                    if selected_value.get() == Some(option_value) {
                                        "choice-segment active"
                                    } else {
                                        "choice-segment"
                                    }
                                }
                                type="button"
                                disabled=move || !control_supported || is_busy.get()
                                on:click=move |_| {
                                    if selected_value.get_untracked() == Some(option_value) {
                                        return;
                                    }

                                    if power_mode_requires_confirmation(option_value) {
                                        pending_power_value.set(Some(option_value));
                                        show_power_confirm.set(true);
                                        return;
                                    }

                                    let previous_value = selected_value.get_untracked();
                                    selected_value.set(Some(option_value));
                                    is_busy.set(true);
                                    local_error.set(String::new());

                                    let monitor_id = monitor_id.clone();
                                    let control_code = control_code.clone();
                                    let control_label = control_label.clone();

                                    spawn_local(async move {
                                        match interop::set_feature(&monitor_id, &control_code, option_value).await {
                                            Ok(updated) => replace_monitor_snapshot(monitors, updated),
                                            Err(error) => {
                                                selected_value.set(previous_value);
                                                local_error.set(format!("{control_label}: {error}"));
                                            }
                                        }

                                        is_busy.set(false);
                                    });
                                }
                            >
                                {option_label}
                            </button>
                        }
                    }
                />
            </div>

            <Show when=move || show_power_confirm.get()>
                <div class="modal-overlay" on:click=move |_| show_power_confirm.set(false)>
                    <div class="confirm-modal" on:click=move |event| event.stop_propagation()>
                        <div class="confirm-copy">
                            <p class="panel-label">{label_for_modal.clone()}</p>
                            <strong>
                                {move || {
                                    let target = pending_power_value.get().unwrap_or(0x04);
                                    format!(
                                        "Switch to {}?",
                                        option_label(&options_for_value.get_value(), target)
                                    )
                                }}
                            </strong>
                            <p class="support-note">
                                "You may need to power it back on manually."
                            </p>
                        </div>

                        <div class="confirm-actions">
                            <button
                                class="button ghost confirm-button"
                                type="button"
                                on:click=move |_| show_power_confirm.set(false)
                            >
                                "Cancel"
                            </button>

                            <button
                                class="button confirm-button danger"
                                type="button"
                                on:click=move |_| {
                                    let Some(target_value) = pending_power_value.get() else {
                                        show_power_confirm.set(false);
                                        return;
                                    };

                                    let previous_value = selected_value.get_untracked();
                                    selected_value.set(Some(target_value));
                                    show_power_confirm.set(false);
                                    pending_power_value.set(None);
                                    is_busy.set(true);
                                    local_error.set(String::new());

                                    let monitor_id = monitor_id_confirm.get_value();
                                    let control_code = control_code_confirm.get_value();
                                    let control_label = power_label_confirm.get_value();

                                    spawn_local(async move {
                                        match interop::set_feature(&monitor_id, &control_code, target_value).await {
                                            Ok(updated) => replace_monitor_snapshot(monitors, updated),
                                            Err(error) => {
                                                selected_value.set(previous_value);
                                                local_error.set(format!("{control_label}: {error}"));
                                            }
                                        }

                                        is_busy.set(false);
                                    });
                                }
                            >
                                "Apply"
                            </button>
                        </div>
                    </div>
                </div>
            </Show>

            <Show when=move || !control_supported>
                <p class="support-note warning">
                    {control_error
                        .clone()
                        .unwrap_or_else(|| format!("{} is unavailable.", label_for_warning))}
                </p>
            </Show>
        </section>
    }
}

fn power_mode_requires_confirmation(value: u16) -> bool {
    matches!(value, 0x04 | 0x05)
}

fn glide_label(delay_ms: u16) -> String {
    match delay_ms {
        0 => String::from("Instant apply"),
        1..=10 => format!("{delay_ms} ms per step"),
        _ => format!("Soft ramp · {delay_ms} ms per step"),
    }
}

fn glide_compact_label(delay_ms: u16) -> String {
    match delay_ms {
        0 => String::from("Instant"),
        _ => format!("{delay_ms} ms"),
    }
}

fn preset_tone_class(label: &str) -> &'static str {
    let lower = label.to_ascii_lowercase();

    if lower.contains("srgb") || lower.contains("graphics") || lower.contains("photo") {
        "tone-paper"
    } else if lower.contains("5000") || lower.contains("reading") || lower.contains("print") {
        "tone-daylight"
    } else if lower.contains("6500") || lower.contains("web") || lower.contains("gaming") {
        "tone-crisp"
    } else if lower.contains("7500") || lower.contains("8200") || lower.contains("cool") {
        "tone-cool"
    } else if lower.contains("9300")
        || lower.contains("11500")
        || lower.contains("blue")
        || lower.contains("ice")
    {
        "tone-blue"
    } else {
        "tone-custom"
    }
}

fn warm_scene_tone_class(scene_id: &str) -> &'static str {
    match scene_id {
        "paper" => "tone-paper",
        "sunset" | "ember" | "incandescent" | "candle" => "tone-custom",
        "nocturne" => "tone-blue",
        _ => "tone-custom",
    }
}

fn color_scene_label(scene_id: &str) -> Option<&'static str> {
    COLOR_SCENES
        .iter()
        .find(|scene| scene.id == scene_id)
        .map(|scene| scene.label)
}

fn slider_display(current: u16, maximum: Option<u16>) -> String {
    match maximum {
        Some(maximum) if maximum > 0 => {
            let percent = ((current as f32 / maximum as f32) * 100.0).round() as u16;
            format!("{percent}%  ·  {current}/{maximum}")
        }
        _ => current.to_string(),
    }
}

fn find_control(controls: &[MonitorControl], code: &str) -> Option<MonitorControl> {
    controls
        .iter()
        .find(|control| control.code == code)
        .cloned()
}

fn option_label(options: &[ControlOption], value: u16) -> String {
    options
        .iter()
        .find(|option| option.value == value)
        .map(|option| option.label.clone())
        .unwrap_or_else(|| format!("Value {value}"))
}

fn monitor_subtitle(monitor: &MonitorSnapshot) -> String {
    match (&monitor.manufacturer_id, &monitor.serial_number) {
        (Some(mfg), Some(serial)) => format!("{mfg}  •  {serial}"),
        (Some(mfg), None) => mfg.clone(),
        (None, Some(serial)) => serial.clone(),
        (None, None) => String::new(),
    }
}

fn monitor_identity_label(monitor: &MonitorSnapshot) -> String {
    if let Some(device) = monitor_transport_label(monitor) {
        if let Some(connector) = monitor.connector_name.as_ref() {
            return format!("{device}  •  {connector}");
        }

        return device;
    }

    format!("{}  •  {}", monitor.backend, monitor.id)
}

fn display_switch_meta(monitor: &MonitorSnapshot) -> String {
    monitor_transport_label(monitor)
        .or_else(|| monitor.serial_number.clone())
        .or_else(|| monitor.manufacturer_id.clone())
        .unwrap_or_else(|| monitor.backend.clone())
}

fn monitor_transport_label(monitor: &MonitorSnapshot) -> Option<String> {
    monitor
        .device_path
        .as_ref()
        .map(|path| format!("dev:{path}"))
}

fn queue_range_change(ctx: RangeChangeContext, parsed: u16) {
    ctx.slider_value.set(parsed);
    ctx.pending_value.set(Some(parsed));
    ctx.local_error.set(String::new());

    if !ctx.in_flight.get_untracked() {
        drive_range_change(ctx);
    }
}

fn drive_range_change(ctx: RangeChangeContext) {
    let Some(target) = ctx.pending_value.get_untracked() else {
        return;
    };

    ctx.pending_value.set(None);
    ctx.in_flight.set(true);

    spawn_local(async move {
        let result = if ctx.delay == 0 {
            interop::set_feature(&ctx.monitor_id, &ctx.control_code, target).await
        } else {
            interop::transition_feature(&ctx.monitor_id, &ctx.control_code, target, ctx.delay).await
        };

        match result {
            Ok(updated) => {
                if ctx.pending_value.get_untracked().is_none() {
                    replace_monitor_snapshot(ctx.monitors, updated);
                }
            }
            Err(error) => ctx
                .local_error
                .set(format!("{}: {error}", ctx.control_label)),
        }

        ctx.in_flight.set(false);

        if ctx.pending_value.get_untracked().is_some() {
            drive_range_change(ctx);
        }
    });
}

fn replace_monitor_snapshot(monitors: RwSignal<Vec<MonitorSnapshot>>, updated: MonitorSnapshot) {
    monitors.update(|all| {
        if let Some(existing) = all.iter_mut().find(|monitor| monitor.id == updated.id) {
            *existing = updated;
        }
    });
}
