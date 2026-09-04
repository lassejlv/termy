use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

use crate::config::AppConfig;

const MAX_DIAGNOSTIC_EVENTS: usize = 200;
const MAX_SYSTEM_FIELD_BYTES: usize = 256;

#[derive(Debug, Serialize)]
struct SupportBundle {
    schema_version: u16,
    generated_unix_seconds: u64,
    application: ApplicationInfo,
    system: SystemInfo,
    signature: SignatureInfo,
    config: ConfigInfo,
    session_daemons: Vec<DaemonInfo>,
    diagnostics: DiagnosticsInfo,
    privacy: PrivacyInfo,
}

#[derive(Debug, Serialize)]
struct ApplicationInfo {
    version: &'static str,
    mux_protocol_version: u16,
    bundle_identifier: Option<String>,
    bundle_build_number: Option<String>,
    binary_architectures: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SystemInfo {
    macos_version: Option<String>,
    macos_build: Option<String>,
    process_architecture: &'static str,
    gpu_models: Vec<String>,
    displays: Vec<DisplayInfo>,
}

#[derive(Debug, Serialize)]
struct DisplayInfo {
    resolution: Option<String>,
    pixel_resolution: Option<String>,
    refresh_rate: Option<String>,
    main: bool,
    online: bool,
}

#[derive(Debug, Serialize)]
struct SignatureInfo {
    kind: &'static str,
    team_identifier: Option<String>,
    hardened_runtime: bool,
    gatekeeper_accepted: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ConfigInfo {
    valid: bool,
    contents_included: bool,
}

#[derive(Debug, Serialize)]
struct DaemonInfo {
    protocol_version: u16,
    generation: &'static str,
    state: &'static str,
}

#[derive(Debug, Serialize)]
struct DiagnosticsInfo {
    status: &'static str,
    maximum_events: usize,
    events: Vec<DiagnosticEvent>,
}

#[derive(Debug, Serialize)]
struct DiagnosticEvent {
    unix_seconds: u64,
    code: &'static str,
}

#[derive(Debug, Serialize)]
struct PrivacyInfo {
    local_only: bool,
    user_review_required_before_sharing: bool,
    excluded_by_default: [&'static str; 6],
}

pub(crate) fn write(output: &Path) -> Result<()> {
    if output.as_os_str().is_empty() {
        bail!("support bundle output path must not be empty");
    }
    let report = collect();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)
        .with_context(|| {
            format!(
                "creating support bundle at {}; choose a new path in an existing directory",
                output.display()
            )
        })?;
    serde_json::to_writer_pretty(&mut file, &report).context("encoding support bundle")?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn collect() -> SupportBundle {
    let bundle = enclosing_app_bundle();
    let displays = display_inventory();
    let diagnostic_records = diagnostics::recent_events(MAX_DIAGNOSTIC_EVENTS);
    let (diagnostic_status, events) = match diagnostic_records {
        Ok(records) => (
            "available",
            records
                .into_iter()
                .map(|record| DiagnosticEvent {
                    unix_seconds: record.unix_seconds,
                    code: record.event.code(),
                })
                .collect(),
        ),
        Err(_) => ("unavailable_private_log_contract_failed", Vec::new()),
    };
    SupportBundle {
        schema_version: 1,
        generated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
        application: ApplicationInfo {
            version: env!("CARGO_PKG_VERSION"),
            mux_protocol_version: mux::PROTOCOL_VERSION,
            bundle_identifier: bundle
                .as_deref()
                .and_then(|path| plist_value(path, "CFBundleIdentifier")),
            bundle_build_number: bundle
                .as_deref()
                .and_then(|path| plist_value(path, "CFBundleVersion")),
            binary_architectures: current_executable()
                .as_deref()
                .and_then(binary_architectures)
                .unwrap_or_default(),
        },
        system: SystemInfo {
            macos_version: command_field("/usr/bin/sw_vers", &["-productVersion"]),
            macos_build: command_field("/usr/bin/sw_vers", &["-buildVersion"]),
            process_architecture: std::env::consts::ARCH,
            gpu_models: displays.0,
            displays: displays.1,
        },
        signature: signature_info(bundle.as_deref()),
        config: ConfigInfo {
            valid: AppConfig::load().is_ok(),
            contents_included: false,
        },
        session_daemons: mux::inspect_daemons()
            .unwrap_or_default()
            .into_iter()
            .map(|daemon| DaemonInfo {
                protocol_version: daemon.protocol_version,
                generation: if daemon.current { "current" } else { "older" },
                state: if !daemon.secure {
                    "unsafe_path_not_connected"
                } else if daemon.live {
                    "running"
                } else {
                    "stale_socket"
                },
            })
            .collect(),
        diagnostics: DiagnosticsInfo {
            status: diagnostic_status,
            maximum_events: MAX_DIAGNOSTIC_EVENTS,
            events,
        },
        privacy: PrivacyInfo {
            local_only: true,
            user_review_required_before_sharing: true,
            excluded_by_default: [
                "terminal_contents",
                "command_history_and_arguments",
                "clipboard",
                "environment_variables",
                "configuration_contents",
                "filesystem_paths",
            ],
        },
    }
}

fn current_executable() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

fn enclosing_app_bundle() -> Option<PathBuf> {
    current_executable()?.ancestors().find_map(|path| {
        (path.extension().and_then(|extension| extension.to_str()) == Some("app"))
            .then(|| path.to_owned())
    })
}

fn plist_value(bundle: &Path, key: &str) -> Option<String> {
    let plist = bundle.join("Contents/Info.plist");
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| bounded_field(&output.stdout))
        .flatten()
}

fn binary_architectures(executable: &Path) -> Option<Vec<String>> {
    let output = Command::new("/usr/bin/lipo")
        .arg("-archs")
        .arg(executable)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let architectures = String::from_utf8(output.stdout).ok()?;
    Some(
        architectures
            .split_ascii_whitespace()
            .filter(|architecture| {
                !architecture.is_empty()
                    && architecture
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            .take(4)
            .map(str::to_owned)
            .collect(),
    )
}

fn signature_info(bundle: Option<&Path>) -> SignatureInfo {
    let target = bundle
        .map(Path::to_owned)
        .or_else(current_executable)
        .unwrap_or_default();
    let output = Command::new("/usr/bin/codesign")
        .arg("-dvvv")
        .arg(&target)
        .output();
    let details = output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stderr).ok())
        .unwrap_or_default();
    let kind = if details.contains("Signature=adhoc") {
        "ad_hoc"
    } else if details.contains("Authority=Developer ID Application:") {
        "developer_id_application"
    } else if details.is_empty() {
        "unsigned_or_unavailable"
    } else {
        "other"
    };
    let team_identifier = details.lines().find_map(|line| {
        line.strip_prefix("TeamIdentifier=")
            .filter(|value| *value != "not set")
            .and_then(|value| bounded_text(value.as_bytes()))
    });
    SignatureInfo {
        kind,
        team_identifier,
        hardened_runtime: details
            .lines()
            .any(|line| line.starts_with("CodeDirectory ") && line.contains("runtime")),
        gatekeeper_accepted: bundle.map(|bundle| {
            Command::new("/usr/sbin/spctl")
                .args(["--assess", "--type", "execute"])
                .arg(bundle)
                .output()
                .is_ok_and(|output| output.status.success())
        }),
    }
}

fn command_field(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| bounded_field(&output.stdout))
        .flatten()
}

fn bounded_field(bytes: &[u8]) -> Option<String> {
    bounded_text(bytes).filter(|value| {
        value
            .bytes()
            .all(|byte| !byte.is_ascii_control() || byte == b' ')
    })
}

fn bounded_text(bytes: &[u8]) -> Option<String> {
    let value = std::str::from_utf8(bytes).ok()?.trim();
    (!value.is_empty() && value.len() <= MAX_SYSTEM_FIELD_BYTES).then(|| value.to_owned())
}

fn display_inventory() -> (Vec<String>, Vec<DisplayInfo>) {
    let Some(output) = Command::new("/usr/sbin/system_profiler")
        .args(["SPDisplaysDataType", "-json", "-detailLevel", "basic"])
        .output()
        .ok()
        .filter(|output| output.status.success())
    else {
        return (Vec::new(), Vec::new());
    };
    parse_display_inventory(&output.stdout)
}

fn parse_display_inventory(bytes: &[u8]) -> (Vec<String>, Vec<DisplayInfo>) {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return (Vec::new(), Vec::new());
    };
    let Some(gpus) = value.get("SPDisplaysDataType").and_then(Value::as_array) else {
        return (Vec::new(), Vec::new());
    };
    let mut models = Vec::new();
    let mut displays = Vec::new();
    for gpu in gpus.iter().take(8) {
        if let Some(model) = gpu
            .get("sppci_model")
            .or_else(|| gpu.get("_name"))
            .and_then(Value::as_str)
            .and_then(|value| bounded_text(value.as_bytes()))
        {
            models.push(model);
        }
        let Some(attached) = gpu.get("spdisplays_ndrvs").and_then(Value::as_array) else {
            continue;
        };
        for display in attached.iter().take(8) {
            displays.push(DisplayInfo {
                resolution: safe_json_field(display, "_spdisplays_resolution"),
                pixel_resolution: safe_json_field(display, "spdisplays_pixelresolution"),
                refresh_rate: safe_json_field(display, "spdisplays_refreshRate"),
                main: json_yes(display, "spdisplays_main"),
                online: json_yes(display, "spdisplays_online"),
            });
        }
    }
    models.sort();
    models.dedup();
    (models, displays)
}

