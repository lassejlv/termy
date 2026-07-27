use super::*;
use tempfile::TempDir;

fn write_plugin(plugins: &Path, id: &str, name: &str, source: &str) -> PathBuf {
    let plugin_dir = plugins.join(id);
    fs::create_dir_all(&plugin_dir).expect("create plugin directory");
    fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "apiVersion": 1,
            "id": id,
            "name": name,
            "capabilities": [],
        }))
        .expect("encode manifest"),
    )
    .expect("write manifest");
    fs::write(plugin_dir.join("plugin.ts"), source).expect("write plugin source");
    plugin_dir
}

fn bun_is_available() -> bool {
    match resolve_bun_binary() {
        Ok(Some(_)) => true,
        Ok(None) if std::env::var_os("CI").is_some() => {
            panic!("Bun is required for plugin runtime tests in CI")
        }
        Ok(None) => false,
        Err(error) => panic!("Invalid Bun runtime configuration: {error}"),
    }
}

#[cfg(unix)]
#[test]
fn plugin_host_environment_restores_gui_missing_path_entries() {
    const CHILD_MARKER: &str = "TERMY_PLUGIN_PATH_TEST_CHILD";

    if std::env::var_os(CHILD_MARKER).is_none() {
        let Some(bun) = resolve_bun_binary().expect("resolve Bun") else {
            if std::env::var_os("CI").is_some() {
                panic!("Bun is required for plugin runtime tests in CI");
            }
            return;
        };
        let empty_path = TempDir::new().expect("empty PATH directory");
        let output = Command::new(std::env::current_exe().expect("current test executable"))
            .arg("--exact")
            .arg("tests::plugin_host_environment_restores_gui_missing_path_entries")
            .arg("--nocapture")
            .env(CHILD_MARKER, "1")
            .env("TERMY_BUN_PATH", bun)
            .env("PATH", empty_path.path())
            .output()
            .expect("run isolated plugin PATH test");
        assert!(
            output.status.success(),
            "plugin host did not restore Bun's directory to PATH\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        return;
    }

    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    write_plugin(
        &plugins,
        "path-check",
        "PATH Check",
        r#"
export default definePlugin({
  commands: [{
    id: "run",
    title: "PATH Check: Run",
    run() {
      const pathEntries = (process.env.PATH || "").split(":");
      if (process.platform === "darwin" && !pathEntries.includes("/opt/homebrew/bin")) {
        throw new Error("/opt/homebrew/bin is missing from PATH");
      }
      const result = Bun.spawnSync(["bun", "--version"], {
        env: process.env,
        stdout: "pipe",
        stderr: "pipe",
      });
      if (result.exitCode !== 0) {
        throw new Error(result.stderr.toString() || "bun failed");
      }
      return {
        type: "toast",
        level: "success",
        message: result.stdout.toString().trim(),
      };
    },
  }],
});
"#,
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.errors.is_empty(), "errors: {:?}", refresh.errors);
    let revision = runtime
        .command_with_revision("path-check", "run")
        .expect("PATH check command")
        .1;
    let actions = runtime
        .invoke(
            "path-check",
            "run",
            &revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
        .expect("plugin should resolve Bun by name");
    assert!(matches!(
        actions.as_slice(),
        [PluginAction::Toast {
            level: PluginToastLevel::Success,
            message,
        }] if !message.is_empty()
    ));
}

#[test]
fn tsx_views_render_and_round_trip_actions() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_dir = temp.path().join("config");
    let plugins = config_dir.join("plugins");
    fs::create_dir_all(&config_dir).expect("create config directory");
    let config_path = config_dir.join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugin_dir = write_plugin(&plugins, "todos", "Todos", "export default {};\n");
    fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"apiVersion":1,"id":"todos","name":"Todos","main":"plugin.tsx","capabilities":["storage","native-ui"]}"#,
    )
    .expect("write TSX manifest");
    fs::write(
        plugin_dir.join("plugin.tsx"),
        r#"
/** @jsxRuntime classic */
/** @jsx TermyUI.createElement */
/** @jsxFrag TermyUI.Fragment */

type Todo = { id: string; title: string; done: boolean };

export default definePlugin({
  commands: [
    {
      id: "open",
      title: "Todos: Open",
      run() {
        return { type: "view.open", view: "todos", target: "commandPalette" };
      },
    },
    {
      id: "open-modal",
      title: "Todos: Open Modal",
      run() {
        return { type: "view.open", view: "todos" };
      },
    },
  ],
  views: {
    todos: {
      title: "Todos",
      async render({ context }) {
        const todos = (await context.storage.get<Todo[]>("todos")) ?? [];
        return (
          <TermyUI.Column gap="medium">
            <TermyUI.Text variant="heading">Todos</TermyUI.Text>
            <TermyUI.Row gap="small" align="center">
              <TermyUI.TextInput
                id="title"
                placeholder="Add a task…"
                submit="add"
              />
              <TermyUI.Button id="add-button" action="add" variant="primary">
                Add
              </TermyUI.Button>
            </TermyUI.Row>
            <TermyUI.Divider />
            {todos.length === 0 ? (
              <TermyUI.Text tone="muted">No tasks yet</TermyUI.Text>
            ) : todos.map((todo) => (
              <TermyUI.Checkbox
                id={`todo-${todo.id}`}
                action="toggle"
                payload={todo.id}
                checked={todo.done}
              >
                {todo.title}
              </TermyUI.Checkbox>
            ))}
          </TermyUI.Column>
        );
      },
      async onAction({ action, values, context }) {
        const todos = (await context.storage.get<Todo[]>("todos")) ?? [];
        if (action.id === "add") {
          const title = String(values.title ?? "").trim();
          if (!title) return;
          await context.storage.set("todos", [
            ...todos,
            { id: String(todos.length + 1), title, done: false },
          ]);
          context.toasts.success("Todo added");
        }
        if (action.id === "toggle" && action.payload) {
          await context.storage.set("todos", todos.map((todo) =>
            todo.id === action.payload ? { ...todo, done: !todo.done } : todo
          ));
        }
      },
    },
    emoji: {
      title: "Emoji",
      render() {
        return <TermyUI.TextInput id="emoji" value="😀" maxLength={1} />;
      },
    },
    "invalid-children": {
      title: "Invalid children",
      render() {
        return <TermyUI.TextInput id="invalid">Nope</TermyUI.TextInput>;
      },
    },
  },
} satisfies TermyPlugin);
"#,
    )
    .expect("write TSX plugin");

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.changed, "refresh errors: {:?}", refresh.errors);
    assert!(
        refresh.errors.is_empty(),
        "refresh errors: {:?}",
        refresh.errors
    );
    assert_eq!(runtime.views().len(), 3);
    let revision = runtime
        .command_with_revision("todos", "open")
        .expect("command revision")
        .1;
    let actions = runtime
        .invoke(
            "todos",
            "open",
            &revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
        .expect("open view action");
    assert_eq!(
        actions,
        vec![PluginAction::ViewOpen {
            view: "todos".to_string(),
            target: PluginViewTarget::CommandPalette,
            plugin_id: "todos".to_string(),
            revision: revision.clone(),
        }]
    );
    let modal_revision = runtime
        .command_with_revision("todos", "open-modal")
        .expect("modal command revision")
        .1;
    let modal_actions = runtime
        .invoke(
            "todos",
            "open-modal",
            &modal_revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
        .expect("open modal view action");
    assert!(matches!(
        modal_actions.as_slice(),
        [PluginAction::ViewOpen {
            target: PluginViewTarget::Modal,
            ..
        }]
    ));

    let first = runtime
        .render_view("todos", "todos", &revision, test_plugin_context())
        .expect("render empty todo view");
    assert_eq!(first.plugin_id, "todos");
    assert_eq!(first.revision, revision);
    assert!(first.actions.is_empty());
    assert_eq!(first.nodes.len(), 1);
    runtime
        .render_view("todos", "emoji", &revision, test_plugin_context())
        .expect("one non-BMP character must satisfy maxLength one");
    let invalid_children = runtime
        .render_view(
            "todos",
            "invalid-children",
            &revision,
            test_plugin_context(),
        )
        .expect_err("leaf components must reject children");
    assert!(invalid_children.contains("does not support children"));

    let updated = runtime
        .invoke_view_action(
            "todos",
            "todos",
            &revision,
            PluginViewAction {
                id: "add".to_string(),
                control_id: "add-button".to_string(),
                payload: None,
                value: None,
            },
            BTreeMap::from([(
                "title".to_string(),
                PluginViewValue::Text("Ship JSX".to_string()),
            )]),
            test_plugin_context(),
        )
        .expect("add todo and rerender");
    assert_eq!(
        updated.actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Success,
            message: "Todo added".to_string(),
        }]
    );
    let rendered = format!("{:?}", updated.nodes);
    assert!(rendered.contains("Ship JSX"));
    assert!(rendered.contains("todo-1"));
}

