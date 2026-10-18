fn main() {
    println!("cargo::rustc-check-cfg=cfg(macos_sdk_26)");

    #[cfg(target_os = "macos")]
    {
        use std::path::{Path, PathBuf};
        use std::process::Command;

        fn swift_toolchain_lib_dir() -> Option<PathBuf> {
            let output = Command::new("xcrun")
                .args(["--find", "swiftc"])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }

            let swiftc = String::from_utf8(output.stdout).ok()?;
            let swiftc = PathBuf::from(swiftc.trim());
            let bin_dir = swiftc.parent()?;
            let usr_dir = bin_dir.parent()?;
            Some(usr_dir.join("lib"))
        }

        fn swift_macos_rpaths(toolchain_lib_dir: &Path) -> Vec<PathBuf> {
            let mut rpaths = vec![PathBuf::from("/usr/lib/swift")];
            let bundled_runtime = toolchain_lib_dir.join("swift/macosx");
            if bundled_runtime.is_dir() {
                rpaths.push(bundled_runtime);
            }

            if let Ok(entries) = std::fs::read_dir(toolchain_lib_dir) {
                let mut compatibility_paths = entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.is_dir()
                            && path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| name.starts_with("swift-"))
                    })
                    .map(|path| path.join("macosx"))
                    .filter(|path| path.is_dir())
                    .collect::<Vec<_>>();
                compatibility_paths.sort();
                rpaths.extend(compatibility_paths);
            }

            rpaths
        }

        /// The macOS version the Swift code targets: the crate's deployment
        /// target, but at least 11.0. The SwiftUI window itself checks for
        /// macOS 13 at runtime.
        fn deployment_target() -> String {
            println!("cargo::rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
            let parse = |version: &str| -> Option<(u32, u32)> {
                let mut parts = version.trim().split('.');
                let major = parts.next()?.parse().ok()?;
                let minor = parts.next().map_or(Some(0), |minor| minor.parse().ok())?;
                Some((major, minor))
            };
            let (major, minor) = std::env::var("MACOSX_DEPLOYMENT_TARGET")
                .ok()
                .and_then(|version| parse(&version))
                .map_or((11, 0), |version| version.max((11, 0)));
            format!("{major}.{minor}")
        }

        /// Compile the SwiftUI Settings window into a static library and link
        /// it with the Swift runtime and the frameworks it uses.
        fn build_native_settings(toolchain_lib_dir: Option<&Path>) {
            const SOURCE: &str = "native/macos/SettingsWindow.swift";
            println!("cargo::rerun-if-changed={SOURCE}");

            let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set"));
            let target = std::env::var("TARGET").expect("TARGET is set");
            let arch = if target.starts_with("aarch64") {
                "arm64"
            } else {
                "x86_64"
            };
            let library = out_dir.join("libtermy_native_settings.a");
            let status = Command::new("xcrun")
                .args(["--sdk", "macosx", "swiftc"])
                .args(["-parse-as-library", "-emit-library", "-static", "-O"])
                .args(["-swift-version", "5"])
                .args(["-module-name", "TermyNativeSettings"])
                .args([
                    "-target",
                    &format!("{arch}-apple-macos{}", deployment_target()),
                ])
                .arg("-module-cache-path")
                .arg(out_dir.join("swift-module-cache"))
                .arg("-o")
                .arg(&library)
                .arg(SOURCE)
                .status()
                .expect("failed to run swiftc for the native Settings window");
            assert!(
                status.success(),
                "swiftc failed to build the native Settings window"
            );

            println!("cargo::rustc-link-search=native={}", out_dir.display());
            println!("cargo::rustc-link-lib=static=termy_native_settings");
            if let Ok(output) = Command::new("xcrun")
                .args(["--sdk", "macosx", "--show-sdk-path"])
                .output()
                && let Ok(sdk) = String::from_utf8(output.stdout)
            {
                println!(
                    "cargo::rustc-link-search=native={}/usr/lib/swift",
                    sdk.trim()
                );
            }
            if let Some(toolchain_lib_dir) = toolchain_lib_dir {
                println!(
                    "cargo::rustc-link-search=native={}",
                    toolchain_lib_dir.join("swift/macosx").display()
                );
            }
            for framework in ["SwiftUI", "AppKit", "Combine"] {
                println!("cargo::rustc-link-lib=framework={framework}");
            }
        }

        println!("cargo::rerun-if-env-changed=DEVELOPER_DIR");
        let toolchain_lib_dir = swift_toolchain_lib_dir();
        if let Some(toolchain_lib_dir) = &toolchain_lib_dir {
            for rpath in swift_macos_rpaths(toolchain_lib_dir) {
                println!("cargo::rustc-link-arg=-Wl,-rpath,{}", rpath.display());
            }
        }
        // The host is a Mac here; only a macOS target links the window.
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
            build_native_settings(toolchain_lib_dir.as_deref());
        }

        match Command::new("xcrun")
            .args(["--sdk", "macosx", "--show-sdk-version"])
            .output()
        {
            Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
                Ok(sdk_version) => {
                    let major_version = sdk_version
                        .trim()
                        .split('.')
                        .next()
                        .and_then(|v| v.parse::<u32>().ok());

                    if let Some(major) = major_version
                        && major >= 26
                    {
                        println!("cargo::rustc-cfg=macos_sdk_26");
                    }
                }
                Err(err) => {
                    println!(
                        "cargo:warning=skipping macOS SDK cfg detection; non-UTF8 xcrun output: {err}"
                    );
                }
            },
            Ok(output) => {
                println!(
                    "cargo:warning=skipping macOS SDK cfg detection; xcrun exited with status {}",
                    output.status
                );
            }
            Err(err) => {
                println!(
                    "cargo:warning=skipping macOS SDK cfg detection; failed to execute xcrun: {err}"
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let icon_path = "../../assets/termy.ico";
        if std::path::Path::new(icon_path).exists() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon(icon_path);
            if let Err(err) = res.compile() {
                panic!("failed to compile Windows resources: {err}");
            }
        }
    }
}
