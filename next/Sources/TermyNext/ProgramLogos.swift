import AppKit

/// Logos of coding agents and developer tools, from svgl.app, shown in tabs,
/// the sidebar and notifications when such a program runs in a pane:
/// `claude`, `codex`, `grok`, `pi`, `git`, `npm`, `cargo`, `docker`, `nvim`
/// and more. Each logo comes in a version for light and one for dark
/// backgrounds where the brand has both. Shells keep the plain prompt icon.
@MainActor
enum ProgramLogos {
    /// Program and OSC 7501 app names (after `AppIcons.normalized`, which
    /// drops "-code") mapped to a logo.
    static let aliases: [String: String] = [
        "claude": "claude",
        "codex": "codex",
        "grok": "grok", "grok-cli": "grok", "grok-build": "grok",
        "gemini": "gemini", "gemini-cli": "gemini",
        "copilot": "copilot", "github-copilot": "copilot", "gh-copilot": "copilot",
        "cursor": "cursor", "cursor-agent": "cursor",
        "opencode": "opencode",
        "qwen": "qwen",
        "kimi": "kimi", "kimi-cli": "kimi",
        "vibe": "mistral", "mistral": "mistral", "mistral-vibe": "mistral",
        "deepseek": "deepseek",
        "windsurf": "windsurf",
        "warp": "warp",
        "ollama": "ollama",
        "pi": "pi",
        "kilo": "kilocode", "kilocode": "kilocode",
        "zed": "zed",
        // Tools
        "git": "git", "lazygit": "git", "tig": "git", "gitui": "git",
        "gh": "github",
        "npm": "npm", "npx": "npm",
        "pnpm": "pnpm", "pnpx": "pnpm",
        "yarn": "yarn",
        "bun": "bun", "bunx": "bun",
        "node": "node",
        "deno": "deno",
        "python": "python", "python3": "python", "ipython": "python", "pip": "python", "pip3": "python",
        "uv": "uv", "uvx": "uv",
        "cargo": "rust", "rustc": "rust", "rustup": "rust",
        "go": "go",
        "docker": "docker", "docker-compose": "docker", "lazydocker": "docker",
        "kubectl": "kubernetes", "k9s": "kubernetes", "helm": "kubernetes",
        "terraform": "terraform",
        "ruby": "ruby", "irb": "ruby", "bundle": "ruby", "rails": "ruby",
        "java": "java", "javac": "java",
        "swift": "swift", "swiftc": "swift",
        "nvim": "neovim", "neovim": "neovim",
        "vim": "vim", "vi": "vim",
        "psql": "postgresql", "postgres": "postgresql", "pgcli": "postgresql",
        "redis-cli": "redis", "redis-server": "redis",
        "mysql": "mysql",
        "mongosh": "mongodb", "mongod": "mongodb",
        "gcloud": "gcloud",
        "vercel": "vercel",
        "wrangler": "cloudflare", "cloudflared": "cloudflare",
        "supabase": "supabase",
        "turbo": "turborepo",
        "vite": "vite",
        "vitest": "vitest",
        "next": "nextjs",
        "prisma": "prisma",
        "zig": "zig",
        "php": "php", "composer": "php",
        "brew": "homebrew",
        "kotlin": "kotlin", "kotlinc": "kotlin",
        "lua": "lua", "luajit": "lua",
        "nginx": "nginx",
        "aws": "aws",
        "fly": "fly", "flyctl": "fly",
        "sqlite3": "sqlite", "sqlite": "sqlite",
        "code": "vscode",
    ]

    private static var cache: [String: NSImage] = [:]

    /// The logo for `program`, in its version for a dark or light background.
    static func image(for program: String?, dark: Bool) -> NSImage? {
        guard let program, let name = aliases[AppIcons.normalized(program)] else { return nil }
        // "-dark" is the version made for dark backgrounds, as on svgl.app.
        let candidates = dark ? ["\(name)-dark", name, "\(name)-light"] : ["\(name)-light", name, "\(name)-dark"]
        for key in candidates {
            if let image = cache[key] { return image }
            guard let encoded = ProgramLogoData.svgs[key], let data = Data(base64Encoded: encoded),
                  let image = NSImage(data: Self.normalized(data)) else { continue }
            cache[key] = image
            return image
        }
        return nil
    }

    /// Makes an svgl.app SVG read the same in AppKit as in a browser: adds the
    /// namespace svgo drops, and copies a `fill-rule` set on the root onto
    /// shapes, since AppKit doesn't inherit it and would fill the holes of a
    /// logo like Codex's.
    static func normalized(_ data: Data) -> Data {
        guard var text = String(data: data, encoding: .utf8), let open = text.range(of: "<svg") else { return data }
        if !text.contains("xmlns=") {
            text.replaceSubrange(open, with: "<svg xmlns=\"http://www.w3.org/2000/svg\"")
        }
        // #rgba colors (GitHub's and Zed's white) aren't read; spell them #rrggbb.
        text = text.replacingOccurrences(of: #"#([0-9a-fA-F])([0-9a-fA-F])([0-9a-fA-F])[0-9a-fA-F](?=["';)\s])"#,
                                         with: "#$1$1$2$2$3$3", options: .regularExpression)
        // IDs with colons (React's, in Next.js's mask) break url(#…) references.
        text = text.replacingOccurrences(of: ##"(id="|url\(#|href="#)([^")]*?):([^")]*?):"##, with: "$1$2_$3_", options: .regularExpression)
        let rootEnd = text.range(of: ">")?.upperBound ?? text.endIndex
        let root = String(text[..<rootEnd])
        if let rule = root.range(of: #"fill-rule="(evenodd|nonzero)""#, options: .regularExpression) {
            let attribute = String(root[rule])
            var body = String(text[rootEnd...])
            for shape in ["<path", "<circle", "<rect", "<ellipse", "<polygon"] {
                body = body.replacingOccurrences(of: shape + " ", with: shape + " " + attribute + " ")
            }
            // A shape's own fill-rule, now the second one, still wins: drop the copy there.
            body = body.replacingOccurrences(of: #"fill-rule="(evenodd|nonzero)" ([^>]*?)fill-rule="(evenodd|nonzero)""#,
                                             with: "$2fill-rule=\"$3\"", options: .regularExpression)
            text = root + body
        }
        return Data(text.utf8)
    }

    /// Draws the logo fitted into `rect`, keeping its proportions.
    static func draw(_ image: NSImage, in rect: NSRect) {
        let size = image.size
        guard size.width > 0, size.height > 0 else { return }
        let scale = min(rect.width / size.width, rect.height / size.height) * 0.92
        let fitted = NSSize(width: size.width * scale, height: size.height * scale)
        image.drawUpright(in: NSRect(x: rect.midX - fitted.width / 2, y: rect.midY - fitted.height / 2,
                                     width: fitted.width, height: fitted.height))
    }
}
