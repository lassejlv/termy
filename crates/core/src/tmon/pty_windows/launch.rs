fn resolve_launch_command(
    config: &SpawnConfig,
    lookup_directory: &Path,
) -> io::Result<LaunchCommand> {
    let program = resolve_program(&config.program, &config.environment, lookup_directory)?;
    if !is_batch_program(&program) {
        return Ok(LaunchCommand {
            application: program,
            command_line: command_line(&config.program, &config.args)?,
        });
    }

    let command_processor = environment_override(&config.environment, "COMSPEC")
        .or_else(|| std::env::var_os("COMSPEC"))
        .unwrap_or_else(|| OsString::from("cmd.exe"));
    let Some(command_processor_text) = command_processor.to_str() else {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "COMSPEC must be representable as Unicode for batch-file launch",
        ));
    };
    let command_processor = resolve_program(
        command_processor_text,
        &config.environment,
        lookup_directory,
    )?;
    if is_batch_program(&command_processor) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "COMSPEC must identify cmd.exe or another native executable, not a batch file",
        ));
    }
    Ok(LaunchCommand {
        command_line: batch_command_line(&command_processor, &program, &config.args)?,
        application: command_processor,
    })
}

fn resolve_program(
    program_text: &str,
    environment: &[(String, String)],
    lookup_directory: &Path,
) -> io::Result<PathBuf> {
    if program_text.is_empty() || program_text.encode_utf16().any(|unit| unit == 0) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "terminal program cannot be empty or contain NUL bytes",
        ));
    }
    let program = PathBuf::from(program_text);
    let path_extensions = environment_override(environment, "PATHEXT")
        .or_else(|| std::env::var_os("PATHEXT"))
        .unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
    let has_path = program.is_absolute()
        || program_text.contains('\\')
        || program_text.contains('/')
        || program_text.get(1..2) == Some(":");
    if has_path {
        let candidate = if program.is_absolute() {
            program
        } else {
            lookup_directory.join(program)
        };
        return executable_candidate(&candidate, &path_extensions)
            .ok_or_else(|| program_not_found(program_text));
    }

    let path = environment_override(environment, "PATH")
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let directory = if directory.as_os_str().is_empty() {
            lookup_directory.to_path_buf()
        } else if directory.is_absolute() {
            directory
        } else {
            lookup_directory.join(directory)
        };
        if let Some(candidate) = executable_candidate(&directory.join(&program), &path_extensions) {
            return Ok(candidate);
        }
    }
    Err(program_not_found(program_text))
}

fn environment_override(environment: &[(String, String)], name: &str) -> Option<OsString> {
    environment
        .iter()
        .rev()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| OsString::from(value))
}

fn normalize_working_directory(directory: Option<&Path>) -> io::Result<Option<PathBuf>> {
    let Some(directory) = directory else {
        return Ok(None);
    };
    reject_windows_namespace_path(directory)?;
    let absolute = if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        normalized_current_directory()?.join(directory)
    };
    Ok(Some(normalize_absolute_path(&absolute)?))
}

fn normalized_current_directory() -> io::Result<PathBuf> {
    normalize_absolute_path(&std::env::current_dir()?)
}

fn normalize_absolute_path(path: &Path) -> io::Result<PathBuf> {
    reject_windows_namespace_path(path)?;
    if !path.is_absolute() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "Windows terminal working directory must resolve to an absolute path: {}",
                path.display()
            ),
        ));
    }

    let mut normalized = PathBuf::new();
    let mut normal_components = 0usize;
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                if matches!(
                    prefix.kind(),
                    Prefix::Verbatim(_)
                        | Prefix::VerbatimDisk(_)
                        | Prefix::VerbatimUNC(_, _)
                        | Prefix::DeviceNS(_)
                ) {
                    return Err(verbatim_path_error(path));
                }
                normalized.push(component.as_os_str());
            }
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir if normal_components != 0 => {
                normalized.pop();
                normal_components -= 1;
            }
            Component::ParentDir => {}
            Component::Normal(part) => {
                normalized.push(part);
                normal_components += 1;
            }
        }
    }
    if !normalized.is_absolute() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "Windows terminal working directory is drive-relative or incomplete: {}",
                path.display()
            ),
        ));
    }
    reject_windows_namespace_path(&normalized)?;
    Ok(normalized)
}

fn reject_windows_namespace_path(path: &Path) -> io::Result<()> {
    let units = path.as_os_str().encode_wide().take(4).collect::<Vec<_>>();
    let separator = |unit| matches!(unit, 0x2F | 0x5C);
    if units.len() == 4
        && separator(units[0])
        && separator(units[1])
        && matches!(units[2], 0x2E | 0x3F)
        && separator(units[3])
    {
        return Err(verbatim_path_error(path));
    }
    Ok(())
}

fn verbatim_path_error(path: &Path) -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        format!(
            "Windows verbatim and device working-directory paths are unsupported: {}",
            path.display()
        ),
    )
}

