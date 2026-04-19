#[cfg(target_os = "linux")]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::io;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    use anyhow::{Context, Result, anyhow, bail};
    use shared::{ControlOption, MonitorControl, MonitorControlType, MonitorSnapshot};

    #[derive(Clone, Copy)]
    enum FeatureKind {
        Range,
        Choice,
        Toggle,
        Action,
    }

    #[derive(Clone, Copy)]
    struct FeatureDefinition {
        code: &'static str,
        label: &'static str,
        kind: FeatureKind,
    }

    #[derive(Clone, Debug)]
    struct LinuxMonitor {
        id: String,
        display_number: u8,
        bus_number: Option<u8>,
        device_path: Option<String>,
        connector_name: Option<String>,
        manufacturer_id: Option<String>,
        model_name: Option<String>,
        serial_number: Option<String>,
    }

    #[derive(Clone, Debug, Default)]
    struct CapabilityFeature {
        label: Option<String>,
        options: Vec<ControlOption>,
    }

    #[derive(Clone, Debug)]
    struct FeatureReadout {
        current_value: u16,
        max_value: Option<u16>,
    }

    #[derive(Clone, Copy)]
    struct ColorSceneProfile {
        red_percent: u8,
        green_percent: u8,
        blue_percent: u8,
    }

    const FEATURE_DEFINITIONS: &[FeatureDefinition] = &[
        FeatureDefinition {
            code: "10",
            label: "Brightness",
            kind: FeatureKind::Range,
        },
        FeatureDefinition {
            code: "12",
            label: "Contrast",
            kind: FeatureKind::Range,
        },
        FeatureDefinition {
            code: "62",
            label: "Volume",
            kind: FeatureKind::Range,
        },
        FeatureDefinition {
            code: "14",
            label: "Color Preset",
            kind: FeatureKind::Choice,
        },
        FeatureDefinition {
            code: "16",
            label: "Red Gain",
            kind: FeatureKind::Range,
        },
        FeatureDefinition {
            code: "18",
            label: "Green Gain",
            kind: FeatureKind::Range,
        },
        FeatureDefinition {
            code: "1A",
            label: "Blue Gain",
            kind: FeatureKind::Range,
        },
        FeatureDefinition {
            code: "8D",
            label: "Mute",
            kind: FeatureKind::Toggle,
        },
        FeatureDefinition {
            code: "CA",
            label: "OSD",
            kind: FeatureKind::Choice,
        },
        FeatureDefinition {
            code: "CC",
            label: "OSD Language",
            kind: FeatureKind::Choice,
        },
        FeatureDefinition {
            code: "D6",
            label: "Power Mode",
            kind: FeatureKind::Toggle,
        },
        FeatureDefinition {
            code: "04",
            label: "Restore Factory Defaults",
            kind: FeatureKind::Action,
        },
        FeatureDefinition {
            code: "05",
            label: "Restore Brightness / Contrast",
            kind: FeatureKind::Action,
        },
        FeatureDefinition {
            code: "08",
            label: "Restore Color Defaults",
            kind: FeatureKind::Action,
        },
    ];
    const RANGE_TARGET_STEP_SIZE: u16 = 2;
    const RANGE_MAX_TRANSITION_WRITES: u16 = 24;
    const COLOR_SCENE_GAIN_STEP_DELAY_MS: u64 = 18;

    pub fn list_monitors() -> Result<Vec<MonitorSnapshot>> {
        let monitors = detect_monitors()?;
        let mut snapshots = Vec::with_capacity(monitors.len());

        for monitor in monitors {
            snapshots.push(snapshot_for_monitor(&monitor));
        }

        Ok(snapshots)
    }

    pub fn set_monitor_feature(
        monitor_id: &str,
        code: &str,
        value: u16,
    ) -> Result<MonitorSnapshot> {
        let monitor = find_monitor(monitor_id)?;
        let normalized_code = code.to_ascii_uppercase();
        if normalized_code == "60" {
            bail!("Input source control is disabled in this app");
        }
        if is_rgb_gain_code(normalized_code.as_str()) {
            if set_feature_value(&monitor, "14", 0x0b).is_ok() {
                thread::sleep(Duration::from_millis(140));
            }
        }
        set_feature_value(&monitor, code, value)?;
        if matches!(normalized_code.as_str(), "14" | "04" | "05" | "08") {
            thread::sleep(Duration::from_millis(220));
        }
        Ok(snapshot_for_monitor(&monitor))
    }

    pub fn transition_monitor_feature(
        monitor_id: &str,
        code: &str,
        value: u16,
        step_delay_ms: u64,
    ) -> Result<MonitorSnapshot> {
        let monitor = find_monitor(monitor_id)?;
        let normalized_code = code.to_ascii_uppercase();
        if normalized_code == "60" {
            bail!("Input source control is disabled in this app");
        }
        if is_rgb_gain_code(normalized_code.as_str()) {
            if set_feature_value(&monitor, "14", 0x0b).is_ok() {
                thread::sleep(Duration::from_millis(140));
            }
        }
        let definition =
            feature_definition(code).with_context(|| format!("Unsupported feature code {code}"))?;

        if !matches!(definition.kind, FeatureKind::Range) || step_delay_ms == 0 {
            set_feature_value(&monitor, code, value)?;
            return Ok(snapshot_for_monitor(&monitor));
        }

        let current = read_feature(&monitor, code)?;
        let maximum = current.max_value.unwrap_or(value).max(1);
        let target = value.min(maximum);

        if current.current_value == target {
            return Ok(snapshot_for_monitor(&monitor));
        }

        let sequence = build_transition_sequence(
            current.current_value,
            target,
            RANGE_TARGET_STEP_SIZE,
            RANGE_MAX_TRANSITION_WRITES,
        );
        let delay = Duration::from_millis(step_delay_ms);
        for (index, next) in sequence.iter().enumerate() {
            set_feature_value(&monitor, code, *next)?;
            if index + 1 < sequence.len() {
                thread::sleep(delay);
            }
        }

        Ok(snapshot_for_monitor(&monitor))
    }

    pub fn apply_color_scene(monitor_id: &str, scene_id: &str) -> Result<MonitorSnapshot> {
        let monitor = find_monitor(monitor_id)?;
        let profile = color_scene_profile(scene_id)
            .with_context(|| format!("Unknown color scene {scene_id}"))?;

        set_feature_value(&monitor, "14", 0x0b)?;
        thread::sleep(Duration::from_millis(140));
        apply_rgb_gain_percent(
            &monitor,
            "16",
            profile.red_percent,
            COLOR_SCENE_GAIN_STEP_DELAY_MS,
        )?;
        apply_rgb_gain_percent(
            &monitor,
            "18",
            profile.green_percent,
            COLOR_SCENE_GAIN_STEP_DELAY_MS,
        )?;
        apply_rgb_gain_percent(
            &monitor,
            "1A",
            profile.blue_percent,
            COLOR_SCENE_GAIN_STEP_DELAY_MS,
        )?;

        Ok(snapshot_for_monitor(&monitor))
    }

    fn is_rgb_gain_code(code: &str) -> bool {
        matches!(code, "16" | "18" | "1A")
    }

    fn find_monitor(monitor_id: &str) -> Result<LinuxMonitor> {
        detect_monitors()?
            .into_iter()
            .find(|monitor| monitor.id == monitor_id)
            .with_context(|| format!("Monitor {monitor_id} was not found"))
    }

    fn snapshot_for_monitor(monitor: &LinuxMonitor) -> MonitorSnapshot {
        let capabilities = read_capabilities(monitor).ok();
        let mut controls = Vec::with_capacity(FEATURE_DEFINITIONS.len());

        for definition in FEATURE_DEFINITIONS {
            let capability = capabilities
                .as_ref()
                .and_then(|entries| entries.get(definition.code));

            if matches!(definition.kind, FeatureKind::Action) {
                controls.push(action_control_snapshot(
                    definition,
                    capability,
                    capabilities.is_some(),
                ));
                continue;
            }

            match read_feature(monitor, definition.code) {
                Ok(readout) => {
                    let label = capability
                        .and_then(|feature| feature.label.clone())
                        .unwrap_or_else(|| definition.label.to_string());

                    controls.push(MonitorControl {
                        code: definition.code.to_string(),
                        label,
                        control_type: control_type_for_kind(definition.kind),
                        current_value: Some(readout.current_value),
                        max_value: readout.max_value,
                        options: resolved_options(definition, capability),
                        supported: true,
                        error: None,
                    });
                }
                Err(error) => {
                    controls.push(MonitorControl {
                        code: definition.code.to_string(),
                        label: definition.label.to_string(),
                        control_type: control_type_for_kind(definition.kind),
                        current_value: None,
                        max_value: None,
                        options: resolved_options(definition, capability),
                        supported: false,
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        let error = if controls.iter().any(|control| control.supported) {
            None
        } else {
            Some(String::from(
                "No supported writable controls were detected for this display.",
            ))
        };

        MonitorSnapshot {
            id: monitor.id.clone(),
            backend: String::from("ddcutil"),
            device_path: monitor.device_path.clone(),
            connector_name: monitor.connector_name.clone(),
            manufacturer_id: monitor.manufacturer_id.clone(),
            model_name: monitor.model_name.clone(),
            serial_number: monitor.serial_number.clone(),
            controls,
            error,
        }
    }

    fn detect_monitors() -> Result<Vec<LinuxMonitor>> {
        let output = run_ddcutil(["detect".to_string()])?;
        parse_detect_output(&output)
    }

    fn read_capabilities(monitor: &LinuxMonitor) -> Result<BTreeMap<String, CapabilityFeature>> {
        let mut args = monitor_selector_args(monitor);
        args.push("capabilities".to_string());
        let output = run_ddcutil(args)
            .with_context(|| format!("Failed to query capabilities for {}", monitor.id))?;
        Ok(parse_capabilities_output(&output))
    }

    fn read_feature(monitor: &LinuxMonitor, code: &str) -> Result<FeatureReadout> {
        let mut args = monitor_selector_args(monitor);
        args.extend([
            "--brief".to_string(),
            "getvcp".to_string(),
            code.to_string(),
        ]);

        let output = run_ddcutil(args)
            .with_context(|| format!("Failed to query feature {code} for {}", monitor.id))?;

        parse_feature_output(code, &output)
    }

    fn set_feature_value(monitor: &LinuxMonitor, code: &str, value: u16) -> Result<()> {
        let mut args = monitor_selector_args(monitor);
        args.extend([
            "--noverify".to_string(),
            "setvcp".to_string(),
            code.to_string(),
            value.to_string(),
        ]);

        run_ddcutil(args)
            .with_context(|| format!("Failed to set feature {code} for {}", monitor.id))?;
        Ok(())
    }

    fn monitor_selector_args(monitor: &LinuxMonitor) -> Vec<String> {
        if let Some(bus_number) = monitor.bus_number {
            vec![format!("--bus={bus_number}")]
        } else {
            vec![format!("--display={}", monitor.display_number)]
        }
    }

    fn run_ddcutil<I, S>(args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new("ddcutil")
            .args(args)
            .output()
            .map_err(map_spawn_error)?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let details = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("ddcutil exited with status {}", output.status)
        };

        Err(anyhow!(details))
    }

    fn map_spawn_error(error: io::Error) -> anyhow::Error {
        if error.kind() == io::ErrorKind::NotFound {
            anyhow!(
                "ddcutil is not installed or not on PATH. On Linux this build uses ddcutil for monitor control."
            )
        } else {
            anyhow!("Failed to launch ddcutil: {error}")
        }
    }

    fn feature_definition(code: &str) -> Option<&'static FeatureDefinition> {
        FEATURE_DEFINITIONS
            .iter()
            .find(|definition| definition.code.eq_ignore_ascii_case(code))
    }

    fn control_type_for_kind(kind: FeatureKind) -> MonitorControlType {
        match kind {
            FeatureKind::Range => MonitorControlType::Range,
            FeatureKind::Choice => MonitorControlType::Choice,
            FeatureKind::Toggle => MonitorControlType::Toggle,
            FeatureKind::Action => MonitorControlType::Action,
        }
    }

    fn action_control_snapshot(
        definition: &FeatureDefinition,
        capability: Option<&CapabilityFeature>,
        capabilities_available: bool,
    ) -> MonitorControl {
        let supported = capability.is_some();
        let label = capability
            .and_then(|feature| feature.label.clone())
            .unwrap_or_else(|| definition.label.to_string());
        let error = if supported {
            None
        } else if capabilities_available {
            Some(format!(
                "{} is not reported in the monitor capabilities.",
                definition.label
            ))
        } else {
            Some(format!(
                "{} could not be verified because the monitor capabilities were unavailable.",
                definition.label
            ))
        };

        MonitorControl {
            code: definition.code.to_string(),
            label,
            control_type: MonitorControlType::Action,
            current_value: None,
            max_value: None,
            options: resolved_options(definition, capability),
            supported,
            error,
        }
    }

    fn color_scene_profile(scene_id: &str) -> Option<ColorSceneProfile> {
        match scene_id {
            "paper" => Some(ColorSceneProfile {
                red_percent: 94,
                green_percent: 92,
                blue_percent: 88,
            }),
            "sunset" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 72,
                blue_percent: 46,
            }),
            "ember" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 62,
                blue_percent: 32,
            }),
            "incandescent" => Some(ColorSceneProfile {
                red_percent: 96,
                green_percent: 68,
                blue_percent: 38,
            }),
            "candle" => Some(ColorSceneProfile {
                red_percent: 92,
                green_percent: 56,
                blue_percent: 24,
            }),
            "nocturne" => Some(ColorSceneProfile {
                red_percent: 84,
                green_percent: 46,
                blue_percent: 14,
            }),
            _ => None,
        }
    }

    fn apply_rgb_gain_percent(
        monitor: &LinuxMonitor,
        code: &str,
        percent: u8,
        step_delay_ms: u64,
    ) -> Result<()> {
        let readout = read_feature(monitor, code)?;
        let maximum = readout.max_value.unwrap_or(100).max(1);
        let clamped = percent.min(100) as u32;
        let target = (((maximum as u32) * clamped) + 50) / 100;
        let target = target.min(maximum as u32) as u16;
        let current = readout.current_value.min(maximum);
        if current == target {
            return Ok(());
        }

        let sequence = build_transition_sequence(
            current,
            target,
            RANGE_TARGET_STEP_SIZE,
            RANGE_MAX_TRANSITION_WRITES,
        );
        let delay = Duration::from_millis(step_delay_ms);
        for (index, next) in sequence.iter().enumerate() {
            set_feature_value(monitor, code, *next)?;
            if index + 1 < sequence.len() {
                thread::sleep(delay);
            }
        }
        Ok(())
    }

    fn build_transition_sequence(
        current: u16,
        target: u16,
        target_step_size: u16,
        max_writes: u16,
    ) -> Vec<u16> {
        if current == target {
            return Vec::new();
        }

        let distance = current.abs_diff(target);
        let desired_writes = ((distance + target_step_size.saturating_sub(1))
            / target_step_size.max(1))
        .max(1);
        let writes = desired_writes.min(max_writes.max(1)).min(distance).max(1);
        let mut sequence = Vec::with_capacity(writes as usize);
        let ascending = target > current;
        let mut last = current;
        let total = u32::from(distance);
        let writes_u32 = u32::from(writes);

        for step in 1..=writes_u32 {
            let progressed = ((total * step) + (writes_u32 / 2)) / writes_u32;
            let progressed = progressed.min(total) as u16;
            let next = if ascending {
                current.saturating_add(progressed).min(target)
            } else {
                current.saturating_sub(progressed).max(target)
            };
            if next != last {
                sequence.push(next);
                last = next;
            }
        }

        if sequence.last().copied() != Some(target) {
            sequence.push(target);
        }

        sequence
    }

    fn resolved_options(
        definition: &FeatureDefinition,
        capability: Option<&CapabilityFeature>,
    ) -> Vec<ControlOption> {
        let capability_options = capability
            .map(|feature| feature.options.clone())
            .unwrap_or_default();

        if !capability_options.is_empty() {
            return capability_options;
        }

        match definition.code {
            "8D" => vec![
                ControlOption {
                    value: 0x02,
                    label: String::from("Unmuted"),
                },
                ControlOption {
                    value: 0x01,
                    label: String::from("Muted"),
                },
            ],
            "D6" => vec![
                ControlOption {
                    value: 0x01,
                    label: String::from("On"),
                },
                ControlOption {
                    value: 0x02,
                    label: String::from("Standby"),
                },
                ControlOption {
                    value: 0x03,
                    label: String::from("Suspend"),
                },
                ControlOption {
                    value: 0x04,
                    label: String::from("Off"),
                },
                ControlOption {
                    value: 0x05,
                    label: String::from("Turn Off"),
                },
            ],
            "CA" => vec![
                ControlOption {
                    value: 0x01,
                    label: String::from("Disabled"),
                },
                ControlOption {
                    value: 0x02,
                    label: String::from("Enabled"),
                },
            ],
            "CC" => vec![
                ControlOption {
                    value: 0x02,
                    label: String::from("English"),
                },
                ControlOption {
                    value: 0x03,
                    label: String::from("French"),
                },
                ControlOption {
                    value: 0x04,
                    label: String::from("German"),
                },
                ControlOption {
                    value: 0x05,
                    label: String::from("Italian"),
                },
                ControlOption {
                    value: 0x09,
                    label: String::from("Russian"),
                },
                ControlOption {
                    value: 0x0A,
                    label: String::from("Spanish"),
                },
                ControlOption {
                    value: 0x0B,
                    label: String::from("Swedish"),
                },
            ],
            "04" => vec![ControlOption {
                value: 0x01,
                label: String::from("Restore Factory Defaults"),
            }],
            "05" => vec![ControlOption {
                value: 0x01,
                label: String::from("Restore Brightness and Contrast"),
            }],
            "08" => vec![ControlOption {
                value: 0x01,
                label: String::from("Restore Color Defaults"),
            }],
            _ => Vec::new(),
        }
    }

    fn parse_detect_output(output: &str) -> Result<Vec<LinuxMonitor>> {
        let mut monitors = Vec::new();
        let mut current: Option<LinuxMonitor> = None;

        for raw_line in output.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                if let Some(monitor) = current.take() {
                    monitors.push(monitor);
                }
                continue;
            }

            if let Some(rest) = line.strip_prefix("Display ") {
                if let Some(monitor) = current.take() {
                    monitors.push(monitor);
                }

                let display_number = rest.trim().parse::<u8>().with_context(|| {
                    format!("Failed to parse ddcutil display number from: {line}")
                })?;

                current = Some(LinuxMonitor {
                    id: display_number.to_string(),
                    display_number,
                    bus_number: None,
                    device_path: None,
                    connector_name: None,
                    manufacturer_id: None,
                    model_name: None,
                    serial_number: None,
                });
                continue;
            }

            let Some(monitor) = current.as_mut() else {
                continue;
            };

            if let Some(path) = line.strip_prefix("I2C bus:") {
                let path = path.trim();
                monitor.device_path = Some(path.to_string());
                monitor.bus_number = path
                    .rsplit('-')
                    .next()
                    .and_then(|number| number.parse::<u8>().ok());
                if let Some(bus_number) = monitor.bus_number {
                    monitor.id = bus_number.to_string();
                }
                continue;
            }

            if let Some(value) = line.strip_prefix("DRM_connector:") {
                let connector = value.trim();
                if !connector.is_empty() {
                    monitor.connector_name = Some(connector.to_string());
                }
                continue;
            }

            if let Some(value) = line.strip_prefix("Mfg id:") {
                let mfg = value.trim().split_whitespace().next().unwrap_or_default();
                if !mfg.is_empty() {
                    monitor.manufacturer_id = Some(mfg.to_string());
                }
                continue;
            }

            if let Some(value) = line.strip_prefix("Model:") {
                let model = value.trim();
                if !model.is_empty() {
                    monitor.model_name = Some(model.to_string());
                }
                continue;
            }

            if let Some(value) = line.strip_prefix("Serial number:") {
                let serial = value.trim();
                if !serial.is_empty() {
                    monitor.serial_number = Some(serial.to_string());
                }
            }
        }

        if let Some(monitor) = current.take() {
            monitors.push(monitor);
        }

        if monitors.is_empty() {
            bail!("ddcutil did not report any detected monitors");
        }

        Ok(monitors)
    }

    fn parse_capabilities_output(output: &str) -> BTreeMap<String, CapabilityFeature> {
        let mut features = BTreeMap::<String, CapabilityFeature>::new();
        let mut current_code: Option<String> = None;
        let mut in_values = false;

        for raw_line in output.lines() {
            let line = raw_line.trim();
            if let Some(rest) = line.strip_prefix("Feature:") {
                in_values = false;

                let code = rest.split_whitespace().next().map(str::to_uppercase);

                let label = rest
                    .split_once('(')
                    .and_then(|(_, tail)| tail.strip_suffix(')'))
                    .map(str::trim)
                    .map(str::to_string);

                if let Some(code) = code {
                    features.entry(code.clone()).or_default().label = label;
                    current_code = Some(code);
                }
                continue;
            }

            if line == "Values:" {
                in_values = true;
                continue;
            }

            if !in_values {
                continue;
            }

            let Some(code) = current_code.as_ref() else {
                continue;
            };

            let Some((value, label)) = line.split_once(':') else {
                continue;
            };

            let parsed_value = u16::from_str_radix(value.trim(), 16);
            if let Ok(parsed_value) = parsed_value {
                features
                    .entry(code.clone())
                    .or_default()
                    .options
                    .push(ControlOption {
                        value: parsed_value,
                        label: semantic_option_label(code, parsed_value, label.trim()),
                    });
            }
        }

        features
    }

    fn semantic_option_label(code: &str, value: u16, label: &str) -> String {
        if code != "14" {
            return label.to_string();
        }

        match value {
            0x01 => String::from("sRGB · Graphics / Photos"),
            0x04 => String::from("5000 K · Reading / Print"),
            0x05 => String::from("6500 K · Web / Gaming"),
            0x06 => String::from("7500 K · Crisp Daylight"),
            0x07 => String::from("8200 K · Cool Focus"),
            0x08 => String::from("9300 K · Cold Blue"),
            0x0a => String::from("11500 K · Ice Blue"),
            0x0b => String::from("User 1 · Custom"),
            _ => label.to_string(),
        }
    }

    fn parse_feature_output(code: &str, output: &str) -> Result<FeatureReadout> {
        let line = output
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .ok_or_else(|| anyhow!("ddcutil returned no data for feature {code}"))?;

        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 4 || parts[0] != "VCP" || !parts[1].eq_ignore_ascii_case(code) {
            bail!("Unexpected ddcutil output for feature {code}: {line}");
        }

        if parts[2] == "ERR" {
            bail!("ddcutil reported that feature {code} is unavailable for this monitor");
        }

        if parts[2] == "C" {
            if parts.len() < 5 {
                bail!("Unexpected continuous output for feature {code}: {line}");
            }

            let current_value = parts[3]
                .parse::<u16>()
                .with_context(|| format!("Failed to parse current value from: {line}"))?;
            let max_value = parts[4]
                .parse::<u16>()
                .with_context(|| format!("Failed to parse max value from: {line}"))?;

            return Ok(FeatureReadout {
                current_value,
                max_value: Some(max_value.max(1)),
            });
        }

        let raw_value = parts[3].trim_start_matches('x');
        let current_value = u16::from_str_radix(raw_value, 16)
            .with_context(|| format!("Failed to parse non-continuous value from: {line}"))?;

        Ok(FeatureReadout {
            current_value,
            max_value: None,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{parse_capabilities_output, parse_detect_output, parse_feature_output};

        #[test]
        fn parses_detect_output_from_ddcutil() {
            let output = r#"
Display 1
   I2C bus:  /dev/i2c-7
   EDID synopsis:
      Mfg id:               SAM - Samsung Electric Company
      Model:                C49J89x
      Product code:         3873  (0x0f21)
      Serial number:        HTJKC00543
      Manufacture year:     2018,  Week: 52
   VCP version:         2.1

Display 2
   I2C bus:  /dev/i2c-9
   DRM_connector:           card2-DP-3
   EDID synopsis:
      Mfg id:               SAM - Samsung Electric Company
      Model:                C34H89x
      Product code:         3621  (0x0e25)
      Serial number:        H4ZRC04847
      Manufacture year:     2021,  Week: 52
   VCP version:         2.1
"#;

            let monitors = parse_detect_output(output).expect("detect output should parse");
            assert_eq!(monitors.len(), 2);
            assert_eq!(monitors[0].display_number, 1);
            assert_eq!(monitors[0].bus_number, Some(7));
            assert_eq!(monitors[0].device_path.as_deref(), Some("/dev/i2c-7"));
            assert_eq!(monitors[0].manufacturer_id.as_deref(), Some("SAM"));
            assert_eq!(monitors[0].model_name.as_deref(), Some("C49J89x"));
            assert_eq!(monitors[0].serial_number.as_deref(), Some("HTJKC00543"));
            assert_eq!(monitors[1].display_number, 2);
            assert_eq!(monitors[1].bus_number, Some(9));
            assert_eq!(monitors[1].device_path.as_deref(), Some("/dev/i2c-9"));
            assert_eq!(monitors[1].connector_name.as_deref(), Some("card2-DP-3"));
            assert_eq!(monitors[1].manufacturer_id.as_deref(), Some("SAM"));
            assert_eq!(monitors[1].model_name.as_deref(), Some("C34H89x"));
            assert_eq!(monitors[1].serial_number.as_deref(), Some("H4ZRC04847"));
        }

        #[test]
        fn parses_capabilities_values() {
            let output = r#"
VCP Features:
   Feature: 14 (Select color preset)
      Values:
         01: sRGB
         0b: User 1
   Feature: 60 (Input Source)
      Values:
         01: VGA-1
         03: DVI-1
"#;

            let features = parse_capabilities_output(output);
            let preset = features.get("14").expect("color preset feature");
            assert_eq!(preset.options[0].label, "sRGB · Graphics / Photos");
            assert_eq!(preset.options[1].label, "User 1 · Custom");

            let input_source = features.get("60").expect("input source feature");
            assert_eq!(input_source.label.as_deref(), Some("Input Source"));
            assert_eq!(input_source.options.len(), 2);
            assert_eq!(input_source.options[1].value, 3);
            assert_eq!(input_source.options[1].label, "DVI-1");
        }

        #[test]
        fn parses_range_feature_output() {
            let feature = parse_feature_output("10", "VCP 10 C 40 100\n")
                .expect("brightness output should parse");
            assert_eq!(feature.current_value, 40);
            assert_eq!(feature.max_value, Some(100));
        }

        #[test]
        fn parses_choice_feature_output() {
            let feature = parse_feature_output("60", "VCP 60 SNC x03\n")
                .expect("input source output should parse");
            assert_eq!(feature.current_value, 3);
            assert_eq!(feature.max_value, None);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::ffi::{CString, c_char, c_void};
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, anyhow, bail};
    use ddc_hi::{Ddc, Display};
    use shared::{MonitorControl, MonitorControlType, MonitorSnapshot};
    use tracing::debug;

    const LUMINANCE_CODE: u8 = 0x10;
    const BRIGHTNESS_CODE: &str = "10";
    const COLOR_PRESET_CODE: &str = "14";
    const INPUT_SOURCE_CODE: &str = "60";
    const COLOR_PRESET_USER_1_VALUE: u16 = 0x0B;
    const RED_GAIN_CODE: &str = "16";
    const GREEN_GAIN_CODE: &str = "18";
    const BLUE_GAIN_CODE: &str = "1A";
    const COLOR_PRESET_SETTLE_MS: u64 = 140;
    const COLOR_SCENE_GAIN_STEP_DELAY_MS: u64 = 18;
    const INPUT_SOURCE_SETTLE_MS: u64 = 180;
    const DDC_MONITOR_PREFIX: &str = "ddc:";
    const BRIGHTNESS_KEY: &str = "brightness";
    const DISPLAY_CONNECT_CLASS_NAME: &str = "IODisplayConnect";
    const HAS_BACKLIGHT_KEY: &str = "IODisplayHasBacklight";
    const DISPLAY_PARAMETERS_KEY: &str = "IODisplayParameters";
    const DISPLAY_VENDOR_ID_KEY: &str = "DisplayVendorID";
    const DISPLAY_PRODUCT_ID_KEY: &str = "DisplayProductID";
    const DISPLAY_SERIAL_NUMBER_KEY: &str = "DisplaySerialNumber";
    const VALUE_KEY: &str = "value";
    const MIN_KEY: &str = "min";
    const MAX_KEY: &str = "max";
    const BACKLIGHT_CLASS_NAMES: &[&str] = &[
        "AppleARMBacklight",
        "AppleBacklightDisplay",
        "IOBacklightDisplay",
    ];
    const MAX_CG_DISPLAYS: usize = 16;
    const CG_DISPLAY_NO_ERR: i32 = 0;
    const DISPLAY_SERVICES_SUCCESS: i32 = 0;
    const DISPLAY_SERVICES_FRAMEWORK_PATH: &str =
        "/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices";
    const CORE_DISPLAY_FRAMEWORK_PATH: &str =
        "/System/Library/PrivateFrameworks/CoreDisplay.framework/CoreDisplay";
    const RTLD_LAZY: i32 = 1;
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const CF_NUMBER_SINT64_TYPE: i32 = 4;
    const CF_NUMBER_FLOAT64_TYPE: i32 = 6;
    const KERN_SUCCESS: KernReturn = 0;
    const DDC_READ_RETRY_ATTEMPTS: usize = 3;
    const DDC_WRITE_RETRY_ATTEMPTS: usize = 4;
    const DDC_READ_RETRY_DELAY_MS: u64 = 40;
    const DDC_WRITE_RETRY_DELAY_MS: u64 = 22;
    const DDC_MIN_TRANSACTION_GAP_MS: u64 = 130;
    const DDC_UNRECOVERABLE_READ_FAILURE_THRESHOLD: usize = 2;
    const DDC_TRANSIENT_PARSE_FAILURE_THRESHOLD: usize = 3;
    const DDC_TARGET_TRANSITION_STEP_SIZE: u16 = 2;
    const DDC_MAX_TRANSITION_WRITES: u16 = 18;
    const RGB_GAIN_FALLBACK_MAX: u16 = 100;

    type DdcShadowState = HashMap<String, HashMap<String, u16>>;
    type DdcReadbackState = HashMap<String, HashMap<String, DdcReadbackHealth>>;
    type DdcCapabilityState = HashMap<String, Option<HashMap<String, Vec<u16>>>>;

    #[derive(Clone, Debug)]
    enum DdcReadbackHealth {
        Readable,
        Broken(String),
    }

    #[derive(Clone, Copy)]
    enum DdcFeatureKind {
        Range,
        Choice,
        Toggle,
        Action,
    }

    #[derive(Clone, Copy)]
    struct DdcFeatureDefinition {
        code: &'static str,
        label: &'static str,
        kind: DdcFeatureKind,
    }

    #[derive(Clone, Copy)]
    struct ColorSceneProfile {
        red_percent: u8,
        green_percent: u8,
        blue_percent: u8,
    }

    #[derive(Clone)]
    struct RgbGainTransitionPlan {
        code: String,
        feature_code: u8,
        current: u16,
        target: u16,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum GainWriteReadiness {
        Ready,
        Unverified,
        PresetAliased(u16),
    }

    const DDC_FEATURE_DEFINITIONS: &[DdcFeatureDefinition] = &[
        DdcFeatureDefinition {
            code: "10",
            label: "Brightness",
            kind: DdcFeatureKind::Range,
        },
        DdcFeatureDefinition {
            code: "12",
            label: "Contrast",
            kind: DdcFeatureKind::Range,
        },
        DdcFeatureDefinition {
            code: "62",
            label: "Volume",
            kind: DdcFeatureKind::Range,
        },
        DdcFeatureDefinition {
            code: "14",
            label: "Color Preset",
            kind: DdcFeatureKind::Choice,
        },
        DdcFeatureDefinition {
            code: "16",
            label: "Red Gain",
            kind: DdcFeatureKind::Range,
        },
        DdcFeatureDefinition {
            code: "18",
            label: "Green Gain",
            kind: DdcFeatureKind::Range,
        },
        DdcFeatureDefinition {
            code: "1A",
            label: "Blue Gain",
            kind: DdcFeatureKind::Range,
        },
        DdcFeatureDefinition {
            code: "8D",
            label: "Mute",
            kind: DdcFeatureKind::Toggle,
        },
        DdcFeatureDefinition {
            code: "CA",
            label: "OSD",
            kind: DdcFeatureKind::Choice,
        },
        DdcFeatureDefinition {
            code: "CC",
            label: "OSD Language",
            kind: DdcFeatureKind::Choice,
        },
        DdcFeatureDefinition {
            code: "D6",
            label: "Power Mode",
            kind: DdcFeatureKind::Toggle,
        },
        DdcFeatureDefinition {
            code: "04",
            label: "Restore Factory Defaults",
            kind: DdcFeatureKind::Action,
        },
        DdcFeatureDefinition {
            code: "05",
            label: "Restore Brightness / Contrast",
            kind: DdcFeatureKind::Action,
        },
        DdcFeatureDefinition {
            code: "08",
            label: "Restore Color Defaults",
            kind: DdcFeatureKind::Action,
        },
    ];

    type KernReturn = i32;
    type IoObject = u32;
    type IoService = u32;
    type IoIterator = u32;
    type IoRegistryEntry = u32;
    type IoOptionBits = u32;
    type MachPort = u32;
    type CfStringRef = *const c_void;
    type CfMutableDictionaryRef = *mut c_void;
    type CgDirectDisplayId = u32;
    type CgDisplayCount = u32;
    type CgError = i32;

    type DisplayServicesGetBrightnessFn = unsafe extern "C" fn(CgDirectDisplayId, *mut f32) -> i32;
    type DisplayServicesSetBrightnessFn = unsafe extern "C" fn(CgDirectDisplayId, f32) -> i32;
    type DisplayServicesBrightnessChangedFn = unsafe extern "C" fn(CgDirectDisplayId, f64);
    type CoreDisplayGetUserBrightnessFn = unsafe extern "C" fn(CgDirectDisplayId) -> f64;
    type CoreDisplaySetUserBrightnessFn = unsafe extern "C" fn(CgDirectDisplayId, f64);

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> CfMutableDictionaryRef;
        fn IOServiceGetMatchingServices(
            master_port: MachPort,
            matching: CfMutableDictionaryRef,
            existing: *mut IoIterator,
        ) -> KernReturn;
        fn IOIteratorNext(iterator: IoIterator) -> IoObject;
        fn IOObjectRelease(object: IoObject) -> KernReturn;
        fn IORegistryEntryGetRegistryEntryID(
            entry: IoRegistryEntry,
            entry_id: *mut u64,
        ) -> KernReturn;
        fn IODisplayGetFloatParameter(
            service: IoService,
            options: IoOptionBits,
            parameter_name: CfStringRef,
            value: *mut f32,
        ) -> KernReturn;
        fn IODisplaySetFloatParameter(
            service: IoService,
            options: IoOptionBits,
            parameter_name: CfStringRef,
            value: f32,
        ) -> KernReturn;
        fn IODisplayCreateInfoDictionary(
            framebuffer: IoService,
            options: IoOptionBits,
        ) -> *const c_void;
        fn IORegistryEntryCreateCFProperty(
            entry: IoRegistryEntry,
            key: CfStringRef,
            allocator: *const c_void,
            options: IoOptionBits,
        ) -> *const c_void;
        fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFBooleanTrue: *const c_void;
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CfStringRef;
        fn CFDictionaryGetValue(the_dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFNumberGetValue(number: *const c_void, the_type: i32, value_ptr: *mut c_void) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetOnlineDisplayList(
            max_displays: u32,
            active_displays: *mut CgDirectDisplayId,
            display_count: *mut CgDisplayCount,
        ) -> CgError;
        fn CGDisplayIsBuiltin(display: CgDirectDisplayId) -> u32;
        fn CGDisplayVendorNumber(display: CgDirectDisplayId) -> u32;
        fn CGDisplayModelNumber(display: CgDirectDisplayId) -> u32;
        fn CGDisplaySerialNumber(display: CgDirectDisplayId) -> u32;
    }

    struct DisplayServicesApi {
        _handle: usize,
        get_brightness: Option<DisplayServicesGetBrightnessFn>,
        set_brightness: Option<DisplayServicesSetBrightnessFn>,
        brightness_changed: Option<DisplayServicesBrightnessChangedFn>,
    }

    struct CoreDisplayApi {
        _handle: usize,
        get_user_brightness: Option<CoreDisplayGetUserBrightnessFn>,
        set_user_brightness: Option<CoreDisplaySetUserBrightnessFn>,
    }

    struct IoObjectGuard(IoObject);

    impl IoObjectGuard {
        fn raw(&self) -> IoObject {
            self.0
        }
    }

    impl Drop for IoObjectGuard {
        fn drop(&mut self) {
            if self.0 != 0 {
                // SAFETY: IOObjectRelease accepts any valid io_object_t and
                // must be called once when ownership is held by this process.
                unsafe {
                    let _ = IOObjectRelease(self.0);
                }
            }
        }
    }

    struct CfString(CfStringRef);

    impl CfString {
        fn as_raw(&self) -> CfStringRef {
            self.0
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: CFRelease is required for CoreFoundation create-rule
                // objects and this wrapper owns the reference.
                unsafe {
                    CFRelease(self.0);
                }
            }
        }
    }

    struct CfType(*const c_void);

    impl CfType {
        fn as_raw(&self) -> *const c_void {
            self.0
        }
    }

    impl Drop for CfType {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: CFRelease is required for CoreFoundation create-rule
                // objects and this wrapper owns the reference.
                unsafe {
                    CFRelease(self.0);
                }
            }
        }
    }

    pub fn list_monitors() -> Result<Vec<MonitorSnapshot>> {
        debug!("listing monitors");
        let native_handle = thread::spawn(list_backlight_monitors);
        let ddc_handle = thread::spawn(|| {
            let displays = Display::enumerate();
            debug!(ddc_enumerated = displays.len(), "enumerated DDC displays");
            let mut snapshots = Vec::with_capacity(displays.len());
            for (index, mut display) in displays.into_iter().enumerate() {
                if ddc_display_maps_to_builtin(&display) {
                    let monitor_id = display.info.id.as_str();
                    debug!(
                        ddc_index = index,
                        monitor_id, "skipping DDC display mapped to built-in display"
                    );
                    continue;
                }
                snapshots.push(snapshot_from_ddc_display(&mut display, index));
            }
            debug!(
                ddc_snapshots = snapshots.len(),
                "completed DDC snapshot build"
            );
            snapshots
        });

        let mut snapshots = Vec::new();
        let backlight_result = native_handle
            .join()
            .map_err(|_| anyhow!("Native display detection thread panicked"))?;
        let backlight_error = match backlight_result {
            Ok(mut monitors) => {
                snapshots.append(&mut monitors);
                None
            }
            Err(error) => Some(error),
        };

        let mut ddc_monitors = ddc_handle
            .join()
            .map_err(|_| anyhow!("DDC display detection thread panicked"))?;
        prune_broken_duplicate_ddc_snapshots(&mut ddc_monitors);
        snapshots.append(&mut ddc_monitors);
        ensure_unique_monitor_ids(&mut snapshots);
        debug!(snapshot_count = snapshots.len(), "monitor listing complete");

        if snapshots.is_empty()
            && let Some(error) = backlight_error
        {
            return Err(error);
        }

        Ok(snapshots)
    }

    pub fn set_monitor_feature(
        monitor_id: &str,
        code: &str,
        value: u16,
    ) -> Result<MonitorSnapshot> {
        let monitor_id = canonical_monitor_id(monitor_id);
        let normalized_code = code.to_ascii_uppercase();
        debug!(monitor_id, code, value, "set monitor feature");
        if normalized_code == INPUT_SOURCE_CODE {
            bail!("Input source control is disabled in this app");
        }
        if let Some(display_id) = parse_builtin_monitor_id(monitor_id) {
            return set_builtin_feature(display_id, code, value);
        }

        if parse_backlight_monitor_id(monitor_id).is_some() {
            return set_backlight_feature(monitor_id, code, value);
        }

        let mut display = find_ddc_display(monitor_id)?;

        if normalized_code == BRIGHTNESS_CODE {
            let readback_broken = ddc_readback_broken_reason(monitor_id, BRIGHTNESS_CODE).is_some();
            if readback_broken {
                debug!(
                    monitor_id,
                    "using cached write-only mode for brightness set"
                );
                let clamped = value.min(100);
                set_external_ddc_brightness_write_only(monitor_id, &mut display, clamped)?;
                ddc_shadow_set(monitor_id, BRIGHTNESS_CODE, clamped);
            } else {
                match retry_ddc(
                    &format!("read VCP Brightness (10) for monitor {monitor_id} before set"),
                    || display.handle.get_vcp_feature(LUMINANCE_CODE),
                ) {
                    Ok(current) => {
                        ddc_mark_readable(monitor_id, BRIGHTNESS_CODE);
                        let maximum = current.maximum();
                        if maximum == 0 {
                            bail!("Monitor reported an invalid brightness range");
                        }

                        let clamped = value.min(maximum);
                        set_external_ddc_brightness(&mut display, clamped)?;
                        ddc_shadow_set(monitor_id, BRIGHTNESS_CODE, clamped);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if is_invalid_length_error(&message) {
                            ddc_mark_readback_broken(monitor_id, BRIGHTNESS_CODE, message);
                        }
                        // Some adapters do not support VCP readback, but writes still work.
                        let clamped = value.min(100);
                        set_external_ddc_brightness_write_only(monitor_id, &mut display, clamped)?;
                        ddc_shadow_set(monitor_id, BRIGHTNESS_CODE, clamped);
                    }
                }
            }
        } else {
            if is_rgb_gain_code(normalized_code.as_str()) {
                match set_user_color_preset_if_possible(monitor_id, &mut display) {
                    GainWriteReadiness::Ready
                    | GainWriteReadiness::Unverified
                    | GainWriteReadiness::PresetAliased(_) => {}
                }
            }
            if normalized_code == INPUT_SOURCE_CODE {
                let verified = set_external_ddc_input_source(monitor_id, &mut display, value)?;
                if verified {
                    ddc_shadow_set(monitor_id, normalized_code.as_str(), value);
                } else {
                    debug!(
                        monitor_id,
                        requested_value = value,
                        "input source write is unverified; leaving cached value unchanged"
                    );
                }
            } else {
                let feature_code = parse_ddc_feature_code(normalized_code.as_str())?;
                set_external_ddc_feature(
                    &mut display,
                    feature_code,
                    value,
                    normalized_code.as_str(),
                )?;
                ddc_shadow_set(monitor_id, normalized_code.as_str(), value);
            }
        }

        let ddc_index = parse_ddc_monitor_id(monitor_id)
            .map(|(index, _)| index)
            .unwrap_or(0);
        Ok(snapshot_from_ddc_display_cached(&mut display, ddc_index))
    }

    pub fn transition_monitor_feature(
        monitor_id: &str,
        code: &str,
        value: u16,
        step_delay_ms: u64,
    ) -> Result<MonitorSnapshot> {
        let monitor_id = canonical_monitor_id(monitor_id);
        debug!(
            monitor_id,
            code, value, step_delay_ms, "transition monitor feature"
        );
        let normalized_code = code.to_ascii_uppercase();
        if normalized_code == INPUT_SOURCE_CODE {
            bail!("Input source control is disabled in this app");
        }
        if step_delay_ms == 0 || !is_ddc_range_code(normalized_code.as_str()) {
            return set_monitor_feature(monitor_id, normalized_code.as_str(), value);
        }

        if let Some(display_id) = parse_builtin_monitor_id(monitor_id) {
            if normalized_code != BRIGHTNESS_CODE {
                return set_monitor_feature(monitor_id, normalized_code.as_str(), value);
            }
            let current = percent_from_unit(read_builtin_display_brightness(display_id)?);
            let target = value.min(100);
            ramp_brightness(current, target, step_delay_ms, |next| {
                set_builtin_display_brightness(display_id, next)
            })?;
            return Ok(snapshot_from_builtin_display(display_id));
        }

        if parse_backlight_monitor_id(monitor_id).is_some() {
            if normalized_code != BRIGHTNESS_CODE {
                return set_monitor_feature(monitor_id, normalized_code.as_str(), value);
            }
            return transition_backlight_feature(monitor_id, value, step_delay_ms);
        }

        let mut display = find_ddc_display(monitor_id)?;
        if normalized_code == BRIGHTNESS_CODE {
            let readback_broken = ddc_readback_broken_reason(monitor_id, BRIGHTNESS_CODE).is_some();
            if readback_broken {
                debug!(
                    monitor_id,
                    "using cached write-only mode for brightness transition"
                );
                let target = value.min(100);
                // Write-only links are typically unstable under repeated writes and can
                // visibly blank/flicker. Apply the final value directly.
                set_external_ddc_brightness_write_only(monitor_id, &mut display, target)?;
                ddc_shadow_set(monitor_id, BRIGHTNESS_CODE, target);
            } else {
                match retry_ddc(
                    &format!(
                        "read VCP Brightness (10) for monitor {monitor_id} before transition to {value}"
                    ),
                    || display.handle.get_vcp_feature(LUMINANCE_CODE),
                ) {
                    Ok(current) => {
                        ddc_mark_readable(monitor_id, BRIGHTNESS_CODE);
                        let maximum = current.maximum();
                        if maximum == 0 {
                            bail!("Monitor reported an invalid brightness range");
                        }
                        let target = value.min(maximum);
                        ramp_ddc_brightness(current.value(), target, step_delay_ms, |next| {
                            set_external_ddc_brightness(&mut display, next)
                        })?;
                        ddc_shadow_set(monitor_id, BRIGHTNESS_CODE, target);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if is_invalid_length_error(&message) {
                            ddc_mark_readback_broken(monitor_id, BRIGHTNESS_CODE, message);
                        }
                        let target = value.min(100);
                        // If readback fails, treat the link as write-only and avoid ramping.
                        // Repeated DDC writes are what triggers visible blank/flicker on
                        // unstable bridges.
                        set_external_ddc_brightness_write_only(monitor_id, &mut display, target)?;
                        ddc_shadow_set(monitor_id, BRIGHTNESS_CODE, target);
                    }
                }
            }
        } else {
            transition_external_ddc_range_feature(
                monitor_id,
                &mut display,
                normalized_code.as_str(),
                value,
                step_delay_ms,
            )?;
        }

        let ddc_index = parse_ddc_monitor_id(monitor_id)
            .map(|(index, _)| index)
            .unwrap_or(0);
        Ok(snapshot_from_ddc_display_cached(&mut display, ddc_index))
    }

    pub fn apply_color_scene(monitor_id: &str, scene_id: &str) -> Result<MonitorSnapshot> {
        let monitor_id = canonical_monitor_id(monitor_id);
        debug!(monitor_id, scene_id, "apply color scene requested");
        if parse_builtin_monitor_id(monitor_id).is_some()
            || parse_backlight_monitor_id(monitor_id).is_some()
        {
            bail!("Color scenes are not supported for native macOS displays");
        }

        let profile = color_scene_profile(scene_id)
            .with_context(|| format!("Unknown color scene {scene_id}"))?;
        let mut display = find_ddc_display(monitor_id)?;

        match set_user_color_preset_if_possible(monitor_id, &mut display) {
            GainWriteReadiness::Ready
            | GainWriteReadiness::Unverified
            | GainWriteReadiness::PresetAliased(_) => {}
        }

        let plans = vec![
            prepare_rgb_gain_transition_plan(
                monitor_id,
                &mut display,
                RED_GAIN_CODE,
                profile.red_percent,
            )?,
            prepare_rgb_gain_transition_plan(
                monitor_id,
                &mut display,
                GREEN_GAIN_CODE,
                profile.green_percent,
            )?,
            prepare_rgb_gain_transition_plan(
                monitor_id,
                &mut display,
                BLUE_GAIN_CODE,
                profile.blue_percent,
            )?,
        ];
        drop(display);

        thread::scope(|scope| -> Result<()> {
            let mut handles = Vec::with_capacity(plans.len());
            for plan in plans {
                let monitor_id_owned = monitor_id.to_string();
                handles.push(scope.spawn(move || -> Result<(String, u16)> {
                    apply_rgb_gain_transition_plan(monitor_id_owned.as_str(), &plan)?;
                    Ok((plan.code.clone(), plan.target))
                }));
            }

            let mut first_error: Option<anyhow::Error> = None;
            for handle in handles {
                match handle.join() {
                    Ok(Ok((code, target))) => ddc_shadow_set(monitor_id, code.as_str(), target),
                    Ok(Err(error)) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                    Err(_) => {
                        if first_error.is_none() {
                            first_error = Some(anyhow!("RGB gain transition task panicked"));
                        }
                    }
                }
            }

            if let Some(error) = first_error {
                return Err(error);
            }
            Ok(())
        })?;

        let mut display = find_ddc_display(monitor_id)?;

        let ddc_index = parse_ddc_monitor_id(monitor_id)
            .map(|(index, _)| index)
            .unwrap_or(0);
        Ok(snapshot_from_ddc_display_cached(&mut display, ddc_index))
    }

    fn snapshot_from_ddc_display(display: &mut Display, index: usize) -> MonitorSnapshot {
        let monitor_id = display.info.id.as_str();
        let snapshot_id = ddc_snapshot_monitor_id(index, display);
        let model = format!("{:?}", display.info.model_name);
        let vendor = format!("{:?}", display.info.manufacturer_id);
        debug!(
            ddc_index = index,
            monitor_id, model, vendor, "building DDC snapshot"
        );

        let mut probe_brightness = None;
        let brightness_probe_error = if let Some(reason) =
            ddc_readback_broken_reason(snapshot_id.as_str(), BRIGHTNESS_CODE)
        {
            Some(reason)
        } else {
            let read_probe = retry_ddc(
                &format!(
                    "probe read VCP Brightness (10) for monitor {}",
                    snapshot_id.as_str()
                ),
                || display.handle.get_vcp_feature(LUMINANCE_CODE),
            );
            match read_probe {
                Ok(feature) => {
                    ddc_mark_readable(snapshot_id.as_str(), BRIGHTNESS_CODE);
                    ddc_shadow_set(snapshot_id.as_str(), BRIGHTNESS_CODE, feature.value());
                    probe_brightness = Some((feature.value(), feature.maximum()));
                    debug!(
                        ddc_index = index,
                        monitor_id,
                        value = feature.value(),
                        max = feature.maximum(),
                        "DDC probe read succeeded"
                    );
                    None
                }
                Err(error) => {
                    let message = error.to_string();
                    if is_invalid_length_error(&message) {
                        debug!(
                            ddc_index = index,
                            monitor_id,
                            error = %message,
                            "DDC probe indicates brightness readback is broken; using write-only fallback for brightness"
                        );
                        ddc_mark_readback_broken(
                            snapshot_id.as_str(),
                            BRIGHTNESS_CODE,
                            message.clone(),
                        );
                        Some(message)
                    } else {
                        None
                    }
                }
            }
        };

        let mut controls = Vec::with_capacity(DDC_FEATURE_DEFINITIONS.len());
        let capability_vcp_values =
            read_ddc_capability_vcp_values(snapshot_id.as_str(), display, true).unwrap_or_default();
        let options_for = |code: &str, kind: DdcFeatureKind| {
            resolved_ddc_feature_options(code, kind, capability_vcp_values.get(code))
        };
        let mut unrecoverable_read_failures = 0usize;
        let mut consecutive_transient_parse_failures = 0usize;
        let mut skip_remaining_non_brightness_reads_reason: Option<String> = None;
        for definition in DDC_FEATURE_DEFINITIONS {
            if definition.code == BRIGHTNESS_CODE
                && let Some((value, max)) = probe_brightness
            {
                controls.push(MonitorControl {
                    code: definition.code.to_string(),
                    label: definition.label.to_string(),
                    control_type: control_type_from_kind(definition.kind),
                    current_value: Some(value),
                    max_value: Some(max),
                    options: options_for(definition.code, definition.kind),
                    supported: true,
                    error: None,
                });
                continue;
            }
            if definition.code == BRIGHTNESS_CODE
                && let Some(reason) = brightness_probe_error.as_ref()
            {
                let seeded_value = ddc_shadow_get(snapshot_id.as_str(), definition.code);
                controls.push(MonitorControl {
                    code: definition.code.to_string(),
                    label: definition.label.to_string(),
                    control_type: control_type_from_kind(definition.kind),
                    current_value: seeded_value,
                    max_value: Some(100),
                    options: options_for(definition.code, definition.kind),
                    supported: true,
                    error: Some(format!(
                        "{} readback unavailable ({reason}); write will be attempted.",
                        definition.label
                    )),
                });
                continue;
            }
            if definition.code != BRIGHTNESS_CODE
                && let Some(reason) =
                    ddc_readback_broken_reason(snapshot_id.as_str(), definition.code)
            {
                let seeded_value = ddc_shadow_get(snapshot_id.as_str(), definition.code);
                controls.push(MonitorControl {
                    code: definition.code.to_string(),
                    label: definition.label.to_string(),
                    control_type: control_type_from_kind(definition.kind),
                    current_value: seeded_value,
                    max_value: if matches!(definition.kind, DdcFeatureKind::Range) {
                        Some(100)
                    } else {
                        None
                    },
                    options: options_for(definition.code, definition.kind),
                    supported: true,
                    error: Some(format!(
                        "{} readback unavailable ({reason}); write will be attempted.",
                        definition.label
                    )),
                });
                continue;
            }
            match definition.kind {
                DdcFeatureKind::Action => {
                    controls.push(MonitorControl {
                        code: definition.code.to_string(),
                        label: definition.label.to_string(),
                        control_type: MonitorControlType::Action,
                        current_value: None,
                        max_value: None,
                        options: options_for(definition.code, definition.kind),
                        supported: true,
                        error: None,
                    });
                }
                _ => {
                    if definition.code != BRIGHTNESS_CODE
                        && let Some(reason) = skip_remaining_non_brightness_reads_reason.as_ref()
                    {
                        controls.push(MonitorControl {
                            code: definition.code.to_string(),
                            label: definition.label.to_string(),
                            control_type: control_type_from_kind(definition.kind),
                            current_value: ddc_shadow_get(snapshot_id.as_str(), definition.code),
                            max_value: if matches!(definition.kind, DdcFeatureKind::Range) {
                                Some(100)
                            } else {
                                None
                            },
                            options: options_for(definition.code, definition.kind),
                            supported: true,
                            error: Some(format!(
                                "{} readback skipped ({reason}); write will be attempted.",
                                definition.label
                            )),
                        });
                        continue;
                    }

                    let feature_code = match parse_ddc_feature_code(definition.code) {
                        Ok(code) => code,
                        Err(error) => {
                            controls.push(MonitorControl {
                                code: definition.code.to_string(),
                                label: definition.label.to_string(),
                                control_type: control_type_from_kind(definition.kind),
                                current_value: None,
                                max_value: None,
                                options: options_for(definition.code, definition.kind),
                                supported: false,
                                error: Some(error.to_string()),
                            });
                            continue;
                        }
                    };

                    match retry_ddc(
                        &format!(
                            "read VCP {} ({}) for monitor {}",
                            definition.label,
                            definition.code,
                            snapshot_id.as_str()
                        ),
                        || display.handle.get_vcp_feature(feature_code),
                    ) {
                        Ok(feature) => {
                            let monitor_id = display.info.id.as_str();
                            consecutive_transient_parse_failures = 0;
                            ddc_mark_readable(snapshot_id.as_str(), definition.code);
                            ddc_shadow_set(snapshot_id.as_str(), definition.code, feature.value());
                            debug!(
                                ddc_index = index,
                                monitor_id,
                                code = definition.code,
                                label = definition.label,
                                value = feature.value(),
                                max = feature.maximum(),
                                "DDC feature read"
                            );
                            controls.push(MonitorControl {
                                code: definition.code.to_string(),
                                label: definition.label.to_string(),
                                control_type: control_type_from_kind(definition.kind),
                                current_value: Some(feature.value()),
                                max_value: if matches!(definition.kind, DdcFeatureKind::Range) {
                                    Some(feature.maximum())
                                } else {
                                    None
                                },
                                options: options_for(definition.code, definition.kind),
                                supported: true,
                                error: None,
                            });
                        }
                        Err(error) => {
                            let monitor_id = display.info.id.as_str();
                            let message = error.to_string();
                            if is_transient_ddc_parse_error(&message) {
                                consecutive_transient_parse_failures += 1;
                                if definition.code != BRIGHTNESS_CODE
                                    && consecutive_transient_parse_failures
                                        >= DDC_TRANSIENT_PARSE_FAILURE_THRESHOLD
                                    && skip_remaining_non_brightness_reads_reason.is_none()
                                {
                                    skip_remaining_non_brightness_reads_reason =
                                        Some(String::from("repeated transient DDC parse failures"));
                                    debug!(
                                        ddc_index = index,
                                        monitor_id,
                                        failures = consecutive_transient_parse_failures,
                                        "skipping remaining non-brightness DDC reads after repeated transient parse failures"
                                    );
                                }
                            } else {
                                consecutive_transient_parse_failures = 0;
                            }
                            if is_unrecoverable_ddc_read_error(&message) {
                                ddc_mark_readback_broken(
                                    snapshot_id.as_str(),
                                    definition.code,
                                    message.clone(),
                                );
                                unrecoverable_read_failures += 1;
                                if definition.code != BRIGHTNESS_CODE
                                    && unrecoverable_read_failures
                                        >= DDC_UNRECOVERABLE_READ_FAILURE_THRESHOLD
                                    && skip_remaining_non_brightness_reads_reason.is_none()
                                {
                                    skip_remaining_non_brightness_reads_reason =
                                        Some(message.clone());
                                    debug!(
                                        ddc_index = index,
                                        monitor_id,
                                        failures = unrecoverable_read_failures,
                                        "skipping remaining non-brightness DDC reads after repeated unrecoverable failures"
                                    );
                                }
                            }
                            debug!(
                                ddc_index = index,
                                monitor_id,
                                code = definition.code,
                                label = definition.label,
                                error = %message,
                                "DDC feature read failed"
                            );
                            let current_value =
                                ddc_shadow_get(snapshot_id.as_str(), definition.code);
                            controls.push(MonitorControl {
                                code: definition.code.to_string(),
                                label: definition.label.to_string(),
                                control_type: control_type_from_kind(definition.kind),
                                current_value,
                                max_value: if matches!(definition.kind, DdcFeatureKind::Range) {
                                    Some(100)
                                } else {
                                    None
                                },
                                options: options_for(definition.code, definition.kind),
                                supported: true,
                                error: Some(format!(
                                    "{} readback failed over DDC/CI ({}); writes will still be attempted.",
                                    definition.label
                                    ,
                                    message
                                )),
                            });
                        }
                    }
                }
            }
        }
        if let Some(input_control) = controls
            .iter()
            .find(|control| control.code == INPUT_SOURCE_CODE)
        {
            let option_values: Vec<String> = input_control
                .options
                .iter()
                .map(|option| format!("{:02X}", option.value))
                .collect();
            debug!(
                ddc_index = index,
                monitor_id = snapshot_id.as_str(),
                current_value = ?input_control.current_value,
                option_values = ?option_values,
                "resolved input source options for snapshot"
            );
        }

        let model_name = display
            .info
            .model_name
            .clone()
            .or_else(|| Some(format!("External Display {}", index + 1)));
        let manufacturer_id = display
            .info
            .manufacturer_id
            .clone()
            .or_else(|| ddc_vendor_from_id(&display.info.id));
        let has_supported = controls.iter().any(|control| control.supported);
        let error = if has_supported {
            None
        } else {
            Some(String::from(
                "No usable DDC/CI controls could be read for this display.",
            ))
        };

        MonitorSnapshot {
            id: snapshot_id,
            backend: format!("{:?}", display.info.backend),
            device_path: None,
            connector_name: None,
            manufacturer_id,
            model_name,
            serial_number: display.info.serial_number.clone(),
            error,
            controls,
        }
    }

    fn snapshot_from_ddc_display_cached(display: &mut Display, index: usize) -> MonitorSnapshot {
        let snapshot_id = ddc_snapshot_monitor_id(index, display);
        let mut controls = Vec::with_capacity(DDC_FEATURE_DEFINITIONS.len());
        let capability_vcp_values =
            read_ddc_capability_vcp_values(snapshot_id.as_str(), display, false)
                .unwrap_or_default();
        let options_for = |code: &str, kind: DdcFeatureKind| {
            resolved_ddc_feature_options(code, kind, capability_vcp_values.get(code))
        };
        for definition in DDC_FEATURE_DEFINITIONS {
            if matches!(definition.kind, DdcFeatureKind::Action) {
                controls.push(MonitorControl {
                    code: definition.code.to_string(),
                    label: definition.label.to_string(),
                    control_type: MonitorControlType::Action,
                    current_value: None,
                    max_value: None,
                    options: options_for(definition.code, definition.kind),
                    supported: true,
                    error: None,
                });
                continue;
            }

            let readback_error = ddc_readback_broken_reason(snapshot_id.as_str(), definition.code)
                .map(|reason| {
                    format!(
                        "{} readback unavailable ({reason}); write will be attempted.",
                        definition.label
                    )
                });
            let current_value = ddc_shadow_get(snapshot_id.as_str(), definition.code);
            controls.push(MonitorControl {
                code: definition.code.to_string(),
                label: definition.label.to_string(),
                control_type: control_type_from_kind(definition.kind),
                current_value,
                max_value: if matches!(definition.kind, DdcFeatureKind::Range) {
                    Some(100)
                } else {
                    None
                },
                options: options_for(definition.code, definition.kind),
                supported: true,
                error: readback_error,
            });
        }
        if let Some(input_control) = controls
            .iter()
            .find(|control| control.code == INPUT_SOURCE_CODE)
        {
            let option_values: Vec<String> = input_control
                .options
                .iter()
                .map(|option| format!("{:02X}", option.value))
                .collect();
            debug!(
                ddc_index = index,
                monitor_id = snapshot_id.as_str(),
                current_value = ?input_control.current_value,
                option_values = ?option_values,
                "resolved input source options for cached snapshot"
            );
        }

        let model_name = display
            .info
            .model_name
            .clone()
            .or_else(|| Some(format!("External Display {}", index + 1)));
        let manufacturer_id = display
            .info
            .manufacturer_id
            .clone()
            .or_else(|| ddc_vendor_from_id(&display.info.id));

        MonitorSnapshot {
            id: snapshot_id,
            backend: format!("{:?}", display.info.backend),
            device_path: None,
            connector_name: None,
            manufacturer_id,
            model_name,
            serial_number: display.info.serial_number.clone(),
            error: None,
            controls,
        }
    }

    fn list_backlight_monitors() -> Result<Vec<MonitorSnapshot>> {
        let builtin_ids = native_display_ids()?;
        debug!(
            native_display_ids = builtin_ids.len(),
            "resolved native display ids"
        );
        if !builtin_ids.is_empty() {
            return Ok(builtin_ids
                .into_iter()
                .map(snapshot_from_builtin_display)
                .collect());
        }

        let has_backlight_key = create_cf_string(HAS_BACKLIGHT_KEY)?;
        let mut snapshots = Vec::new();
        let iterator = display_service_iterator()?;
        loop {
            let service = next_io_object(iterator.raw());
            if service == 0 {
                break;
            }
            let service = IoObjectGuard(service);
            if !display_service_has_backlight(service.raw(), &has_backlight_key)? {
                continue;
            }

            let entry_id = registry_entry_id(service.raw())
                .with_context(|| "Failed to resolve macOS backlight display identifier")?;
            snapshots.push(snapshot_from_backlight_service(service.raw(), entry_id));
        }

        if !snapshots.is_empty() {
            debug!(
                native_snapshots = snapshots.len(),
                "collected native displays from IODisplayConnect"
            );
            return Ok(snapshots);
        }

        for class_name in BACKLIGHT_CLASS_NAMES {
            let iterator = match service_iterator(class_name) {
                Ok(iterator) => iterator,
                Err(_) => continue,
            };

            loop {
                let service = next_io_object(iterator.raw());
                if service == 0 {
                    break;
                }
                let service = IoObjectGuard(service);
                let entry_id = registry_entry_id(service.raw())
                    .with_context(|| "Failed to resolve macOS backlight display identifier")?;
                snapshots.push(snapshot_from_backlight_service(service.raw(), entry_id));
            }
        }

        Ok(snapshots)
    }

    fn set_builtin_feature(
        display_id: CgDirectDisplayId,
        code: &str,
        value: u16,
    ) -> Result<MonitorSnapshot> {
        debug!(display_id, code, value, "set built-in feature");
        if !code.eq_ignore_ascii_case(BRIGHTNESS_CODE) {
            bail!("Only brightness is supported for native macOS displays");
        }

        set_builtin_display_brightness(display_id, value.min(100))
            .with_context(|| format!("Failed to set brightness for display {display_id}"))?;

        Ok(snapshot_from_builtin_display(display_id))
    }

    fn set_backlight_feature(monitor_id: &str, code: &str, value: u16) -> Result<MonitorSnapshot> {
        debug!(monitor_id, code, value, "set backlight feature");
        if !code.eq_ignore_ascii_case(BRIGHTNESS_CODE) {
            bail!("Only brightness is supported for native macOS displays");
        }

        let target_entry_id = parse_backlight_monitor_id(monitor_id)
            .with_context(|| format!("Invalid macOS monitor id {monitor_id}"))?;

        let has_backlight_key = create_cf_string(HAS_BACKLIGHT_KEY)?;
        let iterator = display_service_iterator()?;
        loop {
            let service = next_io_object(iterator.raw());
            if service == 0 {
                break;
            }
            let service = IoObjectGuard(service);
            if !display_service_has_backlight(service.raw(), &has_backlight_key)? {
                continue;
            }

            let entry_id = registry_entry_id(service.raw())?;
            if entry_id != target_entry_id {
                continue;
            }

            let clamped = value.min(100);
            set_backlight_brightness(service.raw(), clamped)
                .with_context(|| format!("Failed to set brightness for {monitor_id}"))?;

            return Ok(snapshot_from_backlight_service(service.raw(), entry_id));
        }

        for class_name in BACKLIGHT_CLASS_NAMES {
            let iterator = match service_iterator(class_name) {
                Ok(iterator) => iterator,
                Err(_) => continue,
            };

            loop {
                let service = next_io_object(iterator.raw());
                if service == 0 {
                    break;
                }
                let service = IoObjectGuard(service);
                let entry_id = registry_entry_id(service.raw())?;
                if entry_id != target_entry_id {
                    continue;
                }

                let clamped = value.min(100);
                set_backlight_brightness(service.raw(), clamped)
                    .with_context(|| format!("Failed to set brightness for {monitor_id}"))?;

                return Ok(snapshot_from_backlight_service(service.raw(), entry_id));
            }
        }

        bail!("Monitor {monitor_id} was not found");
    }

    fn transition_backlight_feature(
        monitor_id: &str,
        value: u16,
        step_delay_ms: u64,
    ) -> Result<MonitorSnapshot> {
        debug!(
            monitor_id,
            value, step_delay_ms, "transition backlight feature"
        );
        let target_entry_id = parse_backlight_monitor_id(monitor_id)
            .with_context(|| format!("Invalid macOS monitor id {monitor_id}"))?;
        let target = value.min(100);

        let has_backlight_key = create_cf_string(HAS_BACKLIGHT_KEY)?;
        let iterator = display_service_iterator()?;
        loop {
            let service = next_io_object(iterator.raw());
            if service == 0 {
                break;
            }
            let service = IoObjectGuard(service);
            if !display_service_has_backlight(service.raw(), &has_backlight_key)? {
                continue;
            }

            let entry_id = registry_entry_id(service.raw())?;
            if entry_id != target_entry_id {
                continue;
            }

            let current = percent_from_unit(read_backlight_brightness(service.raw())?);
            ramp_brightness(current, target, step_delay_ms, |next| {
                set_backlight_brightness(service.raw(), next)
            })?;

            return Ok(snapshot_from_backlight_service(service.raw(), entry_id));
        }

        for class_name in BACKLIGHT_CLASS_NAMES {
            let iterator = match service_iterator(class_name) {
                Ok(iterator) => iterator,
                Err(_) => continue,
            };

            loop {
                let service = next_io_object(iterator.raw());
                if service == 0 {
                    break;
                }
                let service = IoObjectGuard(service);
                let entry_id = registry_entry_id(service.raw())?;
                if entry_id != target_entry_id {
                    continue;
                }

                let current = percent_from_unit(read_backlight_brightness(service.raw())?);
                ramp_brightness(current, target, step_delay_ms, |next| {
                    set_backlight_brightness(service.raw(), next)
                })?;

                return Ok(snapshot_from_backlight_service(service.raw(), entry_id));
            }
        }

        bail!("Monitor {monitor_id} was not found");
    }

    fn snapshot_from_backlight_service(service: IoService, entry_id: u64) -> MonitorSnapshot {
        let control = match read_backlight_brightness(service) {
            Ok(value) => MonitorControl {
                code: String::from(BRIGHTNESS_CODE),
                label: String::from("Brightness"),
                control_type: MonitorControlType::Range,
                current_value: Some(percent_from_unit(value)),
                max_value: Some(100),
                options: Vec::new(),
                supported: true,
                error: None,
            },
            Err(error) => MonitorControl {
                code: String::from(BRIGHTNESS_CODE),
                label: String::from("Brightness"),
                control_type: MonitorControlType::Range,
                current_value: None,
                max_value: Some(100),
                options: Vec::new(),
                supported: false,
                error: Some(error.to_string()),
            },
        };

        MonitorSnapshot {
            id: backlight_monitor_id(entry_id),
            backend: String::from("iokit"),
            device_path: None,
            connector_name: None,
            manufacturer_id: Some(String::from("Apple")),
            model_name: Some(String::from("Built-in Display")),
            serial_number: Some(entry_id.to_string()),
            error: if control.supported {
                None
            } else {
                control.error.clone()
            },
            controls: vec![control],
        }
    }

    fn snapshot_from_builtin_display(display_id: CgDirectDisplayId) -> MonitorSnapshot {
        let control = match read_builtin_display_brightness(display_id) {
            Ok(value) => MonitorControl {
                code: String::from(BRIGHTNESS_CODE),
                label: String::from("Brightness"),
                control_type: MonitorControlType::Range,
                current_value: Some(percent_from_unit(value)),
                max_value: Some(100),
                options: Vec::new(),
                supported: true,
                error: None,
            },
            Err(error) => MonitorControl {
                code: String::from(BRIGHTNESS_CODE),
                label: String::from("Brightness"),
                control_type: MonitorControlType::Range,
                current_value: None,
                max_value: Some(100),
                options: Vec::new(),
                supported: false,
                error: Some(error.to_string()),
            },
        };

        let serial = {
            // SAFETY: `display_id` is returned by CoreGraphics display enumeration.
            let value = unsafe { CGDisplaySerialNumber(display_id) };
            if value == 0 {
                None
            } else {
                Some(value.to_string())
            }
        };

        MonitorSnapshot {
            id: builtin_monitor_id(display_id),
            backend: String::from("displayservices"),
            device_path: None,
            connector_name: None,
            manufacturer_id: Some(String::from("Apple")),
            model_name: Some(String::from("Built-in Display")),
            serial_number: serial,
            error: if control.supported {
                None
            } else {
                control.error.clone()
            },
            controls: vec![control],
        }
    }

    fn display_service_iterator() -> Result<IoObjectGuard> {
        service_iterator(DISPLAY_CONNECT_CLASS_NAME)
    }

    fn service_iterator(class_name: &str) -> Result<IoObjectGuard> {
        let class_name =
            CString::new(class_name).map_err(|_| anyhow!("Invalid IOKit class name constant"))?;

        // SAFETY: IOKit expects a NUL-terminated class name and returns a retained dictionary.
        let matching = unsafe { IOServiceMatching(class_name.as_ptr()) };
        if matching.is_null() {
            bail!("IOKit did not return a matching dictionary for {class_name:?}");
        }

        let mut iterator: IoIterator = 0;
        // SAFETY: `matching` originates from IOServiceMatching and `iterator` is a valid out ptr.
        let result = unsafe { IOServiceGetMatchingServices(0, matching, &mut iterator) };
        if result != KERN_SUCCESS {
            // SAFETY: Matching dictionary wasn't consumed on failure.
            unsafe {
                CFRelease(matching.cast());
            }
            bail!(
                "IOServiceGetMatchingServices failed for class {class_name:?} (IOReturn 0x{:08x})",
                result as u32
            );
        }

        Ok(IoObjectGuard(iterator))
    }

    fn online_display_ids() -> Result<Vec<CgDirectDisplayId>> {
        let mut displays = [0_u32; MAX_CG_DISPLAYS];
        let mut count = 0_u32;
        // SAFETY: output buffers are valid and sized for `max_displays`.
        let result = unsafe {
            CGGetOnlineDisplayList(
                MAX_CG_DISPLAYS as u32,
                displays.as_mut_ptr(),
                &mut count as *mut CgDisplayCount,
            )
        };
        if result != CG_DISPLAY_NO_ERR {
            bail!("CGGetOnlineDisplayList failed with error {}", result);
        }

        Ok(displays.into_iter().take(count as usize).collect())
    }

    fn native_display_ids() -> Result<Vec<CgDirectDisplayId>> {
        let mut native = Vec::new();
        for display_id in online_display_ids()? {
            // SAFETY: display_id comes from CoreGraphics online display list.
            let is_builtin = unsafe { CGDisplayIsBuiltin(display_id) } != 0;
            let has_backlight = display_has_backlight(display_id).unwrap_or(false);

            if is_builtin || has_backlight {
                native.push(display_id);
            }
        }

        Ok(native)
    }

    fn display_has_backlight(display_id: CgDirectDisplayId) -> Result<bool> {
        let Some(service) = io_service_for_display(display_id)? else {
            return Ok(false);
        };
        let has_backlight_key = create_cf_string(HAS_BACKLIGHT_KEY)?;
        display_service_has_backlight(service.raw(), &has_backlight_key)
    }

    fn io_service_for_display(display_id: CgDirectDisplayId) -> Result<Option<IoObjectGuard>> {
        // SAFETY: display_id comes from CoreGraphics.
        let vendor = unsafe { CGDisplayVendorNumber(display_id) };
        // SAFETY: display_id comes from CoreGraphics.
        let model = unsafe { CGDisplayModelNumber(display_id) };
        // SAFETY: display_id comes from CoreGraphics.
        let serial = unsafe { CGDisplaySerialNumber(display_id) };
        let vendor_key = create_cf_string(DISPLAY_VENDOR_ID_KEY)?;
        let model_key = create_cf_string(DISPLAY_PRODUCT_ID_KEY)?;
        let serial_key = create_cf_string(DISPLAY_SERIAL_NUMBER_KEY)?;
        let iterator = display_service_iterator()?;

        loop {
            let service = next_io_object(iterator.raw());
            if service == 0 {
                break;
            }

            let service = IoObjectGuard(service);
            // SAFETY: service is a valid io_service_t and this API returns a create-rule dictionary.
            let info = unsafe { IODisplayCreateInfoDictionary(service.raw(), 0) };
            if info.is_null() {
                continue;
            }
            let info = CfType(info);

            let service_vendor = cf_dictionary_u32(info.as_raw(), &vendor_key).unwrap_or(0);
            let service_model = cf_dictionary_u32(info.as_raw(), &model_key).unwrap_or(0);
            let service_serial = cf_dictionary_u32(info.as_raw(), &serial_key).unwrap_or(0);

            if service_vendor == vendor && service_model == model && service_serial == serial {
                return Ok(Some(service));
            }
        }

        Ok(None)
    }

    fn display_service_has_backlight(
        service: IoService,
        has_backlight_key: &CfString,
    ) -> Result<bool> {
        // SAFETY: service is a valid io_service_t and this API returns a create-rule dictionary.
        let info = unsafe { IODisplayCreateInfoDictionary(service, 0) };
        if info.is_null() {
            return Ok(false);
        }
        let info = CfType(info);

        // SAFETY: info is a valid CFDictionary and key is a valid CFString.
        let value = unsafe { CFDictionaryGetValue(info.as_raw(), has_backlight_key.as_raw()) };
        // SAFETY: kCFBooleanTrue is a global constant pointer.
        Ok(value == unsafe { kCFBooleanTrue })
    }

    fn read_backlight_brightness(service: IoService) -> Result<f32> {
        let key = create_cf_string(BRIGHTNESS_KEY)?;
        let mut value = 0.0_f32;
        // SAFETY: `service` is a valid io_service_t from IOKit enumeration and `key` is a valid CFStringRef.
        let result =
            unsafe { IODisplayGetFloatParameter(service, 0, key.as_raw(), &mut value as *mut f32) };
        if result == KERN_SUCCESS {
            debug!(
                service,
                brightness = value,
                "read native backlight brightness via IODisplayGetFloatParameter"
            );
            return Ok(value.clamp(0.0, 1.0));
        }

        read_backlight_brightness_from_parameters(service).with_context(|| {
            format!(
                "Failed to query native display brightness (IOReturn 0x{:08x})",
                result as u32
            )
        })
    }

    fn read_builtin_display_brightness(display_id: CgDirectDisplayId) -> Result<f32> {
        if let Some(api) = display_services_api()
            && let Some(get_brightness) = api.get_brightness
        {
            let mut value = 0.0_f32;
            // SAFETY: `display_id` comes from CoreGraphics and output pointer is valid.
            let result = unsafe { get_brightness(display_id, &mut value as *mut f32) };
            if result == DISPLAY_SERVICES_SUCCESS {
                debug!(
                    display_id,
                    brightness = value,
                    "read built-in brightness via DisplayServices"
                );
                return Ok(value.clamp(0.0, 1.0));
            }
            bail!(
                "DisplayServicesGetBrightness failed for display {} (code {})",
                display_id,
                result
            );
        }

        if let Some(api) = core_display_api()
            && let Some(get_user_brightness) = api.get_user_brightness
        {
            // SAFETY: display_id comes from CoreGraphics.
            let value = unsafe { get_user_brightness(display_id) as f32 };
            debug!(
                display_id,
                brightness = value,
                "read built-in brightness via CoreDisplay"
            );
            return Ok(value.clamp(0.0, 1.0));
        }

        if let Some(service) = io_service_for_display(display_id)? {
            return read_backlight_brightness(service.raw());
        }

        bail!("Native brightness APIs are unavailable for display {display_id}");
    }

    fn set_builtin_display_brightness(display_id: CgDirectDisplayId, percent: u16) -> Result<()> {
        let value = (percent.min(100) as f32) / 100.0;
        debug!(
            display_id,
            percent,
            normalized = value,
            "set built-in brightness requested"
        );

        if let Some(api) = display_services_api()
            && let Some(set_brightness) = api.set_brightness
        {
            // SAFETY: `display_id` comes from CoreGraphics and value is in [0,1].
            let result = unsafe { set_brightness(display_id, value) };
            if result == DISPLAY_SERVICES_SUCCESS {
                return Ok(());
            }
            bail!(
                "DisplayServicesSetBrightness failed for display {} (code {})",
                display_id,
                result
            );
        }

        if let Some(api) = core_display_api()
            && let Some(set_user_brightness) = api.set_user_brightness
        {
            // SAFETY: display_id comes from CoreGraphics and value is normalized.
            unsafe { set_user_brightness(display_id, value as f64) };

            if let Some(brightness_changed) =
                display_services_api().and_then(|it| it.brightness_changed)
            {
                // SAFETY: display_id comes from CoreGraphics and value is normalized.
                unsafe { brightness_changed(display_id, value as f64) };
            }
            return Ok(());
        }

        if let Some(service) = io_service_for_display(display_id)? {
            return set_backlight_brightness(service.raw(), percent);
        }

        bail!("Native brightness APIs are unavailable for display {display_id}");
    }

    fn set_backlight_brightness(service: IoService, percent: u16) -> Result<()> {
        let key = create_cf_string(BRIGHTNESS_KEY)?;
        let value = (percent.min(100) as f32) / 100.0;
        debug!(
            service,
            percent,
            normalized = value,
            "set native backlight brightness requested"
        );
        // SAFETY: `service` and `key` are valid and the brightness range is normalized to [0, 1].
        let result = unsafe { IODisplaySetFloatParameter(service, 0, key.as_raw(), value) };
        if result != KERN_SUCCESS {
            bail!(
                "Failed to write native display brightness (IOReturn 0x{:08x})",
                result as u32
            );
        }

        Ok(())
    }

    fn registry_entry_id(service: IoService) -> Result<u64> {
        let mut entry_id = 0_u64;
        // SAFETY: `service` is valid and `entry_id` out-pointer is valid for writes.
        let result =
            unsafe { IORegistryEntryGetRegistryEntryID(service, &mut entry_id as *mut u64) };
        if result != KERN_SUCCESS {
            bail!(
                "IORegistryEntryGetRegistryEntryID failed (IOReturn 0x{:08x})",
                result as u32
            );
        }

        Ok(entry_id)
    }

    fn next_io_object(iterator: IoObject) -> IoObject {
        // SAFETY: iterator is created by IOServiceGetMatchingServices and valid during enumeration.
        unsafe { IOIteratorNext(iterator) }
    }

    fn create_cf_string(value: &str) -> Result<CfString> {
        let value =
            CString::new(value).map_err(|_| anyhow!("CoreFoundation string contains NUL byte"))?;
        // SAFETY: Passing a valid UTF-8 C string and default allocator.
        let cf_string = unsafe {
            CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), CF_STRING_ENCODING_UTF8)
        };
        if cf_string.is_null() {
            bail!("Failed to create CoreFoundation string for display parameter");
        }

        Ok(CfString(cf_string))
    }

    fn backlight_monitor_id(entry_id: u64) -> String {
        format!("macos-backlight:{entry_id}")
    }

    fn ddc_snapshot_monitor_id(index: usize, display: &Display) -> String {
        format!("{DDC_MONITOR_PREFIX}{index}:{}", display.info.id)
    }

    fn parse_ddc_monitor_id(monitor_id: &str) -> Option<(usize, Option<&str>)> {
        let remainder = monitor_id.strip_prefix(DDC_MONITOR_PREFIX)?;
        let (index, raw_id) = remainder.split_once(':')?;
        let index = index.parse::<usize>().ok()?;
        Some((index, Some(raw_id)))
    }

    fn ddc_raw_monitor_id(monitor_id: &str) -> Option<&str> {
        let remainder = monitor_id.strip_prefix(DDC_MONITOR_PREFIX)?;
        if let Some((index, raw_id)) = remainder.split_once(':')
            && index.parse::<usize>().is_ok()
        {
            return Some(raw_id);
        }
        Some(remainder)
    }

    fn find_ddc_display(monitor_id: &str) -> Result<Display> {
        let displays = Display::enumerate();
        if let Some((index, raw_hint)) = parse_ddc_monitor_id(monitor_id)
            && let Some(display) = displays.into_iter().nth(index)
            && raw_hint.is_none_or(|raw| display.info.id == raw)
        {
            return Ok(display);
        }

        let raw_id = ddc_raw_monitor_id(monitor_id).unwrap_or(monitor_id);
        Display::enumerate()
            .into_iter()
            .find(|display| display.info.id == raw_id)
            .with_context(|| format!("Monitor {monitor_id} was not found"))
    }

    fn ddc_display_maps_to_builtin(display: &Display) -> bool {
        let Some(display_id) = parse_builtin_monitor_id(display.info.id.as_str()) else {
            return false;
        };

        // SAFETY: display_id came from parsed monitor ID string from backend enumeration.
        unsafe { CGDisplayIsBuiltin(display_id) != 0 }
    }

    fn prune_broken_duplicate_ddc_snapshots(snapshots: &mut Vec<MonitorSnapshot>) {
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, snapshot) in snapshots.iter().enumerate() {
            let Some(raw_id) = ddc_raw_monitor_id(snapshot.id.as_str()) else {
                continue;
            };
            groups.entry(raw_id.to_string()).or_default().push(index);
        }

        let mut drop = vec![false; snapshots.len()];
        for (raw_id, indexes) in groups {
            if indexes.len() < 2 {
                continue;
            }

            let has_healthy_path = indexes
                .iter()
                .copied()
                .any(|index| ddc_snapshot_has_brightness_readback(&snapshots[index]));
            if !has_healthy_path {
                continue;
            }

            for index in indexes {
                if ddc_snapshot_has_any_readback(&snapshots[index]) {
                    continue;
                }
                drop[index] = true;
                debug!(
                    raw_id,
                    snapshot_id = snapshots[index].id.as_str(),
                    "dropping duplicate DDC snapshot with no readable controls"
                );
            }
        }

        if !drop.iter().any(|flag| *flag) {
            return;
        }

        let mut deduped = Vec::with_capacity(snapshots.len());
        for (index, snapshot) in snapshots.drain(..).enumerate() {
            if !drop[index] {
                deduped.push(snapshot);
            }
        }
        *snapshots = deduped;
    }

    fn ddc_snapshot_has_any_readback(snapshot: &MonitorSnapshot) -> bool {
        snapshot
            .controls
            .iter()
            .any(|control| control.current_value.is_some())
    }

    fn ddc_snapshot_has_brightness_readback(snapshot: &MonitorSnapshot) -> bool {
        snapshot
            .controls
            .iter()
            .find(|control| control.code.eq_ignore_ascii_case(BRIGHTNESS_CODE))
            .and_then(|control| control.current_value)
            .is_some()
    }

    fn parse_ddc_feature_code(code: &str) -> Result<u8> {
        u8::from_str_radix(code, 16).with_context(|| format!("Unsupported VCP feature code {code}"))
    }

    fn control_type_from_kind(kind: DdcFeatureKind) -> MonitorControlType {
        match kind {
            DdcFeatureKind::Range => MonitorControlType::Range,
            DdcFeatureKind::Choice => MonitorControlType::Choice,
            DdcFeatureKind::Toggle => MonitorControlType::Toggle,
            DdcFeatureKind::Action => MonitorControlType::Action,
        }
    }

    fn ddc_feature_options(code: &str, kind: DdcFeatureKind) -> Vec<shared::ControlOption> {
        match code {
            "8D" => vec![
                shared::ControlOption {
                    value: 0x02,
                    label: String::from("Unmuted"),
                },
                shared::ControlOption {
                    value: 0x01,
                    label: String::from("Muted"),
                },
            ],
            "D6" => vec![
                shared::ControlOption {
                    value: 0x01,
                    label: String::from("On"),
                },
                shared::ControlOption {
                    value: 0x02,
                    label: String::from("Standby"),
                },
                shared::ControlOption {
                    value: 0x03,
                    label: String::from("Suspend"),
                },
                shared::ControlOption {
                    value: 0x04,
                    label: String::from("Off"),
                },
                shared::ControlOption {
                    value: 0x05,
                    label: String::from("Turn Off"),
                },
            ],
            "14" => vec![
                shared::ControlOption {
                    value: 0x01,
                    label: String::from("sRGB"),
                },
                shared::ControlOption {
                    value: 0x04,
                    label: String::from("5000K"),
                },
                shared::ControlOption {
                    value: 0x05,
                    label: String::from("6500K"),
                },
                shared::ControlOption {
                    value: 0x08,
                    label: String::from("9300K"),
                },
                shared::ControlOption {
                    value: 0x0B,
                    label: String::from("User"),
                },
            ],
            "60" => vec![
                shared::ControlOption {
                    value: 0x01,
                    label: String::from("VGA-1"),
                },
                shared::ControlOption {
                    value: 0x03,
                    label: String::from("DVI-1"),
                },
                shared::ControlOption {
                    value: 0x0F,
                    label: String::from("DisplayPort-1"),
                },
                shared::ControlOption {
                    value: 0x10,
                    label: String::from("DisplayPort-2"),
                },
                shared::ControlOption {
                    value: 0x11,
                    label: String::from("HDMI-1"),
                },
                shared::ControlOption {
                    value: 0x12,
                    label: String::from("HDMI-2"),
                },
                shared::ControlOption {
                    value: 0x1B,
                    label: String::from("USB-C"),
                },
            ],
            "CA" => vec![
                shared::ControlOption {
                    value: 0x01,
                    label: String::from("Disabled"),
                },
                shared::ControlOption {
                    value: 0x02,
                    label: String::from("Enabled"),
                },
            ],
            "CC" => vec![
                shared::ControlOption {
                    value: 0x02,
                    label: String::from("English"),
                },
                shared::ControlOption {
                    value: 0x03,
                    label: String::from("French"),
                },
                shared::ControlOption {
                    value: 0x04,
                    label: String::from("German"),
                },
                shared::ControlOption {
                    value: 0x05,
                    label: String::from("Spanish"),
                },
                shared::ControlOption {
                    value: 0x06,
                    label: String::from("Italian"),
                },
            ],
            "04" | "05" | "08" if matches!(kind, DdcFeatureKind::Action) => {
                vec![shared::ControlOption {
                    value: 0x01,
                    label: String::from("Apply"),
                }]
            }
            _ => Vec::new(),
        }
    }

    fn ddc_vendor_from_id(id: &str) -> Option<String> {
        let mut parts = id.split_whitespace();
        let vendor = parts.next()?;
        if vendor.is_empty() {
            None
        } else {
            Some(vendor.to_string())
        }
    }

    fn read_ddc_capability_vcp_values(
        monitor_id: &str,
        display: &mut Display,
        allow_probe: bool,
    ) -> Option<HashMap<String, Vec<u16>>> {
        if let Some(cached) = ddc_capability_get(monitor_id) {
            return cached;
        }
        if !allow_probe {
            return None;
        }

        let capabilities = retry_ddc(
            &format!("read MCCS capabilities for monitor {monitor_id}"),
            || display.handle.capabilities_string(),
        )
        .ok();
        let Some(capabilities) = capabilities else {
            ddc_capability_set(monitor_id, None);
            return None;
        };

        let capabilities_text = String::from_utf8_lossy(capabilities.as_slice());
        let vcp_values = parse_ddc_capability_vcp_values(capabilities_text.as_ref());
        if let Some(input_values) = vcp_values.get(INPUT_SOURCE_CODE) {
            debug!(
                monitor_id,
                values = ?input_values,
                "parsed input source values from MCCS capabilities"
            );
        }
        if vcp_values.is_empty() {
            debug!(
                monitor_id,
                "monitor did not expose parseable VCP capability values; using fallback options"
            );
            ddc_capability_set(monitor_id, None);
            return None;
        }

        ddc_capability_set(monitor_id, Some(vcp_values.clone()));
        Some(vcp_values)
    }

    fn parse_ddc_capability_vcp_values(capabilities: &str) -> HashMap<String, Vec<u16>> {
        let mut features = HashMap::<String, Vec<u16>>::new();
        let Some(vcp_section) = extract_capability_group(capabilities, "vcp") else {
            return features;
        };

        let bytes = vcp_section.as_bytes();
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            while cursor < bytes.len() && !bytes[cursor].is_ascii_hexdigit() {
                cursor += 1;
            }
            if cursor + 1 >= bytes.len() {
                break;
            }
            if !bytes[cursor + 1].is_ascii_hexdigit() {
                cursor += 1;
                continue;
            }

            let code = vcp_section[cursor..cursor + 2].to_ascii_uppercase();
            cursor += 2;

            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }

            if cursor >= bytes.len() || bytes[cursor] != b'(' {
                features.entry(code).or_default();
                continue;
            }

            let Some(group_end) = find_matching_parenthesis(vcp_section, cursor) else {
                break;
            };
            let raw_values = &vcp_section[cursor + 1..group_end];
            let mut values = Vec::new();
            for token in raw_values
                .split(|ch: char| !ch.is_ascii_hexdigit())
                .filter(|token| !token.is_empty())
            {
                if let Ok(value) = u16::from_str_radix(token, 16)
                    && !values.contains(&value)
                {
                    values.push(value);
                }
            }
            features.insert(code, values);
            cursor = group_end + 1;
        }

        features
    }

    fn extract_capability_group<'a>(capabilities: &'a str, group: &str) -> Option<&'a str> {
        let marker = format!("{group}(");
        let start = capabilities.find(marker.as_str())?;
        let open_index = start + group.len();
        let close_index = find_matching_parenthesis(capabilities, open_index)?;
        Some(&capabilities[open_index + 1..close_index])
    }

    fn find_matching_parenthesis(text: &str, open_index: usize) -> Option<usize> {
        let bytes = text.as_bytes();
        if bytes.get(open_index) != Some(&b'(') {
            return None;
        }

        let mut depth = 0usize;
        for (index, byte) in bytes.iter().enumerate().skip(open_index) {
            if *byte == b'(' {
                depth += 1;
                continue;
            }
            if *byte == b')' {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
        }

        None
    }

    fn resolved_ddc_feature_options(
        code: &str,
        kind: DdcFeatureKind,
        capability_values: Option<&Vec<u16>>,
    ) -> Vec<shared::ControlOption> {
        let fallback = ddc_feature_options(code, kind);
        if code == INPUT_SOURCE_CODE {
            return merged_input_source_options(capability_values, &fallback);
        }

        let Some(capability_values) = capability_values else {
            return fallback;
        };
        if capability_values.is_empty() {
            return fallback;
        }

        let mut resolved = Vec::with_capacity(capability_values.len());
        for value in capability_values {
            let label = if code == INPUT_SOURCE_CODE {
                // Monitors often repurpose MCCS input codes; avoid misleading fixed names.
                format!("Input 0x{value:02X}")
            } else {
                fallback
                    .iter()
                    .find(|option| option.value == *value)
                    .map(|option| option.label.clone())
                    .unwrap_or_else(|| format!("0x{value:02X}"))
            };
            resolved.push(shared::ControlOption {
                value: *value,
                label,
            });
        }

        if resolved.is_empty() {
            fallback
        } else {
            resolved
        }
    }

    fn merged_input_source_options(
        capability_values: Option<&Vec<u16>>,
        fallback: &[shared::ControlOption],
    ) -> Vec<shared::ControlOption> {
        if let Some(capability_values) = capability_values
            && !capability_values.is_empty()
        {
            let capability_max = *capability_values.iter().max().unwrap_or(&0);
            // Some monitors report sparse capability values like [01, 03] while runtime
            // readback shows max=3. Treat this as a compact enumerated domain 1..=max.
            if capability_max <= 4
                && capability_values
                    .iter()
                    .all(|value| (1..=capability_max).contains(value))
            {
                return (1..=capability_max)
                    .map(|value| shared::ControlOption {
                        value,
                        label: format!("Input 0x{value:02X}"),
                    })
                    .collect();
            }
        }

        // Capabilities are frequently incomplete/wrong for VCP 60. Include known/common
        // source codes so users can test the real panel mapping directly.
        const COMMON_INPUT_CODES: &[u16] = &[
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x1B,
        ];

        let mut values = Vec::<u16>::new();
        if let Some(capability_values) = capability_values {
            for value in capability_values {
                if !values.contains(value) {
                    values.push(*value);
                }
            }
        }
        for option in fallback {
            if !values.contains(&option.value) {
                values.push(option.value);
            }
        }
        for value in COMMON_INPUT_CODES {
            if !values.contains(value) {
                values.push(*value);
            }
        }

        values
            .into_iter()
            .map(|value| shared::ControlOption {
                value,
                label: format!("Input 0x{value:02X}"),
            })
            .collect()
    }

    fn set_external_ddc_brightness(display: &mut Display, value: u16) -> Result<()> {
        let monitor_id = display.info.id.as_str();
        debug!(monitor_id, value, "set external DDC brightness requested");
        set_external_ddc_feature(display, LUMINANCE_CODE, value, BRIGHTNESS_CODE)
    }

    fn set_external_ddc_brightness_write_only(
        monitor_id: &str,
        display: &mut Display,
        value: u16,
    ) -> Result<()> {
        debug!(
            monitor_id,
            value, "set external DDC brightness in write-only mode"
        );
        set_external_ddc_feature(display, LUMINANCE_CODE, value, BRIGHTNESS_CODE)
    }

    fn set_external_ddc_feature(
        display: &mut Display,
        code: u8,
        value: u16,
        label: &str,
    ) -> Result<()> {
        let monitor_id = display.info.id.as_str();
        let code_hex = format!("{code:02X}");
        debug!(
            monitor_id,
            code = code_hex.as_str(),
            label,
            value,
            "set external DDC feature requested"
        );
        let action =
            format!("write VCP {label} ({code_hex}) for monitor {monitor_id} with value {value}");
        let result = retry_ddc(&action, || display.handle.set_vcp_feature(code, value));
        if result.is_ok() {
            debug!(
                monitor_id,
                code = code_hex.as_str(),
                label,
                value,
                "set external DDC feature completed"
            );
        }
        result
    }

    fn set_external_ddc_input_source(
        monitor_id: &str,
        display: &mut Display,
        value: u16,
    ) -> Result<bool> {
        let feature_code = parse_ddc_feature_code(INPUT_SOURCE_CODE)?;
        set_external_ddc_feature(display, feature_code, value, INPUT_SOURCE_CODE)?;
        thread::sleep(Duration::from_millis(INPUT_SOURCE_SETTLE_MS));

        match retry_ddc(
            &format!("read VCP {INPUT_SOURCE_CODE} for monitor {monitor_id} after input write"),
            || display.handle.get_vcp_feature(feature_code),
        ) {
            Ok(feature) => {
                ddc_mark_readable(monitor_id, INPUT_SOURCE_CODE);
                let observed = feature.value();
                if observed == value {
                    return Ok(true);
                }

                debug!(
                    monitor_id,
                    requested_value = value,
                    observed_value = observed,
                    "input source write did not match readback"
                );
                // Samsung and some MST/bridge paths apply the source switch but keep
                // reporting the previous source immediately after the write.
                return Ok(false);
            }
            Err(error) => {
                let message = error.to_string();
                if is_unrecoverable_ddc_read_error(&message) {
                    ddc_mark_readback_broken(monitor_id, INPUT_SOURCE_CODE, message);
                }
                // Switching input can make immediate readback unavailable on some links.
                return Ok(false);
            }
        }
    }

    fn set_user_color_preset_if_possible(
        monitor_id: &str,
        display: &mut Display,
    ) -> GainWriteReadiness {
        let preset_code = match parse_ddc_feature_code(COLOR_PRESET_CODE) {
            Ok(code) => code,
            Err(error) => {
                debug!(
                    monitor_id,
                    error = %error,
                    "failed to resolve color preset VCP code before gain write"
                );
                return GainWriteReadiness::Unverified;
            }
        };

        if let Err(error) = set_external_ddc_feature(
            display,
            preset_code,
            COLOR_PRESET_USER_1_VALUE,
            COLOR_PRESET_CODE,
        ) {
            debug!(
                monitor_id,
                error = %error,
                "failed to switch display to User 1 preset before gain write"
            );
            return GainWriteReadiness::Unverified;
        }

        ddc_shadow_set(monitor_id, COLOR_PRESET_CODE, COLOR_PRESET_USER_1_VALUE);
        thread::sleep(Duration::from_millis(COLOR_PRESET_SETTLE_MS));

        let current_preset = retry_ddc(
            &format!("read VCP {COLOR_PRESET_CODE} for monitor {monitor_id} after preset switch"),
            || display.handle.get_vcp_feature(preset_code),
        );
        let Ok(current_preset) = current_preset else {
            return GainWriteReadiness::Unverified;
        };

        if current_preset.value() == COLOR_PRESET_USER_1_VALUE {
            return GainWriteReadiness::Ready;
        }

        debug!(
            monitor_id,
            expected = COLOR_PRESET_USER_1_VALUE,
            actual = current_preset.value(),
            "display did not keep User 1 preset; continuing with reported preset"
        );

        ddc_shadow_set(monitor_id, COLOR_PRESET_CODE, current_preset.value());
        GainWriteReadiness::PresetAliased(current_preset.value())
    }

    fn prepare_rgb_gain_transition_plan(
        monitor_id: &str,
        display: &mut Display,
        code: &str,
        percent: u8,
    ) -> Result<RgbGainTransitionPlan> {
        let feature_code = parse_ddc_feature_code(code)?;
        let (current_value, maximum) = match retry_ddc(
            &format!("read VCP {code} for monitor {monitor_id} before warm scene apply"),
            || display.handle.get_vcp_feature(feature_code),
        ) {
            Ok(current) => {
                ddc_mark_readable(monitor_id, code);
                (Some(current.value()), current.maximum().max(1))
            }
            Err(error) => {
                let message = error.to_string();
                if is_invalid_length_error(&message) {
                    ddc_mark_readback_broken(monitor_id, code, message);
                }
                debug!(
                    monitor_id,
                    code,
                    fallback_max = RGB_GAIN_FALLBACK_MAX,
                    "falling back to default RGB gain maximum after read failure"
                );
                (ddc_shadow_get(monitor_id, code), RGB_GAIN_FALLBACK_MAX)
            }
        };

        let clamped_percent = percent.min(100) as u32;
        let target = (((maximum as u32) * clamped_percent) + 50) / 100;
        let value = target.min(maximum as u32) as u16;
        let current = current_value.unwrap_or(value).min(maximum);
        Ok(RgbGainTransitionPlan {
            code: code.to_string(),
            feature_code,
            current,
            target: value,
        })
    }

    fn apply_rgb_gain_transition_plan(
        monitor_id: &str,
        plan: &RgbGainTransitionPlan,
    ) -> Result<()> {
        let mut display = find_ddc_display(monitor_id)?;
        if plan.current == plan.target {
            set_external_ddc_feature(
                &mut display,
                plan.feature_code,
                plan.target,
                plan.code.as_str(),
            )
        } else {
            ramp_ddc_brightness(
                plan.current,
                plan.target,
                COLOR_SCENE_GAIN_STEP_DELAY_MS,
                |next| {
                    set_external_ddc_feature(
                        &mut display,
                        plan.feature_code,
                        next,
                        plan.code.as_str(),
                    )
                },
            )
        }
    }

    fn transition_external_ddc_range_feature(
        monitor_id: &str,
        display: &mut Display,
        code: &str,
        value: u16,
        step_delay_ms: u64,
    ) -> Result<()> {
        if is_rgb_gain_code(code) {
            match set_user_color_preset_if_possible(monitor_id, display) {
                GainWriteReadiness::Ready
                | GainWriteReadiness::Unverified
                | GainWriteReadiness::PresetAliased(_) => {}
            }
        }

        let feature_code = parse_ddc_feature_code(code)?;
        let (current_value, maximum) = match retry_ddc(
            &format!("read VCP {code} for monitor {monitor_id} before transition to {value}"),
            || display.handle.get_vcp_feature(feature_code),
        ) {
            Ok(current) => {
                ddc_mark_readable(monitor_id, code);
                (Some(current.value()), current.maximum().max(1))
            }
            Err(error) => {
                let message = error.to_string();
                if is_unrecoverable_ddc_read_error(&message) {
                    ddc_mark_readback_broken(monitor_id, code, message);
                }
                let fallback_max = if is_rgb_gain_code(code) {
                    RGB_GAIN_FALLBACK_MAX
                } else {
                    100
                };
                (ddc_shadow_get(monitor_id, code), fallback_max.max(1))
            }
        };

        let target = value.min(maximum);
        let current = current_value.unwrap_or(target).min(maximum);
        if current == target {
            set_external_ddc_feature(display, feature_code, target, code)?;
        } else {
            ramp_ddc_brightness(current, target, step_delay_ms, |next| {
                set_external_ddc_feature(display, feature_code, next, code)
            })?;
        }
        ddc_shadow_set(monitor_id, code, target);

        Ok(())
    }

    fn color_scene_profile(scene_id: &str) -> Option<ColorSceneProfile> {
        match scene_id {
            "paper" => Some(ColorSceneProfile {
                red_percent: 94,
                green_percent: 92,
                blue_percent: 88,
            }),
            "sunset" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 72,
                blue_percent: 46,
            }),
            "ember" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 62,
                blue_percent: 32,
            }),
            "incandescent" => Some(ColorSceneProfile {
                red_percent: 96,
                green_percent: 68,
                blue_percent: 38,
            }),
            "candle" => Some(ColorSceneProfile {
                red_percent: 92,
                green_percent: 56,
                blue_percent: 24,
            }),
            "nocturne" => Some(ColorSceneProfile {
                red_percent: 84,
                green_percent: 46,
                blue_percent: 14,
            }),
            _ => None,
        }
    }

    fn is_rgb_gain_code(code: &str) -> bool {
        matches!(code, RED_GAIN_CODE | GREEN_GAIN_CODE | BLUE_GAIN_CODE)
    }

    fn is_ddc_range_code(code: &str) -> bool {
        DDC_FEATURE_DEFINITIONS.iter().any(|definition| {
            definition.code.eq_ignore_ascii_case(code)
                && matches!(definition.kind, DdcFeatureKind::Range)
        })
    }

    fn retry_ddc<T, E, F>(action: &str, mut op: F) -> Result<T>
    where
        E: std::fmt::Display,
        F: FnMut() -> std::result::Result<T, E>,
    {
        let ddc_lock = ddc_io_lock().lock().ok();
        let is_read = action.contains(" read ")
            || action.starts_with("read ")
            || action.starts_with("probe read ");
        let is_scene_gain_probe = action.contains("before warm scene apply");
        let attempts_limit = if is_read {
            if is_scene_gain_probe {
                1
            } else {
                DDC_READ_RETRY_ATTEMPTS
            }
        } else {
            DDC_WRITE_RETRY_ATTEMPTS
        };
        let retry_delay_ms = if is_read {
            DDC_READ_RETRY_DELAY_MS
        } else {
            DDC_WRITE_RETRY_DELAY_MS
        };
        let mut last_error = String::from("unknown DDC error");
        let mut attempts_made = 0usize;
        for attempt in 0..attempts_limit {
            attempts_made = attempt + 1;
            ddc_wait_before_transaction();
            match op() {
                Ok(value) => {
                    ddc_mark_transaction_end();
                    return Ok(value);
                }
                Err(error) => {
                    ddc_mark_transaction_end();
                    last_error = error.to_string();
                    debug!(
                        action,
                        attempt = attempt + 1,
                        attempts = attempts_limit,
                        error = %last_error,
                        "DDC attempt failed"
                    );
                    if is_read && is_unrecoverable_ddc_read_error(&last_error) {
                        break;
                    }
                    if attempt + 1 < attempts_limit {
                        thread::sleep(Duration::from_millis(retry_delay_ms));
                    }
                }
            }
        }

        drop(ddc_lock);
        bail!("{action} failed after {attempts_made} attempts: {last_error}")
    }

    fn ddc_wait_before_transaction() {
        let minimum_gap = Duration::from_millis(DDC_MIN_TRANSACTION_GAP_MS);
        let Some(previous_end) = ddc_last_transaction_end()
            .lock()
            .ok()
            .and_then(|state| *state)
        else {
            return;
        };

        let elapsed = previous_end.elapsed();
        if elapsed < minimum_gap {
            thread::sleep(minimum_gap - elapsed);
        }
    }

    fn ddc_mark_transaction_end() {
        if let Ok(mut state) = ddc_last_transaction_end().lock() {
            *state = Some(Instant::now());
        }
    }

    fn is_invalid_length_error(message: &str) -> bool {
        message
            .to_ascii_lowercase()
            .contains("invalid ddc/ci length")
    }

    fn is_transient_ddc_parse_error(message: &str) -> bool {
        let lower = message.to_ascii_lowercase();
        lower.contains("unable to parse ddc/ci response payload")
            || lower.contains("invalid ddc/ci frame")
    }

    fn is_unsupported_vcp_error(message: &str) -> bool {
        let lower = message.to_ascii_lowercase();
        lower.contains("unsupported vcp")
            || lower.contains("unsupported transaction type")
            || lower.contains("unsupported command")
    }

    fn is_unrecoverable_ddc_read_error(message: &str) -> bool {
        is_invalid_length_error(message) || is_unsupported_vcp_error(message)
    }

    fn ddc_shadow_state() -> &'static Mutex<DdcShadowState> {
        static DDC_SHADOW_STATE: OnceLock<Mutex<DdcShadowState>> = OnceLock::new();
        DDC_SHADOW_STATE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn ddc_io_lock() -> &'static Mutex<()> {
        static DDC_IO_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        DDC_IO_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn ddc_last_transaction_end() -> &'static Mutex<Option<Instant>> {
        static DDC_LAST_TRANSACTION_END: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
        DDC_LAST_TRANSACTION_END.get_or_init(|| Mutex::new(None))
    }

    fn ddc_readback_state() -> &'static Mutex<DdcReadbackState> {
        static DDC_READBACK_STATE: OnceLock<Mutex<DdcReadbackState>> = OnceLock::new();
        DDC_READBACK_STATE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn ddc_capability_state() -> &'static Mutex<DdcCapabilityState> {
        static DDC_CAPABILITY_STATE: OnceLock<Mutex<DdcCapabilityState>> = OnceLock::new();
        DDC_CAPABILITY_STATE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn ddc_capability_get(monitor_id: &str) -> Option<Option<HashMap<String, Vec<u16>>>> {
        ddc_capability_state()
            .lock()
            .ok()
            .and_then(|state| state.get(monitor_id).cloned())
    }

    fn ddc_capability_set(monitor_id: &str, value: Option<HashMap<String, Vec<u16>>>) {
        if let Ok(mut state) = ddc_capability_state().lock() {
            state.insert(monitor_id.to_string(), value);
        }
    }

    fn ddc_mark_readback_broken(monitor_id: &str, code: &str, reason: String) {
        let normalized_code = code.to_ascii_uppercase();
        if let Ok(mut state) = ddc_readback_state().lock() {
            state
                .entry(monitor_id.to_string())
                .or_default()
                .insert(normalized_code, DdcReadbackHealth::Broken(reason));
        }
    }

    fn ddc_mark_readable(monitor_id: &str, code: &str) {
        let normalized_code = code.to_ascii_uppercase();
        if let Ok(mut state) = ddc_readback_state().lock() {
            state
                .entry(monitor_id.to_string())
                .or_default()
                .insert(normalized_code, DdcReadbackHealth::Readable);
        }
    }

    fn ddc_readback_broken_reason(monitor_id: &str, code: &str) -> Option<String> {
        let normalized_code = code.to_ascii_uppercase();
        ddc_readback_state().lock().ok().and_then(|state| {
            state
                .get(monitor_id)
                .and_then(|codes| codes.get(normalized_code.as_str()))
                .and_then(|health| match health {
                    DdcReadbackHealth::Broken(reason) => Some(reason.clone()),
                    DdcReadbackHealth::Readable => None,
                })
        })
    }

    fn ddc_shadow_get(monitor_id: &str, code: &str) -> Option<u16> {
        let normalized_code = code.to_ascii_uppercase();
        ddc_shadow_state().lock().ok().and_then(|state| {
            state
                .get(monitor_id)
                .and_then(|controls| controls.get(normalized_code.as_str()).copied())
        })
    }

    fn ddc_shadow_set(monitor_id: &str, code: &str, value: u16) {
        let normalized_code = code.to_ascii_uppercase();
        if let Ok(mut state) = ddc_shadow_state().lock() {
            state
                .entry(monitor_id.to_string())
                .or_default()
                .insert(normalized_code, value);
        }
    }

    fn ensure_unique_monitor_ids(snapshots: &mut [MonitorSnapshot]) {
        let mut seen: HashMap<String, usize> = HashMap::new();
        for snapshot in snapshots.iter_mut() {
            let count = seen.entry(snapshot.id.clone()).or_insert(0);
            if *count > 0 {
                snapshot.id = format!("{}@dup{}", snapshot.id, *count);
            }
            *count += 1;
        }
    }

    fn canonical_monitor_id(monitor_id: &str) -> &str {
        if let Some((base, suffix)) = monitor_id.rsplit_once("@dup")
            && !suffix.is_empty()
            && suffix.chars().all(|char| char.is_ascii_digit())
        {
            return base;
        }

        monitor_id
    }

    fn parse_backlight_monitor_id(monitor_id: &str) -> Option<u64> {
        monitor_id
            .strip_prefix("macos-backlight:")
            .and_then(|value| value.parse::<u64>().ok())
    }

    fn parse_builtin_monitor_id(monitor_id: &str) -> Option<CgDirectDisplayId> {
        monitor_id
            .strip_prefix("macos-display:")
            .and_then(|value| value.parse::<CgDirectDisplayId>().ok())
    }

    fn builtin_monitor_id(display_id: CgDirectDisplayId) -> String {
        format!("macos-display:{display_id}")
    }

    fn read_backlight_brightness_from_parameters(service: IoService) -> Result<f32> {
        let parameters_key = create_cf_string(DISPLAY_PARAMETERS_KEY)?;
        let brightness_key = create_cf_string(BRIGHTNESS_KEY)?;
        let value_key = create_cf_string(VALUE_KEY)?;
        let min_key = create_cf_string(MIN_KEY)?;
        let max_key = create_cf_string(MAX_KEY)?;

        // SAFETY: service is a valid io_registry_entry_t and returns a create-rule CF object.
        let parameters = unsafe {
            IORegistryEntryCreateCFProperty(service, parameters_key.as_raw(), std::ptr::null(), 0)
        };
        if parameters.is_null() {
            bail!("IORegistryEntryCreateCFProperty returned null for IODisplayParameters");
        }
        let parameters = CfType(parameters);

        // SAFETY: parameters is a CFDictionary and brightness_key is a valid CFString.
        let brightness_entry =
            unsafe { CFDictionaryGetValue(parameters.as_raw(), brightness_key.as_raw()) };
        if brightness_entry.is_null() {
            bail!("IODisplayParameters does not contain a brightness entry");
        }

        let value = cf_dictionary_number(brightness_entry, &value_key)
            .with_context(|| "Missing brightness value")?;
        let minimum = cf_dictionary_number(brightness_entry, &min_key).unwrap_or(0.0);
        let maximum = cf_dictionary_number(brightness_entry, &max_key).unwrap_or(65536.0);

        if maximum <= minimum {
            bail!("Invalid brightness range in IODisplayParameters");
        }

        Ok(((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0) as f32)
    }

    fn cf_dictionary_number(dictionary: *const c_void, key: &CfString) -> Result<f64> {
        // SAFETY: dictionary is expected to be a CFDictionary and key is a valid CFString.
        let value = unsafe { CFDictionaryGetValue(dictionary, key.as_raw()) };
        if value.is_null() {
            bail!("Dictionary key missing");
        }

        let mut number = 0.0_f64;
        // SAFETY: value is expected to be a CFNumber and output points to valid storage.
        let ok = unsafe {
            CFNumberGetValue(
                value,
                CF_NUMBER_FLOAT64_TYPE,
                (&mut number as *mut f64).cast(),
            )
        };
        if ok == 0 {
            bail!("Dictionary value is not a numeric CFNumber");
        }

        Ok(number)
    }

    fn cf_dictionary_u32(dictionary: *const c_void, key: &CfString) -> Result<u32> {
        // SAFETY: dictionary is expected to be a CFDictionary and key is a valid CFString.
        let value = unsafe { CFDictionaryGetValue(dictionary, key.as_raw()) };
        if value.is_null() {
            bail!("Dictionary key missing");
        }

        let mut number = 0_i64;
        // SAFETY: value is expected to be a CFNumber and output points to valid storage.
        let ok = unsafe {
            CFNumberGetValue(
                value,
                CF_NUMBER_SINT64_TYPE,
                (&mut number as *mut i64).cast(),
            )
        };
        if ok == 0 || number < 0 || number > u32::MAX as i64 {
            bail!("Dictionary value cannot be represented as u32");
        }

        Ok(number as u32)
    }

    fn percent_from_unit(value: f32) -> u16 {
        (value.clamp(0.0, 1.0) * 100.0).round() as u16
    }

    fn ramp_brightness<F>(current: u16, target: u16, step_delay_ms: u64, mut apply: F) -> Result<()>
    where
        F: FnMut(u16) -> Result<()>,
    {
        if current == target {
            return Ok(());
        }

        let delay = Duration::from_millis(step_delay_ms);
        if current < target {
            for next in (current + 1)..=target {
                apply(next)?;
                thread::sleep(delay);
            }
        } else {
            for next in (target..current).rev() {
                apply(next)?;
                thread::sleep(delay);
            }
        }

        Ok(())
    }

    fn ramp_ddc_brightness<F>(
        current: u16,
        target: u16,
        step_delay_ms: u64,
        mut apply: F,
    ) -> Result<()>
    where
        F: FnMut(u16) -> Result<()>,
    {
        if current == target {
            return Ok(());
        }

        let delay = Duration::from_millis(step_delay_ms);
        let distance = current.abs_diff(target);
        let desired_writes = ((distance + DDC_TARGET_TRANSITION_STEP_SIZE.saturating_sub(1))
            / DDC_TARGET_TRANSITION_STEP_SIZE.max(1))
        .max(1);
        let writes = desired_writes.min(DDC_MAX_TRANSITION_WRITES.max(1)).min(distance);
        let sequence = build_transition_sequence(current, target, writes.max(1));

        for (index, next) in sequence.iter().enumerate() {
            apply(*next)?;
            if index + 1 < sequence.len() {
                thread::sleep(delay);
            }
        }
        Ok(())
    }

    fn build_transition_sequence(current: u16, target: u16, writes: u16) -> Vec<u16> {
        if current == target {
            return Vec::new();
        }

        let distance = current.abs_diff(target);
        let writes = writes.max(1).min(distance).max(1);
        let mut sequence = Vec::with_capacity(writes as usize);
        let ascending = target > current;
        let mut last = current;
        let total = u32::from(distance);
        let writes_u32 = u32::from(writes);

        for step in 1..=writes_u32 {
            let progressed = ((total * step) + (writes_u32 / 2)) / writes_u32;
            let progressed = progressed.min(total) as u16;
            let next = if ascending {
                current.saturating_add(progressed).min(target)
            } else {
                current.saturating_sub(progressed).max(target)
            };
            if next != last {
                sequence.push(next);
                last = next;
            }
        }

        if sequence.last().copied() != Some(target) {
            sequence.push(target);
        }

        sequence
    }

    fn display_services_api() -> Option<&'static DisplayServicesApi> {
        static API: OnceLock<Option<DisplayServicesApi>> = OnceLock::new();
        API.get_or_init(load_display_services_api).as_ref()
    }

    fn load_display_services_api() -> Option<DisplayServicesApi> {
        let path = CString::new(DISPLAY_SERVICES_FRAMEWORK_PATH).ok()?;
        // SAFETY: path is a valid NUL-terminated C string.
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_LAZY) };
        if handle.is_null() {
            return None;
        }

        let get_brightness = resolve_symbol::<DisplayServicesGetBrightnessFn>(
            handle,
            "DisplayServicesGetBrightness",
        );
        let set_brightness = resolve_symbol::<DisplayServicesSetBrightnessFn>(
            handle,
            "DisplayServicesSetBrightness",
        );
        let brightness_changed = resolve_symbol::<DisplayServicesBrightnessChangedFn>(
            handle,
            "DisplayServicesBrightnessChanged",
        );

        if get_brightness.is_none() && set_brightness.is_none() && brightness_changed.is_none() {
            // SAFETY: handle originates from dlopen.
            unsafe {
                let _ = dlclose(handle);
            }
            return None;
        }

        Some(DisplayServicesApi {
            _handle: handle as usize,
            get_brightness,
            set_brightness,
            brightness_changed,
        })
    }

    fn core_display_api() -> Option<&'static CoreDisplayApi> {
        static API: OnceLock<Option<CoreDisplayApi>> = OnceLock::new();
        API.get_or_init(load_core_display_api).as_ref()
    }

    fn load_core_display_api() -> Option<CoreDisplayApi> {
        let path = CString::new(CORE_DISPLAY_FRAMEWORK_PATH).ok()?;
        // SAFETY: path is a valid NUL-terminated C string.
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_LAZY) };
        if handle.is_null() {
            return None;
        }

        let get_user_brightness = resolve_symbol::<CoreDisplayGetUserBrightnessFn>(
            handle,
            "CoreDisplay_Display_GetUserBrightness",
        );
        let set_user_brightness = resolve_symbol::<CoreDisplaySetUserBrightnessFn>(
            handle,
            "CoreDisplay_Display_SetUserBrightness",
        );

        if get_user_brightness.is_none() && set_user_brightness.is_none() {
            // SAFETY: handle originates from dlopen.
            unsafe {
                let _ = dlclose(handle);
            }
            return None;
        }

        Some(CoreDisplayApi {
            _handle: handle as usize,
            get_user_brightness,
            set_user_brightness,
        })
    }

    fn resolve_symbol<T>(handle: *mut c_void, name: &str) -> Option<T> {
        let symbol_name = CString::new(name).ok()?;
        // SAFETY: handle is from dlopen and symbol name is NUL-terminated.
        let symbol = unsafe { dlsym(handle, symbol_name.as_ptr()) };
        if symbol.is_null() {
            return None;
        }

        // SAFETY: caller chooses T matching the symbol function signature.
        Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&symbol) })
    }
}

