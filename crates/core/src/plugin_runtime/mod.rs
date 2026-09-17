//! On-demand Bun runtime for trusted local Termy TypeScript plugins.

use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, RwLock, TryLockError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

mod context;
mod events;

pub use context::{
    PluginContext, PluginOriginContext, PluginPaneContext, PluginPaneKind, PluginRuntimeKind,
    PluginTabContext,
};
pub use events::{PluginEvent, PluginEventDispatch, PluginEventKind};
use events::{PluginEventSubscriptionDescriptor, RegisteredPluginEvent};

const MAX_PROTOCOL_BYTES: usize = 1024 * 1024;
const MAX_PROTOCOL_HANDSHAKE_BYTES: usize = 1024;
const LOAD_TIMEOUT: Duration = Duration::from_secs(90);
const HOST_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_INVOKE_TIMEOUT_MS: u64 = 10_000;
const MAX_INVOKE_TIMEOUT_MS: u64 = 30_000;
const MAX_INVOKE_QUEUE_WAIT_MS: u64 = 30_000;
const MAX_PLUGIN_COMMANDS: usize = 512;
const MAX_INPUTS_PER_COMMAND: usize = 16;
const MAX_SELECT_OPTIONS: usize = 128;
const MAX_PLUGIN_SETTINGS: usize = 64;
const MAX_SETTING_VALUE_LENGTH: usize = 4_096;
const MAX_SETTINGS_FILE_BYTES: u64 = 64 * 1024;
const MAX_ACTIONS: usize = 32;
const MAX_PLUGIN_VIEWS: usize = 32;
const MAX_VIEW_NODES: usize = 256;
const MAX_VIEW_DEPTH: usize = 16;
const MAX_VIEW_CHILDREN: usize = 64;
const MAX_VIEW_VALUES: usize = 64;
const MAX_VIEW_PARAMS_BYTES: usize = 64 * 1024;
const MAX_LAUNCH_ARGS: usize = 128;
const MAX_LAUNCH_TEXT_LENGTH: usize = 4_096;
const PLUGIN_CAPABILITIES: [&str; 2] = ["storage", "native-ui"];
pub const MAX_INSTALLED_PLUGINS: usize = 32;
pub const MAX_PLUGIN_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_PLUGIN_SOURCE_FILES: usize = 4_096;
const BUNDLE_CACHE_FORMAT: &[u8] = b"termy-plugin-bundle-v2\0";
const DISABLED_MARKER: &str = ".termy-disabled";
const SOURCE_METADATA_FILE: &str = ".termy-source.json";
const SETTINGS_FILE: &str = "settings.json";
#[cfg(not(test))]
const KEYRING_SERVICE: &str = "com.lassevestergaard.termy.plugin";

const HOST_SOURCE: &str = include_str!("host.ts");
const WORKER_SOURCE: &str = include_str!("worker.ts");
const TYPE_DECLARATIONS: &str = include_str!("termy.d.ts");

#[derive(Clone)]
pub struct PluginRuntime {
    inner: Arc<PluginRuntimeInner>,
}

struct PluginRuntimeInner {
    plugins_dir: Option<PathBuf>,
    refresh: Mutex<()>,
    settings: Mutex<()>,
    lifecycle_generations: Mutex<BTreeMap<String, u64>>,
    catalog: RwLock<PluginCatalog>,
    host: Mutex<PluginHostState>,
}

#[derive(Default)]
struct PluginCatalog {
    fingerprint: Option<[u8; 32]>,
    commands: Vec<PluginCommand>,
    events: Vec<RegisteredPluginEvent>,
    views: Vec<PluginViewDescriptor>,
    settings: BTreeMap<String, Vec<PluginSetting>>,
    revisions: BTreeMap<String, String>,
}

#[derive(Default)]
struct PluginHostState {
    connection: Option<Arc<HostConnection>>,
    next_request_id: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginRefresh {
    pub changed: bool,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub path: PathBuf,
    pub source: Option<PluginSourceMetadata>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSourceMetadata {
    pub repository_url: String,
    #[serde(default)]
    pub requested_ref: Option<String>,
    pub revision: String,
    #[serde(default)]
    pub subdirectory: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginInventory {
    pub plugins: Vec<InstalledPlugin>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PluginSetting {
    Toggle {
        id: String,
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default, rename = "defaultValue")]
        default_value: bool,
    },
    Text {
        id: String,
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default, rename = "defaultValue")]
        default_value: String,
        #[serde(default = "default_setting_max_length", rename = "maxLength")]
        max_length: usize,
    },
    Select {
        id: String,
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default, rename = "defaultValue")]
        default_value: String,
        options: Vec<PluginSettingOption>,
    },
    Secret {
        id: String,
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default = "default_setting_max_length", rename = "maxLength")]
        max_length: usize,
    },
}

impl PluginSetting {
    pub fn id(&self) -> &str {
        match self {
            Self::Toggle { id, .. }
            | Self::Text { id, .. }
            | Self::Select { id, .. }
            | Self::Secret { id, .. } => id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::Toggle { title, .. }
            | Self::Text { title, .. }
            | Self::Select { title, .. }
            | Self::Secret { title, .. } => title,
        }
    }

    pub fn description(&self) -> Option<&str> {
        match self {
            Self::Toggle { description, .. }
            | Self::Text { description, .. }
            | Self::Select { description, .. }
            | Self::Secret { description, .. } => description.as_deref(),
        }
    }

