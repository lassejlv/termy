//! Build step for the WinUI tab strip (`native/windows/WinUiTabStrip.cpp`).
//!
//! Fetches the pinned Microsoft.UI.Xaml 2.8 package (and the WebView2
//! metadata it references) from NuGet, generates C++/WinRT headers with the
//! Windows SDK's cppwinrt.exe, compiles the helper, and stages beside the
//! exe what it needs at run time: Microsoft.UI.Xaml.dll, its resources.pri
//! and termy.manifest (see that file for why it is applied after linking).
//!
//! Set `TERMY_WINUI_PACKAGES` to a directory holding the two .nupkg files to
//! build offline.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const SOURCE: &str = "native/windows/WinUiTabStrip.cpp";
const MANIFEST_TEMPLATE: &str = "native/windows/termy.manifest";

struct Package {
    id: &'static str,
    version: &'static str,
    sha256: &'static str,
}

const MUX: Package = Package {
    id: "microsoft.ui.xaml",
    version: "2.8.7",
    sha256: "79207b10fe243eb1a8dcdc29beceba2f472f145ed31f147f7ae3f43b0659c9f7",
};

/// Microsoft.UI.Xaml.winmd references WebView2's types.
const WEBVIEW2: Package = Package {
    id: "microsoft.web.webview2",
    version: "1.0.2849.39",
    sha256: "099ec21f62e0e29ef9d9a991d096421efb256bdfdb64984216395736043560fd",
};

pub fn build() {
    println!("cargo::rerun-if-changed={SOURCE}");
    println!("cargo::rerun-if-changed={MANIFEST_TEMPLATE}");
    println!("cargo::rerun-if-changed=build_winui.rs");
    println!("cargo::rerun-if-env-changed=TERMY_WINUI_PACKAGES");

    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x64",
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        other => panic!("WinUI tab strip: unsupported target architecture {other:?}"),
    };
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set"));

    let mux_dir = extract_package(&MUX, &out_dir);
    let webview2_dir = extract_package(&WEBVIEW2, &out_dir);
    let appx_dir = out_dir.join(format!("mux-appx-{arch}"));
    extract_zip(
        &mux_dir.join(format!(
            "tools/AppX/{arch}/Release/Microsoft.UI.Xaml.2.8.appx"
        )),
        &appx_dir,
    );

    let compiler = cc::Build::new().cpp(true).get_compiler();
    let sdk = WindowsSdk::from_compiler(&compiler);
    let headers = out_dir.join("cppwinrt");
    generate_headers(
        &sdk,
        &[
            appx_dir.join("Microsoft.UI.Xaml.winmd"),
            webview2_dir.join("lib/Microsoft.Web.WebView2.Core.winmd"),
        ],
        &headers,
    );

    cc::Build::new()
        .cpp(true)
        .file(SOURCE)
        .include(&headers)
        .flag("/std:c++20")
        .flag("/EHsc")
        .flag("/permissive-")
        .flag("/bigobj")
        .flag("/utf-8")
        .define("WIN32_LEAN_AND_MEAN", None)
        .define("NOMINMAX", None)
        .warnings(false)
        .compile("termy_winui");
    println!("cargo::rustc-link-lib=windowsapp");

    let exe_dir = exe_dir(&out_dir);
    for file in ["Microsoft.UI.Xaml.dll", "resources.pri"] {
        copy_if_changed(&appx_dir.join(file), &exe_dir.join(file));
    }
    write_manifest(
        &appx_dir.join("AppxManifest.xml"),
        &exe_dir.join("termy.manifest"),
    );
}

/// `target/[<triple>/]<profile>`, where cargo puts termy.exe.
fn exe_dir(out_dir: &Path) -> PathBuf {
    out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR is target/<profile>/build/<crate>/out")
        .to_path_buf()
}

fn extract_package(package: &Package, out_dir: &Path) -> PathBuf {
    let file_name = format!("{}.{}.nupkg", package.id, package.version);
    let nupkg = if let Some(dir) = std::env::var_os("TERMY_WINUI_PACKAGES") {
        PathBuf::from(dir).join(&file_name)
    } else {
        let cached = out_dir.join(&file_name);
        if !matches_sha256(&cached, package.sha256) {
            download(
                &format!(
                    "https://api.nuget.org/v3-flatcontainer/{id}/{version}/{file_name}",
                    id = package.id,
                    version = package.version
                ),
                &cached,
            );
        }
        cached
    };
    if !matches_sha256(&nupkg, package.sha256) {
        panic!(
            "{} does not match the pinned SHA-256 {}",
            nupkg.display(),
            package.sha256
        );
    }
    let dir = out_dir.join(format!("{}.{}", package.id, package.version));
    extract_zip(&nupkg, &dir);
    dir
}

fn matches_sha256(path: &Path, expected: &str) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let digest = Sha256::digest(&bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    hex == expected
}

fn system32_tool(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Path::new(&root).join("System32").join(name)
}