fn is_batch_program(path: &Path) -> bool {
    // Normal Win32 paths ignore trailing spaces and periods. Classify the
    // effective file name so `task.cmd. ` cannot bypass shell wrapping.
    let Some(file_name) = path.file_name() else {
        return false;
    };
    let file_name = file_name.to_string_lossy();
    let file_name = file_name.trim_end_matches([' ', '.']);
    Path::new(file_name).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
    })
}

fn executable_candidate(candidate: &Path, path_extensions: &OsStr) -> Option<PathBuf> {
    if candidate.extension().is_none() {
        let with_exe = candidate.with_extension("exe");
        if with_exe.is_file() {
            return Some(with_exe);
        }
        for extension in path_extensions.to_string_lossy().split(';') {
            let extension = extension.trim().trim_start_matches('.');
            if extension.is_empty() || extension.eq_ignore_ascii_case("exe") {
                continue;
            }
            let candidate = candidate.with_extension(extension);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    candidate.is_file().then(|| candidate.to_path_buf())
}

fn program_not_found(program: &str) -> io::Error {
    io::Error::new(
        ErrorKind::NotFound,
        format!("terminal program '{program}' was not found in PATH"),
    )
}

fn command_line(program: &str, args: &[String]) -> io::Result<Vec<u16>> {
    let mut command = Vec::new();
    append_quoted_argument(&mut command, program)?;
    for argument in args {
        command.push(b' ' as u16);
        append_quoted_argument(&mut command, argument)?;
    }
    finish_command_line(command)
}

fn batch_command_line(
    command_processor: &Path,
    script: &Path,
    args: &[String],
) -> io::Result<Vec<u16>> {
    let mut command = Vec::new();
    append_quoted_os_argument(
        &mut command,
        command_processor.as_os_str(),
        "Windows command processor",
    )?;
    command.extend(" /D /V:OFF /S /C ".encode_utf16());

    // `/S /C` removes this outer quote pair, leaving every script token
    // individually quoted. This keeps spaces and cmd metacharacters such as
    // `&`, `|`, `<`, and `>` inside argument quotes. Percent and quote
    // characters are rejected below because cmd.exe expands/reparses them and
    // cannot preserve arbitrary values without changing batch semantics.
    command.push(b'"' as u16);
    append_batch_argument(&mut command, script.as_os_str(), "batch program path")?;
    for argument in args {
        command.push(b' ' as u16);
        append_batch_argument(&mut command, OsStr::new(argument), "batch program argument")?;
    }
    command.push(b'"' as u16);
    finish_command_line(command)
}

fn finish_command_line(mut command: Vec<u16>) -> io::Result<Vec<u16>> {
    if command.len() + 1 > MAX_COMMAND_LINE_UNITS {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "terminal command line exceeds the Windows 32767 UTF-16 unit limit",
        ));
    }
    command.push(0);
    Ok(command)
}

fn append_quoted_argument(output: &mut Vec<u16>, argument: &str) -> io::Result<()> {
    append_quoted_os_argument(output, OsStr::new(argument), "terminal argument")
}

fn append_quoted_os_argument(
    output: &mut Vec<u16>,
    argument: &OsStr,
    label: &str,
) -> io::Result<()> {
    let units = argument.encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("{label} cannot contain NUL bytes"),
        ));
    }
    let quote = units.is_empty() || units.iter().any(|unit| matches!(*unit, 0x09 | 0x20 | 0x22));
    if !quote {
        output.extend_from_slice(&units);
        return Ok(());
    }

    output.push(b'"' as u16);
    let mut backslashes = 0usize;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
            output.push(unit);
        } else {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            output.push(unit);
        }
        backslashes = 0;
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

fn append_batch_argument(output: &mut Vec<u16>, argument: &OsStr, label: &str) -> io::Result<()> {
    let units = argument.encode_wide().collect::<Vec<_>>();
    if let Some(unit) = units
        .iter()
        .find(|unit| matches!(**unit, 0 | 0x0A | 0x0D | 0x22 | 0x25))
    {
        let reason = match *unit {
            0 => "NUL",
            0x0A | 0x0D => "line break",
            0x22 => "double quote",
            0x25 => "percent sign",
            _ => unreachable!("batch validation only selects known characters"),
        };
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("{label} cannot contain a {reason} when launched through cmd.exe"),
        ));
    }
    output.push(b'"' as u16);
    output.extend_from_slice(&units);
    output.push(b'"' as u16);
    Ok(())
}