    fn default_value(&self) -> Option<Value> {
        match self {
            Self::Toggle { default_value, .. } => Some(Value::Bool(*default_value)),
            Self::Text { default_value, .. } | Self::Select { default_value, .. } => {
                Some(Value::String(default_value.clone()))
            }
            Self::Secret { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingOption {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSettingState {
    pub definition: PluginSetting,
    pub value: Option<Value>,
    pub configured: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginSettingsSnapshot {
    pub plugins: BTreeMap<String, Vec<PluginSettingState>>,
    pub errors: Vec<String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginSettingsFile {
    #[serde(default)]
    values: BTreeMap<String, Value>,
    #[serde(default)]
    secret_keys: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommand {
    pub plugin_id: String,
    pub plugin_name: String,
    pub id: String,
    pub title: String,
    #[serde(default = "default_command_placements")]
    pub placements: Vec<PluginCommandPlacement>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub disabled_reason: Option<String>,
    #[serde(default)]
    pub icon: PluginIcon,
    #[serde(default)]
    pub inputs: Vec<PluginInput>,
    #[serde(default)]
    pub when: PluginCommandWhen,
    #[serde(default = "default_invoke_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCommandWhen {
    #[serde(default)]
    pub has_selection: Option<bool>,
    #[serde(default)]
    pub has_working_directory: Option<bool>,
    #[serde(default)]
    pub runtimes: Vec<PluginRuntimeKind>,
    #[serde(default)]
    pub platforms: Vec<String>,
}

impl PluginCommandWhen {
    pub fn matches(&self, context: &PluginContext) -> bool {
        self.has_selection
            .is_none_or(|required| context.selected_text.is_some() == required)
            && self
                .has_working_directory
                .is_none_or(|required| context.working_directory.is_some() == required)
            && (self.runtimes.is_empty() || self.runtimes.contains(&context.runtime))
            && (self.platforms.is_empty() || self.platforms.contains(&context.platform))
    }
}

impl PluginCommand {
    pub fn qualified_id(&self) -> String {
        format!("{}.{}", self.plugin_id, self.id)
    }
}

fn default_command_placements() -> Vec<PluginCommandPlacement> {
    vec![PluginCommandPlacement::CommandPalette]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginCommandPlacement {
    CommandPalette,
    TerminalContextMenu,
    TabContextMenu,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginIcon {
    #[default]
    Command,
    Play,
    Terminal,
    Folder,
    Link,
    Clipboard,
    Settings,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PluginInput {
    Text {
        id: String,
        label: String,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default, rename = "defaultValue")]
        default_value: Option<String>,
        #[serde(default)]
        required: bool,
        #[serde(default = "default_text_max_length", rename = "maxLength")]
        max_length: usize,
    },
    Select {
        id: String,
        label: String,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default, rename = "defaultValue")]
        default_value: Option<String>,
        #[serde(default)]
        required: bool,
        options: Vec<PluginSelectOption>,
    },
    Pick {
        id: String,
        label: String,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default, rename = "defaultValue")]
        default_value: Option<String>,
        #[serde(default)]
        required: bool,
    },
    Confirm {
        id: String,
        label: String,
        #[serde(default, rename = "defaultValue")]
        default_value: bool,
    },
}

impl PluginInput {
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. }
            | Self::Select { id, .. }
            | Self::Pick { id, .. }
            | Self::Confirm { id, .. } => id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Text { label, .. }
            | Self::Select { label, .. }
            | Self::Pick { label, .. }
            | Self::Confirm { label, .. } => label,
        }
    }

    pub fn placeholder(&self) -> Option<&str> {
        match self {
            Self::Text { placeholder, .. }
            | Self::Select { placeholder, .. }
            | Self::Pick { placeholder, .. } => placeholder.as_deref(),
            Self::Confirm { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSelectOption {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum PluginTerminalTarget {
    Named(PluginTerminalTargetKind),
    Exact {
        #[serde(rename = "windowId")]
        window_id: String,
        #[serde(default, rename = "tabId")]
        tab_id: Option<String>,
        #[serde(default, rename = "paneId")]
        pane_id: Option<String>,
    },
}

impl Default for PluginTerminalTarget {
    fn default() -> Self {
        Self::Named(PluginTerminalTargetKind::Origin)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginTerminalTargetKind {
    #[default]
    Origin,
    Active,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginTerminalOpenLocation {
    #[default]
    Tab,
    SplitRight,
    SplitDown,
    Window,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum PluginTerminalLaunch {
    Shell {
        command: String,
    },
    Program {
        program: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum PluginAction {
    #[serde(rename = "terminal.run")]
    TerminalRun {
        command: String,
        #[serde(default, rename = "workingDirectory")]
        working_directory: Option<String>,
    },
    #[serde(rename = "terminal.sendText")]
    TerminalSendText {
        text: String,
        #[serde(default)]
        submit: bool,
        #[serde(default)]
        target: PluginTerminalTarget,
    },
    #[serde(rename = "terminal.open")]
    TerminalOpen {
        #[serde(default)]
        location: PluginTerminalOpenLocation,
        #[serde(default, rename = "workingDirectory")]
        working_directory: Option<String>,
        #[serde(default)]
        launch: Option<PluginTerminalLaunch>,
        #[serde(default)]
        target: PluginTerminalTarget,
        #[serde(default = "default_true")]
        focus: bool,
    },
    #[serde(rename = "termy.command")]
    TermyCommand { command: String },
    #[serde(rename = "clipboard.write")]
    ClipboardWrite { text: String },
    #[serde(rename = "url.open")]
    UrlOpen { url: String },
    #[serde(rename = "toast")]
    Toast {
        level: PluginToastLevel,
        message: String,
    },
    #[serde(rename = "view.open")]
    ViewOpen {
        view: String,
        #[serde(default)]
        target: PluginViewTarget,
        #[serde(default = "default_view_params")]
        params: Value,
        #[serde(skip)]
        plugin_id: String,
        #[serde(skip)]
        revision: String,
    },
    #[serde(rename = "view.replace")]
    ViewReplace {
        view: String,
        #[serde(default = "default_view_params")]
        params: Value,
        #[serde(skip)]
        plugin_id: String,
        #[serde(skip)]
        revision: String,
    },
    #[serde(rename = "view.close")]
    ViewClose {
        #[serde(skip)]
        plugin_id: String,
        #[serde(skip)]
        revision: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginViewTarget {
    #[default]
    Modal,
    CommandPalette,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginViewDescriptor {
    pub plugin_id: String,
    pub plugin_name: String,
    pub id: String,
    pub title: String,
    #[serde(default = "default_invoke_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginUiGap {
    None,
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginUiAlignment {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginUiTextVariant {
    Heading,
    #[default]
    Body,
    Caption,
    Code,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginUiTone {
    #[default]
    Default,
    Muted,
    Success,
    Danger,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginUiButtonVariant {
    #[default]
    Secondary,
    Primary,
    Danger,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum PluginUiNode {
    Column {
        #[serde(default)]
        gap: PluginUiGap,
        #[serde(default)]
        align: PluginUiAlignment,
        #[serde(default)]
        children: Vec<PluginUiNode>,
    },
    Row {
        #[serde(default)]
        gap: PluginUiGap,
        #[serde(default)]
        align: PluginUiAlignment,
        #[serde(default)]
        children: Vec<PluginUiNode>,
    },
    Text {
        text: String,
        #[serde(default)]
        variant: PluginUiTextVariant,
        #[serde(default)]
        tone: PluginUiTone,
    },
    TextInput {
        id: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        value: String,
        #[serde(default = "default_text_max_length", rename = "maxLength")]
        max_length: usize,
        #[serde(default)]
        submit: Option<String>,
        #[serde(default)]
        disabled: bool,
    },
    TextArea {
        id: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        value: String,
        #[serde(default = "default_text_max_length", rename = "maxLength")]
        max_length: usize,
        #[serde(default = "default_text_area_rows")]
        rows: usize,
        #[serde(default)]
        submit: Option<String>,
        #[serde(default)]
        disabled: bool,
    },
    Select {
        id: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        value: String,
        options: Vec<PluginSelectOption>,
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        disabled: bool,
    },
    List {
        id: String,
        #[serde(default)]
        action: Option<String>,
        #[serde(default, rename = "selectedId")]
        selected_id: Option<String>,
        #[serde(default, rename = "searchPlaceholder")]
        search_placeholder: Option<String>,
        #[serde(default = "default_true")]
        filtering: bool,
        #[serde(default, rename = "isLoading")]
        is_loading: bool,
        #[serde(default)]
        children: Vec<PluginUiNode>,
    },
    ListItem {
        id: String,
        title: String,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default)]
        keywords: Vec<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        payload: Option<String>,
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        disabled: bool,
    },
    EmptyState {
        title: String,
        #[serde(default)]
        description: Option<String>,
    },
    Progress {
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        value: Option<u8>,
    },
    Button {
        id: String,
        action: String,
        label: String,
        #[serde(default)]
        payload: Option<String>,
        #[serde(default)]
        variant: PluginUiButtonVariant,
        #[serde(default)]
        disabled: bool,
    },
    Checkbox {
        id: String,
        action: String,
        label: String,
        #[serde(default)]
        payload: Option<String>,
        #[serde(default)]
        checked: bool,
        #[serde(default)]
        disabled: bool,
    },
    Divider,
    Spacer {
        #[serde(default)]
        size: PluginUiGap,
    },
}

impl PluginUiNode {
    pub fn children(&self) -> &[PluginUiNode] {
        match self {
            Self::Column { children, .. }
            | Self::Row { children, .. }
            | Self::List { children, .. } => children,
            Self::Text { .. }
            | Self::TextInput { .. }
            | Self::TextArea { .. }
            | Self::Select { .. }
            | Self::ListItem { .. }
            | Self::EmptyState { .. }
            | Self::Progress { .. }
            | Self::Button { .. }
            | Self::Checkbox { .. }
            | Self::Divider
            | Self::Spacer { .. } => &[],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PluginViewValue {
    Text(String),
    Toggle(bool),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginViewAction {
    pub id: String,
    pub control_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<PluginViewValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginViewRender {
    pub plugin_id: String,
    pub revision: String,
    pub params: Value,
    pub nodes: Vec<PluginUiNode>,
    pub actions: Vec<PluginAction>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProgress {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub percentage: Option<u8>,
}

#[derive(Clone, Default)]
pub struct PluginInvocationControl {
    cancelled: Arc<AtomicBool>,
    progress: Option<Arc<dyn Fn(PluginProgress) + Send + Sync>>,
}

impl PluginInvocationControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_progress_handler(handler: impl Fn(PluginProgress) + Send + Sync + 'static) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress: Some(Arc::new(handler)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn report(&self, progress: PluginProgress) {
        if let Some(handler) = &self.progress {
            handler(progress);
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginSource {
    id: String,
    root: String,
    cache_key: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginManifestFile {
    #[serde(default, rename = "$schema")]
    _schema: Option<String>,
    api_version: u32,
    id: String,
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    main: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
}

type PluginFiles = Vec<(String, Vec<u8>)>;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum HostRequest<'a> {
    Load {
        id: u64,
        plugins: &'a [PluginSource],
    },
    Invoke {
        id: u64,
        #[serde(rename = "pluginId")]
        plugin_id: &'a str,
        #[serde(rename = "commandId")]
        command_id: &'a str,
        revision: &'a str,
        inputs: &'a BTreeMap<String, Value>,
        context: &'a PluginContext,
    },
    #[serde(rename = "input.options")]
    InputOptions {
        id: u64,
        #[serde(rename = "pluginId")]
        plugin_id: &'a str,
        #[serde(rename = "commandId")]
        command_id: &'a str,
        #[serde(rename = "inputId")]
        input_id: &'a str,
        revision: &'a str,
        query: &'a str,
        context: &'a PluginContext,
    },
    Event {
        id: u64,
        #[serde(rename = "pluginId")]
        plugin_id: &'a str,
        revision: &'a str,
        event: &'a PluginEvent,
        context: &'a PluginContext,
    },
    #[serde(rename = "view.render")]
    ViewRender {
        id: u64,
        #[serde(rename = "pluginId")]
        plugin_id: &'a str,
        #[serde(rename = "viewId")]
        view_id: &'a str,
        revision: &'a str,
        params: &'a Value,
        context: &'a PluginContext,
    },
    #[serde(rename = "view.action")]
    ViewAction {
        id: u64,
        #[serde(rename = "pluginId")]
        plugin_id: &'a str,
        #[serde(rename = "viewId")]
        view_id: &'a str,
        revision: &'a str,
        params: &'a Value,
        action: &'a PluginViewAction,
        values: &'a BTreeMap<String, PluginViewValue>,
        context: &'a PluginContext,
    },
    Cancel {
        id: u64,
        #[serde(rename = "requestId")]
        request_id: u64,
    },
}

impl HostRequest<'_> {
    fn id(&self) -> u64 {
        match self {
            Self::Load { id, .. }
            | Self::Invoke { id, .. }
            | Self::InputOptions { id, .. }
            | Self::Event { id, .. }
            | Self::ViewRender { id, .. }
            | Self::ViewAction { id, .. }
            | Self::Cancel { id, .. } => *id,
        }
    }
}

#[derive(Deserialize)]
struct HostResponse {
    id: u64,
    #[serde(default)]
    progress: Option<PluginProgress>,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostLoadResult {
    plugins: Vec<HostLoadedPlugin>,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostLoadedPlugin {
    plugin_id: String,
    commands: Value,
    events: Value,
    views: Value,
    settings: Value,
}

#[derive(Deserialize)]
struct HostInvokeResult {
    #[serde(default)]
    actions: Vec<PluginAction>,
}

struct ActionRequestContext<'a> {
    plugin_id: &'a str,
    revision: &'a str,
    lifecycle_generation: u64,
    origin: &'a PluginOriginContext,
    control: Option<&'a PluginInvocationControl>,
}

#[derive(Deserialize)]
struct HostPickResult {
    #[serde(default)]
    options: Vec<PluginSelectOption>,
}

#[derive(Deserialize)]
struct HostViewRenderResult {
    #[serde(default)]
    nodes: Vec<PluginUiNode>,
    #[serde(default)]
    actions: Vec<PluginAction>,
}

impl PluginRuntime {
    pub fn new(config_path: Option<&Path>) -> Self {
        let plugins_dir = config_path
            .and_then(Path::parent)
            .map(|parent| parent.join("plugins"));
        Self {
            inner: Arc::new(PluginRuntimeInner {
                plugins_dir,
                refresh: Mutex::new(()),
                settings: Mutex::new(()),
                lifecycle_generations: Mutex::new(BTreeMap::new()),
                catalog: RwLock::new(PluginCatalog::default()),
                host: Mutex::new(PluginHostState::default()),
            }),
        }
    }

    pub fn plugins_directory(&self) -> Option<PathBuf> {
        self.inner.plugins_dir.clone()
    }

    pub fn bun_path(&self) -> Result<Option<PathBuf>, String> {
        resolve_bun_binary()
    }

    pub fn installed_plugins(&self) -> Result<PluginInventory, String> {
        let plugins_dir = self
            .inner
            .plugins_dir
            .as_deref()
            .ok_or_else(|| "Termy config path is unavailable".to_string())?;
        inventory_plugins(plugins_dir)
    }

    pub fn install_from_directory(&self, source: &Path) -> Result<InstalledPlugin, String> {
        self.install_from_directory_inner(source, None)
    }

    pub fn install_from_directory_with_source(
        &self,
        source: &Path,
        source_metadata: PluginSourceMetadata,
    ) -> Result<InstalledPlugin, String> {
        validate_source_metadata(&source_metadata)?;
        self.install_from_directory_inner(source, Some(source_metadata))
    }

    fn install_from_directory_inner(
        &self,
        source: &Path,
        source_metadata: Option<PluginSourceMetadata>,
    ) -> Result<InstalledPlugin, String> {
        let plugins_dir = self
            .inner
            .plugins_dir
            .as_deref()
            .ok_or_else(|| "Termy config path is unavailable".to_string())?;
        let manifest = source_plugin_manifest(source)?;
        let (manifest, files) = inspect_plugin_root(source, &manifest.id)?;

        fs::create_dir_all(plugins_dir).map_err(|error| {
            format!(
                "Failed to create plugin directory {}: {error}",
                plugins_dir.display()
            )
        })?;
        let installed_count = fs::read_dir(plugins_dir)
            .map_err(|error| format!("Failed to read {}: {error}", plugins_dir.display()))?
            .filter_map(Result::ok)
            .filter(|entry| {
                !entry.file_name().to_string_lossy().starts_with('.')
                    && entry.file_type().is_ok_and(|file_type| file_type.is_dir())
            })
            .count();
        if installed_count >= MAX_INSTALLED_PLUGINS {
            return Err(format!(
                "Termy supports at most {MAX_INSTALLED_PLUGINS} installed plugins"
            ));
        }
        let destination = plugins_dir.join(&manifest.id);
        match fs::symlink_metadata(&destination) {
            Ok(_) => return Err(format!("Plugin `{}` is already installed", manifest.id)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Failed to inspect plugin destination {}: {error}",
                    destination.display()
                ));
            }
        }

        let mut random = [0_u8; 8];
        OsRng.fill_bytes(&mut random);
        let temporary =
            plugins_dir.join(format!(".install-{}-{}", manifest.id, hex_digest(&random)));
        let install_result = (|| {
            write_plugin_installation(&temporary, &files, source_metadata.as_ref(), true)?;
            fs::rename(&temporary, &destination)
                .map_err(|error| format!("Failed to finish plugin installation: {error}"))
        })();
        if let Err(error) = install_result {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error);
        }
        self.invalidate_after_management(&manifest.id);
        Ok(installed_plugin_from_manifest(
            manifest,
            destination,
            true,
            source_metadata,
            None,
        ))
    }

    pub fn update_plugin_from_directory(
        &self,
        id: &str,
        source: &Path,
        source_metadata: PluginSourceMetadata,
    ) -> Result<InstalledPlugin, String> {
        validate_source_metadata(&source_metadata)?;
        self.replace_plugin_from_directory(id, source, Some(source_metadata))
    }

    pub fn sync_from_directory(&self, source: &Path) -> Result<InstalledPlugin, String> {
        let manifest = source_plugin_manifest(source)?;
        let plugins_dir = self
            .inner
            .plugins_dir
            .as_deref()
            .ok_or_else(|| "Termy config path is unavailable".to_string())?;
        let destination = plugins_dir.join(&manifest.id);
        match fs::symlink_metadata(&destination) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.install_from_directory(source);
            }
            Err(error) => {
                return Err(format!(
                    "Failed to inspect plugin destination {}: {error}",
                    destination.display()
                ));
            }
        }
        let destination = self.plugin_root_for_management(&manifest.id)?;
        let source_path = fs::canonicalize(source).map_err(|error| {
            format!(
                "Failed to resolve plugin source {}: {error}",
                source.display()
            )
        })?;
        let destination_path = fs::canonicalize(&destination).map_err(|error| {
            format!(
                "Failed to resolve plugin destination {}: {error}",
                destination.display()
            )
        })?;
        if source_path == destination_path {
            return Err(
                "Plugin development source must be outside Termy's managed plugins directory"
                    .to_string(),
            );
        }
        if read_source_metadata(&destination)?.is_some() {
            return Err(format!(
                "Plugin `{}` is tracked from GitHub; uninstall it before starting local development",
                manifest.id
            ));
        }
        self.replace_plugin_from_directory(&manifest.id, source, None)
    }

    fn replace_plugin_from_directory(
        &self,
        id: &str,
        source: &Path,
        source_metadata: Option<PluginSourceMetadata>,
    ) -> Result<InstalledPlugin, String> {
        let destination = self.plugin_root_for_management(id)?;
        let enabled = plugin_is_enabled(&destination, id)?;
        let (manifest, files) = inspect_plugin_root(source, id)?;
        let plugins_dir = destination
            .parent()
            .ok_or_else(|| "Managed plugin directory has no parent".to_string())?;
        let mut random = [0_u8; 8];
        OsRng.fill_bytes(&mut random);
        let suffix = hex_digest(&random);
        let temporary = plugins_dir.join(format!(".update-{id}-{suffix}"));
        let backup = plugins_dir.join(format!(".backup-{id}-{suffix}"));

        if let Err(error) =
            write_plugin_installation(&temporary, &files, source_metadata.as_ref(), enabled)
        {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error);
        }
        if let Err(error) = fs::rename(&destination, &backup) {
            let _ = fs::remove_dir_all(&temporary);
            return Err(format!("Failed to prepare plugin `{id}` update: {error}"));
        }
        if let Err(error) = fs::rename(&temporary, &destination) {
            let restore_error = fs::rename(&backup, &destination).err();
            let _ = fs::remove_dir_all(&temporary);
            return Err(match restore_error {
                Some(restore_error) => format!(
                    "Failed to install plugin `{id}` update: {error}; restoring the previous plugin also failed: {restore_error}"
                ),
                None => format!("Failed to install plugin `{id}` update: {error}"),
            });
        }
        let _ = fs::remove_dir_all(&backup);
        self.clear_plugin_bundle_cache(id);
        self.invalidate_after_management(id);
        Ok(installed_plugin_from_manifest(
            manifest,
            destination,
            enabled,
            source_metadata,
            None,
        ))
    }

    pub fn set_plugin_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let root = self.plugin_root_for_management(id)?;
        let marker = root.join(DISABLED_MARKER);
        if enabled {
            match fs::remove_file(&marker) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Failed to enable plugin `{id}`: {error}")),
            }
        } else {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)
            {
                Ok(mut file) => file
                    .write_all(b"Managed by Termy settings.\n")
                    .map_err(|error| format!("Failed to disable plugin `{id}`: {error}"))?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(&marker).map_err(|error| {
                        format!("Failed to inspect disabled state for plugin `{id}`: {error}")
                    })?;
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(format!(
                            "Plugin `{id}` has an invalid {DISABLED_MARKER} marker"
                        ));
                    }
                }
                Err(error) => return Err(format!("Failed to disable plugin `{id}`: {error}")),
            }
        }
        self.invalidate_after_management(id);
        Ok(())
    }

    pub fn uninstall_plugin(&self, id: &str) -> Result<(), String> {
        let root = self.plugin_root_for_management(id)?;
        {
            let _settings = self
                .inner
                .settings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let stored = self.read_plugin_settings_file(id)?;
            for key in stored.secret_keys {
                delete_plugin_secret(id, &key)?;
            }
        }
        fs::remove_dir_all(&root)
            .map_err(|error| format!("Failed to uninstall plugin `{id}`: {error}"))?;
        self.clear_plugin_bundle_cache(id);
        self.clear_plugin_storage(id);
        self.invalidate_after_management(id);
        Ok(())
    }

    fn clear_plugin_bundle_cache(&self, id: &str) {
        let Some(plugins_dir) = self.inner.plugins_dir.as_deref() else {
            return;
        };
        let cache = plugins_dir.join(".termy-cache/bundles").join(id);
        let _ = fs::remove_dir_all(cache);
    }

    fn clear_plugin_storage(&self, id: &str) {
        let Some(plugins_dir) = self.inner.plugins_dir.as_deref() else {
            return;
        };
        let _ = fs::remove_dir_all(plugins_dir.join(".termy-data").join(id));
        let _ = fs::remove_dir_all(plugins_dir.join(".termy-cache/data").join(id));
    }

    fn plugin_root_for_management(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err(format!("Invalid plugin ID `{id}`"));
        }
        let plugins_dir = self
            .inner
            .plugins_dir
            .as_deref()
            .ok_or_else(|| "Termy config path is unavailable".to_string())?;
        let root = plugins_dir.join(id);
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| format!("Plugin `{id}` is not installed: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!("Plugin `{id}` is not a managed plugin directory"));
        }
        Ok(root)
    }

    fn invalidate_after_management(&self, plugin_id: &str) {
        self.bump_plugin_lifecycle(plugin_id);
        self.inner
            .host
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .connection
            .take();
        self.inner
            .catalog
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .fingerprint = None;
    }

    fn plugin_lifecycle_generation(&self, plugin_id: &str) -> u64 {
        self.inner
            .lifecycle_generations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(plugin_id)
            .copied()
            .unwrap_or_default()
    }

    fn bump_plugin_lifecycle(&self, plugin_id: &str) {
        let mut generations = self
            .inner
            .lifecycle_generations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = generations.entry(plugin_id.to_string()).or_default();
        *generation = generation.wrapping_add(1);
    }

    fn revoke_changed_plugin_lifecycles(
        &self,
        previous: &BTreeMap<String, String>,
        current: &BTreeMap<String, String>,
    ) {
        let changed = previous
            .keys()
            .chain(current.keys())
            .filter(|plugin_id| previous.get(*plugin_id) != current.get(*plugin_id))
            .cloned()
            .collect::<BTreeSet<_>>();
        if changed.is_empty() {
            return;
        }
        let mut generations = self
            .inner
            .lifecycle_generations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for plugin_id in changed {
            let generation = generations.entry(plugin_id).or_default();
            *generation = generation.wrapping_add(1);
        }
    }

    fn ensure_plugin_lifecycle(
        &self,
        plugin_id: &str,
        expected_generation: u64,
    ) -> Result<(), String> {
        if self.plugin_lifecycle_generation(plugin_id) == expected_generation {
            Ok(())
        } else {
            Err(format!(
                "Plugin `{plugin_id}` changed, was disabled, or was removed while its request was running; try again"
            ))
        }
    }

    pub fn commands(&self) -> Vec<PluginCommand> {
        self.inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .commands
            .clone()
    }

    pub fn views(&self) -> Vec<PluginViewDescriptor> {
        self.inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .views
            .clone()
    }

    pub fn plugin_settings_snapshot(&self) -> PluginSettingsSnapshot {
        let definitions = self
            .inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .settings
            .clone();
        let _settings = self
            .inner
            .settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut snapshot = PluginSettingsSnapshot::default();
        for (plugin_id, settings) in definitions {
            let stored = match self.read_plugin_settings_file(&plugin_id) {
                Ok(stored) => stored,
                Err(error) => {
                    snapshot.errors.push(error);
                    PluginSettingsFile::default()
                }
            };
            let mut states = Vec::with_capacity(settings.len());
            for definition in settings {
                let (value, configured) = if let PluginSetting::Secret { id, .. } = &definition {
                    if !stored.secret_keys.contains(id) {
                        (None, false)
                    } else {
                        match read_plugin_secret(&plugin_id, id) {
                            Ok(value) => (None, value.is_some()),
                            Err(error) => {
                                snapshot.errors.push(error);
                                (None, false)
                            }
                        }
                    }
                } else {
                    let value = stored
                        .values
                        .get(definition.id())
                        .filter(|value| validate_setting_value(&definition, value).is_ok())
                        .cloned()
                        .or_else(|| definition.default_value());
                    let configured = stored.values.contains_key(definition.id());
                    (value, configured)
                };
                states.push(PluginSettingState {
                    definition,
                    value,
                    configured,
                });
            }
            snapshot.plugins.insert(plugin_id, states);
        }
        snapshot
    }

    pub fn set_plugin_setting(
        &self,
        plugin_id: &str,
        key: &str,
        value: Value,
    ) -> Result<(), String> {
        let definition = self.plugin_setting_definition(plugin_id, key)?;
        validate_setting_value(&definition, &value)?;
        let _settings = self
            .inner
            .settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut stored = self.read_plugin_settings_file(plugin_id)?;
        match &definition {
            PluginSetting::Secret { .. } => {
                let secret = value
                    .as_str()
                    .expect("validated secret setting must be a string");
                if secret.is_empty() {
                    delete_plugin_secret(plugin_id, key)?;
                    stored.secret_keys.remove(key);
                } else {
                    write_plugin_secret(plugin_id, key, secret)?;
                    stored.secret_keys.insert(key.to_string());
                }
            }
            _ if definition.default_value().as_ref() == Some(&value) => {
                stored.values.remove(key);
            }
            _ => {
                stored.values.insert(key.to_string(), value);
            }
        }
        self.write_plugin_settings_file(plugin_id, &stored)
    }

    pub fn reset_plugin_setting(&self, plugin_id: &str, key: &str) -> Result<(), String> {
        let definition = self.plugin_setting_definition(plugin_id, key)?;
        let _settings = self
            .inner
            .settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut stored = self.read_plugin_settings_file(plugin_id)?;
        stored.values.remove(key);
        if matches!(definition, PluginSetting::Secret { .. }) {
            delete_plugin_secret(plugin_id, key)?;
            stored.secret_keys.remove(key);
        }
        self.write_plugin_settings_file(plugin_id, &stored)
    }

    fn plugin_setting_definition(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Result<PluginSetting, String> {
        self.plugin_root_for_management(plugin_id)?;
        let catalog = self
            .inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        catalog
            .settings
            .get(plugin_id)
            .and_then(|settings| settings.iter().find(|setting| setting.id() == key))
            .cloned()
            .ok_or_else(|| format!("Plugin setting `{plugin_id}.{key}` is not available"))
    }

    fn resolved_plugin_settings(&self, plugin_id: &str) -> Result<BTreeMap<String, Value>, String> {
        let definitions = self
            .inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .settings
            .get(plugin_id)
            .cloned()
            .unwrap_or_default();
        let _settings = self
            .inner
            .settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stored = self.read_plugin_settings_file(plugin_id)?;
        let mut values = BTreeMap::new();
        for definition in definitions {
            let value = match &definition {
                PluginSetting::Secret { id, .. } if stored.secret_keys.contains(id) => {
                    read_plugin_secret(plugin_id, id)?.map(Value::String)
                }
                PluginSetting::Secret { .. } => None,
                _ => stored
                    .values
                    .get(definition.id())
                    .filter(|value| validate_setting_value(&definition, value).is_ok())
                    .cloned()
                    .or_else(|| definition.default_value()),
            };
            if let Some(value) = value {
                values.insert(definition.id().to_string(), value);
            }
        }
        Ok(values)
    }

    fn plugin_settings_file_path(&self, plugin_id: &str) -> Result<PathBuf, String> {
        if !valid_id(plugin_id) {
            return Err(format!("Invalid plugin ID `{plugin_id}`"));
        }
        let plugins_dir = self
            .inner
            .plugins_dir
            .as_deref()
            .ok_or_else(|| "Termy config path is unavailable".to_string())?;
        Ok(plugins_dir
            .join(".termy-data")
            .join(plugin_id)
            .join(SETTINGS_FILE))
    }

    fn read_plugin_settings_file(&self, plugin_id: &str) -> Result<PluginSettingsFile, String> {
        let path = self.plugin_settings_file_path(plugin_id)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PluginSettingsFile::default());
            }
            Err(error) => {
                return Err(format!(
                    "Failed to inspect plugin settings {}: {error}",
                    path.display()
                ));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "Plugin settings must be a regular file: {}",
                path.display()
            ));
        }
        if metadata.len() > MAX_SETTINGS_FILE_BYTES {
            return Err(format!(
                "Plugin settings exceed the 64 KiB limit: {plugin_id}"
            ));
        }
        serde_json::from_slice(
            &fs::read(&path)
                .map_err(|error| format!("Failed to read {}: {error}", path.display()))?,
        )
        .map_err(|error| format!("Invalid plugin settings for `{plugin_id}`: {error}"))
    }

    fn write_plugin_settings_file(
        &self,
        plugin_id: &str,
        settings: &PluginSettingsFile,
    ) -> Result<(), String> {
        let path = self.plugin_settings_file_path(plugin_id)?;
        if settings.values.is_empty() && settings.secret_keys.is_empty() {
            return match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("Failed to clear {}: {error}", path.display())),
            };
        }
        let contents = serde_json::to_vec_pretty(settings)
            .map_err(|error| format!("Failed to encode plugin settings: {error}"))?;
        if contents.len() as u64 > MAX_SETTINGS_FILE_BYTES {
            return Err(format!(
                "Plugin settings exceed the 64 KiB limit: {plugin_id}"
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| "Plugin settings path has no parent".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
        let mut random = [0_u8; 8];
        OsRng.fill_bytes(&mut random);
        let temporary = parent.join(format!(".settings-{}.tmp", hex_digest(&random)));
        let result = (|| {
            fs::write(&temporary, contents)
                .map_err(|error| format!("Failed to write {}: {error}", temporary.display()))?;
            fs::rename(&temporary, &path)
                .map_err(|error| format!("Failed to save {}: {error}", path.display()))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn command_with_revision(
        &self,
        plugin_id: &str,
        command_id: &str,
    ) -> Option<(PluginCommand, String)> {
        let catalog = self
            .inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let command = catalog
            .commands
            .iter()
            .find(|command| command.plugin_id == plugin_id && command.id == command_id)?
            .clone();
        let revision = catalog.revisions.get(plugin_id)?.clone();
        Some((command, revision))
    }

    pub fn view_with_revision(
        &self,
        plugin_id: &str,
        view_id: &str,
    ) -> Option<(PluginViewDescriptor, String)> {
        let catalog = self
            .inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let view = catalog
            .views
            .iter()
            .find(|view| view.plugin_id == plugin_id && view.id == view_id)?
            .clone();
        let revision = catalog.revisions.get(plugin_id)?.clone();
        Some((view, revision))
    }

    pub fn refresh_if_changed(&self) -> PluginRefresh {
        match self.refresh_if_changed_inner(false) {
            Ok(refresh) => refresh,
            Err(error) => PluginRefresh {
                changed: false,
                errors: vec![error],
            },
        }
    }

    /// Stop the external Bun host when the loaded catalog has no lifecycle
    /// subscribers. Commands and native views remain available from the Rust
    /// catalog and restart the host on demand.
    pub fn suspend_if_eventless(&self) -> bool {
        let has_event_subscribers = !self
            .inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .events
            .is_empty();
        if has_event_subscribers {
            return false;
        }
        self.inner
            .host
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .connection
            .take()
            .is_some()
    }

    fn refresh_if_changed_inner(&self, require_host: bool) -> Result<PluginRefresh, String> {
        let Some(plugins_dir) = self.inner.plugins_dir.as_deref() else {
            return Ok(PluginRefresh::default());
        };
        let _refresh = self
            .inner
            .refresh
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let discovered = discover_plugins(plugins_dir)?;
        let (previous_fingerprint, had_commands, previous_revisions) = {
            let catalog = self
                .inner
                .catalog
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                catalog.fingerprint,
                !catalog.commands.is_empty(),
                catalog.revisions.clone(),
            )
        };
        let (host_available, host_failed) = if discovered.sources.is_empty() {
            (false, false)
        } else {
            let mut host = self
                .inner
                .host
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let failed = host
                .connection
                .as_ref()
                .is_some_and(|connection| connection.is_failed());
            if failed {
                host.connection.take();
            }
            (host.connection.is_some(), failed)
        };
        if previous_fingerprint == Some(discovered.fingerprint)
            && !host_failed
            && (!require_host || host_available)
        {
            return Ok(PluginRefresh::default());
        }
        if previous_fingerprint != Some(discovered.fingerprint) {
            let discovered_revisions = discovered
                .sources
                .iter()
                .map(|source| (source.id.clone(), source.cache_key.clone()))
                .collect::<BTreeMap<_, _>>();
            self.revoke_changed_plugin_lifecycles(&previous_revisions, &discovered_revisions);
        }

        ensure_managed_files(plugins_dir)?;
        if discovered.sources.is_empty() {
            let mut host = self
                .inner
                .host
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            host.connection.take();
            let mut catalog = self
                .inner
                .catalog
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            catalog.fingerprint = Some(discovered.fingerprint);
            catalog.commands.clear();
            catalog.events.clear();
            catalog.views.clear();
            catalog.settings.clear();
            catalog.revisions.clear();
            let bundles = plugins_dir.join(".termy-cache/bundles");
            if bundles.exists() {
                fs::remove_dir_all(&bundles).map_err(|error| {
                    format!(
                        "Failed to clear plugin bundle cache {}: {error}",
                        bundles.display()
                    )
                })?;
            }
            return Ok(PluginRefresh {
                changed: previous_fingerprint.is_some() || had_commands,
                errors: Vec::new(),
            });
        }

        let (request_id, connection) = {
            let mut host = self
                .inner
                .host
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if host.connection.is_none() {
                host.connection = Some(Arc::new(HostConnection::spawn(plugins_dir)?));
            }
            let request_id = host.next_id();
            let connection = Arc::clone(
                host.connection
                    .as_ref()
                    .expect("plugin host connection must exist"),
            );
            (request_id, connection)
        };
        let request = HostRequest::Load {
            id: request_id,
            plugins: &discovered.sources,
        };
        let load_result = match connection.request::<HostLoadResult>(&request, LOAD_TIMEOUT, None) {
            Ok(result) => result,
            Err(error) => {
                if matches!(error, HostRequestError::Transport(_)) {
                    let mut host = self
                        .inner
                        .host
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if host
                        .connection
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &connection))
                    {
                        host.connection.take();
                    }
                }
                return Err(error.into_message());
            }
        };
        let mut errors = load_result.errors;
        let source_revisions = discovered
            .sources
            .iter()
            .map(|source| (source.id.as_str(), source.cache_key.as_str()))
            .collect::<HashMap<_, _>>();
        let mut seen_plugins = HashSet::new();
        let mut commands = Vec::new();
        let mut events = Vec::new();
        let mut views = Vec::new();
        let mut settings = BTreeMap::new();
        let mut revisions = BTreeMap::new();
        for loaded in load_result.plugins {
            let Some(revision) = source_revisions.get(loaded.plugin_id.as_str()) else {
                errors.push(format!(
                    "{}: runtime returned an unknown plugin",
                    loaded.plugin_id
                ));
                continue;
            };
            if !seen_plugins.insert(loaded.plugin_id.clone()) {
                errors.push(format!(
                    "{}: runtime returned the plugin more than once",
                    loaded.plugin_id
                ));
                continue;
            }
            let plugin_commands =
                match serde_json::from_value::<Vec<PluginCommand>>(loaded.commands) {
                    Ok(commands) => commands,
                    Err(error) => {
                        errors.push(format!(
                            "{}: invalid command descriptor: {error}",
                            loaded.plugin_id
                        ));
                        continue;
                    }
                };
            if plugin_commands
                .iter()
                .any(|command| command.plugin_id != loaded.plugin_id)
            {
                errors.push(format!(
                    "{}: command descriptor used the wrong plugin ID",
                    loaded.plugin_id
                ));
                continue;
            }
            if let Err(error) = validate_commands(&plugin_commands) {
                errors.push(format!("{}: {error}", loaded.plugin_id));
                continue;
            }
            let plugin_events = match serde_json::from_value::<Vec<PluginEventSubscriptionDescriptor>>(
                loaded.events,
            ) {
                Ok(events) => events,
                Err(error) => {
                    errors.push(format!(
                        "{}: invalid event subscription: {error}",
                        loaded.plugin_id
                    ));
                    continue;
                }
            };
            if plugin_events
                .iter()
                .any(|subscription| subscription.plugin_id != loaded.plugin_id)
            {
                errors.push(format!(
                    "{}: event subscription used the wrong plugin ID",
                    loaded.plugin_id
                ));
                continue;
            }
            if let Err(error) = validate_event_subscriptions(&plugin_events) {
                errors.push(format!("{}: {error}", loaded.plugin_id));
                continue;
            }
            let plugin_views =
                match serde_json::from_value::<Vec<PluginViewDescriptor>>(loaded.views) {
                    Ok(views) => views,
                    Err(error) => {
                        errors.push(format!(
                            "{}: invalid view descriptor: {error}",
                            loaded.plugin_id
                        ));
                        continue;
                    }
                };
            if plugin_views
                .iter()
                .any(|view| view.plugin_id != loaded.plugin_id)
            {
                errors.push(format!(
                    "{}: view descriptor used the wrong plugin ID",
                    loaded.plugin_id
                ));
                continue;
            }
            if let Err(error) = validate_views(&plugin_views) {
                errors.push(format!("{}: {error}", loaded.plugin_id));
                continue;
            }
            let plugin_settings =
                match serde_json::from_value::<Vec<PluginSetting>>(loaded.settings) {
                    Ok(settings) => settings,
                    Err(error) => {
                        errors.push(format!(
                            "{}: invalid settings descriptor: {error}",
                            loaded.plugin_id
                        ));
                        continue;
                    }
                };
            if let Err(error) = validate_plugin_settings(&plugin_settings) {
                errors.push(format!("{}: {error}", loaded.plugin_id));
                continue;
            }
            commands.extend(plugin_commands);
            views.extend(plugin_views);
            events.extend(
                plugin_events
                    .into_iter()
                    .map(|subscription| RegisteredPluginEvent {
                        plugin_id: subscription.plugin_id,
                        event: subscription.event,
                        timeout_ms: subscription.timeout_ms,
                        revision: (*revision).to_string(),
                    }),
            );
            settings.insert(loaded.plugin_id.clone(), plugin_settings);
            revisions.insert(loaded.plugin_id, (*revision).to_string());
        }
        validate_commands(&commands)?;
        validate_views(&views)?;
        let mut catalog = self
            .inner
            .catalog
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        catalog.fingerprint = errors.is_empty().then_some(discovered.fingerprint);
        catalog.commands = commands;
        catalog.events = events;
        catalog.views = views;
        catalog.settings = settings;
        catalog.revisions = revisions;
        Ok(PluginRefresh {
            changed: true,
            errors,
        })
    }

    pub fn invoke(
        &self,
        plugin_id: &str,
        command_id: &str,
        expected_revision: &str,
        inputs: BTreeMap<String, Value>,
        context: PluginContext,
    ) -> Result<Vec<PluginAction>, String> {
        self.invoke_inner(
            plugin_id,
            command_id,
            expected_revision,
            inputs,
            context,
            None,
        )
    }

    pub fn invoke_with_control(
        &self,
        plugin_id: &str,
        command_id: &str,
        expected_revision: &str,
        inputs: BTreeMap<String, Value>,
        context: PluginContext,
        control: &PluginInvocationControl,
    ) -> Result<Vec<PluginAction>, String> {
        self.invoke_inner(
            plugin_id,
            command_id,
            expected_revision,
            inputs,
            context,
            Some(control),
        )
    }

    fn invoke_inner(
        &self,
        plugin_id: &str,
        command_id: &str,
        expected_revision: &str,
        inputs: BTreeMap<String, Value>,
        mut context: PluginContext,
        control: Option<&PluginInvocationControl>,
    ) -> Result<Vec<PluginAction>, String> {
        let (command, current_revision) = self
            .command_with_revision(plugin_id, command_id)
            .ok_or_else(|| format!("Plugin command {plugin_id}.{command_id} is not available"))?;
        if current_revision != expected_revision {
            return Err(
                "Plugin changed while its input form was open; run the command again".to_string(),
            );
        }
        validate_inputs(&command, &inputs)?;
        context.settings = self.resolved_plugin_settings(plugin_id)?;
        let timeout_ms = command.timeout_ms.clamp(100, MAX_INVOKE_TIMEOUT_MS);
        let lifecycle_generation = self.plugin_lifecycle_generation(plugin_id);
        let (request_id, connection) = self.next_host_request()?;
        let request = HostRequest::Invoke {
            id: request_id,
            plugin_id,
            command_id,
            revision: expected_revision,
            inputs: &inputs,
            context: &context,
        };
        self.request_actions(
            &connection,
            &request,
            timeout_ms,
            ActionRequestContext {
                plugin_id,
                revision: expected_revision,
                lifecycle_generation,
                origin: &context.origin,
                control,
            },
        )
    }

    pub fn resolve_pick_options(
        &self,
        plugin_id: &str,
        command_id: &str,
        input_id: &str,
        expected_revision: &str,
        query: &str,
        mut context: PluginContext,
    ) -> Result<Vec<PluginSelectOption>, String> {
        let (command, current_revision) = self
            .command_with_revision(plugin_id, command_id)
            .ok_or_else(|| format!("Plugin command {plugin_id}.{command_id} is not available"))?;
        if current_revision != expected_revision {
            return Err(
                "Plugin changed while its picker was open; run the command again".to_string(),
            );
        }
        if !command
            .inputs
            .iter()
            .any(|input| matches!(input, PluginInput::Pick { id, .. } if id == input_id))
        {
            return Err(format!(
                "Plugin command {plugin_id}.{command_id} has no async picker `{input_id}`"
            ));
        }
        validate_optional_text(query, 1_024, "picker query")?;
        context.settings = self.resolved_plugin_settings(plugin_id)?;
        let timeout_ms = command.timeout_ms.clamp(100, MAX_INVOKE_TIMEOUT_MS);
        let lifecycle_generation = self.plugin_lifecycle_generation(plugin_id);
        let (request_id, connection) = self.next_host_request()?;
        let request = HostRequest::InputOptions {
            id: request_id,
            plugin_id,
            command_id,
            input_id,
            revision: expected_revision,
            query,
            context: &context,
        };
        let result =
            self.request_host::<HostPickResult>(&connection, &request, timeout_ms, None)?;
        self.ensure_plugin_lifecycle(plugin_id, lifecycle_generation)?;
        self.ensure_command_revision(plugin_id, command_id, expected_revision)?;
        validate_select_options(&result.options, "picker")?;
        Ok(result.options)
    }

    pub fn render_view(
        &self,
        plugin_id: &str,
        view_id: &str,
        expected_revision: &str,
        params: Value,
        mut context: PluginContext,
    ) -> Result<PluginViewRender, String> {
        let (view, current_revision) = self
            .view_with_revision(plugin_id, view_id)
            .ok_or_else(|| format!("Plugin view {plugin_id}.{view_id} is not available"))?;
        if current_revision != expected_revision {
            return Err("Plugin changed while its view was open; reopen the view".to_string());
        }
        context.settings = self.resolved_plugin_settings(plugin_id)?;
        validate_view_params(&params)?;
        let timeout_ms = view.timeout_ms.clamp(100, MAX_INVOKE_TIMEOUT_MS);
        let lifecycle_generation = self.plugin_lifecycle_generation(plugin_id);
        let (request_id, connection) = self.next_host_request()?;
        let request = HostRequest::ViewRender {
            id: request_id,
            plugin_id,
            view_id,
            revision: expected_revision,
            params: &params,
            context: &context,
        };
        let result =
            self.request_host::<HostViewRenderResult>(&connection, &request, timeout_ms, None)?;
        self.ensure_plugin_lifecycle(plugin_id, lifecycle_generation)?;
        self.ensure_view_revision(plugin_id, view_id, expected_revision)?;
        self.prepare_view_render(
            result,
            plugin_id,
            expected_revision,
            params,
            &context.origin,
        )
    }

    pub fn invoke_view_action(
        &self,
        plugin_id: &str,
        view_id: &str,
        expected_revision: &str,
        params: Value,
        action: PluginViewAction,
        values: BTreeMap<String, PluginViewValue>,
        mut context: PluginContext,
    ) -> Result<PluginViewRender, String> {
        let (view, current_revision) = self
            .view_with_revision(plugin_id, view_id)
            .ok_or_else(|| format!("Plugin view {plugin_id}.{view_id} is not available"))?;
        if current_revision != expected_revision {
            return Err("Plugin changed while its view was open; reopen the view".to_string());
        }
        validate_view_action(&action, &values)?;
        validate_view_params(&params)?;
        context.settings = self.resolved_plugin_settings(plugin_id)?;
        let timeout_ms = view.timeout_ms.clamp(100, MAX_INVOKE_TIMEOUT_MS);
        let lifecycle_generation = self.plugin_lifecycle_generation(plugin_id);
        let (request_id, connection) = self.next_host_request()?;
        let request = HostRequest::ViewAction {
            id: request_id,
            plugin_id,
            view_id,
            revision: expected_revision,
            params: &params,
            action: &action,
            values: &values,
            context: &context,
        };
        let result =
            self.request_host::<HostViewRenderResult>(&connection, &request, timeout_ms, None)?;
        self.ensure_plugin_lifecycle(plugin_id, lifecycle_generation)?;
        self.ensure_view_revision(plugin_id, view_id, expected_revision)?;
        self.prepare_view_render(
            result,
            plugin_id,
            expected_revision,
            params,
            &context.origin,
        )
    }

    pub fn has_event_subscribers(&self, event: PluginEventKind) -> bool {
        self.inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .events
            .iter()
            .any(|subscription| subscription.event == event)
    }

    pub fn dispatch_event(
        &self,
        event: PluginEvent,
        context: PluginContext,
    ) -> PluginEventDispatch {
        let subscriptions = self
            .inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .events
            .iter()
            .filter(|subscription| subscription.event == event.kind())
            .cloned()
            .collect::<Vec<_>>();
        if subscriptions.is_empty() {
            return PluginEventDispatch::default();
        }

        let results = thread::scope(|scope| {
            subscriptions
                .iter()
                .map(|subscription| {
                    let event = &event;
                    let context = context.clone();
                    scope.spawn(move || self.dispatch_event_to_plugin(subscription, event, context))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join())
                .collect::<Vec<_>>()
        });

        let mut dispatch = PluginEventDispatch::default();
        for (subscription, result) in subscriptions.iter().zip(results) {
            match result {
                Ok(Ok(actions)) => dispatch.actions.extend(actions),
                Ok(Err(error)) => dispatch
                    .errors
                    .push(format!("{}: {error}", subscription.plugin_id)),
                Err(_) => dispatch.errors.push(format!(
                    "{}: plugin event worker panicked",
                    subscription.plugin_id
                )),
            }
        }
        dispatch
    }

    fn dispatch_event_to_plugin(
        &self,
        subscription: &RegisteredPluginEvent,
        event: &PluginEvent,
        mut context: PluginContext,
    ) -> Result<Vec<PluginAction>, String> {
        context.settings = self.resolved_plugin_settings(&subscription.plugin_id)?;
        let lifecycle_generation = self.plugin_lifecycle_generation(&subscription.plugin_id);
        let (request_id, connection) = self.next_host_request()?;
        let request = HostRequest::Event {
            id: request_id,
            plugin_id: &subscription.plugin_id,
            revision: &subscription.revision,
            event,
            context: &context,
        };
        self.request_actions(
            &connection,
            &request,
            subscription.timeout_ms,
            ActionRequestContext {
                plugin_id: &subscription.plugin_id,
                revision: &subscription.revision,
                lifecycle_generation,
                origin: &context.origin,
                control: None,
            },
        )
    }

    fn next_host_request(&self) -> Result<(u64, Arc<HostConnection>), String> {
        {
            let mut host = self
                .inner
                .host
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(connection) = host
                .connection
                .as_ref()
                .filter(|connection| !connection.is_failed())
                .map(Arc::clone)
            {
                let request_id = host.next_id();
                return Ok((request_id, connection));
            }
            host.connection.take();
        }
        self.refresh_if_changed_inner(true)?;

        let mut host = self
            .inner
            .host
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request_id = host.next_id();
        let connection = host.connection.as_ref().map(Arc::clone).ok_or_else(|| {
            "Plugin runtime is unavailable; reload plugins and try again".to_string()
        })?;
        Ok((request_id, connection))
    }

    fn request_actions(
        &self,
        connection: &Arc<HostConnection>,
        request: &HostRequest<'_>,
        timeout_ms: u64,
        context: ActionRequestContext<'_>,
    ) -> Result<Vec<PluginAction>, String> {
        let invoke_result = self.request_host::<HostInvokeResult>(
            connection,
            request,
            timeout_ms,
            context.control,
        )?;
        self.ensure_plugin_lifecycle(context.plugin_id, context.lifecycle_generation)?;
        self.prepare_actions(
            invoke_result.actions,
            context.plugin_id,
            context.revision,
            context.origin,
        )
    }

    fn request_host<T: DeserializeOwned>(
        &self,
        connection: &Arc<HostConnection>,
        request: &HostRequest<'_>,
        timeout_ms: u64,
        control: Option<&PluginInvocationControl>,
    ) -> Result<T, String> {
        let timeout = Duration::from_millis(
            timeout_ms
                .saturating_add(MAX_INVOKE_QUEUE_WAIT_MS)
                .saturating_add(1_000),
        );
        let result = match connection.request::<T>(request, timeout, control) {
            Ok(result) => result,
            Err(error) => {
                if matches!(error, HostRequestError::Transport(_)) {
                    let mut host = self
                        .inner
                        .host
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if host
                        .connection
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, connection))
                    {
                        host.connection.take();
                    }
                }
                if !matches!(error, HostRequestError::Local(_)) {
                    self.inner
                        .catalog
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .fingerprint = None;
                }
                return Err(error.into_message());
            }
        };
        Ok(result)
    }

    fn prepare_view_render(
        &self,
        result: HostViewRenderResult,
        plugin_id: &str,
        revision: &str,
        params: Value,
        origin: &PluginOriginContext,
    ) -> Result<PluginViewRender, String> {
        validate_view_nodes(&result.nodes)?;
        let actions = self.prepare_actions(result.actions, plugin_id, revision, origin)?;
        Ok(PluginViewRender {
            plugin_id: plugin_id.to_string(),
            revision: revision.to_string(),
            params,
            nodes: result.nodes,
            actions,
        })
    }

    fn ensure_view_revision(
        &self,
        plugin_id: &str,
        view_id: &str,
        expected_revision: &str,
    ) -> Result<(), String> {
        let (_, current_revision) = self
            .view_with_revision(plugin_id, view_id)
            .ok_or_else(|| format!("Plugin view {plugin_id}.{view_id} is no longer available"))?;
        if current_revision != expected_revision {
            return Err(
                "Plugin changed while its view request was running; reopen the view".to_string(),
            );
        }
        Ok(())
    }

    fn ensure_command_revision(
        &self,
        plugin_id: &str,
        command_id: &str,
        expected_revision: &str,
    ) -> Result<(), String> {
        let (_, current_revision) = self
            .command_with_revision(plugin_id, command_id)
            .ok_or_else(|| {
                format!("Plugin command {plugin_id}.{command_id} is no longer available")
            })?;
        if current_revision != expected_revision {
            return Err(
                "Plugin changed while its picker request was running; run the command again"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn prepare_actions(
        &self,
        mut actions: Vec<PluginAction>,
        plugin_id: &str,
        revision: &str,
        origin: &PluginOriginContext,
    ) -> Result<Vec<PluginAction>, String> {
        validate_actions(&actions)?;
        for action in &mut actions {
            match action {
                PluginAction::TerminalSendText { target, .. }
                | PluginAction::TerminalOpen { target, .. } => {
                    if matches!(
                        target,
                        PluginTerminalTarget::Named(PluginTerminalTargetKind::Origin)
                    ) {
                        *target = PluginTerminalTarget::Exact {
                            window_id: origin.window_id.clone(),
                            tab_id: origin.tab_id.clone(),
                            pane_id: origin.pane_id.clone(),
                        };
                    }
                }
                PluginAction::ViewOpen {
                    view,
                    plugin_id: origin_plugin_id,
                    revision: origin_revision,
                    ..
                }
                | PluginAction::ViewReplace {
                    view,
                    plugin_id: origin_plugin_id,
                    revision: origin_revision,
                    ..
                } => {
                    let Some((_, current_revision)) = self.view_with_revision(plugin_id, view)
                    else {
                        return Err(format!("Plugin returned unknown view `{view}`"));
                    };
                    if current_revision != revision {
                        return Err(
                            "Plugin changed while returning a view action; try again".to_string()
                        );
                    }
                    *origin_plugin_id = plugin_id.to_string();
                    *origin_revision = revision.to_string();
                }
                PluginAction::ViewClose {
                    plugin_id: origin_plugin_id,
                    revision: origin_revision,
                } => {
                    *origin_plugin_id = plugin_id.to_string();
                    *origin_revision = revision.to_string();
                }
                _ => {}
            }
        }
        Ok(actions)
    }
}

impl PluginHostState {
    fn next_id(&mut self) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.next_request_id
    }
}

struct HostConnection {
    child: Arc<Mutex<Child>>,
    writer: Mutex<TcpStream>,
    pending: PendingHostRequests,
    failed: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
}

#[derive(Clone, Debug)]
enum HostRequestError {
    Local(String),
    Remote(String),
    Transport(String),
}

enum HostFrame {
    Progress(PluginProgress),
    Result(Result<Value, HostRequestError>),
}

type HostResponseSender = mpsc::Sender<HostFrame>;
type PendingHostRequests = Arc<Mutex<HashMap<u64, HostResponseSender>>>;

impl HostRequestError {
    fn into_message(self) -> String {
        match self {
            Self::Local(message) | Self::Remote(message) | Self::Transport(message) => message,
        }
    }
}

fn valid_protocol_handshake(line: &str, bytes: usize, protocol_secret: &str) -> bool {
    bytes <= MAX_PROTOCOL_HANDSHAKE_BYTES
        && line.ends_with('\n')
        && serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("secret")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .as_deref()
            == Some(protocol_secret)
}

fn read_protocol_handshake(
    stream: &mut TcpStream,
    protocol_secret: &str,
    deadline: Instant,
) -> bool {
    let mut frame = Vec::with_capacity(MAX_PROTOCOL_HANDSHAKE_BYTES);
    let mut buffer = [0_u8; 256];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match stream.read(&mut buffer) {
            Ok(0) => return false,
            Ok(bytes) => {
                if frame.len().saturating_add(bytes) > MAX_PROTOCOL_HANDSHAKE_BYTES {
                    return false;
                }
                frame.extend_from_slice(&buffer[..bytes]);
                if let Some(newline) = frame.iter().position(|byte| *byte == b'\n') {
                    if newline + 1 != frame.len() {
                        return false;
                    }
                    let Ok(line) = std::str::from_utf8(&frame) else {
                        return false;
                    };
                    return valid_protocol_handshake(line, frame.len(), protocol_secret);
                }
                if frame.len() == MAX_PROTOCOL_HANDSHAKE_BYTES {
                    return false;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(remaining.min(Duration::from_millis(2)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
}

fn write_protocol_frame(
    stream: &mut TcpStream,
    frame: &[u8],
    deadline: Instant,
) -> std::io::Result<()> {
    let mut written = 0;
    while written < frame.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "plugin protocol write deadline elapsed",
            ));
        }
        stream.set_write_timeout(Some(remaining))?;
        match stream.write(&frame[written..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "failed to write plugin protocol frame",
                ));
            }
            Ok(bytes) => written += bytes,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    stream.flush()
}

impl HostConnection {
    fn spawn(plugins_dir: &Path) -> Result<Self, String> {
        let bun = resolve_bun_binary()?
            .ok_or_else(|| "Plugins require Bun; install Bun or set TERMY_BUN_PATH".to_string())?;
        let runtime_dir = managed_runtime_dir(plugins_dir);
        let host_path = runtime_dir.join("host.ts");
        let worker_path = runtime_dir.join("worker.ts");
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("Failed to create plugin protocol socket: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("Failed to configure plugin protocol socket: {error}"))?;
        let protocol_port = listener
            .local_addr()
            .map_err(|error| format!("Failed to inspect plugin protocol socket: {error}"))?
            .port();
        let mut secret = [0_u8; 32];
        OsRng.fill_bytes(&mut secret);
        let protocol_secret = hex_digest(&secret);
        let mut command = Command::new(&bun);
        command
            .arg("run")
            .arg("--no-env-file")
            .arg("--no-install")
            .arg(&host_path)
            .current_dir(plugins_dir)
            .env_clear()
            .env("TERMY_PLUGIN_WORKER_PATH", &worker_path)
            .env("TERMY_PLUGIN_PROTOCOL_PORT", protocol_port.to_string())
            .env("TERMY_PLUGIN_PROTOCOL_SECRET", &protocol_secret)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        copy_safe_environment(&mut command, &bun);
        let child = command
            .spawn()
            .map_err(|error| format!("Failed to start Bun plugin runtime: {error}"))?;
        let child = Arc::new(Mutex::new(child));
        let deadline = Instant::now() + HOST_CONNECT_TIMEOUT;
        let (writer, mut reader) = loop {
            if Instant::now() >= deadline {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err("Bun plugin runtime did not connect in time".to_string());
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(true).map_err(|error| {
                        format!("Failed to configure plugin protocol handshake: {error}")
                    })?;
                    stream.set_nodelay(true).map_err(|error| {
                        format!("Failed to configure plugin protocol connection: {error}")
                    })?;
                    if !read_protocol_handshake(&mut stream, protocol_secret.as_str(), deadline) {
                        continue;
                    }
                    stream.set_nonblocking(false).map_err(|error| {
                        format!("Failed to configure plugin protocol connection: {error}")
                    })?;
                    let reader_stream = stream.try_clone().map_err(|error| {
                        format!("Failed to clone plugin protocol connection: {error}")
                    })?;
                    break (stream, BufReader::new(reader_stream));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Ok(mut child) = child.lock()
                        && let Ok(Some(status)) = child.try_wait()
                    {
                        return Err(format!(
                            "Bun plugin runtime exited before connecting ({status})"
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(format!(
                        "Failed to accept plugin protocol connection: {error}"
                    ));
                }
            }
        };
        reader
            .get_mut()
            .set_read_timeout(None)
            .map_err(|error| format!("Failed to configure plugin protocol reader: {error}"))?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let failed = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        spawn_host_reader(
            reader,
            Arc::clone(&child),
            Arc::clone(&pending),
            Arc::clone(&failed),
            Arc::clone(&failure),
        );
        Ok(Self {
            child,
            writer: Mutex::new(writer),
            pending,
            failed,
            failure,
        })
    }

    fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    fn request<T: DeserializeOwned>(
        &self,
        request: &HostRequest<'_>,
        timeout: Duration,
        control: Option<&PluginInvocationControl>,
    ) -> Result<T, HostRequestError> {
        let deadline = Instant::now() + timeout;
        let mut encoded = serde_json::to_vec(request).map_err(|error| {
            HostRequestError::Local(format!("Failed to encode plugin request: {error}"))
        })?;
        encoded.push(b'\n');
        if encoded.len() > MAX_PROTOCOL_BYTES {
            return Err(HostRequestError::Local(
                "Plugin request exceeds the 1 MiB protocol limit".to_string(),
            ));
        }
        let (response_tx, response_rx) = mpsc::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.failed.load(Ordering::Acquire) {
                return Err(HostRequestError::Transport(self.failure_message()));
            }
            pending.insert(request.id(), response_tx);
        }

        let timeout_error = || {
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&request.id());
            let message = format!("Plugin runtime timed out after {} ms", timeout.as_millis());
            fail_host_connection(
                &self.child,
                &self.pending,
                &self.failed,
                &self.failure,
                &message,
            );
            HostRequestError::Transport(message)
        };

        let mut writer = loop {
            if self.failed.load(Ordering::Acquire) {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&request.id());
                return Err(HostRequestError::Transport(self.failure_message()));
            }
            match self.writer.try_lock() {
                Ok(writer) => break writer,
                Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(timeout_error());
                    }
                    thread::sleep(remaining.min(Duration::from_millis(2)));
                }
            }
        };
        let write_timeout = deadline.saturating_duration_since(Instant::now());
        if write_timeout.is_zero() {
            return Err(timeout_error());
        }
        let write_result = write_protocol_frame(&mut writer, &encoded, deadline);
        let _ = writer.set_write_timeout(None);
        drop(writer);
        if let Err(error) = write_result {
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&request.id());
            let message = format!("Failed to write to plugin runtime: {error}");
            fail_host_connection(
                &self.child,
                &self.pending,
                &self.failed,
                &self.failure,
                &message,
            );
            return Err(HostRequestError::Transport(message));
        }
        let value = loop {
            if control.is_some_and(PluginInvocationControl::is_cancelled) {
                self.send_cancel(request.id());
                return Err(HostRequestError::Remote(
                    "Plugin invocation cancelled".to_string(),
                ));
            }
            let response_timeout = deadline.saturating_duration_since(Instant::now());
            if response_timeout.is_zero() {
                return Err(timeout_error());
            }
            match response_rx.recv_timeout(response_timeout.min(Duration::from_millis(50))) {
                Ok(HostFrame::Progress(progress)) => {
                    validate_progress(&progress).map_err(HostRequestError::Remote)?;
                    if let Some(control) = control {
                        control.report(progress);
                    }
                }
                Ok(HostFrame::Result(result)) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(HostRequestError::Transport(self.failure_message()));
                }
            }
        };
        serde_json::from_value(value).map_err(|error| {
            HostRequestError::Remote(format!(
                "Plugin runtime returned an invalid result: {error}"
            ))
        })
    }

    fn send_cancel(&self, request_id: u64) {
        let request = HostRequest::Cancel { id: 0, request_id };
        let Ok(mut encoded) = serde_json::to_vec(&request) else {
            return;
        };
        encoded.push(b'\n');
        if encoded.len() > MAX_PROTOCOL_BYTES {
            return;
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = write_protocol_frame(
            &mut writer,
            &encoded,
            Instant::now() + Duration::from_secs(1),
        );
        let _ = writer.set_write_timeout(None);
    }

    fn failure_message(&self) -> String {
        self.failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| "Plugin runtime connection is unavailable".to_string())
    }
}

fn spawn_host_reader(
    mut reader: BufReader<TcpStream>,
    child: Arc<Mutex<Child>>,
    pending: PendingHostRequests,
    failed: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
) {
    thread::spawn(move || {
        loop {
            let mut line = String::new();
            let bytes = match (&mut reader)
                .take((MAX_PROTOCOL_BYTES + 1) as u64)
                .read_line(&mut line)
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    fail_host_connection(
                        &child,
                        &pending,
                        &failed,
                        &failure,
                        &format!("Failed to read from plugin runtime: {error}"),
                    );
                    return;
                }
            };
            if bytes == 0 {
                fail_host_connection(
                    &child,
                    &pending,
                    &failed,
                    &failure,
                    "Plugin runtime closed its protocol connection unexpectedly",
                );
                return;
            }
            if bytes > MAX_PROTOCOL_BYTES || !line.ends_with('\n') {
                fail_host_connection(
                    &child,
                    &pending,
                    &failed,
                    &failure,
                    "Plugin response exceeds the 1 MiB protocol limit",
                );
                return;
            }
            let response: HostResponse = match serde_json::from_str(&line) {
                Ok(response) => response,
                Err(error) => {
                    fail_host_connection(
                        &child,
                        &pending,
                        &failed,
                        &failure,
                        &format!("Plugin runtime returned invalid JSON: {error}"),
                    );
                    return;
                }
            };
            if let Some(progress) = response.progress {
                let response_tx = pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&response.id)
                    .cloned();
                let Some(response_tx) = response_tx else {
                    continue;
                };
                let _ = response_tx.send(HostFrame::Progress(progress));
                continue;
            }
            let Some(response_tx) = pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&response.id)
            else {
                fail_host_connection(
                    &child,
                    &pending,
                    &failed,
                    &failure,
                    &format!(
                        "Plugin runtime returned unknown response ID {}",
                        response.id
                    ),
                );
                return;
            };
            let result = if response.ok {
                response.result.ok_or_else(|| {
                    HostRequestError::Transport("Plugin runtime response has no result".to_string())
                })
            } else {
                Err(HostRequestError::Remote(
                    response
                        .error
                        .unwrap_or_else(|| "Plugin command failed".to_string()),
                ))
            };
            let _ = response_tx.send(HostFrame::Result(result));
        }
    });
}

fn fail_host_connection(
    child: &Arc<Mutex<Child>>,
    pending: &PendingHostRequests,
    failed: &Arc<AtomicBool>,
    failure: &Arc<Mutex<Option<String>>>,
    message: &str,
) {
    if failed.swap(true, Ordering::AcqRel) {
        return;
    }
    *failure
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message.to_string());
    let requests = pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
        .map(|(_, sender)| sender)
        .collect::<Vec<_>>();
    for sender in requests {
        let _ = sender.send(HostFrame::Result(Err(HostRequestError::Transport(
            message.to_string(),
        ))));
    }
    if let Ok(mut child) = child.lock() {
        let _ = child.kill();
    }
}

impl Drop for HostConnection {
    fn drop(&mut self) {
        if let Ok(stream) = self.writer.lock() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Debug)]
struct DiscoveredPlugins {
    sources: Vec<PluginSource>,
    fingerprint: [u8; 32],
}

fn write_plugin_installation(
    destination: &Path,
    files: &PluginFiles,
    source_metadata: Option<&PluginSourceMetadata>,
    enabled: bool,
) -> Result<(), String> {
    fs::create_dir(destination)
        .map_err(|error| format!("Failed to prepare plugin installation: {error}"))?;
    for (relative_path, contents) in files {
        let target = relative_path
            .split('/')
            .fold(destination.to_path_buf(), |path, component| {
                path.join(component)
            });
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Failed to create plugin directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&target, contents)
            .map_err(|error| format!("Failed to copy plugin file {}: {error}", target.display()))?;
    }
    if let Some(source_metadata) = source_metadata {
        let contents = serde_json::to_vec_pretty(source_metadata)
            .map_err(|error| format!("Failed to encode plugin source metadata: {error}"))?;
        fs::write(destination.join(SOURCE_METADATA_FILE), contents)
            .map_err(|error| format!("Failed to save plugin source metadata: {error}"))?;
    }
    if !enabled {
        fs::write(
            destination.join(DISABLED_MARKER),
            b"Managed by Termy settings.\n",
        )
        .map_err(|error| format!("Failed to preserve disabled plugin state: {error}"))?;
    }
    Ok(())
}

fn validate_source_metadata(metadata: &PluginSourceMetadata) -> Result<(), String> {
    let repository = url::Url::parse(&metadata.repository_url)
        .map_err(|error| format!("Plugin repository URL is invalid: {error}"))?;
    if repository.scheme() != "https" || repository.host_str() != Some("github.com") {
        return Err("Plugin repository URL must use https://github.com".to_string());
    }
    if metadata.revision.len() < 40
        || metadata.revision.len() > 64
        || !metadata
            .revision
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("Plugin source revision must be a full Git commit hash".to_string());
    }
    if let Some(requested_ref) = metadata.requested_ref.as_deref() {
        validate_text(requested_ref, 256, "source ref")?;
    }
    if !metadata.subdirectory.is_empty()
        && normalized_relative_plugin_path(&metadata.subdirectory).as_deref()
            != Some(metadata.subdirectory.as_str())
    {
        return Err("Plugin source subdirectory must be a normalized relative path".to_string());
    }
    Ok(())
}

fn read_source_metadata(root: &Path) -> Result<Option<PluginSourceMetadata>, String> {
    let path = root.join(SOURCE_METADATA_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to inspect plugin source metadata: {error}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{SOURCE_METADATA_FILE} must be a regular file"));
    }
    let source_metadata: PluginSourceMetadata = serde_json::from_slice(
        &fs::read(&path)
            .map_err(|error| format!("Failed to read plugin source metadata: {error}"))?,
    )
    .map_err(|error| format!("Invalid plugin source metadata: {error}"))?;
    validate_source_metadata(&source_metadata)?;
    Ok(Some(source_metadata))
}

fn inventory_plugins(plugins_dir: &Path) -> Result<PluginInventory, String> {
    fs::create_dir_all(plugins_dir).map_err(|error| {
        format!(
            "Failed to create plugin directory {}: {error}",
            plugins_dir.display()
        )
    })?;
    let mut entries = fs::read_dir(plugins_dir)
        .map_err(|error| format!("Failed to read {}: {error}", plugins_dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to inspect {}: {error}", plugins_dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    let mut inventory = PluginInventory::default();
    for entry in entries {
        let id = entry.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') {
            continue;
        }
        let root = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Failed to inspect {}: {error}", root.display()))?;
        if file_type.is_symlink() {
            inventory.errors.push(format!(
                "Plugin source cannot be a symlink: {}",
                root.display()
            ));
            continue;
        }
        if !file_type.is_dir() || !root.join("plugin.json").is_file() {
            continue;
        }
        if !valid_id(&id) {
            inventory
                .errors
                .push(format!("Invalid plugin directory ID `{id}`"));
            continue;
        }
        let enabled = match plugin_is_enabled(&root, &id) {
            Ok(enabled) => enabled,
            Err(error) => {
                inventory.plugins.push(InstalledPlugin {
                    id: id.clone(),
                    name: id,
                    version: None,
                    enabled: false,
                    path: root,
                    source: None,
                    error: Some(error),
                });
                continue;
            }
        };
        match validated_plugin_manifest(&root, &id) {
            Ok((manifest, _)) => match read_source_metadata(&root) {
                Ok(source) => inventory.plugins.push(installed_plugin_from_manifest(
                    manifest, root, enabled, source, None,
                )),
                Err(error) => inventory.plugins.push(installed_plugin_from_manifest(
                    manifest,
                    root,
                    enabled,
                    None,
                    Some(error),
                )),
            },
            Err(error) => inventory.plugins.push(InstalledPlugin {
                id: id.clone(),
                name: id,
                version: None,
                enabled,
                path: root,
                source: None,
                error: Some(error),
            }),
        }
    }
    Ok(inventory)
}

fn installed_plugin_from_manifest(
    manifest: PluginManifestFile,
    path: PathBuf,
    enabled: bool,
    source: Option<PluginSourceMetadata>,
    error: Option<String>,
) -> InstalledPlugin {
    InstalledPlugin {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        enabled,
        path,
        source,
        error,
    }
}

fn source_plugin_manifest(source: &Path) -> Result<PluginManifestFile, String> {
    let source_directory_metadata = fs::symlink_metadata(source).map_err(|error| {
        format!(
            "Failed to inspect plugin source {}: {error}",
            source.display()
        )
    })?;
    if source_directory_metadata.file_type().is_symlink() || !source_directory_metadata.is_dir() {
        return Err("Plugin source must be a real directory, not a symlink".to_string());
    }
    let manifest_path = source.join("plugin.json");
    let manifest_metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|error| format!("Plugin source is missing plugin.json: {error}"))?;
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_file() {
        return Err("plugin.json must be a regular file".to_string());
    }
    serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|error| format!("Failed to read plugin.json: {error}"))?,
    )
    .map_err(|error| format!("Invalid plugin.json: {error}"))
}

fn inspect_plugin_root(
    root: &Path,
    expected_id: &str,
) -> Result<(PluginManifestFile, PluginFiles), String> {
    let (manifest, entrypoint) = validated_plugin_manifest(root, expected_id)?;
    let files = plugin_tree_files(root)?;
    if !files.iter().any(|(path, _)| path == &entrypoint) {
        return Err(format!("Plugin entrypoint does not exist: {entrypoint}"));
    }
    Ok((manifest, files))
}

fn validated_plugin_manifest(
    root: &Path,
    expected_id: &str,
) -> Result<(PluginManifestFile, String), String> {
    if !valid_id(expected_id) {
        return Err(format!("Invalid plugin ID `{expected_id}`"));
    }
    let manifest_path = root.join("plugin.json");
    let manifest_metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|error| format!("Plugin source is missing plugin.json: {error}"))?;
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_file() {
        return Err("plugin.json must be a regular file".to_string());
    }
    let manifest: PluginManifestFile = serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|error| format!("Failed to read plugin.json: {error}"))?,
    )
    .map_err(|error| format!("Invalid plugin.json: {error}"))?;
    if manifest.api_version != 1 {
        return Err("plugin.json apiVersion must be 1".to_string());
    }
    if manifest.id != expected_id {
        return Err(format!(
            "plugin.json id `{}` must match plugin directory `{expected_id}`",
            manifest.id
        ));
    }
    validate_text(&manifest.name, 200, "name")?;
    if let Some(version) = manifest.version.as_deref() {
        validate_text(version, 100, "version")?;
    }
    validate_plugin_capabilities(&manifest.capabilities)?;
    let main = manifest.main.as_deref().unwrap_or("plugin.ts");
    validate_text(main, 1_024, "entrypoint")?;
    let entrypoint = normalized_relative_plugin_path(main)
        .ok_or_else(|| "plugin.json main must stay inside the plugin directory".to_string())?;
    let components = entrypoint.split('/').collect::<Vec<_>>();
    let mut entrypoint_path = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        entrypoint_path.push(component);
        let metadata = fs::symlink_metadata(&entrypoint_path)
            .map_err(|_| format!("Plugin entrypoint does not exist: {main}"))?;
        let is_last = index + 1 == components.len();
        if metadata.file_type().is_symlink()
            || (is_last && !metadata.is_file())
            || (!is_last && !metadata.is_dir())
        {
            return Err(format!(
                "Plugin entrypoint must be a regular file inside the plugin directory: {main}"
            ));
        }
    }
    Ok((manifest, entrypoint))
}

