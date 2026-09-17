use termy_core::config_core::{config_path, prettify_config_contents};

pub fn run() {
    let Some(path) = config_path() else {
        eprintln!("Could not determine config directory");
        return;
    };

    if !path.exists() {
        println!("Config file does not exist yet: {}", path.display());
        println!("Nothing to prettify.");
        return;
    }

    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!("Failed to read config file: {error}");
            return;
        }
    };

    let prettified = prettify_config_contents(&contents);

    match std::fs::write(&path, &prettified) {
        Ok(()) => {
            println!("Config file prettified: {}", path.display());
            println!();
            print!("{prettified}");
        }
        Err(error) => {
            eprintln!("Failed to write config file: {error}");
        }
    }
}
