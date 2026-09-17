fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("session host directory required"))?;
    let root = if first == "--multiplexer-host" {
        args.next()
            .ok_or_else(|| anyhow::anyhow!("session host directory required"))?
    } else {
        first
    };
    termy_multiplexer::serve(std::path::Path::new(&root))
}
