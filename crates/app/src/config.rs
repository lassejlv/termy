use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use render::{RendererConfig, Theme};
use toml_edit::DocumentMut;

const DEFAULT_SCROLLBACK_LIMIT: usize = 5_000;

#[derive(Clone, Debug)]
pub(crate) struct AppConfig {
    pub(crate) font_family: String,
    pub(crate) font_size: f32,
    pub(crate) padding: f32,
    pub(crate) scrollback_limit: usize,
    pub(crate) inactive_scrollback_limit: usize,
    pub(crate) theme: Theme,
    pub(crate) shell: Option<OsString>,
    pub(crate) working_directory: Option<PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let renderer = RendererConfig::default();
        Self {
            font_family: renderer.font_family,
            font_size: renderer.font_size,
            padding: renderer.padding,
            scrollback_limit: DEFAULT_SCROLLBACK_LIMIT,
            inactive_scrollback_limit: 1_000,
            theme: renderer.theme,
            shell: None,
            working_directory: None,
        }
    }
}

impl AppConfig {
    pub(crate) fn load() -> Result<Self> {
        let Some((path, required)) = config_path() else {
            return Ok(Self::default());
        };
        if !path.exists() && !required {
            return Ok(Self::default());
        }
        let source = fs::read_to_string(&path)
            .with_context(|| format!("reading Tmon config at {}", path.display()))?;
        Self::parse(&source).with_context(|| format!("parsing Tmon config at {}", path.display()))
    }

    pub(crate) fn renderer_config(&self) -> RendererConfig {
        RendererConfig {
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            padding: self.padding,
            theme: self.theme,
        }
    }

    fn parse(source: &str) -> Result<Self> {
        let document = source.parse::<DocumentMut>().context("invalid TOML")?;
        let mut config = Self::default();

        if let Some(value) = document.get("font-family") {
            let family = value
                .as_str()
                .context("font-family must be a string")?
                .trim();
            if family.is_empty() || family.len() > 128 {
                bail!("font-family must contain between 1 and 128 bytes");
            }
            family.clone_into(&mut config.font_family);
        }
        if let Some(value) = document.get("font-size") {
            config.font_size = number(value, "font-size")? as f32;
            if !(8.0..=36.0).contains(&config.font_size) {
                bail!("font-size must be between 8 and 36");
            }
        }
        if let Some(value) = document.get("padding") {
            config.padding = number(value, "padding")? as f32;
            if !(0.0..=64.0).contains(&config.padding) {
                bail!("padding must be between 0 and 64");
            }
        }
        if let Some(value) = document.get("scrollback-limit") {
            let limit = value
                .as_integer()
                .context("scrollback-limit must be an integer")?;
            config.scrollback_limit = usize::try_from(limit)
                .ok()
                .filter(|limit| *limit <= 100_000)
                .context("scrollback-limit must be between 0 and 100000")?;
        }
        if let Some(value) = document.get("inactive-scrollback-limit") {
            let limit = value
                .as_integer()
                .context("inactive-scrollback-limit must be an integer")?;
            config.inactive_scrollback_limit = usize::try_from(limit)
                .ok()
                .filter(|limit| *limit <= 100_000)
                .context("inactive-scrollback-limit must be between 0 and 100000")?;
        }
        if config.inactive_scrollback_limit > config.scrollback_limit {
            bail!("inactive-scrollback-limit must not exceed scrollback-limit");
        }
        if let Some(value) = document.get("colors") {
            let colors = value.as_table().context("colors must be a table")?;
            for (name, slot) in [
                ("foreground", &mut config.theme.foreground),
                ("background", &mut config.theme.background),
                ("cursor", &mut config.theme.cursor),
                ("selection-background", &mut config.theme.selection),
                ("search-background", &mut config.theme.search_background),
                ("search-foreground", &mut config.theme.search_foreground),
                ("search-border", &mut config.theme.search_border),
                ("search-no-match", &mut config.theme.search_no_match),
            ] {
                if let Some(value) = colors.get(name) {
                    *slot = parse_color(value, &format!("colors.{name}"))?;
                }
            }
            for (index, name) in [
                "black",
                "red",
                "green",
                "yellow",
                "blue",
                "magenta",
                "cyan",
                "white",
                "bright-black",
                "bright-red",
                "bright-green",
                "bright-yellow",
                "bright-blue",
                "bright-magenta",
                "bright-cyan",
                "bright-white",
            ]
            .into_iter()
            .enumerate()
            {
                if let Some(value) = colors.get(name) {
                    config.theme.ansi[index] = parse_color(value, &format!("colors.{name}"))?;
                }
            }
        }
        if let Some(value) = document.get("shell") {
            let shell = value.as_str().context("shell must be a string")?.trim();
            if shell.is_empty() {
                bail!("shell must not be empty");
            }
            config.shell = Some(OsString::from(shell));
        }
        if let Some(value) = document.get("working-directory") {
            let directory = value
                .as_str()
                .context("working-directory must be a string")?
                .trim();
            if directory.is_empty() {
                bail!("working-directory must not be empty");
            }
            config.working_directory = Some(expand_home(directory));
        }

        Ok(config)
    }
}