/// Validates capability names from a plugin manifest.
pub fn validate_plugin_capabilities(capabilities: &[String]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for capability in capabilities {
        if !PLUGIN_CAPABILITIES.contains(&capability.as_str()) {
            return Err(format!(
                "plugin.json capability `{capability}` is not supported"
            ));
        }
        if !seen.insert(capability) {
            return Err(format!(
                "plugin.json capability `{capability}` is duplicated"
            ));
        }
    }
    Ok(())
}

fn plugin_is_enabled(root: &Path, id: &str) -> Result<bool, String> {
    let marker = root.join(DISABLED_MARKER);
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(format!(
            "Plugin `{id}` has an invalid {DISABLED_MARKER} marker"
        )),
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!(
            "Failed to inspect disabled state for plugin `{id}`: {error}"
        )),
    }
}

fn normalized_relative_plugin_path(path: &str) -> Option<String> {
    let mut components = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => components.push(value.to_str()?.to_string()),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!components.is_empty()).then(|| components.join("/"))
}

fn discover_plugins(plugins_dir: &Path) -> Result<DiscoveredPlugins, String> {
    if !plugins_dir.exists() {
        fs::create_dir_all(plugins_dir).map_err(|error| {
            format!(
                "Failed to create plugin directory {}: {error}",
                plugins_dir.display()
            )
        })?;
    }
    let mut entries = fs::read_dir(plugins_dir)
        .map_err(|error| format!("Failed to read {}: {error}", plugins_dir.display()))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());

    let mut candidates = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let root = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Failed to inspect {}: {error}", root.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "Plugin source cannot be a symlink: {}",
                root.display()
            ));
        }
        if !file_type.is_dir() || !root.join("plugin.json").is_file() {
            continue;
        }
        let id = name;
        if !valid_id(&id) {
            return Err(format!(
                "Invalid plugin path ID `{id}`; use lowercase letters, numbers, dots, underscores, or hyphens"
            ));
        }
        if !plugin_is_enabled(&root, &id)? {
            continue;
        }
        candidates.push((id, root));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    if candidates.len() > MAX_INSTALLED_PLUGINS {
        return Err(format!(
            "Plugin directory contains {} plugins; maximum is {MAX_INSTALLED_PLUGINS}",
            candidates.len()
        ));
    }

    let mut seen = HashSet::new();
    let mut hasher = Sha256::new();
    let mut sources = Vec::with_capacity(candidates.len());
    for (id, root) in candidates {
        if !seen.insert(id.clone()) {
            return Err(format!("Duplicate plugin ID `{id}`"));
        }
        let files = plugin_tree_files(&root)?;
        let mut source_hasher = Sha256::new();
        source_hasher.update(BUNDLE_CACHE_FORMAT);
        for (relative_path, contents) in &files {
            source_hasher.update(relative_path.as_bytes());
            source_hasher.update([0]);
            source_hasher.update(contents);
            source_hasher.update([0]);
        }
        let source_hash = source_hasher.finalize();
        let cache_key = hex_digest(source_hash.as_slice());
        hasher.update(id.as_bytes());
        hasher.update([0]);
        hasher.update(root.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(source_hash);
        sources.push(PluginSource {
            id,
            root: root.to_string_lossy().into_owned(),
            cache_key,
        });
    }
    Ok(DiscoveredPlugins {
        sources,
        fingerprint: hasher.finalize().into(),
    })
}