fn wide_nul(value: &OsStr, label: &str) -> io::Result<Vec<u16>> {
    let mut wide = value.encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("{label} cannot contain NUL bytes"),
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn child_environment(
    overrides: &[(String, String)],
    working_directory: Option<&Path>,
) -> io::Result<Vec<u16>> {
    build_environment_block(std::env::vars_os().collect(), overrides, working_directory)
}

fn build_environment_block(
    mut entries: Vec<(OsString, OsString)>,
    overrides: &[(String, String)],
    working_directory: Option<&Path>,
) -> io::Result<Vec<u16>> {
    for (name, value) in overrides {
        validate_environment_override(name, value)?;
        upsert_environment(&mut entries, OsString::from(name), OsString::from(value));
    }
    if let Some(directory) = working_directory {
        upsert_environment(
            &mut entries,
            OsString::from("PWD"),
            directory.as_os_str().to_os_string(),
        );
        if let Some((name, value)) = drive_current_directory_entry(directory) {
            upsert_environment(&mut entries, name, value);
        }
    }
    entries.sort_by(|(left, _), (right, _)| compare_environment_names(left, right));

    let mut block = Vec::new();
    for (name, value) in entries {
        let name = name.encode_wide().collect::<Vec<_>>();
        let value = value.encode_wide().collect::<Vec<_>>();
        if name.contains(&0) || value.contains(&0) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "terminal environment cannot contain NUL bytes",
            ));
        }
        block.extend_from_slice(&name);
        block.push(b'=' as u16);
        block.extend_from_slice(&value);
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Ok(block)
}

fn validate_environment_override(name: &str, value: &str) -> io::Result<()> {
    if name.is_empty() || name.contains('=') {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "terminal environment names cannot be empty or contain '='",
        ));
    }
    if name.encode_utf16().any(|unit| unit == 0) || value.encode_utf16().any(|unit| unit == 0) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "terminal environment cannot contain NUL bytes",
        ));
    }
    Ok(())
}

fn upsert_environment(entries: &mut Vec<(OsString, OsString)>, name: OsString, value: OsString) {
    entries
        .retain(|(existing, _)| compare_environment_names(existing, &name) != CmpOrdering::Equal);
    entries.push((name, value));
}

fn drive_current_directory_entry(directory: &Path) -> Option<(OsString, OsString)> {
    let Component::Prefix(prefix) = directory.components().next()? else {
        return None;
    };
    let Prefix::Disk(drive) = prefix.kind() else {
        return None;
    };
    let drive = char::from(drive.to_ascii_uppercase());
    Some((
        OsString::from(format!("={drive}:")),
        directory.as_os_str().to_os_string(),
    ))
}

fn compare_environment_names(left: &OsStr, right: &OsStr) -> CmpOrdering {
    let left = left.encode_wide().collect::<Vec<_>>();
    let right = right.encode_wide().collect::<Vec<_>>();
    let (Ok(left_length), Ok(right_length)) =
        (i32::try_from(left.len()), i32::try_from(right.len()))
    else {
        return left.cmp(&right);
    };
    // SAFETY: both pointers remain valid for their exact UTF-16 lengths during
    // this synchronous call. CompareStringOrdinal accepts unpaired surrogate
    // code units, so sorting does not require a lossy UTF-8 conversion.
    match unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left_length,
            right.as_ptr(),
            right_length,
            TRUE,
        )
    } {
        CSTR_LESS_THAN => CmpOrdering::Less,
        CSTR_EQUAL => CmpOrdering::Equal,
        CSTR_GREATER_THAN => CmpOrdering::Greater,
        // Zero means the comparison failed. Exact UTF-16 ordering still forms
        // a deterministic valid environment block without changing code units.
        _ => left.cmp(&right),
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(module_name: *const u16) -> HModule;
    fn GetProcAddress(module: HModule, procedure_name: *const u8) -> *mut c_void;
    fn CompareStringOrdinal(
        string_1: *const u16,
        count_1: i32,
        string_2: *const u16,
        count_2: i32,
        ignore_case: Bool,
    ) -> i32;
    fn CreatePipe(
        read_pipe: *mut Handle,
        write_pipe: *mut Handle,
        pipe_attributes: *mut c_void,
        size: Dword,
    ) -> Bool;
    fn CloseHandle(object: Handle) -> Bool;
    fn ReadFile(
        file: Handle,
        buffer: *mut c_void,
        bytes_to_read: Dword,
        bytes_read: *mut Dword,
        overlapped: *mut c_void,
    ) -> Bool;
    fn WriteFile(
        file: Handle,
        buffer: *const c_void,
        bytes_to_write: Dword,
        bytes_written: *mut Dword,
        overlapped: *mut c_void,
    ) -> Bool;
    fn InitializeProcThreadAttributeList(
        attribute_list: *mut c_void,
        attribute_count: Dword,
        flags: Dword,
        size: *mut usize,
    ) -> Bool;
    fn UpdateProcThreadAttribute(
        attribute_list: *mut c_void,
        flags: Dword,
        attribute: usize,
        value: *mut c_void,
        size: usize,
        previous_value: *mut c_void,
        return_size: *mut usize,
    ) -> Bool;
    fn DeleteProcThreadAttributeList(attribute_list: *mut c_void);
    fn CreateProcessW(
        application_name: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit_handles: Bool,
        creation_flags: Dword,
        environment: *mut c_void,
        current_directory: *const u16,
        startup_info: *mut StartupInfoW,
        process_information: *mut ProcessInformation,
    ) -> Bool;
    fn WaitForSingleObject(handle: Handle, milliseconds: Dword) -> Dword;
    fn TerminateProcess(process: Handle, exit_code: u32) -> Bool;
}