#[test]
fn runtime_enforces_declared_capabilities() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");

    write_plugin(
        &plugins,
        "undeclared",
        "Undeclared",
        r#"
export default definePlugin({
  commands: [
    {
      id: "storage",
      title: "Use storage",
      async run({ context }) {
        await context.storage.get("value");
      },
    },
    {
      id: "paths",
      title: "Use paths",
      run({ context }) {
        return { type: "toast", level: "info", message: context.paths.dataDirectory };
      },
    },
    {
      id: "open",
      title: "Open view",
      run() {
        return { type: "view.open", view: "missing" };
      },
    },
  ],
});
"#,
    );
    write_plugin(
        &plugins,
        "undeclared-view",
        "Undeclared View",
        r#"
export default definePlugin({
  commands: [],
  views: {
    panel: { title: "Panel", render() { return "Hello"; } },
  },
});
"#,
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.changed);
    assert!(
        refresh.errors.iter().any(|error| error
            .contains("Declaring plugin views requires capability `native-ui` in plugin.json")),
        "refresh errors: {:?}",
        refresh.errors
    );

    let revision = runtime
        .command_with_revision("undeclared", "storage")
        .expect("command-only plugin should still load")
        .1;
    let error = runtime
        .invoke(
            "undeclared",
            "storage",
            &revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
        .expect_err("storage API must require its capability");
    assert!(error.contains("Plugin storage requires capability `storage` in plugin.json"));

    let error = runtime
        .invoke(
            "undeclared",
            "paths",
            &revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
        .expect_err("storage paths must require their capability");
    assert!(error.contains("Plugin storage requires capability `storage` in plugin.json"));

    let error = runtime
        .invoke(
            "undeclared",
            "open",
            &revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
        .expect_err("opening native UI must require its capability");
    assert!(error.contains("Opening plugin views requires capability `native-ui` in plugin.json"));

    assert!(!plugins.join(".termy-data/undeclared").exists());
    assert!(!plugins.join(".termy-cache/data/undeclared").exists());
}

#[test]
fn native_view_documents_reject_unknown_props_and_unsafe_shapes() {
    let unknown = serde_json::from_value::<PluginUiNode>(serde_json::json!({
        "type": "text",
        "text": "hello",
        "style": { "background": "red" }
    }))
    .expect_err("arbitrary native props must be rejected");
    assert!(unknown.to_string().contains("unknown field"));

    let duplicate_controls = vec![PluginUiNode::Column {
        gap: PluginUiGap::Medium,
        align: PluginUiAlignment::Start,
        children: vec![
            PluginUiNode::Button {
                id: "save".to_string(),
                action: "save".to_string(),
                label: "Save".to_string(),
                payload: None,
                variant: PluginUiButtonVariant::Primary,
                disabled: false,
            },
            PluginUiNode::Checkbox {
                id: "save".to_string(),
                action: "toggle".to_string(),
                label: "Saved".to_string(),
                payload: None,
                checked: false,
                disabled: false,
            },
        ],
    }];
    let error = validate_view_nodes(&duplicate_controls)
        .expect_err("duplicate native control IDs must be rejected");
    assert!(error.contains("duplicate control ID"));

    let value_controls = (0..=MAX_VIEW_VALUES)
        .map(|index| PluginUiNode::TextInput {
            id: format!("value-{index}"),
            label: None,
            placeholder: None,
            value: String::new(),
            max_length: 32,
            submit: None,
            disabled: false,
        })
        .collect::<Vec<_>>();
    let too_many_values = vec![
        PluginUiNode::Column {
            gap: PluginUiGap::Medium,
            align: PluginUiAlignment::Start,
            children: value_controls[..32].to_vec(),
        },
        PluginUiNode::Column {
            gap: PluginUiGap::Medium,
            align: PluginUiAlignment::Start,
            children: value_controls[32..].to_vec(),
        },
    ];
    let error = validate_view_nodes(&too_many_values)
        .expect_err("documents must not render more values than actions can submit");
    assert!(error.contains("value controls"));

    let mut too_deep = PluginUiNode::Text {
        text: "bottom".to_string(),
        variant: PluginUiTextVariant::Body,
        tone: PluginUiTone::Default,
    };
    for _ in 0..MAX_VIEW_DEPTH {
        too_deep = PluginUiNode::Column {
            gap: PluginUiGap::None,
            align: PluginUiAlignment::Start,
            children: vec![too_deep],
        };
    }
    let error =
        validate_view_nodes(&[too_deep]).expect_err("deep native documents must be rejected");
    assert!(error.contains("maximum depth"));
}

fn test_plugin_context() -> PluginContext {
    PluginContext {
        working_directory: None,
        active_command: None,
        selected_text: None,
        selected_text_truncated: false,
        shell: "/bin/test-shell".to_string(),
        runtime: PluginRuntimeKind::Native,
        active_tab: None,
        active_pane: None,
        platform: std::env::consts::OS.to_string(),
        app_version: "test".to_string(),
        settings: BTreeMap::new(),
    }
}

#[test]
fn discovery_is_sorted_and_fingerprints_contents() {
    let temp = TempDir::new().expect("temp dir");
    let plugins = temp.path().join("plugins");
    write_plugin(
        &plugins,
        "z-last",
        "Z Last",
        "export default { commands: [] };",
    );
    let first_dir = write_plugin(
        &plugins,
        "a-first",
        "A First",
        "export { value } from './helper'; export default { commands: [] };",
    );
    fs::write(first_dir.join("helper.ts"), "export const value = 1;")
        .expect("write imported module");

    let first = discover_plugins(&plugins).expect("discover plugins");
    assert_eq!(
        first
            .sources
            .iter()
            .map(|source| source.id.as_str())
            .collect::<Vec<_>>(),
        ["a-first", "z-last"]
    );
    fs::write(first_dir.join("helper.ts"), "export const value = 2;")
        .expect("change imported module");
    let second = discover_plugins(&plugins).expect("rediscover plugins");
    assert_ne!(first.fingerprint, second.fingerprint);
}

#[test]
fn discovery_requires_a_manifest_and_ignores_loose_typescript() {
    let temp = TempDir::new().expect("temp dir");
    let plugins = temp.path().join("plugins");
    fs::create_dir_all(&plugins).expect("create plugins directory");
    fs::write(plugins.join("hello.ts"), "export default {};").expect("write file plugin");
    let ignored = plugins.join("ignored");
    fs::create_dir_all(&ignored).expect("create ignored directory");
    fs::write(ignored.join("plugin.ts"), "export default {};").expect("write unmanifested plugin");
    write_plugin(
        &plugins,
        "hello",
        "Hello",
        "export default { commands: [] };",
    );

    let discovered = discover_plugins(&plugins).expect("discover plugins");
    assert_eq!(discovered.sources.len(), 1);
    assert_eq!(discovered.sources[0].id, "hello");
}

#[test]
fn manifest_capabilities_are_explicit_and_validated() {
    let temp = TempDir::new().expect("temp dir");
    let plugins = temp.path().join("plugins");
    let plugin = write_plugin(
        &plugins,
        "capabilities",
        "Capabilities",
        "export default definePlugin({ commands: [] });",
    );
    let manifest = plugin.join("plugin.json");

    fs::write(
        &manifest,
        r#"{"apiVersion":1,"id":"capabilities","name":"Capabilities"}"#,
    )
    .expect("write manifest without capabilities");
    validated_plugin_manifest(&plugin, "capabilities")
        .expect("missing capabilities should mean an empty list");

    fs::write(
        &manifest,
        r#"{"apiVersion":1,"id":"capabilities","name":"Capabilities","capabilities":["storage","native-ui"]}"#,
    )
    .expect("write supported capabilities");
    validated_plugin_manifest(&plugin, "capabilities")
        .expect("supported capabilities should be accepted");

    fs::write(
        &manifest,
        r#"{"apiVersion":1,"id":"capabilities","name":"Capabilities","capabilities":["root-access"]}"#,
    )
    .expect("write unsupported capability");
    let error = validated_plugin_manifest(&plugin, "capabilities")
        .expect_err("unknown capabilities must be rejected");
    assert!(error.contains("capability `root-access` is not supported"));

    fs::write(
        &manifest,
        r#"{"apiVersion":1,"id":"capabilities","name":"Capabilities","capabilities":["storage","storage"]}"#,
    )
    .expect("write duplicate capability");
    let error = validated_plugin_manifest(&plugin, "capabilities")
        .expect_err("duplicate capabilities must be rejected");
    assert!(error.contains("capability `storage` is duplicated"));

    fs::write(
        &manifest,
        r#"{"apiVersion":1,"id":"capabilities","name":"Capabilities","capabilities":"storage"}"#,
    )
    .expect("write non-array capabilities");
    let error = validated_plugin_manifest(&plugin, "capabilities")
        .expect_err("capabilities must be an array");
    assert!(error.contains("Invalid plugin.json"));
}

#[test]
fn plugin_manifest_schema_tracks_runtime_capabilities() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../website/public/schemas/plugin.schema.json"
    ))
    .expect("plugin manifest schema must be valid JSON");
    let schema_capabilities = schema["properties"]["capabilities"]["items"]["enum"]
        .as_array()
        .expect("schema capability enum");
    let runtime_capabilities = PLUGIN_CAPABILITIES
        .iter()
        .map(|capability| Value::String((*capability).to_string()))
        .collect::<Vec<_>>();
    assert_eq!(schema_capabilities, &runtime_capabilities);
}

