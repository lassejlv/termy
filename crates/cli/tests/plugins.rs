use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[test]
fn init_add_and_dev_sync_a_managed_local_copy() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let source = temporary.path().join("my-plugin");
    let config_home = temporary.path().join("config");
    let cli = env!("CARGO_BIN_EXE_termy-cli");

    let initialized = Command::new(cli)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("APPDATA", &config_home)
        .args(["plugin", "init"])
        .arg(&source)
        .output()
        .expect("run plugin init");
    assert!(
        initialized.status.success(),
        "plugin init failed: {}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(String::from_utf8_lossy(&initialized.stdout).contains("termy plugin dev"));
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(source.join("plugin.json")).expect("read initialized manifest"),
    )
    .expect("parse initialized manifest");
    assert_eq!(
        manifest["$schema"],
        "https://termy.sh/schemas/plugin.schema.json"
    );
    assert_eq!(manifest["capabilities"], serde_json::json!([]));

    let installed = Command::new(cli)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("APPDATA", &config_home)
        .args(["plugin", "add"])
        .arg(&source)
        .output()
        .expect("run plugin add");
    assert!(
        installed.status.success(),
        "plugin add failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&installed.stderr)
            .contains(termy_core::plugin_runtime::PLUGIN_FULL_ACCESS_WARNING),
        "a local install must still say plugins get full access"
    );
    let help = Command::new(cli)
        .args(["plugin", "add", "--help"])
        .output()
        .expect("run plugin add --help");
    assert!(
        String::from_utf8_lossy(&help.stdout)
            .contains(termy_core::plugin_runtime::PLUGIN_FULL_ACCESS_WARNING)
    );

    let managed = config_home.join("termy/plugins/my-plugin");
    assert!(managed.join("plugin.json").is_file());
    assert!(managed.join("plugin.ts").is_file());
    assert!(source.join("plugin.ts").is_file());
    assert_eq!(
        fs::read(source.join("plugin.ts")).expect("read development source"),
        fs::read(managed.join("plugin.ts")).expect("read managed copy")
    );

    let mut development = Command::new(cli)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("APPDATA", &config_home)
        .args(["plugin", "dev"])
        .arg(&source)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start plugin dev");
    let stdout = development.stdout.take().expect("plugin dev stdout");
    let (ready_sender, ready_receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.contains("Watching ") {
                let _ = ready_sender.send(());
                break;
            }
        }
    });
    if ready_receiver
        .recv_timeout(Duration::from_secs(10))
        .is_err()
    {
        let _ = development.kill();
        let output = development.wait_with_output().expect("collect plugin dev");
        panic!(
            "plugin dev did not start watching: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fs::write(
        source.join("plugin.ts"),
        r#"export default definePlugin({
  commands: [{ id: "changed", title: "Changed", run() {} }],
} satisfies TermyPlugin);
"#,
    )
    .expect("edit development source");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if fs::read_to_string(managed.join("plugin.ts"))
            .is_ok_and(|contents| contents.contains("title: \"Changed\""))
        {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let synced = fs::read_to_string(managed.join("plugin.ts")).expect("read synced plugin");
    let _ = development.kill();
    let _ = development.wait();
    assert!(
        synced.contains("title: \"Changed\""),
        "plugin dev did not sync the edited source"
    );
}
