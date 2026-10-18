#!/usr/bin/env python3
"""Regenerate Termy's built-in theme collection from iTerm2-Color-Schemes
(https://github.com/mbadolato/iTerm2-Color-Schemes, MIT). The script reads the
repo's `key = value` theme files, one per theme. Point it at a checkout:

    curl -fsSL -o schemes.tar.gz \\
      https://codeload.github.com/mbadolato/iTerm2-Color-Schemes/tar.gz/refs/heads/master
    tar -xzf schemes.tar.gz --include='*/ghostty/*' --include='*/LICENSE'
    python3 scripts/import-themes.py iTerm2-Color-Schemes-master

It writes crates/core/src/themes/collection/themes.tsv (one theme per line:
Termy theme id, name, background, foreground, cursor, then the 16 ANSI
colors) and copies the upstream LICENSE next to it.
"""

import re
import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = REPO_ROOT / "crates/core/src/themes/collection"
# Termy's own themes, the ids the config parser maps to the shell-decided
# theme, and Termy's earlier built-in themes (termy_core::themes LEGACY_THEMES).
RESERVED_IDS = {
    "termy",
    "termy-light",
    "termylight",
    "shell",
    "shell-decide",
    "let-shell-decide",
    "tokyo-night",
    "solarized-dark",
    "one-dark",
    "monokai",
    "palenight",
}
# Themes named after another terminal app.
SKIPPED_THEMES = {"Ghostty Default Style Dark"}
HEX = re.compile(r"^#?([0-9a-fA-F]{6})$")


def normalize_theme_id(name: str) -> str:
    """Mirror of termy_core::theme_core::normalize_theme_id (ASCII only)."""
    out = []
    last_dash = False
    for ch in name.strip():
        ch = ch.lower() if "A" <= ch <= "Z" else ch
        if ("a" <= ch <= "z") or ("0" <= ch <= "9"):
            out.append(ch)
            last_dash = False
        elif ch in "-_ " and out and not last_dash:
            out.append("-")
            last_dash = True
        elif ch == "+":
            if out and not last_dash:
                out.append("-")
            out.append("plus")
            last_dash = False
    return "".join(out).rstrip("-")


def color(value: str) -> str | None:
    match = HEX.match(value.strip())
    return match.group(1).lower() if match else None


def parse_theme(path: Path) -> list[str] | None:
    palette: dict[int, str] = {}
    values: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        key, sep, value = line.partition("=")
        if not sep:
            continue
        key = key.strip()
        value = value.strip()
        if key == "palette":
            index, sep, hex_value = value.partition("=")
            parsed = color(hex_value)
            if sep and index.strip().isdigit() and parsed:
                palette[int(index.strip())] = parsed
        elif key in ("background", "foreground", "cursor-color"):
            parsed = color(value)
            if parsed:
                values[key] = parsed
    if "background" not in values or "foreground" not in values:
        return None
    if any(index not in palette for index in range(16)):
        return None
    cursor = values.get("cursor-color", values["foreground"])
    return [
        values["background"],
        values["foreground"],
        cursor,
        *(palette[index] for index in range(16)),
    ]


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    source = Path(sys.argv[1])
    theme_dir = source / "ghostty"
    if not theme_dir.is_dir():
        print(f"error: {theme_dir} is not a directory", file=sys.stderr)
        return 1

    rows = []
    seen: dict[str, str] = {}
    for path in sorted(theme_dir.iterdir(), key=lambda p: p.name.lower()):
        if not path.is_file():
            continue
        name = path.name
        if "\t" in name or "\n" in name:
            print(f"skip {name!r}: unsupported characters", file=sys.stderr)
            continue
        if name in SKIPPED_THEMES:
            continue
        theme_id = normalize_theme_id(name)
        if not theme_id or theme_id in RESERVED_IDS:
            print(f"skip {name!r}: id {theme_id!r} is reserved", file=sys.stderr)
            continue
        if theme_id in seen:
            print(f"skip {name!r}: id {theme_id!r} already used by {seen[theme_id]!r}", file=sys.stderr)
            continue
        colors = parse_theme(path)
        if colors is None:
            print(f"skip {name!r}: incomplete colors", file=sys.stderr)
            continue
        seen[theme_id] = name
        rows.append("\t".join([theme_id, name, *colors]))

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    (OUT_DIR / "themes.tsv").write_text("\n".join(rows) + "\n", encoding="utf-8")
    license_path = source / "LICENSE"
    if license_path.is_file():
        shutil.copyfile(license_path, OUT_DIR / "LICENSE")
    print(f"wrote {len(rows)} themes to {OUT_DIR / 'themes.tsv'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
