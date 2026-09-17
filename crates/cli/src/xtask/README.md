# xtask

Repository automation binary.

## Owner

This crate owns maintainer commands that generate or verify repository
artifacts, including generated documentation and cross-engine benchmark
reports.

Keep product runtime code out of this crate. If an automation command needs
shared domain data, depend on the smallest domain crate that owns that data.

## Validation

```sh
cargo test -p termy_cli --bin xtask
TERMY_CORE_TEST_BACKEND=alacritty cargo test -p termy_cli --example engine_compare
cargo run -p termy_cli --bin xtask -- generate-keybindings-doc --check
cargo run -p termy_cli --bin xtask -- generate-config-doc --check
cargo run -p termy_cli --bin xtask -- check-dependency-policy
```

## Forbidden Dependencies

- `termy_core::ffi`
- `termy` / `crates/desktop_app`
- product runtime workflows

The benchmark example may depend on engine crates because xtask is a leaf
validation owner; those dependencies must not move into product runtime code.