#[test]
fn local_plugin_management_installs_toggles_and_uninstalls() {
    let temp = TempDir::new().expect("temp dir");
    let config_dir = temp.path().join("config");
    fs::create_dir_all(&config_dir).expect("create config directory");
    let config_path = config_dir.join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let source_parent = temp.path().join("source");
    let source = write_plugin(
        &source_parent,
        "managed",
        "Managed Plugin",
        "export default definePlugin({ commands: [] });",
    );
    fs::write(
        source.join("plugin.json"),
        r#"{"apiVersion":1,"id":"managed","name":"Managed Plugin","version":"1.2.3"}"#,
    )
    .expect("write versioned manifest");

    let runtime = PluginRuntime::new(Some(&config_path));
    assert!(
        runtime
            .installed_plugins()
            .expect("empty inventory")
            .plugins
            .is_empty()
    );
    let installed = runtime
        .install_from_directory(&source)
        .expect("install local plugin");
    assert_eq!(installed.id, "managed");
    assert_eq!(installed.version.as_deref(), Some("1.2.3"));
    assert!(installed.enabled);
    assert!(installed.path.join("plugin.ts").is_file());
    let managed_data = config_dir.join("plugins/.termy-data/managed");
    let managed_cache = config_dir.join("plugins/.termy-cache/data/managed");
    fs::create_dir_all(&managed_data).expect("create managed plugin data");
    fs::create_dir_all(&managed_cache).expect("create managed plugin cache");
    fs::write(managed_data.join("storage.json"), "{}").expect("write plugin storage");
    fs::write(managed_cache.join("cached.txt"), "cached").expect("write plugin cache");
    let error = runtime
        .sync_from_directory(&installed.path)
        .expect_err("managed copy cannot become its own development source");
    assert!(error.contains("must be outside"));

    runtime
        .set_plugin_enabled("managed", false)
        .expect("disable plugin");
    fs::write(
        source.join("plugin.ts"),
        "export const revision = 2; export default definePlugin({ commands: [] });",
    )
    .expect("edit local source");
    let synced = runtime
        .sync_from_directory(&source)
        .expect("sync local development source");
    assert!(!synced.enabled, "sync must preserve disabled state");
    assert!(
        fs::read_to_string(synced.path.join("plugin.ts"))
            .expect("read synced plugin")
            .contains("revision = 2")
    );
    let disabled = runtime.installed_plugins().expect("disabled inventory");
    assert!(!disabled.plugins[0].enabled);
    assert!(
        discover_plugins(&config_dir.join("plugins"))
            .expect("discover disabled catalog")
            .sources
            .is_empty()
    );

    runtime
        .set_plugin_enabled("managed", true)
        .expect("enable plugin");
    assert_eq!(
        discover_plugins(&config_dir.join("plugins"))
            .expect("discover enabled catalog")
            .sources
            .len(),
        1
    );
    runtime
        .uninstall_plugin("managed")
        .expect("uninstall plugin");
    assert!(
        runtime
            .installed_plugins()
            .expect("inventory after uninstall")
            .plugins
            .is_empty()
    );
    assert!(
        source.is_dir(),
        "uninstall must preserve the selected source"
    );
    assert!(!managed_data.exists(), "uninstall must remove plugin data");
    assert!(
        !managed_cache.exists(),
        "uninstall must remove plugin cache"
    );
}

