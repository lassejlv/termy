use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginRuntimeKind {
    Native,
    Tmux,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginPaneKind {
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginTabContext {
    pub id: String,
    pub index: usize,
    pub title: String,
    pub pane_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPaneContext {
    pub id: String,
    pub index: usize,
    pub kind: PluginPaneKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginOriginContext {
    pub window_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginContext {
    pub origin: PluginOriginContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_text: Option<String>,
    pub selected_text_truncated: bool,
    pub shell: String,
    pub runtime: PluginRuntimeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tab: Option<PluginTabContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_pane: Option<PluginPaneContext>,
    pub platform: String,
    pub app_version: String,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn context_serializes_the_public_camel_case_contract() {
        let value = serde_json::to_value(PluginContext {
            origin: PluginOriginContext {
                window_id: "window-1".to_string(),
                tab_id: Some("tab-3".to_string()),
                pane_id: Some("pane-2".to_string()),
            },
            working_directory: Some("/repo".to_string()),
            active_command: Some("cargo test".to_string()),
            selected_text: Some("failed assertion".to_string()),
            selected_text_truncated: false,
            shell: "/bin/zsh".to_string(),
            runtime: PluginRuntimeKind::Tmux,
            active_tab: Some(PluginTabContext {
                id: "tab-3".to_string(),
                index: 2,
                title: "tests".to_string(),
                pane_count: 3,
            }),
            active_pane: Some(PluginPaneContext {
                id: "pane-2".to_string(),
                index: 1,
                kind: PluginPaneKind::Terminal,
            }),
            platform: "macos".to_string(),
            app_version: "1.2.3".to_string(),
            settings: BTreeMap::new(),
        })
        .expect("serialize plugin context");

        assert_eq!(
            value,
            json!({
                "origin": {
                    "windowId": "window-1",
                    "tabId": "tab-3",
                    "paneId": "pane-2",
                },
                "workingDirectory": "/repo",
                "activeCommand": "cargo test",
                "selectedText": "failed assertion",
                "selectedTextTruncated": false,
                "shell": "/bin/zsh",
                "runtime": "tmux",
                "activeTab": { "id": "tab-3", "index": 2, "title": "tests", "paneCount": 3 },
                "activePane": { "id": "pane-2", "index": 1, "kind": "terminal" },
                "platform": "macos",
                "appVersion": "1.2.3",
                "settings": {},
            })
        );
    }

    #[test]
    fn unavailable_optional_context_is_omitted() {
        let value = serde_json::to_value(PluginContext {
            origin: PluginOriginContext {
                window_id: "window-2".to_string(),
                tab_id: None,
                pane_id: None,
            },
            working_directory: None,
            active_command: None,
            selected_text: None,
            selected_text_truncated: false,
            shell: "/bin/bash".to_string(),
            runtime: PluginRuntimeKind::Native,
            active_tab: None,
            active_pane: None,
            platform: "linux".to_string(),
            app_version: "test".to_string(),
            settings: BTreeMap::new(),
        })
        .expect("serialize plugin context");

        assert!(value.get("workingDirectory").is_none());
        assert!(value.get("activeCommand").is_none());
        assert!(value.get("selectedText").is_none());
        assert!(value.get("activeTab").is_none());
        assert!(value.get("activePane").is_none());
    }
}