fn plugin_tree_files(root: &Path) -> Result<PluginFiles, String> {
    fn visit(
        root: &Path,
        directory: &Path,
        total_bytes: &mut u64,
        files: &mut Vec<(String, Vec<u8>)>,
    ) -> Result<(), String> {
        let mut entries = fs::read_dir(directory)
            .map_err(|error| format!("Failed to read {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to inspect {}: {error}", directory.display()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some(".git" | "node_modules" | DISABLED_MARKER | SOURCE_METADATA_FILE)
            ) {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| {
                format!("Failed to inspect {}: {error}", entry.path().display())
            })?;
            let path = entry.path();
            if file_type.is_dir() {
                visit(root, &path, total_bytes, files)?;
                continue;
            }
            if !file_type.is_file() {
                return Err(format!(
                    "Plugin source contains unsupported file or symlink {}",
                    path.display()
                ));
            }
            let contents = fs::read(&path).map_err(|error| {
                format!("Failed to read plugin file {}: {error}", path.display())
            })?;
            *total_bytes = total_bytes.saturating_add(contents.len() as u64);
            if *total_bytes > MAX_PLUGIN_SOURCE_BYTES {
                return Err(format!(
                    "Plugin {} exceeds the 16 MiB source-tree limit",
                    root.display()
                ));
            }
            let relative_path = path
                .strip_prefix(root)
                .map_err(|_| format!("Plugin file escaped its root: {}", path.display()))?
                .components()
                .map(|component| {
                    component.as_os_str().to_str().ok_or_else(|| {
                        format!("Plugin source path must be valid UTF-8: {}", path.display())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("/");
            files.push((relative_path, contents));
            if files.len() > MAX_PLUGIN_SOURCE_FILES {
                return Err(format!(
                    "Plugin {} exceeds the {MAX_PLUGIN_SOURCE_FILES}-file source-tree limit",
                    root.display()
                ));
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    let mut total_bytes = 0;
    visit(root, root, &mut total_bytes, &mut files)?;
    files.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    Ok(files)
}

fn ensure_managed_files(plugins_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(plugins_dir).map_err(|error| {
        format!(
            "Failed to create plugin directory {}: {error}",
            plugins_dir.display()
        )
    })?;
    write_if_changed(&plugins_dir.join("termy.d.ts"), TYPE_DECLARATIONS)?;
    let runtime_dir = managed_runtime_dir(plugins_dir);
    fs::create_dir_all(&runtime_dir).map_err(|error| {
        format!(
            "Failed to create plugin runtime directory {}: {error}",
            runtime_dir.display()
        )
    })?;
    write_if_changed(&runtime_dir.join("host.ts"), HOST_SOURCE)?;
    write_if_changed(&runtime_dir.join("worker.ts"), WORKER_SOURCE)
}

fn managed_runtime_dir(plugins_dir: &Path) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(b"termy-plugin-managed-runtime-v1\0");
    hasher.update(HOST_SOURCE.as_bytes());
    hasher.update([0]);
    hasher.update(WORKER_SOURCE.as_bytes());
    let digest = hasher.finalize();
    plugins_dir.join(format!(".termy-runtime-{}", hex_digest(&digest[..16])))
}

fn write_if_changed(path: &Path, contents: &str) -> Result<(), String> {
    if fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    fs::write(path, contents)
        .map_err(|error| format!("Failed to write managed file {}: {error}", path.display()))
}

fn resolve_bun_binary() -> Result<Option<PathBuf>, String> {
    if let Some(value) = std::env::var_os("TERMY_BUN_PATH") {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err("TERMY_BUN_PATH must be an absolute path".to_string());
        }
        if !is_executable_file(&path) {
            return Err(format!(
                "TERMY_BUN_PATH is not an executable file: {}",
                path.display()
            ));
        }
        return Ok(Some(path));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
        && let Some(path) = bun_candidate_in_dir(parent)
    {
        return Ok(Some(path));
    }
    if let Some(path_env) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path_env) {
            if let Some(path) = bun_candidate_in_dir(&directory) {
                return Ok(Some(path));
            }
        }
    }
    let home = dirs::home_dir();
    if let Some(home) = home {
        let candidate = if cfg!(target_os = "windows") {
            home.join(".bun/bin/bun.exe")
        } else {
            home.join(".bun/bin/bun")
        };
        if is_executable_file(&candidate) {
            return Ok(Some(candidate));
        }
    }
    for candidate in ["/opt/homebrew/bin/bun", "/usr/local/bin/bun"] {
        let path = PathBuf::from(candidate);
        if is_executable_file(&path) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn bun_candidate_in_dir(directory: &Path) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["bun.exe", "bun"]
    } else {
        &["bun"]
    };
    names
        .iter()
        .map(|name| directory.join(name))
        .find(|path| is_executable_file(path))
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn plugin_path_env(bun: &Path) -> Option<OsString> {
    let mut entries = Vec::new();
    let mut push = |entry: PathBuf| {
        if !entry.as_os_str().is_empty() && !entries.contains(&entry) {
            entries.push(entry);
        }
    };

    if let Some(path) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&path) {
            push(entry);
        }
    }
    if let Some(parent) = bun.parent() {
        push(parent.to_path_buf());
    }

    #[cfg(not(target_os = "windows"))]
    for entry in [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        push(PathBuf::from(entry));
    }

    std::env::join_paths(entries).ok()
}

fn copy_safe_environment(command: &mut Command, bun: &Path) {
    for key in [
        "HOME",
        "USERPROFILE",
        "SHELL",
        "TMPDIR",
        "TMP",
        "TEMP",
        "LANG",
        "LC_ALL",
        "TERM",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if let Some(path) = plugin_path_env(bun) {
        command.env("PATH", path);
    }
    #[cfg(target_os = "windows")]
    for key in ["SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("DO_NOT_TRACK", "1");
}

fn validate_commands(commands: &[PluginCommand]) -> Result<(), String> {
    if commands.len() > MAX_PLUGIN_COMMANDS {
        return Err(format!(
            "Plugin catalog has {} commands; maximum is {MAX_PLUGIN_COMMANDS}",
            commands.len()
        ));
    }
    let mut command_ids = HashSet::new();
    for command in commands {
        if !valid_id(&command.plugin_id) || !valid_id(&command.id) {
            return Err(format!(
                "Plugin command `{}` has an invalid ID",
                command.qualified_id()
            ));
        }
        if !command_ids.insert(command.qualified_id()) {
            return Err(format!(
                "Duplicate plugin command `{}`",
                command.qualified_id()
            ));
        }
        validate_text(&command.plugin_name, 200, "plugin name")?;
        validate_text(&command.title, 300, "command title")?;
        for keyword in &command.keywords {
            validate_text(keyword, 100, "command keyword")?;
        }
        if let Some(status) = &command.status {
            validate_text(status, 100, "command status")?;
        }
        if let Some(reason) = &command.disabled_reason {
            validate_text(reason, 500, "command disabled reason")?;
        }
        let mut runtimes = HashSet::new();
        if command
            .when
            .runtimes
            .iter()
            .any(|runtime| !runtimes.insert(*runtime))
        {
            return Err(format!(
                "Plugin command `{}` has duplicate runtime conditions",
                command.qualified_id()
            ));
        }
        let mut platforms = HashSet::new();
        for platform in &command.when.platforms {
            if !matches!(platform.as_str(), "macos" | "linux" | "windows") {
                return Err(format!(
                    "Plugin command `{}` has unsupported platform condition `{platform}`",
                    command.qualified_id()
                ));
            }
            if !platforms.insert(platform) {
                return Err(format!(
                    "Plugin command `{}` has duplicate platform conditions",
                    command.qualified_id()
                ));
            }
        }
        let mut placements = HashSet::new();
        if command
            .placements
            .iter()
            .any(|placement| !placements.insert(*placement))
        {
            return Err(format!(
                "Plugin command `{}` has duplicate placements",
                command.qualified_id()
            ));
        }
        if command.inputs.len() > MAX_INPUTS_PER_COMMAND {
            return Err(format!(
                "Plugin command `{}` has too many inputs",
                command.qualified_id()
            ));
        }
        let mut input_ids = HashSet::new();
        for input in &command.inputs {
            if !valid_id(input.id()) || !input_ids.insert(input.id().to_string()) {
                return Err(format!(
                    "Plugin command `{}` has an invalid or duplicate input ID `{}`",
                    command.qualified_id(),
                    input.id()
                ));
            }
            validate_text(input.label(), 200, "input label")?;
            match input {
                PluginInput::Text { max_length, .. }
                    if *max_length == 0 || *max_length > 16_384 =>
                {
                    return Err(format!(
                        "Plugin command `{}` has an invalid text input maxLength",
                        command.qualified_id()
                    ));
                }
                PluginInput::Select { options, .. } => validate_select_options(
                    options,
                    &format!("command `{}` select", command.qualified_id()),
                )?,
                PluginInput::Text {
                    placeholder,
                    default_value,
                    max_length,
                    ..
                } => {
                    if let Some(placeholder) = placeholder {
                        validate_optional_text(placeholder, 300, "input placeholder")?;
                    }
                    if let Some(default_value) = default_value
                        && default_value.chars().count() > *max_length
                    {
                        return Err(format!(
                            "Plugin command `{}` has a text defaultValue longer than maxLength",
                            command.qualified_id()
                        ));
                    }
                }
                PluginInput::Pick {
                    placeholder,
                    default_value,
                    ..
                } => {
                    if let Some(placeholder) = placeholder {
                        validate_optional_text(placeholder, 300, "picker placeholder")?;
                    }
                    if let Some(default_value) = default_value {
                        validate_optional_text(default_value, 1_024, "picker default value")?;
                    }
                }
                PluginInput::Confirm { .. } => {}
            }
        }
    }
    Ok(())
}

fn validate_event_subscriptions(
    subscriptions: &[PluginEventSubscriptionDescriptor],
) -> Result<(), String> {
    let mut events = HashSet::new();
    for subscription in subscriptions {
        if !valid_id(&subscription.plugin_id) {
            return Err("Plugin event subscription has an invalid plugin ID".to_string());
        }
        if !events.insert(subscription.event) {
            return Err("Plugin subscribes to the same event more than once".to_string());
        }
        if !(100..=MAX_INVOKE_TIMEOUT_MS).contains(&subscription.timeout_ms) {
            return Err(format!(
                "Plugin event timeout must be between 100 and {MAX_INVOKE_TIMEOUT_MS} ms"
            ));
        }
    }
    Ok(())
}

fn validate_views(views: &[PluginViewDescriptor]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut counts = HashMap::<&str, usize>::new();
    for view in views {
        if !valid_id(&view.plugin_id) || !valid_id(&view.id) {
            return Err(format!(
                "Plugin view `{}.{}` has an invalid ID",
                view.plugin_id, view.id
            ));
        }
        let qualified_id = format!("{}.{}", view.plugin_id, view.id);
        if !ids.insert(qualified_id.clone()) {
            return Err(format!("Duplicate plugin view `{qualified_id}`"));
        }
        let count = counts.entry(&view.plugin_id).or_default();
        *count += 1;
        if *count > MAX_PLUGIN_VIEWS {
            return Err(format!(
                "Plugin `{}` has more than {MAX_PLUGIN_VIEWS} views",
                view.plugin_id
            ));
        }
        validate_text(&view.plugin_name, 200, "plugin name")?;
        validate_text(&view.title, 300, "view title")?;
        if !(100..=MAX_INVOKE_TIMEOUT_MS).contains(&view.timeout_ms) {
            return Err(format!(
                "Plugin view timeout must be between 100 and {MAX_INVOKE_TIMEOUT_MS} ms"
            ));
        }
    }
    Ok(())
}

fn validate_plugin_settings(settings: &[PluginSetting]) -> Result<(), String> {
    if settings.len() > MAX_PLUGIN_SETTINGS {
        return Err(format!(
            "Plugin has {} settings; maximum is {MAX_PLUGIN_SETTINGS}",
            settings.len()
        ));
    }
    let mut ids = HashSet::new();
    for setting in settings {
        if !valid_id(setting.id()) || !ids.insert(setting.id().to_string()) {
            return Err(format!(
                "Plugin has an invalid or duplicate setting ID `{}`",
                setting.id()
            ));
        }
        validate_text(setting.title(), 200, "setting title")?;
        if let Some(description) = setting.description() {
            validate_text(description, 500, "setting description")?;
        }
        match setting {
            PluginSetting::Toggle { .. } => {}
            PluginSetting::Text {
                placeholder,
                default_value,
                max_length,
                ..
            } => {
                validate_setting_length(*max_length, setting.id())?;
                if let Some(placeholder) = placeholder {
                    validate_text(placeholder, 300, "setting placeholder")?;
                }
                if default_value.chars().count() > *max_length {
                    return Err(format!(
                        "Plugin setting `{}` defaultValue exceeds maxLength",
                        setting.id()
                    ));
                }
            }
            PluginSetting::Select {
                default_value,
                options,
                ..
            } => {
                if options.is_empty() || options.len() > MAX_SELECT_OPTIONS {
                    return Err(format!(
                        "Plugin setting `{}` has an invalid select option count",
                        setting.id()
                    ));
                }
                let mut values = HashSet::new();
                for option in options {
                    validate_text(&option.value, 1_024, "setting option value")?;
                    validate_text(&option.label, 200, "setting option label")?;
                    if !values.insert(option.value.as_str()) {
                        return Err(format!(
                            "Plugin setting `{}` has duplicate option `{}`",
                            setting.id(),
                            option.value
                        ));
                    }
                }
                if !options.iter().any(|option| option.value == *default_value) {
                    return Err(format!(
                        "Plugin setting `{}` defaultValue must match an option",
                        setting.id()
                    ));
                }
            }
            PluginSetting::Secret {
                placeholder,
                max_length,
                ..
            } => {
                validate_setting_length(*max_length, setting.id())?;
                if let Some(placeholder) = placeholder {
                    validate_text(placeholder, 300, "setting placeholder")?;
                }
            }
        }
    }
    Ok(())
}

fn validate_setting_length(max_length: usize, key: &str) -> Result<(), String> {
    if max_length == 0 || max_length > MAX_SETTING_VALUE_LENGTH {
        return Err(format!("Plugin setting `{key}` has an invalid maxLength"));
    }
    Ok(())
}

fn validate_setting_value(setting: &PluginSetting, value: &Value) -> Result<(), String> {
    match setting {
        PluginSetting::Toggle { .. } if value.is_boolean() => Ok(()),
        PluginSetting::Text { max_length, .. } | PluginSetting::Secret { max_length, .. } => {
            let Some(value) = value.as_str() else {
                return Err(format!("Plugin setting `{}` must be text", setting.id()));
            };
            if value.chars().count() > *max_length {
                return Err(format!(
                    "Plugin setting `{}` exceeds {max_length} characters",
                    setting.id()
                ));
            }
            Ok(())
        }
        PluginSetting::Select { options, .. } => {
            let Some(value) = value.as_str() else {
                return Err(format!(
                    "Plugin setting `{}` must be a select option",
                    setting.id()
                ));
            };
            if options.iter().any(|option| option.value == value) {
                Ok(())
            } else {
                Err(format!(
                    "Plugin setting `{}` contains an unknown select value",
                    setting.id()
                ))
            }
        }
        _ => Err(format!(
            "Plugin setting `{}` has the wrong value type",
            setting.id()
        )),
    }
}

fn plugin_secret_account(plugin_id: &str, key: &str) -> String {
    format!("v2:{}:{plugin_id}{key}", plugin_id.len())
}

fn legacy_plugin_secret_account(plugin_id: &str, key: &str) -> String {
    format!("{plugin_id}.{key}")
}

fn can_migrate_legacy_plugin_secret(plugin_id: &str, key: &str) -> bool {
    !plugin_id.contains('.') && !key.contains('.')
}

#[cfg(not(test))]
fn plugin_secret_entry(plugin_id: &str, key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &plugin_secret_account(plugin_id, key))
        .map_err(|error| format!("Failed to access the credential store: {error}"))
}

#[cfg(not(test))]
fn legacy_plugin_secret_entry(plugin_id: &str, key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(
        KEYRING_SERVICE,
        &legacy_plugin_secret_account(plugin_id, key),
    )
    .map_err(|error| format!("Failed to access the credential store: {error}"))
}

#[cfg(not(test))]
fn read_plugin_secret(plugin_id: &str, key: &str) -> Result<Option<String>, String> {
    match plugin_secret_entry(plugin_id, key)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) if !can_migrate_legacy_plugin_secret(plugin_id, key) => {
            Ok(None)
        }
        Err(keyring::Error::NoEntry) => {
            let legacy = legacy_plugin_secret_entry(plugin_id, key)?;
            let secret = match legacy.get_password() {
                Ok(secret) => secret,
                Err(keyring::Error::NoEntry) => return Ok(None),
                Err(error) => {
                    return Err(format!(
                        "Failed to read legacy secret setting `{plugin_id}.{key}`: {error}"
                    ));
                }
            };
            plugin_secret_entry(plugin_id, key)?
                .set_password(&secret)
                .map_err(|error| {
                    format!("Failed to migrate secret setting `{plugin_id}.{key}`: {error}")
                })?;
            match legacy.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(Some(secret)),
                Err(error) => Err(format!(
                    "Failed to clear legacy secret setting `{plugin_id}.{key}` after migration: {error}"
                )),
            }
        }
        Err(error) => Err(format!(
            "Failed to read secret setting `{plugin_id}.{key}`: {error}"
        )),
    }
}

#[cfg(not(test))]
fn write_plugin_secret(plugin_id: &str, key: &str, secret: &str) -> Result<(), String> {
    plugin_secret_entry(plugin_id, key)?
        .set_password(secret)
        .map_err(|error| format!("Failed to save secret setting `{plugin_id}.{key}`: {error}"))?;
    if can_migrate_legacy_plugin_secret(plugin_id, key) {
        match legacy_plugin_secret_entry(plugin_id, key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                return Err(format!(
                    "Failed to clear legacy secret setting `{plugin_id}.{key}`: {error}"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(not(test))]
fn delete_plugin_secret(plugin_id: &str, key: &str) -> Result<(), String> {
    match plugin_secret_entry(plugin_id, key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(error) => {
            return Err(format!(
                "Failed to clear secret setting `{plugin_id}.{key}`: {error}"
            ));
        }
    }
    if can_migrate_legacy_plugin_secret(plugin_id, key) {
        match legacy_plugin_secret_entry(plugin_id, key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                return Err(format!(
                    "Failed to clear legacy secret setting `{plugin_id}.{key}`: {error}"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
static TEST_PLUGIN_SECRETS: std::sync::LazyLock<Mutex<BTreeMap<String, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)]
fn read_plugin_secret(plugin_id: &str, key: &str) -> Result<Option<String>, String> {
    let mut secrets = TEST_PLUGIN_SECRETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let account = plugin_secret_account(plugin_id, key);
    if let Some(secret) = secrets.get(&account) {
        return Ok(Some(secret.clone()));
    }
    if !can_migrate_legacy_plugin_secret(plugin_id, key) {
        return Ok(None);
    }
    let Some(secret) = secrets.remove(&legacy_plugin_secret_account(plugin_id, key)) else {
        return Ok(None);
    };
    secrets.insert(account, secret.clone());
    Ok(Some(secret))
}

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)]
fn write_plugin_secret(plugin_id: &str, key: &str, secret: &str) -> Result<(), String> {
    let mut secrets = TEST_PLUGIN_SECRETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    secrets.insert(plugin_secret_account(plugin_id, key), secret.to_string());
    if can_migrate_legacy_plugin_secret(plugin_id, key) {
        secrets.remove(&legacy_plugin_secret_account(plugin_id, key));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)]
fn delete_plugin_secret(plugin_id: &str, key: &str) -> Result<(), String> {
    let mut secrets = TEST_PLUGIN_SECRETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    secrets.remove(&plugin_secret_account(plugin_id, key));
    if can_migrate_legacy_plugin_secret(plugin_id, key) {
        secrets.remove(&legacy_plugin_secret_account(plugin_id, key));
    }
    Ok(())
}

fn validate_inputs(
    command: &PluginCommand,
    inputs: &BTreeMap<String, Value>,
) -> Result<(), String> {
    let expected = command
        .inputs
        .iter()
        .map(PluginInput::id)
        .collect::<HashSet<_>>();
    if let Some(unknown) = inputs.keys().find(|id| !expected.contains(id.as_str())) {
        return Err(format!(
            "Plugin command `{}` received unknown input `{unknown}`",
            command.qualified_id()
        ));
    }
    for input in &command.inputs {
        match input {
            PluginInput::Text {
                id,
                required,
                max_length,
                ..
            } => {
                let Some(value) = inputs.get(id) else {
                    if *required {
                        return Err(format!("Plugin input `{id}` is required"));
                    }
                    continue;
                };
                let Some(text) = value.as_str() else {
                    return Err(format!("Plugin input `{id}` must be text"));
                };
                if *required && text.trim().is_empty() {
                    return Err(format!("Plugin input `{id}` is required"));
                }
                if text.chars().count() > *max_length {
                    return Err(format!(
                        "Plugin input `{id}` exceeds {max_length} characters"
                    ));
                }
            }
            PluginInput::Select {
                id,
                required,
                options,
                ..
            } => {
                let Some(value) = inputs.get(id) else {
                    if *required {
                        return Err(format!("Plugin input `{id}` is required"));
                    }
                    continue;
                };
                let Some(selected) = value.as_str() else {
                    return Err(format!("Plugin input `{id}` must be a select option"));
                };
                if !options.iter().any(|option| option.value == selected) {
                    return Err(format!(
                        "Plugin input `{id}` contains an unknown select value"
                    ));
                }
            }
            PluginInput::Pick { id, required, .. } => {
                let Some(value) = inputs.get(id) else {
                    if *required {
                        return Err(format!("Plugin input `{id}` is required"));
                    }
                    continue;
                };
                let Some(selected) = value.as_str() else {
                    return Err(format!("Plugin input `{id}` must be a picker value"));
                };
                if *required && selected.trim().is_empty() {
                    return Err(format!("Plugin input `{id}` is required"));
                }
                validate_optional_text(selected, 1_024, "picker value")?;
            }
            PluginInput::Confirm { id, .. } => {
                if inputs.get(id).and_then(Value::as_bool).is_none() {
                    return Err(format!("Plugin input `{id}` must be true or false"));
                }
            }
        }
    }
    Ok(())
}

fn validate_actions(actions: &[PluginAction]) -> Result<(), String> {
    if actions.len() > MAX_ACTIONS {
        return Err(format!(
            "Plugin returned {} actions; maximum is {MAX_ACTIONS}",
            actions.len()
        ));
    }
    for action in actions {
        match action {
            PluginAction::TerminalRun {
                command,
                working_directory,
            } => {
                validate_text(command, 65_536, "terminal command")?;
                if let Some(directory) = working_directory {
                    validate_text(directory, 4_096, "working directory")?;
                }
            }
            PluginAction::TerminalSendText { text, target, .. } => {
                validate_optional_text(text, 262_144, "terminal text")?;
                validate_terminal_target(target)?;
            }
            PluginAction::TerminalOpen {
                working_directory,
                launch,
                target,
                ..
            } => {
                if let Some(directory) = working_directory {
                    validate_text(directory, 4_096, "working directory")?;
                }
                validate_terminal_target(target)?;
                if let Some(launch) = launch {
                    validate_terminal_launch(launch)?;
                }
            }
            PluginAction::TermyCommand { command } => {
                validate_text(command, 128, "Termy command")?;
            }
            PluginAction::ClipboardWrite { text } => {
                validate_text(text, 262_144, "clipboard text")?;
            }
            PluginAction::UrlOpen { url } => {
                validate_text(url, 8_192, "URL")?;
                let parsed = url::Url::parse(url)
                    .map_err(|error| format!("Plugin returned an invalid URL: {error}"))?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err("Plugin URLs must use http or https".to_string());
                }
            }
            PluginAction::Toast { message, .. } => {
                validate_text(message, 4_096, "toast message")?;
            }
            PluginAction::ViewOpen { view, params, .. }
            | PluginAction::ViewReplace { view, params, .. } => {
                if !valid_id(view) {
                    return Err(format!("Plugin returned invalid view ID `{view}`"));
                }
                validate_view_params(params)?;
            }
            PluginAction::ViewClose { .. } => {}
        }
    }
    Ok(())
}

fn validate_terminal_target(target: &PluginTerminalTarget) -> Result<(), String> {
    let PluginTerminalTarget::Exact {
        window_id,
        tab_id,
        pane_id,
    } = target
    else {
        return Ok(());
    };
    validate_text(window_id, 128, "terminal target windowId")?;
    if let Some(tab_id) = tab_id {
        validate_text(tab_id, 128, "terminal target tabId")?;
    }
    if let Some(pane_id) = pane_id {
        validate_text(pane_id, 128, "terminal target paneId")?;
    }
    if pane_id.is_some() && tab_id.is_none() {
        return Err("Plugin terminal targets with paneId must include tabId".to_string());
    }
    Ok(())
}

fn validate_terminal_launch(launch: &PluginTerminalLaunch) -> Result<(), String> {
    match launch {
        PluginTerminalLaunch::Shell { command } => {
            validate_text(command, 65_536, "terminal launch command")
        }
        PluginTerminalLaunch::Program { program, args } => {
            validate_text(program, MAX_LAUNCH_TEXT_LENGTH, "terminal launch program")?;
            if args.len() > MAX_LAUNCH_ARGS {
                return Err(format!(
                    "Plugin terminal launch has {} arguments; maximum is {MAX_LAUNCH_ARGS}",
                    args.len()
                ));
            }
            for argument in args {
                validate_optional_text(
                    argument,
                    MAX_LAUNCH_TEXT_LENGTH,
                    "terminal launch argument",
                )?;
            }
            Ok(())
        }
    }
}

fn validate_view_params(params: &Value) -> Result<(), String> {
    if !params.is_object() {
        return Err("Plugin view params must be an object".to_string());
    }
    let encoded = serde_json::to_vec(params)
        .map_err(|error| format!("Failed to encode plugin view params: {error}"))?;
    if encoded.len() > MAX_VIEW_PARAMS_BYTES {
        return Err(format!(
            "Plugin view params exceed the {MAX_VIEW_PARAMS_BYTES} byte limit"
        ));
    }
    Ok(())
}

fn validate_select_options(options: &[PluginSelectOption], label: &str) -> Result<(), String> {
    if options.is_empty() || options.len() > MAX_SELECT_OPTIONS {
        return Err(format!("Plugin {label} has an invalid option count"));
    }
    let mut option_values = HashSet::new();
    for option in options {
        validate_text(&option.value, 1_024, "select option value")?;
        validate_text(&option.label, 200, "select option label")?;
        for keyword in &option.keywords {
            validate_text(keyword, 100, "select option keyword")?;
        }
        if let Some(status) = &option.status {
            validate_text(status, 100, "select option status")?;
        }
        if !option_values.insert(&option.value) {
            return Err(format!(
                "Plugin {label} has duplicate option value `{}`",
                option.value
            ));
        }
    }
    Ok(())
}

fn validate_view_action(
    action: &PluginViewAction,
    values: &BTreeMap<String, PluginViewValue>,
) -> Result<(), String> {
    if !valid_id(&action.id) {
        return Err(format!(
            "Plugin view action `{}` has an invalid ID",
            action.id
        ));
    }
    if !valid_id(&action.control_id) {
        return Err(format!(
            "Plugin view control `{}` has an invalid ID",
            action.control_id
        ));
    }
    if let Some(payload) = action.payload.as_deref() {
        validate_text(payload, 1_024, "view action payload")?;
    }
    if let Some(PluginViewValue::Text(value)) = action.value.as_ref() {
        validate_view_value_text(value, "view action value")?;
    }
    if values.len() > MAX_VIEW_VALUES {
        return Err(format!(
            "Plugin view submitted {} values; maximum is {MAX_VIEW_VALUES}",
            values.len()
        ));
    }
    for (id, value) in values {
        if !valid_id(id) {
            return Err(format!("Plugin view value `{id}` has an invalid ID"));
        }
        if let PluginViewValue::Text(value) = value {
            validate_view_value_text(value, "view value")?;
        }
    }
    Ok(())
}

fn validate_view_value_text(value: &str, label: &str) -> Result<(), String> {
    if value.chars().count() > 4_096 {
        return Err(format!("Plugin {label} exceeds 4096 characters"));
    }
    Ok(())
}

fn validate_view_nodes(nodes: &[PluginUiNode]) -> Result<(), String> {
    if nodes.len() > MAX_VIEW_CHILDREN {
        return Err(format!(
            "Plugin view has too many root nodes; maximum is {MAX_VIEW_CHILDREN}"
        ));
    }
    let mut node_count = 0;
    let mut value_count = 0;
    let mut control_ids = HashSet::new();
    for node in nodes {
        validate_view_node(node, 1, &mut node_count, &mut value_count, &mut control_ids)?;
    }
    Ok(())
}

fn validate_view_node(
    node: &PluginUiNode,
    depth: usize,
    node_count: &mut usize,
    value_count: &mut usize,
    control_ids: &mut HashSet<String>,
) -> Result<(), String> {
    if depth > MAX_VIEW_DEPTH {
        return Err(format!(
            "Plugin view exceeds the maximum depth of {MAX_VIEW_DEPTH}"
        ));
    }
    *node_count += 1;
    if *node_count > MAX_VIEW_NODES {
        return Err(format!(
            "Plugin view exceeds the maximum of {MAX_VIEW_NODES} nodes"
        ));
    }
    if node.children().len() > MAX_VIEW_CHILDREN {
        return Err(format!(
            "Plugin view node has more than {MAX_VIEW_CHILDREN} children"
        ));
    }

    match node {
        PluginUiNode::Column { .. }
        | PluginUiNode::Row { .. }
        | PluginUiNode::Divider
        | PluginUiNode::Spacer { .. } => {}
        PluginUiNode::Text { text, .. } => validate_text(text, 4_096, "view text")?,
        PluginUiNode::TextInput {
            id,
            label,
            placeholder,
            value,
            max_length,
            submit,
            ..
        } => {
            validate_control_id(id, control_ids)?;
            *value_count += 1;
            if *value_count > MAX_VIEW_VALUES {
                return Err(format!(
                    "Plugin view has more than {MAX_VIEW_VALUES} value controls"
                ));
            }
            if let Some(label) = label {
                validate_text(label, 200, "view input label")?;
            }
            if let Some(placeholder) = placeholder {
                validate_text(placeholder, 300, "view input placeholder")?;
            }
            if !(1..=4_096).contains(max_length) {
                return Err(format!(
                    "Plugin view input `{id}` maxLength must be between 1 and 4096"
                ));
            }
            if value.chars().count() > *max_length {
                return Err(format!("Plugin view input `{id}` value exceeds maxLength"));
            }
            if let Some(submit) = submit
                && !valid_id(submit)
            {
                return Err(format!(
                    "Plugin view input `{id}` has invalid submit action `{submit}`"
                ));
            }
        }
        PluginUiNode::TextArea {
            id,
            label,
            placeholder,
            value,
            max_length,
            rows,
            submit,
            ..
        } => {
            validate_control_id(id, control_ids)?;
            *value_count += 1;
            if *value_count > MAX_VIEW_VALUES {
                return Err(format!(
                    "Plugin view has more than {MAX_VIEW_VALUES} value controls"
                ));
            }
            if let Some(label) = label {
                validate_text(label, 200, "view text area label")?;
            }
            if let Some(placeholder) = placeholder {
                validate_optional_text(placeholder, 300, "view text area placeholder")?;
            }
            if !(1..=4_096).contains(max_length) {
                return Err(format!(
                    "Plugin view text area `{id}` maxLength must be between 1 and 4096"
                ));
            }
            if !(2..=24).contains(rows) {
                return Err(format!(
                    "Plugin view text area `{id}` rows must be between 2 and 24"
                ));
            }
            if value.chars().count() > *max_length {
                return Err(format!(
                    "Plugin view text area `{id}` value exceeds maxLength"
                ));
            }
            if let Some(submit) = submit
                && !valid_id(submit)
            {
                return Err(format!(
                    "Plugin view text area `{id}` has invalid submit action `{submit}`"
                ));
            }
        }
        PluginUiNode::Select {
            id,
            label,
            placeholder,
            value,
            options,
            action,
            ..
        } => {
            validate_control_id(id, control_ids)?;
            *value_count += 1;
            if *value_count > MAX_VIEW_VALUES {
                return Err(format!(
                    "Plugin view has more than {MAX_VIEW_VALUES} value controls"
                ));
            }
            if let Some(label) = label {
                validate_text(label, 200, "view select label")?;
            }
            if let Some(placeholder) = placeholder {
                validate_optional_text(placeholder, 300, "view select placeholder")?;
            }
            validate_select_options(options, &format!("view select `{id}`"))?;
            if !value.is_empty() && !options.iter().any(|option| option.value == *value) {
                return Err(format!(
                    "Plugin view select `{id}` has an unknown selected value"
                ));
            }
            if let Some(action) = action
                && !valid_id(action)
            {
                return Err(format!(
                    "Plugin view select `{id}` has invalid action `{action}`"
                ));
            }
        }
        PluginUiNode::List {
            id,
            action,
            selected_id,
            search_placeholder,
            children,
            ..
        } => {
            validate_control_id(id, control_ids)?;
            *value_count += 1;
            if *value_count > MAX_VIEW_VALUES {
                return Err(format!(
                    "Plugin view has more than {MAX_VIEW_VALUES} value controls"
                ));
            }
            if let Some(action) = action
                && !valid_id(action)
            {
                return Err(format!(
                    "Plugin view list `{id}` has invalid action `{action}`"
                ));
            }
            if let Some(placeholder) = search_placeholder {
                validate_optional_text(placeholder, 300, "view list search placeholder")?;
            }
            if children
                .iter()
                .any(|child| !matches!(child, PluginUiNode::ListItem { .. }))
            {
                return Err(format!(
                    "Plugin view list `{id}` may only contain ListItem children"
                ));
            }
            if let Some(selected_id) = selected_id
                && !children.iter().any(
                    |child| matches!(child, PluginUiNode::ListItem { id, .. } if id == selected_id),
                )
            {
                return Err(format!(
                    "Plugin view list `{id}` has an unknown selectedId `{selected_id}`"
                ));
            }
        }
        PluginUiNode::ListItem {
            id,
            title,
            subtitle,
            keywords,
            status,
            payload,
            action,
            ..
        } => {
            validate_control_id(id, control_ids)?;
            validate_text(title, 300, "view list item title")?;
            if let Some(subtitle) = subtitle {
                validate_text(subtitle, 500, "view list item subtitle")?;
            }
            for keyword in keywords {
                validate_text(keyword, 100, "view list item keyword")?;
            }
            if let Some(status) = status {
                validate_text(status, 100, "view list item status")?;
            }
            if let Some(payload) = payload {
                validate_optional_text(payload, 1_024, "view list item payload")?;
            }
            if let Some(action) = action
                && !valid_id(action)
            {
                return Err(format!(
                    "Plugin view list item `{id}` has invalid action `{action}`"
                ));
            }
        }
        PluginUiNode::EmptyState { title, description } => {
            validate_text(title, 300, "view empty state title")?;
            if let Some(description) = description {
                validate_text(description, 1_000, "view empty state description")?;
            }
        }
        PluginUiNode::Progress { label, value } => {
            if let Some(label) = label {
                validate_text(label, 300, "view progress label")?;
            }
            if value.is_some_and(|value| value > 100) {
                return Err("Plugin view progress value must be between 0 and 100".to_string());
            }
        }
        PluginUiNode::Button {
            id,
            action,
            label,
            payload,
            ..
        }
        | PluginUiNode::Checkbox {
            id,
            action,
            label,
            payload,
            ..
        } => {
            validate_control_id(id, control_ids)?;
            if matches!(node, PluginUiNode::Checkbox { .. }) {
                *value_count += 1;
                if *value_count > MAX_VIEW_VALUES {
                    return Err(format!(
                        "Plugin view has more than {MAX_VIEW_VALUES} value controls"
                    ));
                }
            }
            if !valid_id(action) {
                return Err(format!(
                    "Plugin view control `{id}` has invalid action `{action}`"
                ));
            }
            validate_text(label, 300, "view control label")?;
            if let Some(payload) = payload {
                validate_text(payload, 1_024, "view control payload")?;
            }
        }
    }

    for child in node.children() {
        validate_view_node(child, depth + 1, node_count, value_count, control_ids)?;
    }
    Ok(())
}

fn validate_control_id(id: &str, control_ids: &mut HashSet<String>) -> Result<(), String> {
    if !valid_id(id) || !control_ids.insert(id.to_string()) {
        return Err(format!(
            "Plugin view has invalid or duplicate control ID `{id}`"
        ));
    }
    Ok(())
}

fn validate_progress(progress: &PluginProgress) -> Result<(), String> {
    if let Some(message) = &progress.message {
        validate_text(message, 500, "progress message")?;
    }
    if progress
        .percentage
        .is_some_and(|percentage| percentage > 100)
    {
        return Err("Plugin progress percentage must be between 0 and 100".to_string());
    }
    Ok(())
}

fn validate_optional_text(value: &str, max_chars: usize, label: &str) -> Result<(), String> {
    if value.chars().count() > max_chars {
        return Err(format!("Plugin {label} exceeds {max_chars} characters"));
    }
    Ok(())
}

fn validate_text(value: &str, max_chars: usize, label: &str) -> Result<(), String> {
    let length = value.chars().count();
    if value.trim().is_empty() {
        return Err(format!("Plugin {label} cannot be empty"));
    }
    if length > max_chars {
        return Err(format!("Plugin {label} exceeds {max_chars} characters"));
    }
    Ok(())
}

pub fn valid_plugin_id(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    value.len() <= 64
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        })
}

fn valid_id(value: &str) -> bool {
    valid_plugin_id(value)
}

fn default_invoke_timeout_ms() -> u64 {
    DEFAULT_INVOKE_TIMEOUT_MS
}

fn default_text_max_length() -> usize {
    1_024
}

fn default_text_area_rows() -> usize {
    4
}

fn default_true() -> bool {
    true
}

fn default_view_params() -> Value {
    Value::Object(serde_json::Map::new())
}

fn default_setting_max_length() -> usize {
    MAX_SETTING_VALUE_LENGTH
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests;