#[test]
fn github_plugin_management_tracks_source_and_updates_atomically() {
    let temp = TempDir::new().expect("temp dir");
    let config_dir = temp.path().join("config");
    fs::create_dir_all(&config_dir).expect("create config directory");
    let config_path = config_dir.join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let first_source = write_plugin(
        &temp.path().join("first-source"),
        "github-plugin",
        "GitHub Plugin",
        "export const revision = 1; export default definePlugin({ commands: [] });",
    );
    let first_metadata = PluginSourceMetadata {
        repository_url: "https://github.com/termy-org/plugins".to_string(),
        requested_ref: Some("main".to_string()),
        revision: "1111111111111111111111111111111111111111".to_string(),
        subdirectory: "github-plugin".to_string(),
    };
    let runtime = PluginRuntime::new(Some(&config_path));
    let installed = runtime
        .install_from_directory_with_source(&first_source, first_metadata.clone())
        .expect("install GitHub plugin");
    assert_eq!(installed.source.as_ref(), Some(&first_metadata));
    let error = runtime
        .sync_from_directory(&first_source)
        .expect_err("local development must not replace a GitHub installation");
    assert!(error.contains("tracked from GitHub"));

    let conflicting_source = write_plugin(
        &temp.path().join("conflicting-source"),
        "github-plugin",
        "Replacement",
        "export const revision = 99; export default definePlugin({ commands: [] });",
    );
    let error = runtime
        .install_from_directory_with_source(&conflicting_source, first_metadata.clone())
        .expect_err("conflicting install");
    assert!(error.contains("already installed"));
    assert!(
        fs::read_to_string(installed.path.join("plugin.ts"))
            .expect("installed source")
            .contains("revision = 1")
    );

    runtime
        .set_plugin_enabled("github-plugin", false)
        .expect("disable before update");
    let updated_source = write_plugin(
        &temp.path().join("updated-source"),
        "github-plugin",
        "GitHub Plugin",
        "export const revision = 2; export default definePlugin({ commands: [] });",
    );
    let updated_metadata = PluginSourceMetadata {
        revision: "2222222222222222222222222222222222222222".to_string(),
        ..first_metadata
    };
    let updated = runtime
        .update_plugin_from_directory("github-plugin", &updated_source, updated_metadata.clone())
        .expect("update GitHub plugin");
    assert!(!updated.enabled);
    assert_eq!(updated.source.as_ref(), Some(&updated_metadata));
    assert!(
        fs::read_to_string(updated.path.join("plugin.ts"))
            .expect("updated source")
            .contains("revision = 2")
    );
    let inventory = runtime.installed_plugins().expect("updated inventory");
    assert_eq!(
        inventory.plugins[0].source.as_ref(),
        Some(&updated_metadata)
    );
    assert!(!inventory.plugins[0].enabled);
    assert!(
        plugin_tree_files(&updated.path)
            .expect("managed source tree")
            .iter()
            .all(|(path, _)| path != SOURCE_METADATA_FILE)
    );
}

#[test]
fn plugin_secret_accounts_are_injective_and_legacy_migration_is_unambiguous() {
    assert_ne!(
        plugin_secret_account("a.b", "c"),
        plugin_secret_account("a", "b.c"),
        "plugin and setting boundaries must be preserved in credential identities"
    );
    assert!(can_migrate_legacy_plugin_secret("github", "token"));
    assert!(!can_migrate_legacy_plugin_secret("github.tools", "token"));
    assert!(!can_migrate_legacy_plugin_secret("github", "auth.token"));

    let mut secrets = TEST_PLUGIN_SECRETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    secrets.insert(
        legacy_plugin_secret_account("migration-proof", "token"),
        "legacy-secret".to_string(),
    );
    secrets.insert(
        legacy_plugin_secret_account("ambiguous.plugin", "token"),
        "must-not-leak".to_string(),
    );
    drop(secrets);

    assert_eq!(
        read_plugin_secret("migration-proof", "token").expect("migrate legacy secret"),
        Some("legacy-secret".to_string())
    );
    assert_eq!(
        read_plugin_secret("ambiguous.plugin", "token").expect("skip ambiguous legacy secret"),
        None
    );
    let secrets = TEST_PLUGIN_SECRETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert!(!secrets.contains_key(&legacy_plugin_secret_account("migration-proof", "token")));
    assert_eq!(
        secrets.get(&plugin_secret_account("migration-proof", "token")),
        Some(&"legacy-secret".to_string())
    );
}

#[test]
fn plugin_command_placements_default_and_validate() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    write_plugin(
        &plugins,
        "placements",
        "Placements",
        r#"
export default definePlugin({
  commands: [
    { id: "default", title: "Default", run() {} },
    {
      id: "everywhere",
      title: "Everywhere",
      placements: ["commandPalette", "terminalContextMenu", "tabContextMenu"],
      run() {},
    },
    { id: "shortcut-only", title: "Shortcut only", placements: [], run() {} },
  ],
});
"#,
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.errors.is_empty(), "errors: {:?}", refresh.errors);
    let commands = runtime.commands();
    assert_eq!(
        commands
            .iter()
            .find(|command| command.id == "default")
            .expect("default command")
            .placements,
        vec![PluginCommandPlacement::CommandPalette]
    );
    assert_eq!(
        commands
            .iter()
            .find(|command| command.id == "everywhere")
            .expect("everywhere command")
            .placements,
        vec![
            PluginCommandPlacement::CommandPalette,
            PluginCommandPlacement::TerminalContextMenu,
            PluginCommandPlacement::TabContextMenu,
        ]
    );
    assert!(
        commands
            .iter()
            .find(|command| command.id == "shortcut-only")
            .expect("shortcut-only command")
            .placements
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn invalid_disabled_marker_is_reported_and_not_loaded() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let plugins = temp.path().join("plugins");
    let plugin = write_plugin(
        &plugins,
        "unsafe-marker",
        "Unsafe marker",
        "export default { commands: [] };",
    );
    let outside = temp.path().join("outside");
    fs::write(&outside, "outside").expect("write marker target");
    symlink(&outside, plugin.join(DISABLED_MARKER)).expect("create marker symlink");

    let inventory = inventory_plugins(&plugins).expect("inventory plugins");
    assert_eq!(inventory.plugins.len(), 1);
    assert!(!inventory.plugins[0].enabled);
    assert!(
        inventory.plugins[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("invalid .termy-disabled marker"))
    );
    let error = discover_plugins(&plugins).expect_err("invalid marker must block loading");
    assert!(error.contains("invalid .termy-disabled marker"));
}

