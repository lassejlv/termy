# termy_core::multiplexer

## Owner

Owns the background session host, private local connection, PTY lifetime,
remote terminal client, and the shared desktop session layout. The host uses
`termy_core`; it has no display-server or tmux dependency. The desktop app
provides its own executable as the host entry point for installed builds.

The host keeps the authoritative parser, both screen buffers, scrollback,
images, and live processes. Clients cache the viewport and request history,
search, and links when needed. Dropping a client detaches it; closing a pane
is an explicit host operation. Clipboard operations use the attached UI's
existing permission policy; terminal protocol replies continue when detached.

Clipboard reads and writes (OSC 52 queries, kitty OSC 5522) never block the
pane: the engine parks the request (`TerminalReplyHost::defer_clipboard_request`)
and the host sends it to the newest subscription that subscribed as
host-capable, which a desktop view does and `SessionClient::observe` and the
CLI do not. The answer may take as long as the user's permission prompt. The
program still reads every reply in the order of its requests: protocol
replies to later output (a `CSI c` after an `OSC 52 ?`) wait behind a
request whose answer is still owed, outside synchronized updates. With no
host-capable view the request is denied at once, and a request is denied when
its view detaches or after 60 s; a denied OSC 52 query gets an empty
clipboard. A reset (RIS) drops the requests still waiting. Each request
carries its deadline, and the client drops expired requests instead of
prompting late. Clipboard requests wake a view even while its render wakeups
are suspended (a hidden tab). Commands that write to the PTY, like the kitty
paste notification, travel on the ordered input connection so they never
overtake typed keys.

`SessionClient::snapshot`, `create_detached` and the `Size` request read or
create a pane without subscribing; the CLI uses only these.
`SessionClient::line_texts` reads history as text (`RemoteCommand::LineTexts`)
the same way; `line_texts_anchored` keeps the line numbers of an earlier
reply while output scrolls, which `termy mux wait` uses to search output that
scrolled into history between two polls.

A `Write` the program cannot receive is answered with
`RemoteReply::InputRejected` (the program exited, or the input backlog is
full), and `SessionClient::write` fails with that `InputRejection`. One it
queued is answered with `RemoteReply::Written`, whose anchor (the cursor's
line counted from the first line that scrolled into history) numbers a later
`LineTexts` read from the line the input was typed on. `LineTexts` replies
also carry the current scroll count and screen height, so a reader knows
where the screen is and sees resizes. A pane closed after its program exited
answers requests with "exited and was closed" for 10 minutes (the last 256
such panes; `is_exited_pane`). Each pane
records the directory it actually started in (`PaneInfo::working_directory`),
the runtime config and shell integration it was launched with
(`Request::LaunchConfig`, which `termy mux split` reuses), and its OSC 133
command lifecycle (`PaneInfo::shell`), including what became of the command
the latest input started (`InputCommand`), for `termy mux wait
--command-done`. The worker publishes the events of earlier output before it
writes input, so no earlier event is taken for one that followed it.

Frames are deltas (`remote::frame`). Each subscription keeps the frame it sent
last (without its images) and sends the rows that differ from it, after
shifting it by the lines output scrolled into history
(`RemoteState::lines_scrolled`). A subscriber gets a full viewport first, and
after a resize, a screen switch or a palette change. The client applies the
rows to the frame it shows and reports the shift and the changed cells as
render damage (`RemoteTransport::state_with_damage`), so the renderer replays
the scroll instead of diffing the viewport. When a frame does not fit what the
client shows, or names an image it lacks, the client sends `Request::Resync`
on the subscription and ignores deltas until the host, which starts its frame
and image caches afresh, sends a full frame.

Images travel as `Update::Image`, each in its own message, before the first
frame that shows them. Both ends cache them up to 32 MiB beyond those on
screen; the host evicts the least recently shown and names them in the next
frame, so an image that scrolls back into view or a repeating animation frame
is not sent again.

