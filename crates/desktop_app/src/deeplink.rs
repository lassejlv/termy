use std::path::{Path, PathBuf};

use url::Url;

const MAX_DEEPLINK_DIR_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeepLinkRoute {
    Activate,
    NewTab,
    Settings,
    OpenConfig,
    ThemeInstall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewTabDeepLink {
    pub(crate) command: Option<String>,
    pub(crate) dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeepLinkArgument {
    NewTab(NewTabDeepLink),
    Value(String),
}

impl DeepLinkRoute {
    pub(crate) fn parse(raw: &str) -> Result<(Self, Option<DeepLinkArgument>), String> {
        let url = Url::parse(raw).map_err(|error| format!("Invalid Termy deeplink: {error}"))?;

        if url.scheme() != "termy" {
            return Err(format!(
                "Unsupported deeplink scheme \"{}\"; expected termy://",
                url.scheme()
            ));
        }

        if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
            return Err("Termy deeplinks do not support user info or ports".to_string());
        }

        let mut segments = Vec::new();
        if let Some(host) = url.host_str()
            && !host.is_empty()
        {
            segments.push(host);
        }
        segments.extend(
            url.path_segments()
                .into_iter()
                .flatten()
                .filter(|segment| !segment.is_empty()),
        );

        match segments.as_slice() {
            [] => Ok((Self::Activate, None)),
            ["new"] => {
                // Do not honor ?cmd= from external URL protocol launches. Writing
                // deeplink-controlled bytes into a PTY can execute commands.
                let command = None;
                let dir = parse_query_value(&url, "dir")
                    .map(|value| validate_deeplink_text(value, "dir", MAX_DEEPLINK_DIR_LEN))
                    .transpose()?;
                let argument = if command.is_none() && dir.is_none() {
                    None
                } else {
                    Some(DeepLinkArgument::NewTab(NewTabDeepLink { command, dir }))
                };
                Ok((Self::NewTab, argument))
            }
            ["settings"] => Ok((Self::Settings, None)),
            ["open", "config"] => Ok((Self::OpenConfig, None)),
            ["store", "theme-install"] => {
                let slug = url
                    .query_pairs()
                    .find_map(|(key, value)| {
                        (key.eq_ignore_ascii_case("slug") && !value.trim().is_empty())
                            .then(|| value.into_owned())
                    })
                    .ok_or_else(|| {
                        "Theme install deeplink requires ?slug=<theme-slug>".to_string()
                    })?;
                Ok((Self::ThemeInstall, Some(DeepLinkArgument::Value(slug))))
            }
            _ => Err(format!(
                "Unsupported Termy deeplink route: {}",
                segments.join("/")
            )),
        }
    }
}

fn parse_query_value(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then(|| value.into_owned()))
        .filter(|value| !value.trim().is_empty())
}

pub(crate) fn new_tab_deeplink_for_dir(dir: &str) -> String {
    let mut url = Url::parse("termy://new").expect("static Termy new-tab URL");
    url.query_pairs_mut().append_pair("dir", dir);
    url.to_string()
}

pub(crate) fn directory_from_open_target(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with("termy://") {
        return None;
    }

    let path = if let Ok(url) = Url::parse(trimmed) {
        if url.scheme() != "file" {
            return None;
        }
        url.to_file_path().ok()?
    } else if looks_like_filesystem_path(trimmed) {
        PathBuf::from(trimmed)
    } else {
        return None;
    };

    Some(directory_path_for_open(&path))
}

fn looks_like_filesystem_path(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        || value.starts_with('.')
        || value.starts_with('~')
        || (cfg!(windows) && value.chars().nth(1) == Some(':'))
}

fn directory_path_for_open(path: &Path) -> String {
    if path.is_file() {
        path.parent().unwrap_or(path).to_string_lossy().into_owned()
    } else {
        path.to_string_lossy().into_owned()
    }
}