#[cfg(unix)]
#[test]
fn discovery_rejects_symlinked_plugin_sources() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let plugins = temp.path().join("plugins");
    let plugin_dir = write_plugin(
        &plugins,
        "linked",
        "Linked",
        "export { value } from './helper.ts'; export default { commands: [] };",
    );
    let outside = temp.path().join("outside.ts");
    fs::write(&outside, "export const value = 1;").expect("write symlink target");
    symlink(&outside, plugin_dir.join("helper.ts")).expect("create source symlink");

    let error = discover_plugins(&plugins).expect_err("symlink must be rejected");
    assert!(error.contains("symlink"), "unexpected error: {error}");
}

#[cfg(unix)]
#[test]
fn discovery_rejects_symlinked_plugin_roots() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let plugins = temp.path().join("plugins");
    let external_plugins = temp.path().join("external-plugins");
    let external = write_plugin(
        &external_plugins,
        "linked",
        "Linked",
        "export default definePlugin({ commands: [] });",
    );
    fs::create_dir_all(&plugins).expect("create plugins directory");
    symlink(external, plugins.join("linked")).expect("create plugin root symlink");

    let error = discover_plugins(&plugins).expect_err("plugin root symlink must be rejected");
    assert!(error.contains("symlink"), "unexpected error: {error}");
}

#[test]
fn protocol_handshake_is_bounded_and_requires_authentication() {
    let secret = "0123456789abcdef";
    let valid = format!("{{\"secret\":\"{secret}\"}}\n");
    assert!(valid_protocol_handshake(&valid, valid.len(), secret));
    assert!(!valid_protocol_handshake(
        valid.trim_end(),
        valid.trim_end().len(),
        secret
    ));
    assert!(!valid_protocol_handshake(
        &valid,
        MAX_PROTOCOL_HANDSHAKE_BYTES + 1,
        secret
    ));
    assert!(!valid_protocol_handshake(&valid, valid.len(), "wrong"));
}

#[test]
fn protocol_handshake_honors_its_absolute_deadline() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind handshake listener");
    let address = listener.local_addr().expect("handshake listener address");
    let peer = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).expect("connect handshake peer");
        stream.write_all(b"{").expect("write partial handshake");
        thread::sleep(Duration::from_millis(250));
    });
    let (mut stream, _) = listener.accept().expect("accept handshake peer");
    stream
        .set_nonblocking(true)
        .expect("make handshake peer nonblocking");
    let started = Instant::now();
    assert!(!read_protocol_handshake(
        &mut stream,
        "secret",
        started + Duration::from_millis(30)
    ));
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "partial handshake exceeded its absolute deadline"
    );
    peer.join().expect("join handshake peer");
}

#[test]
fn discovery_caps_plugin_count_before_starting_workers() {
    let temp = TempDir::new().expect("temp dir");
    let plugins = temp.path().join("plugins");
    for index in 0..=MAX_INSTALLED_PLUGINS {
        write_plugin(
            &plugins,
            &format!("plugin-{index:02}"),
            &format!("Plugin {index}"),
            "export default { commands: [] };",
        );
    }

    let error = discover_plugins(&plugins).expect_err("plugin count must be capped");
    assert!(error.contains("maximum is 32"), "unexpected error: {error}");
}

#[test]
fn managed_sdk_files_are_stable() {
    let temp = TempDir::new().expect("temp dir");
    let plugins = temp.path().join("plugins");
    ensure_managed_files(&plugins).expect("write managed files");
    ensure_managed_files(&plugins).expect("rewrite managed files");
    assert_eq!(
        fs::read_to_string(plugins.join("termy.d.ts")).expect("read declarations"),
        TYPE_DECLARATIONS
    );
    assert_eq!(
        fs::read_to_string(managed_runtime_dir(&plugins).join("host.ts")).expect("read host"),
        HOST_SOURCE
    );
    assert_eq!(
        fs::read_to_string(managed_runtime_dir(&plugins).join("worker.ts")).expect("read worker"),
        WORKER_SOURCE
    );
    assert!(
        managed_runtime_dir(&plugins)
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".termy-runtime-"))
    );
}

#[test]
fn ids_are_strict_and_stable() {
    for valid in ["git-tools", "git.tools", "git_tools", "v2"] {
        assert!(valid_id(valid), "expected valid ID: {valid}");
    }
    for invalid in ["", "Git", "-git", "git tools", "git/tool"] {
        assert!(!valid_id(invalid), "expected invalid ID: {invalid}");
    }
}

#[test]
fn action_validation_rejects_unsafe_url_schemes() {
    let actions = vec![PluginAction::UrlOpen {
        url: "file:///tmp/private".to_string(),
    }];
    assert_eq!(
        validate_actions(&actions),
        Err("Plugin URLs must use http or https".to_string())
    );
}

#[test]
fn concurrent_refreshes_share_one_catalog_load() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    write_plugin(
        &plugins,
        "slow-load",
        "Slow Load",
        r#"
await Bun.sleep(300);
export default definePlugin({
  commands: [{ id: "run", title: "Slow Load: Run", run() {} }],
});
"#,
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let refreshes = (0..2)
        .map(|_| {
            let runtime = runtime.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                runtime.refresh_if_changed()
            })
        })
        .collect::<Vec<_>>();

    barrier.wait();
    let refreshes = refreshes
        .into_iter()
        .map(|refresh| refresh.join().expect("join refresh"))
        .collect::<Vec<_>>();
    assert!(
        refreshes.iter().all(|refresh| refresh.errors.is_empty()),
        "errors: {:?}",
        refreshes
            .iter()
            .flat_map(|refresh| &refresh.errors)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        refreshes.iter().filter(|refresh| refresh.changed).count(),
        1
    );
    assert_eq!(runtime.commands().len(), 1);
}

#[test]
fn unchanged_refresh_restarts_a_failed_host() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    write_plugin(
        &plugins,
        "healthy",
        "Healthy",
        "export default definePlugin({ commands: [] });",
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let initial = runtime.refresh_if_changed();
    assert!(initial.errors.is_empty(), "errors: {:?}", initial.errors);
    let first_connection = runtime
        .inner
        .host
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .connection
        .as_ref()
        .map(Arc::clone)
        .expect("initial host connection");
    fail_host_connection(
        &first_connection.child,
        &first_connection.pending,
        &first_connection.failed,
        &first_connection.failure,
        "simulated idle host failure",
    );

    let refresh = runtime.refresh_if_changed();
    assert!(refresh.changed);
    assert!(refresh.errors.is_empty(), "errors: {:?}", refresh.errors);
    let next_connection = runtime
        .inner
        .host
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .connection
        .as_ref()
        .map(Arc::clone)
        .expect("replacement host connection");
    assert!(!Arc::ptr_eq(&first_connection, &next_connection));
}

