# Termy Next changelog

What changed in each release. The release workflow puts a version's section
(or `Unreleased`, when there is none yet) in its GitHub release and in the
update window Sparkle shows, so write for the people updating: what they can
do now, not how it was built. Rename `Unreleased` to the version when you tag
it.

## 0.6.3

### Fixed

- **Panes no longer stay phone-sized.** After you'd watched a pane on your iPhone, the Mac could keep drawing it at the phone's width, cut off in the corner of the pane. Now whichever device you're using sets the size: typing, clicking, scrolling or switching to a pane on the Mac takes it back, and using it on the phone fits it to the phone again. When the phone lets go, the pane returns to your Mac window's current size, even if you resized the window meanwhile.
- **Terminals fill their pane after a resize.** Resizing the window or switching tabs while panes were animating could leave a terminal stuck at its old size inside a bigger pane.
- **No more freezes when a terminal can't draw.** A terminal that couldn't get a frame to draw into retried immediately, over and over, which could make the whole app sluggish. It now waits and tries again.
- **Scrolled output draws in the right rows.** Output that only scrolled blank lines could leave rows drawn where they used to be.

## 0.6.2

### Fixed

- **Rounded Classic icon.** The `>_` icon showed as a square in the Dock, Finder and Settings → Appearance → App Icon. It now has the same rounded shape as other Mac apps.

## 0.6.1

### Changed

- **The `>_` icon is back.** The mascot icons are gone; Termy Next's icon is Classic again. Settings → Appearance → App Icon still offers Pearl, Graphite, Sunset, Phosphor, and Amber. If you picked a mascot icon, Termy Next shows Classic.

### Fixed

- **Smoother panes.** Splitting, closing and zooming panes no longer stutter when animations overlap, and terminals no longer stretch or blur for a frame when a pane or window resizes.
- **No blank frame on show.** New windows, tab switches and panes coming back into view draw their latest output right away.
- **Dragging panes and tabs.** Dropping a pane in its own tab animates once instead of snapping, dropping outside every window opens the new window right where you let go (and on screen), and the drop target follows the window you're actually over, even with other apps in front.
- **Reordering tabs.** The dragged tab stays inside the tab bar, its neighbors move out of its way, and loaders follow it without lagging.
- **Window chrome.** The traffic lights stay centered in the titlebar, a quick click on a tab no longer moves the window, and divider and floating-pane drags are saved.

## 0.6.0

### New

- **A new icon, and 13 more to choose from.** Termy Next's icon is now a friendly glass mascot. Pick another in Settings → Appearance → App Icon: more of the mascot (Doodle, Candy, Matcha, Clay, Neon, Pixel, Holo) or the `>_` prompt (Classic, Pearl, Graphite, Sunset, Phosphor, Amber). Your pick shows in the Dock and the app switcher while Termy Next runs; Finder keeps the Glass icon.

## 0.5.0

### New

- **Antigravity CLI (`agy`) logo and detection.** Tabs, the sidebar, pane headers, notifications, and shell completions now recognize Antigravity CLI and display its logo.

### Changed

- **No relay server.** Termy Next no longer uses or offers a relay: your iPhone always connects to this Mac directly. iPhones paired through a relay are listed under Paired Through the Relay in Settings → Mobile and pair again.
- **Tailscale only.** Your iPhone pairs and connects over Tailscale, from anywhere, and no longer on the same Wi-Fi without it. Start Tailscale on this Mac and the iPhone, on the same tailnet; Settings → Mobile says so when it isn't running.
- **Notifications only while Termy is open on the iPhone.** Notifications while it was closed came through the relay. A connected iPhone still tells you when a long command finishes.

These need the matching Termy iPhone app, which pairs again with Macs it paired through a relay.

### Better

- **Lower CPU usage.** Reduced idle and active CPU usage across terminal wakeups, tab loaders, process polling, and Metal rendering.

### Fixed

- Dragging a selection above or below a pane scrolls it, faster the farther out you go, so you can select more than fits on screen.
- Tab and pane header icons now refresh when the foreground program changes even if the pane has a custom title.

## 0.4.1

### Fixed

- You can drag the window by its titlebar again.

## 0.4.0

### New

- **Your iPhone connects directly.** On the same network, or over Tailscale when both devices are on your tailnet, the Termy iPhone app talks to this Mac itself instead of through the relay. It moves between the two as you come and go, without dropping what you were doing. Settings → Mobile → Direct Connection shows what is in use.
- **No relay needed.** Turn off Use a relay server in Settings → Mobile and Termy Next contacts no server at all: the iPhone pairs with the QR code and reaches this Mac on your network or tailnet. Notifications while Termy is closed on the iPhone, and reaching the Mac from elsewhere without Tailscale, still need a relay. iPhones pair again when you switch.
- **Handoff.** The pane you are in is offered to your iPhone, to carry on there.
- **More from the iPhone.** Jump between prompts, read a command's output, and send photos and files into a pane.

### Better

- A long command that finishes can notify your iPhone while Termy is closed there, on relays set up to send notifications.
- Paired Devices shows an iPhone as online when it is connected directly, whatever the relay sees.

These need the matching Termy iPhone app; older ones keep working through the relay.

## 0.3.0

### New

- **Plugins.** Lua plugins in `~/.config/termy-next/init.lua` add actions, key bindings and modes with `termy.*`. Plugins written for Rex, such as leaf, run as they are, and their actions show up in the command palette. A Plugins tab in Settings lists them.
- **`termy` on the command line.** Open splits and tabs, run commands, read a pane's screen and send it keys from scripts and agents. Panes have it already; Install termy Command (in the Termy Next menu and the palette) adds it to every shell.
- **Welcome tour.** A short tour of sessions, panes, the palette, agents, the shell plugin, plugins and the command line, with switches to turn things on as you go. It shows once after this update, and again from Help → Welcome Tour.
- **Import themes.** Settings → Theme & Font imports the theme Ghostty, Alacritty, kitty or Rex uses, or any of their theme files. The tour's last page lets you search and pick a theme too.
- **Your own shortcuts.** Settings → Keyboard lists every command; click one and press the keys you want. Menus, the palette and the tour follow along.

### Better

- Release notes, like these, now show in the update window, including for updates that already downloaded in the background.
