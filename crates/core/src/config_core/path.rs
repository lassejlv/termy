use std::{
    env,
    path::{Path, PathBuf},
};

pub fn config_path() -> Option<PathBuf> {
    config_path_with(|name| env::var(name).ok(), env::current_dir().ok())
}

pub(crate) fn config_path_with(
    get_var: impl Fn(&str) -> Option<String>,
    current_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Some(app_data) = get_var("APPDATA").filter(|v| !v.trim().is_empty()) {
            return Some(Path::new(&app_data).join("termy").join("config.txt"));
        }

        if let Some(user_profile) = get_var("USERPROFILE").filter(|v| !v.trim().is_empty()) {
            return Some(Path::new(&user_profile).join(".config/termy/config.txt"));
        }
    }

    if let Some(xdg_config_home) = get_var("XDG_CONFIG_HOME").filter(|v| !v.trim().is_empty()) {
        return Some(Path::new(&xdg_config_home).join("termy/config.txt"));
    }

    if let Some(home) = get_var("HOME").filter(|v| !v.trim().is_empty()) {
        return Some(Path::new(&home).join(".config/termy/config.txt"));
    }

    current_dir.map(|dir| dir.join(".config/termy/config.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_prefers_xdg_then_home_then_cwd() {
        let path = config_path_with(
            |name| match name {
                "XDG_CONFIG_HOME" => Some("/tmp/xdg".to_string()),
                "HOME" => Some("/tmp/home".to_string()),
                _ => None,
            },
            Some(PathBuf::from("/tmp/cwd")),
        )
        .expect("config path");

        assert_eq!(path, PathBuf::from("/tmp/xdg/termy/config.txt"));

        let path = config_path_with(
            |name| match name {
                "XDG_CONFIG_HOME" => None,
                "HOME" => Some("/tmp/home".to_string()),
                _ => None,
            },
            Some(PathBuf::from("/tmp/cwd")),
        )
        .expect("config path");

        assert_eq!(path, PathBuf::from("/tmp/home/.config/termy/config.txt"));

        let path =
            config_path_with(|_| None, Some(PathBuf::from("/tmp/cwd"))).expect("config path");
        assert_eq!(path, PathBuf::from("/tmp/cwd/.config/termy/config.txt"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn config_path_prefers_appdata_on_windows() {
        let path = config_path_with(
            |name| match name {
                "APPDATA" => Some(r"C:\\Users\\alice\\AppData\\Roaming".to_string()),
                "USERPROFILE" => Some(r"C:\\Users\\alice".to_string()),
                _ => None,
            },
            None,
        )
        .expect("config path");

        assert_eq!(
            path,
            PathBuf::from(r"C:\\Users\\alice\\AppData\\Roaming\\termy\\config.txt")
        );
    }
}