#[test]
fn oversized_local_request_keeps_the_host_and_catalog_healthy() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    write_plugin(
        &plugins,
        "large-input",
        "Large Input",
        r#"
export default definePlugin({
  commands: [{
    id: "run",
    title: "Large Input: Run",
    inputs: Array.from({ length: 16 }, (_, index) => ({
      id: `input-${index}`,
      type: "text",
      label: `Input ${index}`,
      maxLength: 16384,
    })),
    run() { return { type: "toast", level: "info", message: "still healthy" }; },
  }],
});
"#,
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.errors.is_empty(), "errors: {:?}", refresh.errors);
    let revision = runtime
        .command_with_revision("large-input", "run")
        .expect("large-input command")
        .1;
    let connection = runtime
        .inner
        .host
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .connection
        .as_ref()
        .map(Arc::clone)
        .expect("host connection");
    let large_value = Value::String("😀".repeat(16_384));
    let inputs = (0..16)
        .map(|index| (format!("input-{index}"), large_value.clone()))
        .collect();
    let context = test_plugin_context;

    let error = runtime
        .invoke("large-input", "run", &revision, inputs, context())
        .expect_err("oversized request must be rejected locally");
    assert!(error.contains("1 MiB protocol limit"));
    assert!(!connection.is_failed());
    assert!(
        runtime
            .inner
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .fingerprint
            .is_some()
    );
    let actions = runtime
        .invoke("large-input", "run", &revision, BTreeMap::new(), context())
        .expect("host remains usable after local request rejection");
    assert_eq!(
        actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Info,
            message: "still healthy".to_string(),
        }]
    );
}

#[test]
fn runtime_loads_and_invokes_plain_typescript_when_bun_is_available() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    let plugin_dir = write_plugin(
        &plugins,
        "hello",
        "Hello",
        r#"
import { greeting } from "./helper.ts";
import { join } from "node:path";

let invocationCount = 0;

export default definePlugin({
  settings: {
    enabled: { type: "toggle", title: "Enabled", defaultValue: true },
    greeting: { type: "text", title: "Greeting", defaultValue: "Hello" },
    style: {
      type: "select",
      title: "Style",
      options: [
        { value: "short", label: "Short" },
        { value: "long", label: "Long" },
      ],
      defaultValue: "short",
    },
    token: { type: "secret", title: "Token" },
  },
  commands: [
    {
      id: "fail",
      title: "Hello: Fail",
      run() {
        process.stdout.write("plugin stdout must not corrupt the protocol\n");
        throw new Error("expected plugin failure");
      },
    },
    {
      id: "invalid",
      title: "Hello: Invalid action",
      run() {
        return { type: "toats", message: "typo" };
      },
    },
    {
      id: "noisy",
      title: "Hello: Noisy output",
      async run() {
        await Bun.stdout.write("direct Bun stdout must not corrupt the protocol\n");
        const { writeSync } = await import("node:fs");
        writeSync(1, "direct fd 1 must not corrupt the protocol\n");
        return { type: "toast", level: "info", message: "Still connected" };
      },
    },
    {
      id: "context-toasts",
      title: "Hello: Context toasts",
      run({ context }) {
        context.toasts.info(`Running on ${context.platform}`);
        context.toasts.success([
          context.runtime,
          context.shell,
          context.activeTab?.title,
          context.activePane?.kind,
          context.selectedText,
          context.selectedTextTruncated,
          Object.isFrozen(context),
          Object.isFrozen(context.activeTab),
          Object.isFrozen(context.activePane),
        ].join("|"));
        return { type: "toast", level: "warning", message: "Returned action" };
      },
    },
    {
      id: "settings",
      title: "Hello: Settings",
      run({ context }) {
        return {
          type: "toast",
          level: "info",
          message: `${context.settings.get("enabled")} ${context.settings.get("greeting")} ${context.settings.get("style")} ${context.settings.get("token")}`,
        };
      },
    },
    {
      id: "storage",
      title: "Hello: Storage",
      async run({ context }) {
        const stored = await context.storage.get("count");
        const count = typeof stored === "number" ? stored + 1 : 1;
        await context.storage.set("count", count);
        const removed = await context.storage.delete("missing");
        await Bun.write(join(context.paths.dataDirectory, "count.txt"), String(count));
        await Bun.write(join(context.paths.cacheDirectory, "marker.txt"), "cached");
        return { type: "toast", level: "success", message: `Storage ${count} ${removed}` };
      },
    },
    {
      id: "greet",
      title: "Hello: Greet",
      inputs: [{ id: "name", type: "text", label: "Name", required: true }],
      async run({ inputs }) {
        invocationCount += 1;
        return { type: "toast", level: "success", message: `${invocationCount}: ${greeting(inputs.name)}` };
      },
    },
  ],
  events: {
    "terminal.ready"({ event, context }) {
      return {
        type: "toast",
        level: "success",
        message: `${event.type}|${context.runtime}|${context.settings.get("greeting")}|${Object.isFrozen(event)}`,
      };
    },
  },
});
"#,
    );
    fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"apiVersion":1,"id":"hello","name":"Hello","capabilities":["storage"]}"#,
    )
    .expect("declare storage capability");
    fs::write(
        plugin_dir.join("helper.ts"),
        r#"export const greeting = (name: unknown) => `Hello ${String(name)}`;"#,
    )
    .expect("write imported helper");
    fs::write(
        plugin_dir.join(SOURCE_METADATA_FILE),
        r#"{"repositoryUrl":"https://github.com/example/plugins"}"#,
    )
    .expect("write managed source metadata");
    let cache_key = discover_plugins(&plugins)
        .expect("discover plugin before loading")
        .sources
        .into_iter()
        .find(|source| source.id == "hello")
        .expect("hello source")
        .cache_key;

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.changed, "refresh errors: {:?}", refresh.errors);
    assert!(
        refresh.errors.is_empty(),
        "refresh errors: {:?}",
        refresh.errors
    );
    assert!(
        plugins
            .join(".termy-cache/bundles/hello")
            .join(format!("{cache_key}.mjs"))
            .is_file(),
        "plugin bundle should be cached by content hash"
    );
    assert_eq!(runtime.commands().len(), 7);
    let revision = runtime
        .command_with_revision("hello", "greet")
        .expect("hello command revision")
        .1;
    let context = PluginContext {
        working_directory: Some("/repo".to_string()),
        active_command: Some("cargo test".to_string()),
        selected_text: Some("failed assertion".to_string()),
        shell: "/bin/zsh".to_string(),
        runtime: PluginRuntimeKind::Tmux,
        active_tab: Some(PluginTabContext {
            index: 2,
            title: "tests".to_string(),
            pane_count: 3,
        }),
        active_pane: Some(PluginPaneContext {
            index: 1,
            kind: PluginPaneKind::Terminal,
        }),
        ..test_plugin_context()
    };
    let settings = runtime.plugin_settings_snapshot();
    assert!(settings.errors.is_empty(), "errors: {:?}", settings.errors);
    assert_eq!(settings.plugins["hello"].len(), 4);
    runtime
        .set_plugin_setting("hello", "enabled", Value::Bool(false))
        .expect("set toggle setting");
    runtime
        .set_plugin_setting("hello", "greeting", Value::String("Yo".to_string()))
        .expect("set text setting");
    runtime
        .set_plugin_setting("hello", "style", Value::String("long".to_string()))
        .expect("set select setting");
    runtime
        .set_plugin_setting("hello", "token", Value::String("super-secret".to_string()))
        .expect("set secret setting");
    let settings_file = fs::read_to_string(plugins.join(".termy-data/hello/settings.json"))
        .expect("read settings file");
    assert!(!settings_file.contains("super-secret"));
    let actions = runtime
        .invoke(
            "hello",
            "settings",
            &revision,
            BTreeMap::new(),
            context.clone(),
        )
        .expect("plugin context should receive resolved settings");
    assert_eq!(
        actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Info,
            message: "false Yo long super-secret".to_string(),
        }]
    );
    let error = runtime
        .invoke("hello", "fail", &revision, BTreeMap::new(), context.clone())
        .expect_err("plugin failure should be reported");
    assert!(error.contains("expected plugin failure"));
    let actions = runtime
        .invoke(
            "hello",
            "noisy",
            &revision,
            BTreeMap::new(),
            context.clone(),
        )
        .expect("direct stdout must stay outside the protocol");
    assert_eq!(
        actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Info,
            message: "Still connected".to_string(),
        }]
    );
    let actions = runtime
        .invoke(
            "hello",
            "context-toasts",
            &revision,
            BTreeMap::new(),
            context.clone(),
        )
        .expect("context toast helpers should emit actions");
    assert_eq!(
        actions,
        vec![
            PluginAction::Toast {
                level: PluginToastLevel::Info,
                message: format!("Running on {}", std::env::consts::OS),
            },
            PluginAction::Toast {
                level: PluginToastLevel::Success,
                message: "tmux|/bin/zsh|tests|terminal|failed assertion|false|true|true|true"
                    .to_string(),
            },
            PluginAction::Toast {
                level: PluginToastLevel::Warning,
                message: "Returned action".to_string(),
            },
        ]
    );
    assert!(runtime.has_event_subscribers(PluginEventKind::TerminalReady));
    assert!(!runtime.has_event_subscribers(PluginEventKind::TabActivated));
    let dispatch = runtime.dispatch_event(PluginEvent::TerminalReady, context.clone());
    assert!(dispatch.errors.is_empty(), "errors: {:?}", dispatch.errors);
    assert_eq!(
        dispatch.actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Success,
            message: "terminal.ready|tmux|Yo|true".to_string(),
        }]
    );
    let actions = runtime
        .invoke(
            "hello",
            "storage",
            &revision,
            BTreeMap::new(),
            context.clone(),
        )
        .expect("plugin storage should persist values");
    assert_eq!(
        actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Success,
            message: "Storage 1 false".to_string(),
        }]
    );
    let error = runtime
        .invoke(
            "hello",
            "invalid",
            &revision,
            BTreeMap::new(),
            context.clone(),
        )
        .expect_err("invalid plugin action should be reported");
    assert!(error.contains("invalid result"));
    let error = runtime
        .invoke(
            "hello",
            "greet",
            &revision,
            BTreeMap::from([("name".to_string(), Value::Bool(true))]),
            context.clone(),
        )
        .expect_err("input types must be validated before invocation");
    assert!(error.contains("must be text"));
    let actions = runtime
        .invoke(
            "hello",
            "greet",
            &revision,
            BTreeMap::from([("name".to_string(), Value::String("Termy".to_string()))]),
            context,
        )
        .expect("invoke plugin");
    assert_eq!(
        actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Success,
            message: "1: Hello Termy".to_string(),
        }]
    );

    write_plugin(
        &plugins,
        "unchanged-trigger",
        "Reload Trigger",
        r#"
export default definePlugin({
  commands: [],
});
"#,
    );
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.changed, "refresh errors: {:?}", refresh.errors);
    assert!(
        refresh.errors.is_empty(),
        "refresh errors: {:?}",
        refresh.errors
    );

    let actions = runtime
        .invoke(
            "hello",
            "greet",
            &revision,
            BTreeMap::from([("name".to_string(), Value::String("Again".to_string()))]),
            test_plugin_context(),
        )
        .expect("invoke unchanged plugin after catalog reload");
    assert_eq!(
        actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Success,
            message: "2: Hello Again".to_string(),
        }]
    );

    fs::write(
        plugin_dir.join("helper.ts"),
        r#"export const greeting = (name: unknown) => `Hi ${String(name)}`;"#,
    )
    .expect("change imported helper");
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.changed, "refresh errors: {:?}", refresh.errors);
    assert!(refresh.errors.is_empty());
    let error = runtime
        .invoke(
            "hello",
            "greet",
            &revision,
            BTreeMap::from([("name".to_string(), Value::String("Old".to_string()))]),
            test_plugin_context(),
        )
        .expect_err("stale command schema revision must be rejected");
    assert!(error.contains("Plugin changed"));
    let new_revision = runtime
        .command_with_revision("hello", "greet")
        .expect("reloaded command revision")
        .1;
    assert_ne!(revision, new_revision);
    let actions = runtime
        .invoke(
            "hello",
            "storage",
            &new_revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
        .expect("plugin storage should survive Worker replacement");
    assert_eq!(
        actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Success,
            message: "Storage 2 false".to_string(),
        }]
    );
    assert_eq!(
        fs::read_to_string(plugins.join(".termy-data/hello/files/count.txt"))
            .expect("read plugin data file"),
        "2"
    );
    assert_eq!(
        fs::read_to_string(plugins.join(".termy-cache/data/hello/marker.txt"))
            .expect("read plugin cache file"),
        "cached"
    );
}

