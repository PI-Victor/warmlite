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
            code: "60",
            label: "Input Source",
            kind: FeatureKind::Choice,
        },
        FeatureDefinition {
            code: "DC",
            label: "Display Mode",
            kind: FeatureKind::Choice,
        },
        FeatureDefinition {
            code: "8D",
            label: "Mute",
            kind: FeatureKind::Toggle,
        },
        FeatureDefinition {
            code: "D6",
            label: "Display Power",
            kind: FeatureKind::Toggle,
        },
    ];

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
        set_feature_value(&monitor, code, value)?;
        if code.eq_ignore_ascii_case("14") {
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

        if current.current_value < target {
            for next in (current.current_value + 1)..=target {
                set_feature_value(&monitor, code, next)?;
                thread::sleep(Duration::from_millis(step_delay_ms));
            }
        } else {
            for next in (target..current.current_value).rev() {
                set_feature_value(&monitor, code, next)?;
                thread::sleep(Duration::from_millis(step_delay_ms));
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
        apply_rgb_gain_percent(&monitor, "16", profile.red_percent)?;
        apply_rgb_gain_percent(&monitor, "18", profile.green_percent)?;
        apply_rgb_gain_percent(&monitor, "1A", profile.blue_percent)?;

        Ok(snapshot_for_monitor(&monitor))
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

            match read_feature(monitor, definition.code) {
                Ok(readout) => {
                    let label = capability
                        .and_then(|feature| feature.label.clone())
                        .unwrap_or_else(|| definition.label.to_string());

                    controls.push(MonitorControl {
                        code: definition.code.to_string(),
                        label,
                        control_type: match definition.kind {
                            FeatureKind::Range => MonitorControlType::Range,
                            FeatureKind::Choice => MonitorControlType::Choice,
                            FeatureKind::Toggle => MonitorControlType::Toggle,
                        },
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
                        control_type: match definition.kind {
                            FeatureKind::Range => MonitorControlType::Range,
                            FeatureKind::Choice => MonitorControlType::Choice,
                            FeatureKind::Toggle => MonitorControlType::Toggle,
                        },
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

    fn color_scene_profile(scene_id: &str) -> Option<ColorSceneProfile> {
        match scene_id {
            "paper" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 90,
                blue_percent: 76,
            }),
            "sunset" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 82,
                blue_percent: 62,
            }),
            "ember" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 72,
                blue_percent: 48,
            }),
            "incandescent" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 78,
                blue_percent: 55,
            }),
            "candle" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 66,
                blue_percent: 38,
            }),
            "nocturne" => Some(ColorSceneProfile {
                red_percent: 100,
                green_percent: 58,
                blue_percent: 28,
            }),
            _ => None,
        }
    }

    fn apply_rgb_gain_percent(monitor: &LinuxMonitor, code: &str, percent: u8) -> Result<()> {
        let readout = read_feature(monitor, code)?;
        let maximum = readout.max_value.unwrap_or(100).max(1);
        let clamped = percent.min(100) as u32;
        let target = (((maximum as u32) * clamped) + 50) / 100;
        set_feature_value(monitor, code, target.min(maximum as u32) as u16)
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
                    value: 0x04,
                    label: String::from("Off"),
                },
                ControlOption {
                    value: 0x01,
                    label: String::from("On"),
                },
            ],
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

#[cfg(not(target_os = "linux"))]
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
