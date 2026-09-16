//! Fetch GitHub release notes with `gh`, falling back to `curl`.

use serde::Deserialize;
use std::io;
use std::process::{Command, Output, Stdio};

pub const GITHUB_REPO: &str = "lassejlv/termy";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub tag: String,
    pub title: String,
    pub markdown: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    MissingTools,
    Failed(String),
}

impl FetchError {
    pub fn user_message(&self) -> String {
        match self {
            Self::MissingTools => {
                "Install GitHub CLI (`gh`) or `curl` to load release notes.".to_string()
            }
            Self::Failed(message) => message.clone(),
        }
    }
}

pub(crate) trait CommandRunner {
    fn output(&self, program: &str, args: &[&str]) -> Result<String, FetchError>;
}

pub(crate) struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn output(&self, program: &str, args: &[&str]) -> Result<String, FetchError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        match command.output() {
            Ok(output) if output.status.success() => decode_stdout(output),
            Ok(output) => Err(FetchError::Failed(command_failure_message(
                program, &output,
            ))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(FetchError::MissingTools),
            Err(error) => Err(FetchError::Failed(format!(
                "Could not run {program}: {error}"
            ))),
        }
    }
}

pub fn fetch_release_notes(version: &str) -> Result<ReleaseNotes, FetchError> {
    fetch_release_notes_from(GITHUB_REPO, version, &SystemCommandRunner)
}

pub(crate) fn fetch_release_notes_from(
    repo: &str,
    version: &str,
    runner: &impl CommandRunner,
) -> Result<ReleaseNotes, FetchError> {
    let tags = release_tag_candidates(version);
    match fetch_with_gh(repo, &tags, runner) {
        Ok(notes) => Ok(notes),
        Err(FetchError::MissingTools) => fetch_with_curl(repo, &tags, runner),
        Err(gh_error) => match fetch_with_curl(repo, &tags, runner) {
            Ok(notes) => Ok(notes),
            Err(FetchError::MissingTools) => Err(gh_error),
            Err(curl_error) => Err(curl_error),
        },
    }
}

pub(crate) fn release_tag_candidates(version: &str) -> Vec<String> {
    let trimmed = version.trim();
    let stripped = trimmed.trim_start_matches(['v', 'V']).trim().to_string();
    let with_v = format!("v{stripped}");
    let mut tags = Vec::new();
    if trimmed.starts_with('v') || trimmed.starts_with('V') {
        push_unique(&mut tags, trimmed.to_string());
        push_unique(&mut tags, stripped);
    } else {
        push_unique(&mut tags, with_v);
        push_unique(&mut tags, stripped);
    }
    tags
}

