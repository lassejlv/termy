# Tmon session lifecycle

Tmon's per-user multiplexer owns every PTY. Window and application processes are replaceable
clients; closing the last window or choosing Quit detaches the client and deliberately leaves every
shell and job running for the current login session.

## User actions

| Action | Effect on PTYs | Effect on daemon/socket |
| --- | --- | --- |
| Close or quit Tmon | All survive | Current daemon keeps listening |
| Relaunch Tmon | All survive | Reattaches to the current protocol daemon |
| Close a non-final tab | That tab's complete process group stops | Daemon and other tabs survive |
| `tmon --session-status` | None | Read-only; never starts, removes, or replaces anything |
| `tmon --terminate-sessions` | Every current-protocol process group stops | Current daemon exits and removes only its own socket |
| Reboot or log out | Not promised to survive | No launch-agent restoration is provided |

The final tab cannot be closed destructively from the normal tab UI. Create another tab first, or
use the explicit all-session command from a terminal after reviewing what it does.

## Protocol upgrades and rollback

The socket filename contains the protocol version. A new incompatible Tmon release therefore
starts its own daemon and leaves the previous daemon and its PTYs untouched. `--session-status`
lists current and older generations without exposing paths or terminal data.

There is no automatic live PTY migration between incompatible protocol generations. To recover an
older live session, launch the matching older signed Tmon archive. Keep the previous archive and
checksum as part of every release record. After its tabs have exited, that daemon removes its own
socket when stopped by the matching application. If the older daemon remains live, the current app
must not terminate or clean it up on the user's behalf.

A stale socket is a versioned socket with no listener. Tmon removes a stale socket only for the
exact protocol it is about to start, and only after verifying that the parent is a private,
current-user-owned directory and the entry is a current-user-owned Unix socket. It refuses regular
files, symlinks, foreign-owned entries, and live listeners. Older stale sockets are reported but
left in place; manual deletion is acceptable only after the user confirms the matching daemon and
PTYs are gone.

## Failure recovery

- If status says **running**, relaunch normally to attach.
- If the current entry says **stale socket**, a normal launch performs the guarded replacement.
- If status says **unsafe path**, Tmon does not connect or remove it. Inspect ownership and runtime
  directory permissions before taking any manual action.
- If a daemon dies, its PTYs are terminated by ownership teardown; Tmon never claims recovery of a
  process whose owning daemon is gone.
- If an upgrade starts a new empty generation while an older generation is running, use the matching
  older archive to reach those sessions. Do not use current-generation termination as cleanup.

The runtime directory is mode `0700`; live sockets are mode `0600`; clients and servers verify peer
user credentials. Session status contains no socket path, working directory, command, terminal
text, environment, or clipboard data.
