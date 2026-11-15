# Termy Next changelog

What changed in each release. The release workflow puts a version's section
(or `Unreleased`, when there is none yet) in its GitHub release and in the
update window Sparkle shows, so write for the people updating: what they can
do now, not how it was built. Rename `Unreleased` to the version when you tag
it.

## 0.3.0

### New

- **Plugins.** Lua plugins in `~/.config/termy-next/init.lua` add actions, key bindings and modes with `termy.*`. Plugins written for Rex, such as leaf, run as they are, and their actions show up in the command palette. A Plugins tab in Settings lists them.
- **`termy` on the command line.** Open splits and tabs, run commands, read a pane's screen and send it keys from scripts and agents. Panes have it already; Install termy Command (in the Termy Next menu and the palette) adds it to every shell.
- **Welcome tour.** A short tour of sessions, panes, the palette, agents, the shell plugin, plugins and the command line, with switches to turn things on as you go. It shows once after this update, and again from Help → Welcome Tour.
- **Import themes.** Settings → Theme & Font imports the theme Ghostty, Alacritty, kitty or Rex uses, or any of their theme files. The tour's last page lets you search and pick a theme too.
- **Your own shortcuts.** Settings → Keyboard lists every command; click one and press the keys you want. Menus, the palette and the tour follow along.

### Better

- Release notes, like these, now show in the update window, including for updates that already downloaded in the background.