The worker captures at most one frame per 16 ms, and none while every view of
the pane is paused. A hidden tab sends `Request::SetPaused` on its
subscription; the host confirms it with `Update::Paused` and then sends events
and clipboard requests only. The paused client keeps only the images its frame
shows (`Terminal::frames_paused` reports the confirmation). On resume the
worker captures a frame at once and sends only that one, and both ends start
their image caches afresh. Input does not mark the pane dirty, and a capture
identical to the published frame is not sent.

Cells are still encoded with fixint bincode. Varint encoding would roughly
halve a cell, but deltas already cut a busy pane's frames by well over an
order of magnitude, and a second codec on the subscription stream would need
its own framing rules next to the frozen handshake frames and its own golden
coverage. Revisit it if delta frames show up in profiles.

History queries do not send styled cells of the whole scrollback. Search and
copying read `LineTexts` (text, cell widths and soft-wrap flags) newest lines
first, in chunks of at most 2 MiB, all numbered as of the frame on screen even
while output scrolls; history that output drops meanwhile only shortens the
result at its start. `Lines` replies carry at most 64 Ki cells. Messages are
written in one buffer of their final size. The host encodes every reply before
writing it and answers `Response::Error` when it cannot be sent, instead of
closing the connection. Messages may be 16 MiB larger than the 128 MiB image
quota, so the largest image fits.

Link and hyperlink queries carry the clicked frame and a hash of the clicked
row's text (`ViewportAnchor`). The host finds the row where output moved it
and answers none when it scrolled away or, after other changes, no longer
shows the same text (scrolling on the alternate screen, inside a region or
without scrollback moves rows without counting). Mouse motion is sent as
`WriteMotion`, which may fill only half of the input backlog, and the client
keeps only the newest report queued behind the last one not sent.

Client and host prove knowledge of the `endpoint.json` token to each other
(HMAC challenge-response) before any request, so a process that binds the
port after a host crash neither learns the token nor receives input.

Protocol version 4 requires that challenge-response; the plaintext-token
greeting and the capability fallbacks for older hosts are gone. Clients reject
a host of another version before connecting, and `connect_or_start` never
replaces one while it holds `host.lock`. `shutdown_host` (`termy mux shutdown`)
still stops it: `escape.rs` encodes the handshake and shutdown frames by hand,
and every version must keep their variant indices and shapes (see the wire rules
on `protocol::Request`). `kill_host` (`termy mux shutdown --force`) signals the
PID in `endpoint.json` for a host that no longer answers. Both check
`host.lock` first: without a live host, `endpoint.json` is stale, so they
remove it and report that no host is running. The desktop falls back to plain
local terminals (not tmux) with a toast when the host cannot be used or does
not answer within 8 s.

The host owns the session layout (`layout_store.rs`): it accepts only layouts
that decode as `session_model::StoredMultiplexer`, bumps a revision on every
accepted write, prunes panes it closes (and panes that exit while no client is
attached) with the same tab collapse as `StoredTab::remove_pane`, and streams
new revisions to `WatchLayout` connections. Clients edit it with
`SessionClient::update_layout`, a read, edit and compare-and-set on the
revision with a bounded retry. The desktop rebases its window edits onto the
host's with `session_model::rebase_session` and applies the host's changes to
open windows.

The `wire_golden` test hashes a sample of every wire type and variant. When the
hash changes, bump `VERSION` (unless the unreleased change already did) and
update `WIRE_FINGERPRINT`. `endpoint.json` records the host's build ID; a client
of the same `VERSION` but another build logs a warning. New `endpoint.json`
fields must be `#[serde(default)]`, since `shutdown_host` reads older files.

