import Darwin
import Foundation

/// Reads the foreground program and working directory of a shell from the
/// kernel, the way a pane header names "zsh ~/Dev/termy" or "nvim ~/notes".
public enum ShellProcess {
    /// The foreground process group leader on `shell`'s terminal, or `shell` itself.
    public static func foreground(of shell: pid_t) -> pid_t {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(shell, PROC_PIDTBSDINFO, 0, &info, size) == size else { return shell }
        let group = pid_t(info.e_tpgid)
        return group > 0 ? group : shell
    }

    public static func name(of pid: pid_t) -> String? {
        var buffer = [CChar](repeating: 0, count: Int(MAXCOMLEN) * 2 + 1)
        guard proc_name(pid, &buffer, UInt32(buffer.count)) > 0 else { return nil }
        let name = String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
        // Login shells show as "-zsh".
        return name.hasPrefix("-") ? String(name.dropFirst()) : name
    }

    public static func workingDirectory(of pid: pid_t) -> String? {
        var info = proc_vnodepathinfo()
        let size = Int32(MemoryLayout<proc_vnodepathinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &info, size) == size else { return nil }
        return withUnsafeBytes(of: info.pvi_cdir.vip_path) { raw in
            let path = String(cString: raw.bindMemory(to: CChar.self).baseAddress!)
            return path.isEmpty ? nil : path
        }
    }

    /// The executable's full path, such as `…/claude.app/Contents/MacOS/claude`.
    public static func executablePath(of pid: pid_t) -> String? {
        var buffer = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        guard proc_pidpath(pid, &buffer, UInt32(buffer.count)) > 0 else { return nil }
        return String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }

    /// The process's argv, read from the kernel (KERN_PROCARGS2).
    public static func arguments(of pid: pid_t) -> [String] {
        var mib: [Int32] = [CTL_KERN, KERN_PROCARGS2, pid]
        var size = 0
        guard sysctl(&mib, 3, nil, &size, nil, 0) == 0, size > MemoryLayout<Int32>.size else { return [] }
        var buffer = [UInt8](repeating: 0, count: size)
        guard sysctl(&mib, 3, &buffer, &size, nil, 0) == 0, size > MemoryLayout<Int32>.size else { return [] }
        // argc, the executable path, padding NULs, then argc NUL-terminated arguments.
        let argc = buffer.withUnsafeBytes { $0.load(as: Int32.self) }
        var index = MemoryLayout<Int32>.size
        while index < size, buffer[index] != 0 { index += 1 }
        while index < size, buffer[index] == 0 { index += 1 }
        var arguments: [String] = []
        while arguments.count < argc, index < size {
            let start = index
            while index < size, buffer[index] != 0 { index += 1 }
            arguments.append(String(decoding: buffer[start..<index], as: UTF8.self))
            index += 1
        }
        return arguments
    }

    private static let interpreters: Set<String> = [
        "bash", "sh", "zsh", "dash", "ruby", "python", "python3", "node", "bun", "deno", "perl",
    ]
    private static let scriptExtensions: Set<String> = ["sh", "bash", "rb", "py", "js", "mjs", "cjs", "ts", "pl"]

    /// The tool an interpreter is running, named after its script: `bash
    /// …/Homebrew/brew.sh` and `ruby … …/brew.rb` are both "brew". Nil when
    /// argv[0] isn't an interpreter or no script file is among the first arguments.
    public static func scriptName(arguments: [String]) -> String? {
        guard let first = arguments.first,
              interpreters.contains(((first as NSString).lastPathComponent as NSString).deletingPathExtension.lowercased())
        else { return nil }
        for argument in arguments.dropFirst().prefix(12) where !argument.hasPrefix("-") {
            let file = (argument as NSString).lastPathComponent
            let ext = (file as NSString).pathExtension.lowercased()
            guard scriptExtensions.contains(ext) else { continue }
            let name = (file as NSString).deletingPathExtension
            return name.isEmpty ? nil : name
        }
        return nil
    }

    /// The parent process and the user it runs as, from the kernel.
    public static func parentAndUser(of pid: pid_t) -> (parent: pid_t, uid: uid_t)? {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
        return (pid_t(info.pbi_ppid), info.pbi_uid)
    }

    /// The terminal device a process runs on, such as /dev/ttys012.
    public static func ttyPath(of pid: pid_t) -> String? {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
        let device = dev_t(bitPattern: info.e_tdev)
        guard device != -1, device != 0, let name = devname(device, S_IFCHR) else { return nil }
        return "/dev/" + String(cString: name)
    }

    /// How the terminal treats typed input: echoed, line by line, and
    /// whether ^C and friends send signals. Nil when the device can't be read.
    public static func inputMode(of pid: pid_t) -> (echo: Bool, canonical: Bool, signals: Bool)? {
        guard let path = ttyPath(of: pid) else { return nil }
        let fd = open(path, O_RDONLY | O_NOCTTY | O_NONBLOCK)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        var settings = termios()
        guard tcgetattr(fd, &settings) == 0 else { return nil }
        let flags = settings.c_lflag
        return (flags & tcflag_t(ECHO) != 0, flags & tcflag_t(ICANON) != 0, flags & tcflag_t(ISIG) != 0)
    }

    public static func isAlive(_ pid: pid_t) -> Bool {
        kill(pid, 0) == 0 || errno == EPERM
    }
}