#[test]
fn malformed_plugins_are_isolated_and_empty_refresh_clears_the_catalog() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    let valid_dir = write_plugin(
        &plugins,
        "valid",
        "Valid",
        r#"
export default definePlugin({
  commands: [{ id: "run", title: "Valid: Run", run() {} }],
});
"#,
    );
    let broken_dir = write_plugin(
        &plugins,
        "broken",
        "Broken",
        r#"
export default definePlugin({
  commands: [{
    id: "run",
    title: "Broken: Run",
    enabled: false,
    disabledReason: 42,
    run() {},
  }],
});
"#,
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.changed);
    assert_eq!(refresh.errors.len(), 1, "errors: {:?}", refresh.errors);
    assert!(refresh.errors[0].contains("broken"));
    assert_eq!(runtime.commands().len(), 1);
    assert_eq!(runtime.commands()[0].plugin_id, "valid");

    fs::remove_dir_all(valid_dir).expect("remove valid plugin");
    fs::remove_dir_all(broken_dir).expect("remove broken plugin");
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.changed, "clearing a non-empty catalog is a change");
    assert!(refresh.errors.is_empty());
    assert!(runtime.commands().is_empty());
    assert!(!plugins.join(".termy-cache/bundles").exists());
}

#[test]
fn plugin_load_rejects_imports_outside_the_plugin_directory() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    fs::write(
        temp.path().join("outside.ts"),
        "export const outside = true;",
    )
    .expect("write outside module");
    write_plugin(
        &plugins,
        "escape",
        "Escape",
        r#"
import { outside } from "../../outside.ts";
export default definePlugin({
  commands: [{ id: "run", title: "Escape: Run", run() { return outside; } }],
});
"#,
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert_eq!(refresh.errors.len(), 1, "errors: {:?}", refresh.errors);
    assert!(refresh.errors[0].contains("escapes its directory"));
    assert!(runtime.commands().is_empty());
}