#[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
mod imp {
    use anyhow::{Context, Result, anyhow, bail};
    use ddc_hi::{Ddc, Display};
    use shared::{MonitorControl, MonitorControlType, MonitorSnapshot};

    const LUMINANCE_CODE: u8 = 0x10;

    pub fn list_monitors() -> Result<Vec<MonitorSnapshot>> {
        let displays = Display::enumerate();
        let mut snapshots = Vec::with_capacity(displays.len());

        for mut display in displays {
            snapshots.push(snapshot_from_display(&mut display));
        }

        Ok(snapshots)
    }

    pub fn set_monitor_feature(
        monitor_id: &str,
        code: &str,
        value: u16,
    ) -> Result<MonitorSnapshot> {
        if !code.eq_ignore_ascii_case("10") {
            bail!("Only brightness is supported on this platform right now");
        }

        let mut display = Display::enumerate()
            .into_iter()
            .find(|display| display.info.id == monitor_id)
            .with_context(|| format!("Monitor {monitor_id} was not found"))?;

        let current = display
            .handle
            .get_vcp_feature(LUMINANCE_CODE)
            .map_err(|error| anyhow!("Failed to query current brightness: {error}"))?;

        let maximum = current.maximum();
        if maximum == 0 {
            bail!("Monitor reported an invalid brightness range");
        }

        let clamped = value.min(maximum);

        display
            .handle
            .set_vcp_feature(LUMINANCE_CODE, clamped)
            .map_err(|error| anyhow!("Failed to set hardware brightness: {error}"))?;

        Ok(snapshot_from_display(&mut display))
    }