Terminal events are either state or things that happened. The host keeps
the latest title, working directory, OSC 9;4 progress, OSC 7501 status and
OSC 133 shell phase, and sends them to a new subscriber once, as
`Update::Replay`, before any live event. A prompt (OSC 133;A) drops the kept
progress, as the engine drops working and blocked status records. Unless the
kept phase says a command is running, progress not updated for 30 minutes
(wall-clock time) is left out of the replay, and working records unchanged for
30 minutes are left out of the replay and of later live snapshots. Blocked
records never expire. Record times come from the snapshot order, which the
engine keeps least recently updated first. Bells, clipboard stores and OSC 133 steps are only
delivered live; the client passes only state from a replay to the view, so a
reattached tab does not restart command timers, run command-finished plugins
or set workspace attention. Live queues of clients that are not reading merge
state events and keep the latest 64 OSC 133 events, dropping whole prompt
cycles, so `D` and the `A` that follows it in the same write both arrive.

Hosts write `log` output and panics to `host.log` in the private directory,
rotating it to `host.log.1` past 1 MiB. A message repeated more than 5 times a
minute is counted instead of written, and a panic report is cut at 16 KiB.
`connect_or_start` appends the host's stderr to the same file, and start-up
errors quote its last lines.

The pre-authentication handshake has one 5 s deadline in total, so a slow or
trickling client loses its slot. At most 32 connections can be in the
handshake; another one closes the oldest, so a client that reconnects as soon
as it is dropped cannot lock out real clients, whose handshake takes
milliseconds.

The host is isolated from whatever process starts it. It runs in the home
directory with a reduced environment (identity, locale, temporary and XDG
directories, the login session's display, SSH agent and bus, `TERMY_*`, the
Windows system folders, and the login default PATH; see
`host_keeps_variable`). On Windows it leaves the caller's job, so closing the
Termy tab that ran `termy mux create` does not end it, unless a job it runs in
forbids breakaway. Each pane starts from the environment its client sends in
`PaneLaunch::environment` (the desktop and CLI send their own), in an
absolute working directory the client resolved as for a local tab
(`PaneLaunch::client_working_directory`), so a pane behaves like a local tab
of that client. Every pane also gets `TERMY_MUX_DIR` (the session directory)
and `TERMY_MUX_PANE` (its ID); `termy mux` prefers `TERMY_MUX_DIR` when no
`--session-dir` is given.

The session directory must be private (`discovery::Root`). On Unix it must be
the user's, mode 0700 and not a symlink, and the endpoint, lock and log files
are opened relative to a handle of the checked directory, so a user who can
write a parent directory cannot redirect the token by swapping the path. ACLs
are not checked: on macOS a custom directory under a parent whose inheritable
ACL grants another user access is not protected by the mode. On Windows the
directory is held open without delete sharing and must not be a junction; it
must be owned by the user, SYSTEM or the administrators, a DACL that grants
anyone else access is replaced with a protected owner-only one (or the
directory is refused if that is not allowed), and every file in it must be
owned by the same accounts; the token goes into a newly created file. A
watchdog in the host recreates the directory, `host.lock` and `endpoint.json`
when they are removed or replaced. It writes the token only while it holds the
lock and the directory is still the private one it checked. When another host
takes the directory over, it waits: it takes the directory back once that host
exits, and stops once its own terminals have ended.

Shutdown waits for every pane: each worker hangs up its shell's process group,
kills that group if the shell still runs 250 ms later, and waits until the
shell is reaped, so programs in the group that ignore SIGHUP do not outlive the
host. Jobs an interactive shell put in process groups of their own get only
the hang-up, as when a tab closes.

## Validation

```sh
cargo test -p termy_core
cargo clippy -p termy_core --all-targets -- -D warnings
```

The lifecycle test launches separate client processes. It checks background
progress after client exit, reattaches to the same shell PID, and verifies
both screens, split parser input, colors, input modes, graphics, links,
scrollback, search, layout storage, and explicit child termination.

Set `TERMY_MUX_TEST_HOST_BINARY` to a built Termy executable to run these tests
through the installed app's internal host entry point instead of the test
host executable. IPC tests check authentication, bounded pre-authentication
messages, clipboard bridging, and terminal queries without an attached client.

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