fn validate_deeplink_text(value: String, name: &str, max_len: usize) -> Result<String, String> {
    if value.len() > max_len {
        return Err(format!("Termy deeplink {name} value is too long"));
    }
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(format!(
            "Termy deeplink {name} value contains unsupported control characters"
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{DeepLinkArgument, DeepLinkRoute, DeepLinkRoute::*, NewTabDeepLink};

    #[test]
    fn parses_bare_scheme_as_activate_route() {
        assert_eq!(DeepLinkRoute::parse("termy://"), Ok((Activate, None)));
        assert_eq!(DeepLinkRoute::parse("termy:///"), Ok((Activate, None)));
    }

    #[test]
    fn parses_settings_route() {
        assert_eq!(
            DeepLinkRoute::parse("termy://settings"),
            Ok((Settings, None))
        );
        assert_eq!(
            DeepLinkRoute::parse("termy:///settings"),
            Ok((Settings, None))
        );
        assert_eq!(
            DeepLinkRoute::parse("termy://settings?tab=general#section"),
            Ok((Settings, None))
        );
    }

    #[test]
    fn parses_new_tab_route() {
        assert_eq!(DeepLinkRoute::parse("termy://new"), Ok((NewTab, None)));
        assert_eq!(
            DeepLinkRoute::parse("termy://new?cmd=git%20status"),
            Ok((NewTab, None))
        );
        assert_eq!(
            DeepLinkRoute::parse("termy://new?cmd=git%20status&dir=%2Ftmp%2Fdemo"),
            Ok((
                NewTab,
                Some(DeepLinkArgument::NewTab(NewTabDeepLink {
                    command: None,
                    dir: Some("/tmp/demo".to_string()),
                }))
            ))
        );
    }

    #[test]
    fn ignores_new_tab_command_values() {
        assert_eq!(
            DeepLinkRoute::parse("termy://new?cmd=git%20status%0A"),
            Ok((NewTab, None))
        );
    }

    #[test]
    fn rejects_new_tab_control_characters() {
        assert!(DeepLinkRoute::parse("termy://new?dir=%2Ftmp%2Fdemo%0D").is_err());
    }

    #[test]
    fn parses_open_config_route() {
        assert_eq!(
            DeepLinkRoute::parse("termy://open/config"),
            Ok((OpenConfig, None))
        );
        assert_eq!(
            DeepLinkRoute::parse("termy:///open/config"),
            Ok((OpenConfig, None))
        );
        assert_eq!(
            DeepLinkRoute::parse("termy://open/config?source=browser#top"),
            Ok((OpenConfig, None))
        );
    }

    #[test]
    fn parses_theme_install_route() {
        assert_eq!(
            DeepLinkRoute::parse("termy://store/theme-install?slug=catppuccin-mocha"),
            Ok((
                ThemeInstall,
                Some(DeepLinkArgument::Value("catppuccin-mocha".to_string()))
            ))
        );
    }

    #[test]
    fn rejects_theme_install_without_slug() {
        let error = DeepLinkRoute::parse("termy://store/theme-install")
            .expect_err("theme install without slug should be rejected");
        assert!(error.contains("requires ?slug"));
    }

    #[test]
    fn rejects_wrong_scheme() {
        let error =
            DeepLinkRoute::parse("https://settings").expect_err("scheme should be rejected");
        assert!(error.contains("Unsupported deeplink scheme"));
    }

    #[test]
    fn rejects_unknown_route() {
        let error =
            DeepLinkRoute::parse("termy://workspace").expect_err("route should be rejected");
        assert!(error.contains("Unsupported Termy deeplink route"));
    }

    #[test]
    fn parses_bare_scheme_with_query_and_fragment_as_activate_route() {
        assert_eq!(
            DeepLinkRoute::parse("termy://?source=browser#noop"),
            Ok((Activate, None))
        );
    }

    #[test]
    fn rejects_malformed_url() {
        let error =
            DeepLinkRoute::parse("termy://[").expect_err("malformed deeplink should be rejected");
        assert!(error.contains("Invalid Termy deeplink"));
    }

    #[test]
    fn new_tab_deeplink_encodes_the_directory() {
        assert_eq!(
            super::new_tab_deeplink_for_dir("/tmp/demo"),
            "termy://new?dir=%2Ftmp%2Fdemo"
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_from_file_url() {
        assert_eq!(
            super::directory_from_open_target("file:///tmp/demo"),
            Some("/tmp/demo".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_from_absolute_path() {
        assert_eq!(
            super::directory_from_open_target("/tmp/demo"),
            Some("/tmp/demo".to_string())
        );
    }

    #[test]
    fn directory_open_target_ignores_termy_urls() {
        assert_eq!(
            super::directory_from_open_target("termy://new?dir=%2Ftmp"),
            None
        );
    }
}
