use super::{OPEN_TAB_HERE_LABEL, posix_single_quote};
use std::path::{Path, PathBuf};

pub(super) fn register(
    executable: &Path,
    _on_open_directory: impl Fn(PathBuf) + Send + Sync + 'static,
) -> Result<(), String> {
    install_user_service(executable)
}

fn install_user_service(executable: &Path) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let service_dir = PathBuf::from(home)
        .join("Library/Services")
        .join(format!("{OPEN_TAB_HERE_LABEL}.workflow"))
        .join("Contents");
    std::fs::create_dir_all(&service_dir)
        .map_err(|error| format!("failed to create Finder service: {error}"))?;
    std::fs::write(service_dir.join("Info.plist"), service_info_plist())
        .map_err(|error| format!("failed to write Finder service Info.plist: {error}"))?;
    std::fs::write(
        service_dir.join("document.wflow"),
        service_workflow(executable),
    )
    .map_err(|error| format!("failed to write Finder service workflow: {error}"))?;
    let _ = std::process::Command::new("/System/Library/CoreServices/pbs")
        .arg("-flush")
        .status();
    Ok(())
}

fn service_info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>{OPEN_TAB_HERE_LABEL}</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.folder</string>
				<string>public.directory</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    )
}

fn service_workflow(executable: &Path) -> String {
    let exe = posix_single_quote(&executable.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.path</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>for f in "$@"; do
  if [ -f "$f" ]; then f="$(dirname "$f")"; fi
  {exe} --working-directory "$f" &amp;
done</string>
					<key>CheckedForDataType</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/bash</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>1.0.2</string>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>7c3d1a2e-4b5f-4a6c-9d0e-1f2a3b4c5d6e</string>
				<key>OutputUUID</key>
				<string>8d4e2b3f-5c6a-4b7d-ae1f-2a3b4c5d6e7f</string>
				<key>UUID</key>
				<string>9e5f3c4a-6d7b-4c8e-bf20-3b4c5d6e7f80</string>
			</dict>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowTypeIdentifier</key>
	<string>com.apple.Automator.servicesMenu</string>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::service_workflow;
    use std::path::Path;

    #[test]
    fn workflow_runs_termy_with_working_directory() {
        let workflow = service_workflow(Path::new("/Applications/Termy.app/Contents/MacOS/Termy"));
        assert!(workflow.contains("--working-directory"));
        assert!(workflow.contains("/Applications/Termy.app/Contents/MacOS/Termy"));
    }
}
