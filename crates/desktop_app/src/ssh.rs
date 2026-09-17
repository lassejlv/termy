use std::{
    io::Write,
    path::{Path, PathBuf},
};
use sysinfo::{Pid, ProcessesToUpdate, System, get_current_pid};
use termy_core::ssh_core::{
    AskpassPromptKind, AskpassRequest, HOSTS_FILE_NAME, SshHost, SshHostManager,
    SystemKeyringBackend, parse_askpass_request, resolve_askpass_secret,
};

pub(crate) fn hosts_path(config_path: Option<&Path>) -> Result<PathBuf, String> {
    let config_path = config_path
        .map(Path::to_path_buf)
        .or_else(termy_core::config_core::config_path)
        .ok_or_else(|| "Unable to resolve the Termy configuration directory".to_string())?;
    let parent = config_path
        .parent()
        .ok_or_else(|| "The Termy configuration path has no parent directory".to_string())?;
    Ok(parent.join(HOSTS_FILE_NAME))
}

pub(crate) fn manager(
    config_path: Option<&Path>,
) -> Result<SshHostManager<SystemKeyringBackend>, String> {
    SshHostManager::open(hosts_path(config_path)?, SystemKeyringBackend)
}

pub(crate) fn load_hosts(config_path: Option<&Path>) -> Result<Vec<SshHost>, String> {
    Ok(manager(config_path)?.hosts().to_vec())
}

pub(crate) fn run_askpass_if_requested(cli_args: &[String]) -> Option<i32> {
    let prompt = cli_args.first().map_or("", String::as_str);
    let request = match parse_askpass_request(|key| std::env::var(key).ok(), prompt) {
        Ok(None) => return None,
        Ok(Some(request)) => request,
        Err(error) => {
            eprintln!("{error}");
            return Some(1);
        }
    };

    if !askpass_process_lineage_is_valid(&request) {
        eprintln!("Termy rejected an SSH credential request from an unexpected process");
        return Some(1);
    }

    let response = match request.prompt_kind {
        AskpassPromptKind::Authentication => {
            match resolve_askpass_secret(&request, SystemKeyringBackend) {
                Ok(secret) => secret,
                Err(error) => {
                    eprintln!("{error}");
                    return Some(1);
                }
            }
        }
        AskpassPromptKind::HostKeyConfirmation => {
            let confirmed = crate::native_sdk::confirm("Verify SSH Host Key", prompt);
            if confirmed {
                "yes".to_string()
            } else {
                "no".to_string()
            }
        }
    };
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(response.as_bytes()).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        Some(1)
    } else {
        Some(0)
    }
}

fn askpass_process_lineage_is_valid(request: &AskpassRequest) -> bool {
    let Ok(current_pid) = get_current_pid() else {
        return false;
    };
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let Some(helper_process) = system.process(current_pid) else {
        return false;
    };
    let Some(ssh_pid) = helper_process.parent() else {
        return false;
    };
    let Some(ssh_process) = system.process(ssh_pid) else {
        return false;
    };
    let ssh_name = ssh_process.name().to_string_lossy().to_ascii_lowercase();
    let is_system_ssh = ssh_name == "ssh" || ssh_name == "ssh.exe";
    is_system_ssh && ssh_process.parent() == Some(Pid::from_u32(request.expected_parent_pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_path_sits_beside_the_non_secret_config() {
        assert_eq!(
            hosts_path(Some(Path::new("/tmp/termy/config.txt"))).unwrap(),
            PathBuf::from("/tmp/termy/ssh_hosts.json")
        );
    }
}
