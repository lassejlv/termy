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

    public static func isAlive(_ pid: pid_t) -> Bool {
        kill(pid, 0) == 0 || errno == EPERM
    }
}
