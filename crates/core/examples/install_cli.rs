use std::path::PathBuf;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(args.next().expect("usage: install_cli SOURCE HOME [SHELL]"));
    let home = PathBuf::from(args.next().expect("usage: install_cli SOURCE HOME [SHELL]"));
    let shell = args.next().and_then(|value| value.into_string().ok());
    assert!(
        args.next().is_none(),
        "usage: install_cli SOURCE HOME [SHELL]"
    );

    let result = termy_core::cli_install_core::install_cli_from_source_for_home(
        &source,
        &home,
        shell.as_deref(),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    println!("installed={}", result.install_path.display());
}