#[test]
fn plugin_invocations_run_concurrently_across_workers() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    write_plugin(
        &plugins,
        "slow",
        "Slow",
        r#"
export default definePlugin({
  commands: [{
    id: "run",
    title: "Slow: Run",
    timeoutMs: 2000,
    async run() {
      await Bun.sleep(500);
      return { type: "toast", level: "info", message: "slow" };
    },
  }],
});
"#,
    );
    write_plugin(
        &plugins,
        "fast",
        "Fast",
        r#"
export default definePlugin({
  commands: [{
    id: "run",
    title: "Fast: Run",
    run() { return { type: "toast", level: "info", message: "fast" }; },
  }],
});
"#,
    );

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.errors.is_empty(), "errors: {:?}", refresh.errors);
    let slow_revision = runtime
        .command_with_revision("slow", "run")
        .expect("slow command")
        .1;
    let fast_revision = runtime
        .command_with_revision("fast", "run")
        .expect("fast command")
        .1;
    let context = test_plugin_context;
    let slow_runtime = runtime.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let slow = thread::spawn(move || {
        started_tx.send(()).expect("signal slow invocation");
        slow_runtime.invoke(
            "slow",
            "run",
            &slow_revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
    });
    started_rx.recv().expect("wait for slow invocation");
    thread::sleep(Duration::from_millis(75));
    let started = Instant::now();
    let fast_actions = runtime
        .invoke("fast", "run", &fast_revision, BTreeMap::new(), context())
        .expect("fast plugin invocation");
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "fast plugin waited behind slow plugin for {:?}",
        started.elapsed()
    );
    assert_eq!(
        fast_actions,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Info,
            message: "fast".to_string(),
        }]
    );
    slow.join()
        .expect("join slow invocation")
        .expect("slow invocation succeeds");
}

#[test]
fn disabling_plugin_revokes_in_flight_actions() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    let marker = temp.path().join("revocation-started");
    let marker_json =
        serde_json::to_string(marker.to_string_lossy().as_ref()).expect("encode marker path");
    let source = r#"
const markerPath = __MARKER__;
export default definePlugin({
  commands: [{
    id: "run",
    title: "Revocation: Run",
    timeoutMs: 2000,
    async run() {
      await Bun.write(markerPath, "started");
      await Bun.sleep(300);
      return { type: "toast", level: "info", message: "must be revoked" };
    },
  }],
});
"#
    .replace("__MARKER__", &marker_json);
    write_plugin(&plugins, "revocation", "Revocation", &source);

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.errors.is_empty(), "errors: {:?}", refresh.errors);
    let revision = runtime
        .command_with_revision("revocation", "run")
        .expect("revocation command")
        .1;
    let invoke_runtime = runtime.clone();
    let invocation = thread::spawn(move || {
        invoke_runtime.invoke(
            "revocation",
            "run",
            &revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
    });
    let marker_deadline = Instant::now() + Duration::from_secs(2);
    while !marker.exists() {
        assert!(
            Instant::now() < marker_deadline,
            "revocation plugin did not start in time"
        );
        thread::sleep(Duration::from_millis(10));
    }

    runtime
        .set_plugin_enabled("revocation", false)
        .expect("disable plugin while invocation is running");
    let error = invocation
        .join()
        .expect("join revocation invocation")
        .expect_err("disabled plugin actions must be revoked");
    assert!(
        error.contains("disabled") || error.contains("changed") || error.contains("removed"),
        "unexpected revocation error: {error}"
    );
}

#[test]
fn queued_plugin_invocation_starts_its_timeout_when_execution_begins() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    let marker = temp.path().join("slow-started");
    let marker_json =
        serde_json::to_string(marker.to_string_lossy().as_ref()).expect("encode marker path");
    let source = r#"
const markerPath = __MARKER__;
export default definePlugin({
  commands: [
    {
      id: "slow",
      title: "Queued: Slow",
      timeoutMs: 1000,
      async run() {
        await Bun.write(markerPath, "started");
        await Bun.sleep(350);
        return { type: "toast", level: "info", message: "slow" };
      },
    },
    {
      id: "quick",
      title: "Queued: Quick",
      timeoutMs: 100,
      async run() {
        await Bun.sleep(25);
        return { type: "toast", level: "info", message: "quick" };
      },
    },
  ],
});
"#
    .replace("__MARKER__", &marker_json);
    write_plugin(&plugins, "queued", "Queued", &source);

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.errors.is_empty(), "errors: {:?}", refresh.errors);
    let revision = runtime
        .command_with_revision("queued", "slow")
        .expect("queued command")
        .1;
    let slow_runtime = runtime.clone();
    let slow_revision = revision.clone();
    let slow = thread::spawn(move || {
        slow_runtime.invoke(
            "queued",
            "slow",
            &slow_revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
    });
    let marker_deadline = Instant::now() + Duration::from_secs(2);
    while !marker.exists() {
        assert!(
            Instant::now() < marker_deadline,
            "slow plugin did not start in time"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let started = Instant::now();
    let quick = runtime
        .invoke(
            "queued",
            "quick",
            &revision,
            BTreeMap::new(),
            test_plugin_context(),
        )
        .expect("queued quick invocation should receive its full execution timeout");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "same-plugin invocation did not wait for the active command"
    );
    assert_eq!(
        quick,
        vec![PluginAction::Toast {
            level: PluginToastLevel::Info,
            message: "quick".to_string(),
        }]
    );
    slow.join()
        .expect("join slow invocation")
        .expect("slow invocation succeeds");
}

#[test]
fn plugin_load_removes_abandoned_build_artifacts() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    write_plugin(
        &plugins,
        "clean",
        "Clean",
        "export default definePlugin({ commands: [] });",
    );
    let bundle_dir = plugins.join(".termy-cache/bundles/clean");
    let capture = bundle_dir.join(".capture-abandoned");
    let temporary = bundle_dir.join("abandoned.mjs.1.tmp");
    fs::create_dir_all(&capture).expect("create abandoned capture");
    fs::write(capture.join("plugin.ts"), "stale").expect("write abandoned capture");
    fs::write(&temporary, "stale").expect("write abandoned bundle");

    let runtime = PluginRuntime::new(Some(&config_path));
    let refresh = runtime.refresh_if_changed();
    assert!(refresh.errors.is_empty(), "errors: {:?}", refresh.errors);
    assert!(!capture.exists(), "abandoned capture should be removed");
    assert!(!temporary.exists(), "abandoned bundle should be removed");
}

#[test]
fn failed_plugin_loads_are_retried_when_bun_is_available() {
    if !bun_is_available() {
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.txt");
    fs::write(&config_path, "").expect("write config");
    let plugins = temp.path().join("plugins");
    let plugin_dir = write_plugin(
        &plugins,
        "broken",
        "Broken",
        "export default definePlugin({ commands: [] });",
    );
    fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"apiVersion":99,"id":"broken","name":"Broken"}"#,
    )
    .expect("write broken manifest");

    let runtime = PluginRuntime::new(Some(&config_path));
    let first = runtime.refresh_if_changed();
    assert!(!first.errors.is_empty());
    let second = runtime.refresh_if_changed();
    assert!(
        !second.errors.is_empty(),
        "unchanged failed plugins must be retried"
    );
}