    pub fn transition_monitor_feature(
        monitor_id: &str,
        code: &str,
        value: u16,
        _step_delay_ms: u64,
    ) -> Result<MonitorSnapshot> {
        set_monitor_feature(monitor_id, code, value)
    }

    pub fn apply_color_scene(_monitor_id: &str, _scene_id: &str) -> Result<MonitorSnapshot> {
        bail!("Color scenes are only supported on Linux right now")
    }

    fn snapshot_from_display(display: &mut Display) -> MonitorSnapshot {
        let control = match display.handle.get_vcp_feature(LUMINANCE_CODE) {
            Ok(brightness) => MonitorControl {
                code: String::from("10"),
                label: String::from("Brightness"),
                control_type: MonitorControlType::Range,
                current_value: Some(brightness.value()),
                max_value: Some(brightness.maximum()),
                options: Vec::new(),
                supported: true,
                error: None,
            },
            Err(error) => MonitorControl {
                code: String::from("10"),
                label: String::from("Brightness"),
                control_type: MonitorControlType::Range,
                current_value: None,
                max_value: Some(100),
                options: Vec::new(),
                supported: false,
                error: Some(format!("Brightness query failed over DDC/CI: {error}")),
            },
        };

        MonitorSnapshot {
            id: display.info.id.clone(),
            backend: format!("{:?}", display.info.backend),
            device_path: None,
            connector_name: None,
            manufacturer_id: display.info.manufacturer_id.clone(),
            model_name: display.info.model_name.clone(),
            serial_number: display.info.serial_number.clone(),
            error: if control.supported {
                None
            } else {
                control.error.clone()
            },
            controls: vec![control],
        }
    }
}

pub use imp::{apply_color_scene, list_monitors, set_monitor_feature, transition_monitor_feature};