pub(crate) fn prepare_config_file() -> Result<PathBuf> {
    let Some((path, _)) = config_path() else {
        bail!("HOME is not set, so Tmon cannot locate its config file");
    };
    ensure_config_file(&path)?;
    fs::canonicalize(&path).with_context(|| format!("resolving Tmon config at {}", path.display()))
}

fn ensure_config_file(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating Tmon config directory at {}", parent.display()))?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("creating Tmon config at {}", path.display()));
        }
    }
    if !path.is_file() {
        bail!("Tmon config path is not a file: {}", path.display());
    }
    Ok(())
}

fn number(value: &toml_edit::Item, name: &str) -> Result<f64> {
    value
        .as_float()
        .or_else(|| value.as_integer().map(|value| value as f64))
        .with_context(|| format!("{name} must be a number"))
}

fn parse_color(value: &toml_edit::Item, name: &str) -> Result<[u8; 3]> {
    let color = value
        .as_str()
        .with_context(|| format!("{name} must be a #RRGGBB string"))?;
    let hexadecimal = color.strip_prefix('#').unwrap_or(color);
    if hexadecimal.len() != 6 || !hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{name} must be a #RRGGBB string");
    }
    let bytes = hexadecimal.as_bytes();
    Ok([
        hexadecimal_byte(bytes[0], bytes[1]),
        hexadecimal_byte(bytes[2], bytes[3]),
        hexadecimal_byte(bytes[4], bytes[5]),
    ])
}

fn hexadecimal_byte(high: u8, low: u8) -> u8 {
    hexadecimal_nibble(high) * 16 + hexadecimal_nibble(low)
}

const fn hexadecimal_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}

fn config_path() -> Option<(PathBuf, bool)> {
    if let Some(path) = env::var_os("TMON_CONFIG") {
        return Some((PathBuf::from(path), true));
    }
    // Keep existing installations working through the rename. The new environment variable and
    // directory always win, while the old path remains a read-only fallback.
    if let Some(path) = env::var_os("METALTERM_CONFIG") {
        return Some((PathBuf::from(path), true));
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| (select_default_config_path(&home, Path::exists), false))
}

