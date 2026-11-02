import Darwin
import Foundation

/// Reads the foreground program and working directory of a shell from the
/// kernel, the way a pane header names "zsh ~/Dev/termy" or "nvim ~/notes".
public enum ShellProcess {
    /// Direct children of `pid` (default: this process).
    public static func children(of pid: pid_t = getpid()) -> Set<pid_t> {
        var buffer = [pid_t](repeating: 0, count: 512)
        let count = proc_listchildpids(pid, &buffer, Int32(buffer.count * MemoryLayout<pid_t>.size))
        guard count > 0 else { return [] }
        return Set(buffer.prefix(Int(count)).filter { $0 > 0 })
    }

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

    public static func isAlive(_ pid: pid_t) -> Bool {
        kill(pid, 0) == 0 || errno == EPERM
    }
}
