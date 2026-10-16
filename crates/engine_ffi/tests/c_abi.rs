//! Builds `libtermy_engine.a`, then compiles, links and runs a C program
//! against it and `include/termy_engine.h`: the static, engine-only package
//! hosts consume.
#![cfg(unix)]

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn compiler() -> Option<String> {
    let exists = |compiler: &str| {
        Command::new(compiler)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    std::env::var("CC")
        .ok()
        .filter(|compiler| exists(compiler))
        .or_else(|| {
            ["cc", "clang", "gcc"]
                .into_iter()
                .find(|compiler| exists(compiler))
                .map(str::to_owned)
        })
}

/// The test binary holds the workspace build lock, so the static library is
/// built in its own target directory.
fn build_static_library() -> PathBuf {
    let workspace = manifest_dir().join("../..");
    let target_dir = workspace.join("target/termy-engine-c-abi");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .current_dir(&workspace)
        .args(["build", "-p", "termy_engine_ffi", "--lib", "--target-dir"])
        .arg(&target_dir)
        .status()
        .expect("run cargo build");
    assert!(status.success(), "building libtermy_engine.a failed");
    target_dir.join("debug/libtermy_engine.a")
}

fn system_libraries() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["-lc", "-lm", "-liconv"]
    } else {
        &[
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-lc",
        ]
    }
}

#[test]
fn c_program_links_the_static_library_and_runs() {
    let compiler = compiler().expect("a C compiler (CC, cc, clang or gcc)");
    let library = build_static_library();
    assert!(library.exists(), "{}", library.display());
    let temp = tempfile::tempdir().expect("tempdir");
    let program = temp.path().join("replica");
    let output = Command::new(&compiler)
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&program)
        .arg("-I")
        .arg(manifest_dir().join("include"))
        .arg(manifest_dir().join("tests/c/replica.c"))
        .arg(&library)
        .args(system_libraries())
        .output()
        .expect("run the C compiler");
    assert!(
        output.status.success(),
        "compiling the C program failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(&program).output().expect("run the C program");
    assert!(
        run.status.success(),
        "C program failed:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "ok");
}

fn exported(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "#[unsafe(no_mangle)]" {
            continue;
        }
        let signature = lines.next().unwrap_or_default();
        if let Some(rest) = signature.split("fn ").nth(1) {
            names.insert(rest.split('(').next().unwrap_or_default().to_owned());
        }
    }
    names
}

#[test]
fn header_declares_exactly_the_exported_functions() {
    let sources = [
        "src/lib.rs",
        "src/frame.rs",
        "src/graphics.rs",
        "src/input.rs",
        "src/text.rs",
    ];
    let exports: BTreeSet<String> = sources
        .iter()
        .flat_map(|path| {
            exported(&std::fs::read_to_string(manifest_dir().join(path)).expect("read source"))
        })
        .collect();
    let header = std::fs::read_to_string(Path::new(&manifest_dir()).join("include/termy_engine.h"))
        .expect("read header");
    let declarations: BTreeSet<String> = header
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let start = line.find("termy_engine_")?;
            let rest = &line[start..];
            let end = rest.find('(')?;
            let name = &rest[..end];
            // Declarations, not macros or prose.
            (!line.starts_with('*')
                && !line.starts_with("/*")
                && !line.starts_with("#")
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .then(|| name.to_owned())
        })
        .collect();
    assert!(exports.len() > 30, "{exports:?}");
    assert_eq!(declarations, exports);
}