fn download(url: &str, destination: &Path) {
    let partial = destination.with_extension("partial");
    let status = Command::new(system32_tool("curl.exe"))
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(&partial)
        .arg(url)
        .status()
        .unwrap_or_else(|err| panic!("failed to run curl.exe for {url}: {err}"));
    if !status.success() {
        panic!("downloading {url} failed ({status}); set TERMY_WINUI_PACKAGES to build offline");
    }
    fs::rename(&partial, destination).expect("move downloaded package into place");
}

/// Extracts a zip (.nupkg, .appx) once; a marker records completion.
fn extract_zip(archive: &Path, destination: &Path) {
    let marker = destination.join(".extracted");
    if marker.exists() {
        return;
    }
    let _ = fs::remove_dir_all(destination);
    fs::create_dir_all(destination).expect("create extraction directory");
    // System32's bsdtar reads zip; a Git Bash `tar` earlier on PATH does not.
    let status = Command::new(system32_tool("tar.exe"))
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .status()
        .unwrap_or_else(|err| panic!("failed to run tar.exe on {}: {err}", archive.display()));
    if !status.success() {
        panic!("extracting {} failed ({status})", archive.display());
    }
    fs::write(&marker, b"").expect("write extraction marker");
}

struct WindowsSdk {
    root: PathBuf,
    version: String,
}

impl WindowsSdk {
    /// The SDK the C++ compiler uses, read from its `INCLUDE`
    /// (`<root>\Include\<version>\um`).
    fn from_compiler(compiler: &cc::Tool) -> Self {
        let include = compiler
            .env()
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("INCLUDE"))
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var_os("INCLUDE"))
            .expect("the MSVC environment has INCLUDE");
        for path in std::env::split_paths(&include) {
            if !path.ends_with("um") {
                continue;
            }
            let Some(version_dir) = path.parent() else {
                continue;
            };
            let Some(include_dir) = version_dir.parent() else {
                continue;
            };
            if !include_dir
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("Include"))
            {
                continue;
            }
            if let (Some(root), Some(version)) = (include_dir.parent(), version_dir.file_name()) {
                return Self {
                    root: root.to_path_buf(),
                    version: version.to_string_lossy().into_owned(),
                };
            }
        }
        panic!("no Windows SDK found in the compiler's INCLUDE path");
    }

    fn cppwinrt(&self) -> PathBuf {
        let host = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };
        self.root
            .join("bin")
            .join(&self.version)
            .join(host)
            .join("cppwinrt.exe")
    }
}

fn generate_headers(sdk: &WindowsSdk, winmds: &[PathBuf], out: &Path) {
    let marker = out.join(format!(".generated-{}", sdk.version));
    if marker.exists() {
        return;
    }
    let _ = fs::remove_dir_all(out);
    let mut command = Command::new(sdk.cppwinrt());
    for winmd in winmds {
        command.arg("-in").arg(winmd);
    }
    let status = command
        .args(["-ref", &sdk.version, "-out"])
        .arg(out)
        .status()
        .unwrap_or_else(|err| panic!("failed to run {}: {err}", sdk.cppwinrt().display()));
    if !status.success() {
        panic!("cppwinrt.exe failed ({status})");
    }
    fs::write(&marker, b"").expect("write cppwinrt marker");
}

fn copy_if_changed(from: &Path, to: &Path) {
    let unchanged = match (fs::metadata(from), fs::metadata(to)) {
        (Ok(source), Ok(target)) => {
            source.len() == target.len() && source.modified().ok() <= target.modified().ok()
        }
        _ => false,
    };
    if !unchanged {
        fs::copy(from, to)
            .unwrap_or_else(|err| panic!("copy {} to {}: {err}", from.display(), to.display()));
    }
}

/// termy.manifest with one `activatableClass` per class the package's
/// AppxManifest registers.
fn write_manifest(appx_manifest: &Path, destination: &Path) {
    let appx = fs::read_to_string(appx_manifest).expect("read AppxManifest.xml");
    let mut classes = String::from("<file name=\"Microsoft.UI.Xaml.dll\">\n");
    for element in appx.split("<ActivatableClass ").skip(1) {
        let attribute = |name: &str| {
            let start = element.find(&format!("{name}=\""))? + name.len() + 2;
            let end = element[start..].find('"')? + start;
            Some(&element[start..end])
        };
        let (Some(class), Some(threading)) =
            (attribute("ActivatableClassId"), attribute("ThreadingModel"))
        else {
            continue;
        };
        classes.push_str(&format!(
            "        <activatableClass name=\"{class}\" threadingModel=\"{threading}\" \
             xmlns=\"urn:schemas-microsoft-com:winrt.v1\" />\n"
        ));
    }
    classes.push_str("    </file>");
    let manifest = fs::read_to_string(MANIFEST_TEMPLATE)
        .expect("read termy.manifest")
        .replace("<!-- WINUI_ACTIVATABLE_CLASSES -->", &classes);
    if fs::read_to_string(destination).ok().as_deref() != Some(manifest.as_str()) {
        fs::write(destination, manifest).expect("write termy.manifest");
    }
}