fn fetch_with_gh(
    repo: &str,
    tags: &[String],
    runner: &impl CommandRunner,
) -> Result<ReleaseNotes, FetchError> {
    let mut last_error = FetchError::Failed("GitHub CLI could not load this release.".to_string());
    for tag in tags {
        match runner.output(
            "gh",
            &[
                "release",
                "view",
                tag,
                "--repo",
                repo,
                "--json",
                "body,name,tagName",
            ],
        ) {
            Ok(stdout) => return parse_release_json(&stdout, tag),
            Err(FetchError::MissingTools) => return Err(FetchError::MissingTools),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn fetch_with_curl(
    repo: &str,
    tags: &[String],
    runner: &impl CommandRunner,
) -> Result<ReleaseNotes, FetchError> {
    let mut last_error = FetchError::Failed("curl could not load this release.".to_string());
    for tag in tags {
        let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
        match runner.output(
            "curl",
            &[
                "-fsSL",
                "-A",
                "Termy",
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
                &url,
            ],
        ) {
            Ok(stdout) => return parse_release_json(&stdout, tag),
            Err(FetchError::MissingTools) => return Err(FetchError::MissingTools),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

#[derive(Debug, Deserialize)]
struct GithubReleasePayload {
    body: Option<String>,
    name: Option<String>,
    #[serde(alias = "tagName")]
    tag_name: Option<String>,
}

fn parse_release_json(stdout: &str, fallback_tag: &str) -> Result<ReleaseNotes, FetchError> {
    let payload: GithubReleasePayload = serde_json::from_str(stdout.trim()).map_err(|_| {
        FetchError::Failed("GitHub returned a response that was not release JSON.".to_string())
    })?;
    let tag = payload
        .tag_name
        .filter(|tag| !tag.trim().is_empty())
        .unwrap_or_else(|| fallback_tag.to_string());
    let title = payload
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| tag.clone());
    Ok(ReleaseNotes {
        tag,
        title,
        markdown: payload.body.unwrap_or_default(),
    })
}

fn decode_stdout(output: Output) -> Result<String, FetchError> {
    String::from_utf8(output.stdout)
        .map_err(|_| FetchError::Failed("Release notes were not valid UTF-8.".to_string()))
}

fn command_failure_message(program: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = stderr.trim();
    let detail = if detail.is_empty() {
        stdout.trim()
    } else {
        detail
    };
    if detail.is_empty() {
        format!("{program} could not load this GitHub release.")
    } else {
        truncate_message(detail)
    }
}

fn truncate_message(message: &str) -> String {
    const LIMIT: usize = 240;
    if message.len() <= LIMIT {
        message.to_string()
    } else {
        format!("{}…", &message[..LIMIT])
    }
}

fn push_unique(tags: &mut Vec<String>, tag: String) {
    if !tag.is_empty() && !tags.iter().any(|existing| existing == &tag) {
        tags.push(tag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct ScriptedRunner {
        available: Vec<&'static str>,
        calls: RefCell<Vec<(String, Vec<String>)>>,
        responses: Vec<(String, Result<String, FetchError>)>,
    }

    impl CommandRunner for ScriptedRunner {
        fn output(&self, program: &str, args: &[&str]) -> Result<String, FetchError> {
            self.calls.borrow_mut().push((
                program.to_string(),
                args.iter().map(|arg| (*arg).to_string()).collect(),
            ));
            if !self.available.contains(&program) {
                return Err(FetchError::MissingTools);
            }
            self.responses
                .iter()
                .find(|(name, _)| name == program)
                .map(|(_, result)| result.clone())
                .unwrap_or(Err(FetchError::Failed(format!(
                    "no scripted response for {program}"
                ))))
        }
    }

    #[test]
    fn prefers_gh_when_it_is_installed() {
        let runner = ScriptedRunner {
            available: vec!["gh", "curl"],
            calls: RefCell::new(Vec::new()),
            responses: vec![(
                "gh".to_string(),
                Ok(r#"{"tagName":"v1.2.3","name":"1.2.3","body":"hello"}"#.to_string()),
            )],
        };

        let notes = fetch_release_notes_from("lassejlv/termy", "1.2.3", &runner).expect("notes");
        assert_eq!(notes.markdown, "hello");
        assert_eq!(notes.title, "1.2.3");
        let programs: Vec<_> = runner
            .calls
            .borrow()
            .iter()
            .map(|(program, _)| program.clone())
            .collect();
        assert_eq!(programs, vec!["gh".to_string()]);
    }

    #[test]
    fn falls_back_to_curl_when_gh_is_missing() {
        let runner = ScriptedRunner {
            available: vec!["curl"],
            calls: RefCell::new(Vec::new()),
            responses: vec![(
                "curl".to_string(),
                Ok(
                    r##"{"tag_name":"v1.2.3","name":"Termy 1.2.3","body":"hello from curl"}"##
                        .to_string(),
                ),
            )],
        };

        let notes = fetch_release_notes_from("lassejlv/termy", "v1.2.3", &runner).expect("notes");
        assert_eq!(notes.markdown, "hello from curl");
        assert_eq!(notes.tag, "v1.2.3");
        let programs: Vec<_> = runner
            .calls
            .borrow()
            .iter()
            .map(|(program, _)| program.clone())
            .collect();
        assert_eq!(programs, vec!["gh".to_string(), "curl".to_string()]);
    }

    #[test]
    fn release_tag_candidates_try_v_prefix_first() {
        assert_eq!(
            release_tag_candidates("1.2.3"),
            vec!["v1.2.3".to_string(), "1.2.3".to_string()]
        );
        assert_eq!(
            release_tag_candidates("v1.2.3"),
            vec!["v1.2.3".to_string(), "1.2.3".to_string()]
        );
    }
}