fn select_default_config_path(home: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let application_support = home.join("Library").join("Application Support");
    let current = application_support.join("Tmon").join("config.toml");
    if exists(&current) {
        return current;
    }
    let legacy = application_support.join("MetalTerm").join("config.toml");
    if exists(&legacy) { legacy } else { current }
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return env::var_os("HOME").map_or_else(|| PathBuf::from(path), PathBuf::from);
    }
    if let Some(suffix) = path.strip_prefix("~/")
        && let Some(home) = env::var_os("HOME")
    {
        return PathBuf::from(home).join(suffix);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, time::SystemTime};

    use super::{AppConfig, ensure_config_file, select_default_config_path};

    #[test]
    fn defaults_are_small_and_daily_driver_friendly() {
        let config = AppConfig::default();
        assert_eq!(config.font_family, "Menlo");
        assert!((config.font_size - 15.0).abs() < f32::EPSILON);
        assert_eq!(config.scrollback_limit, 5_000);
        assert_eq!(config.inactive_scrollback_limit, 1_000);
    }

    #[test]
    fn config_parses_render_memory_keyboard_and_session_values() {
        let config = AppConfig::parse(
            r##"
font-family = "SF Mono"
font-size = 14.5
padding = 10
scrollback-limit = 12000
inactive-scrollback-limit = 2000
shell = "/bin/fish"
working-directory = "~/Code"

[colors]
foreground = "#d8dee9"
background = "#101216"
cursor = "#88c0d0"
selection-background = "#4c6eaf"
search-background = "#252932"
search-foreground = "#eceff4"
search-border = "#4c566a"
search-no-match = "#bf616a"
red = "#ff0000"
bright-blue = "#0088ff"
"##,
        )
        .unwrap();

        assert_eq!(config.font_family, "SF Mono");
        assert!((config.font_size - 14.5).abs() < f32::EPSILON);
        assert!((config.padding - 10.0).abs() < f32::EPSILON);
        assert_eq!(config.scrollback_limit, 12_000);
        assert_eq!(config.inactive_scrollback_limit, 2_000);
        assert_eq!(config.theme.foreground, [216, 222, 233]);
        assert_eq!(config.theme.background, [16, 18, 22]);
        assert_eq!(config.theme.cursor, [136, 192, 208]);
        assert_eq!(config.theme.selection, [76, 110, 175]);
        assert_eq!(config.theme.search_background, [37, 41, 50]);
        assert_eq!(config.theme.search_foreground, [236, 239, 244]);
        assert_eq!(config.theme.search_border, [76, 86, 106]);
        assert_eq!(config.theme.search_no_match, [191, 97, 106]);
        assert_eq!(config.theme.ansi[1], [255, 0, 0]);
        assert_eq!(config.theme.ansi[12], [0, 136, 255]);
        assert_eq!(
            config.shell.as_deref(),
            Some(std::ffi::OsStr::new("/bin/fish"))
        );
        assert!(config.working_directory.is_some());
    }

    #[test]
    fn config_rejects_values_that_can_destabilize_rendering_or_memory() {
        assert!(AppConfig::parse("font-size = 100").is_err());
        assert!(AppConfig::parse("padding = -1").is_err());
        assert!(AppConfig::parse("scrollback-limit = 100001").is_err());
        assert!(
            AppConfig::parse("scrollback-limit = 100\ninactive-scrollback-limit = 101").is_err()
        );
        assert!(AppConfig::parse("[colors]\nbackground = 'black'").is_err());
        assert!(AppConfig::parse("[colors]\nred = '#12345g'").is_err());
        assert!(AppConfig::parse("colors = '#000000'").is_err());
    }

    #[test]
    fn renamed_config_path_prefers_tmon_and_falls_back_to_existing_legacy_config() {
        let home = Path::new("/Users/example");
        let legacy =
            select_default_config_path(home, |path| path.ends_with("MetalTerm/config.toml"));
        let current = select_default_config_path(home, |_| true);
        let fresh = select_default_config_path(home, |_| false);

        assert!(legacy.ends_with("MetalTerm/config.toml"));
        assert!(current.ends_with("Tmon/config.toml"));
        assert!(fresh.ends_with("Tmon/config.toml"));
    }

    #[test]
    fn preparing_a_config_creates_it_without_truncating_existing_contents() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("tmon-config-test-{unique}"));
        let path = root.join("Tmon").join("config.toml");

        ensure_config_file(&path).expect("prepare missing config");
        assert!(path.is_file());
        fs::write(&path, "font-size = 17\n").expect("write existing config");
        ensure_config_file(&path).expect("prepare existing config");
        assert_eq!(
            fs::read_to_string(&path).expect("read existing config"),
            "font-size = 17\n"
        );

        fs::remove_dir_all(&root).expect("remove isolated config test directory");
    }
}