fn safe_json_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| bounded_field(value.as_bytes()))
}

fn json_yes(value: &Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "spdisplays_yes" | "Yes"))
}

#[cfg(test)]
mod tests {
    use super::{bounded_field, parse_display_inventory};

    #[test]
    fn display_inventory_keeps_only_whitelisted_fields() {
        let source = br#"{
            "SPDisplaysDataType": [{
                "_name": "GPU",
                "sppci_model": "Apple Test GPU",
                "private_serial": "must-not-escape",
                "spdisplays_ndrvs": [{
                    "_name": "User-named display",
                    "_spdisplays_resolution": "1920 x 1080",
                    "spdisplays_pixelresolution": "3840 x 2160",
                    "spdisplays_refreshRate": "120 Hz",
                    "spdisplays_main": "spdisplays_yes",
                    "spdisplays_online": "spdisplays_yes",
                    "display_serial": "must-not-escape"
                }]
            }]
        }"#;
        let (gpus, displays) = parse_display_inventory(source);
        assert_eq!(gpus, ["Apple Test GPU"]);
        assert_eq!(displays.len(), 1);
        assert_eq!(displays[0].resolution.as_deref(), Some("1920 x 1080"));
        assert_eq!(displays[0].refresh_rate.as_deref(), Some("120 Hz"));
        assert!(displays[0].main);
        assert!(displays[0].online);
        let encoded = serde_json::to_string(&displays).expect("encode display inventory");
        assert!(!encoded.contains("must-not-escape"));
        assert!(!encoded.contains("User-named"));
    }

    #[test]
    fn bounded_fields_reject_controls_and_oversized_values() {
        assert_eq!(bounded_field(b"macOS\n27.0"), None);
        assert_eq!(bounded_field(b"macOS 27.0"), Some("macOS 27.0".to_owned()));
        assert_eq!(bounded_field(&vec![b'a'; 257]), None);
    }
}
