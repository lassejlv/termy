# Terminal engine audit: work list

Audit date: 2026-10-11, against `main` @ `797d0f87`.

Scope:
- `crates/core/src/terminal_engine/**`
- `crates/core/src/runtime/**`
- `crates/core/src/kitty_graphics.rs`
- `crates/pty`
- desktop snapshot/render call sites

These findings were not fixed, and nothing in the repo was changed during the audit. Most items marked **measured** were reproduced with probe crates built against `termy_core`. Those probes lived in a session scratchpad and are not in the repo, so rebuild a repro if you need one. Items marked **plausible** were inferred from reading the code only.

## Progress

Work branch: `fix/engine-audit` (not committed yet). Statuses: ✅ fixed + tested · 🟡 partial · ⏳ todo.

| ID | Status | Notes |
|---|---|---|
| CPU-1 | ✅ | `Grid::repeat_char` (`grid/print.rs`): the count is clamped to one screen + one row. Ordinary bases keep `count % per_row`, so the final screen and cursor are identical. Zero-width chars are capped at `MAX_COMBINING_BYTES`. ASCII goes through `write_ascii` in 256-byte runs. Tests: `clamped_repeat_matches_writing_every_repetition`, `repeating_grapheme_extenders_is_bounded`. The lock-release half is tracked under CPU-2. |
| SEC-1 | ✅ | New config key `clipboard_read = ask \| allow \| deny` (default `ask`): schema, parser, docs, settings UI (Terminal › Clipboard). The desktop host prompts once per drain pass with the native dialog. Denied reads get an empty OSC 52 reply so apps don't hang. The core keeps only `[cpqs0-7]` (deduplicated) in `selection` before echoing it. Core `TerminalReplyHost` semantics are unchanged for embedders. Tests: `clipboard_reply_echoes_only_known_selection_targets`, `clipboard_reads_follow_the_configured_policy_and_ask_once_per_drain`. |
| SEC-2 | ✅ | `forkpty` is replaced by `rustix_openpty::openpty` (both ends close-on-exec before fork) + `libc::fork` + a manual `login_tty` (`setsid`, `TIOCSCTTY`, `dup2`). The child closes every fd above 2 except the status pipe: `close_range` on Linux, a `proc_pidinfo(PROC_PIDLISTFDS)` snapshot on macOS, and a bounded scan as fallback. New code is in `transport/unix/child.rs`. Test: `child_does_not_inherit_descriptors_without_close_on_exec`. On macOS, `openpty` still sets close-on-exec non-atomically, so a microsecond window remains for *other* spawners (std `Command`) in the process. |
| MEM-1 | ✅ | `erase_range` (`grid.rs`): an erase that reaches the row end resets `occupied` to the erase start, or keeps the old smaller value when the background matches, and adopts the erase background as `clear_background`. The fuzz invariant test (`arbitrary_bytes_and_resizes_keep_grid_invariants`) checks the suffix invariant. Test: `erase_to_end_of_line_shrinks_the_occupied_prefix`. |
| CPU-2 | ✅ | New `runtime/custom_backend/lock.rs`. Non-feeder callers announce themselves in `LockWaiters`, and the feeder yields to them (bounded at 2 ms) before relocking. Non-hydrate feeds are applied in 8 KiB slices with the lock released in between. Hydration replays stay atomic. Bench (200×60, 1 KiB feeds, UI lock every 8 ms, release build): UI lock wait went from p50 58 ms / p99 340 ms / max 402 ms to p50 8 µs / p99 18 µs / max 39 µs. Throughput went from 227 to 273 MiB/s. |
| MEM-3 | ✅ | Parser: only APCs starting with `G` may use `MAX_APC_BYTES` (others get the 64 KiB string cap), and buffer growth is 1.5× instead of next power of two. `Handler::apc` takes `&mut Vec<u8>`, and `apply_graphics` hands the buffer over via `mem::take` + `KittyGraphicsCommand::parse_after(bytes, 1, …)` with no copy. kitty: base64 payloads that must exceed the remaining quota are rejected before decoding. `GraphicsImage` stores `Arc<Vec<u8>>` (adopts the buffer, no copy). `MAX_APC_BYTES` is still 256 MiB; lowering it is a product call. Test: `only_graphics_apcs_use_the_larger_apc_limit`. Existing APC/sync tests now use `ESC _ G` payloads. |
| MEM-4 | ✅ | `decompress_zlib(data, raw_bytes)` inflates at most `s*v*channels` for f=24/32 (an overrun gets `EINVAL:pixel data length…`), and at most `MAX_IMAGE_BYTES` for PNG. Test: `zlib_inflation_stops_at_the_declared_raw_size`. `t=f`/`t=s` still read up to the file length before the dimension check. |
| MEM-2 | ✅ | New `terminal_engine/hyperlinks.rs`, a bounded intern table: one `Arc<Hyperlink>` per `(id, uri)`. URIs up to 8 KiB and ids up to 256 B; longer links print as plain text. The table holds at most 4 MiB / 65,536 links, counting key + value copies. When full, it drops links no cell references (`strong_count == 1`). A table still full after that rejects the next 64 overflow requests without rescanning, for amortized O(1). Link-run detection compares by value, so sharing doesn't change hover behavior. Tests in `hyperlinks.rs`. |
| COR-2 | ✅ | Unix + Windows: a protocol reply that can't queue (`WouldBlock` = backlog full) is dropped with `log::warn!`, and the session stays alive. Only a closed writer (`BrokenPipe`) still tears the child down. `Transport::write_protocol_reply_owned` no longer closes the writer on `WouldBlock`. Tests: `unqueueable_reader_reply_is_dropped_and_keeps_the_writer_open`, and `unqueueable_live_reader_reply_is_dropped_without_ending_the_session`, which replaces the old SIGKILL test. The Windows change is mirrored but not compiled here (the cross-check needs MinGW for `ring`). |
| COR-4 | ✅ | `sync.rs`: repeated `?2026h` extends the 150 ms timeout only up to `SYNC_MAX_HOLD` (1 s) after the first begin. Test: `repeated_begin_markers_cannot_hold_output_past_the_maximum`. |
| MEM-5 | ✅ | `MAX_COMBINING_BYTES` cut from 256 to 64, which also bounds the quadratic append and stack buffers. Test: `longest_standard_emoji_sequences_fit_the_combining_limit` covers kiss-with-skin-tones, family, Scotland tag flag, and holding hands. |
| MEM-8 | ✅ | A command carrying transmission keys (`a i I f s v t`) while a chunked upload is pending now abandons that upload and is processed as a new one. Continuations with other foreign keys still fail as before. Test: `a_new_transmission_abandons_an_unfinished_chunked_upload`. No timeout was added. |
| LOW-15 | ✅ | `t=f` rejects canonical paths under `/proc`, `/sys` and `/dev` (except `/dev/shm`) via `is_pseudo_file`. Every `t=f`/`t=t`/`t=s` failure past control validation (missing, special, oversized, offset, short data, bad image) now gets one reply, `EBADF:failed to read image file`, as in kitty; the reason is only logged. Test: `local_file_failures_share_one_reply`. |
| LOW-16 | ✅ | `temporary_path_can_be_removed` requires the marker in the canonical path's file name. `t=t` now removes the canonical file it read, through a directory fd after an `fstatat(AT_SYMLINK_NOFOLLOW)` (dev, inode) match, and only after the upload succeeded. Tests: `temporary_file_marker_must_be_in_the_resolved_file_name`, `temporary_file_transfer_through_a_symlink_removes_only_the_file_it_read`, `temporary_file_replaced_after_reading_is_not_removed`. |
| LOW-17 | ✅ | `t=s` unlinks only after the read and image decode succeed (kitty unlinks any opened object; no name marker, since kitty's icat uses `icat-*`). A successful upload of a nameable object still unlinks it; restricting local media for remote sessions remains a product decision. Linux opens with `O_NONBLOCK` and refuses non-regular objects. Tests: `shared_memory_is_unlinked_only_after_a_successful_upload`, `shared_memory_fifo_is_refused_without_blocking`. |
| CPU-5 | ✅ | `ordinary_base` now includes Hebrew letters `U+05D0–05F2`, Arabic letters and digits (`0620–064A`, `0660–066F`, `0671–06D3`, `06D5`, `06EE–06FF`; no prepend marks) and Thai consonants `0E01–0E2E`, plus PUA planes `F0000–10FFFD`. The exhaustive `ordinary_bases_always_break_from_each_other` and `scalar_properties_match_*` tests pass. Bench (release, 120 cols, ns/char, base → new): Hebrew 43.7 → 20.0, Arabic 36.1 → 13.3, Thai 41.0 → 21.1, Nerd Font PUA 42.1 → 12.4, ASCII unchanged at 2.2. Indic: `indic_base` adds consonants and independent vowels of Devanagari, Bengali, Gurmukhi, Gujarati, Oriya, Tamil, Telugu, Kannada and Malayalam (no prepend marks). Appending any virama (`ends_ordinary_run`, ccc=9 marks) clears `grapheme_ordinary` the way ZWJ does, so a following consonant is segmented (GB9c conjuncts). Bench (release, 120 cols, ns/char, before → after): Hindi 54.6 → 34.4, Bengali 55.5 → 34.3, Telugu 51.5 → 30.9. Tests: `marks_that_join_indic_bases_end_the_ordinary_run` (exhaustive over the block's bases × zero-width marks), `indic_text_matches_grapheme_segmentation` (400 random mixed-script strings vs `graphemes()`; a mutant without the virama hook fails it), plus the existing exhaustive break/width tests. |
| LOW-3 | ✅ | `write_ascii` keeps the run fast path with DECAWM off. At the margin only the last byte of the run is written, `pending_wrap` follows `autowrap`, and `grapheme_at_margin` tracks reaching the margin. Test: `ascii_runs_without_autowrap_match_scalar_output`. |
| LOW-11 | ✅ | `escape_byte`: a byte ≥ 0x80 abandons the escape and is reprocessed in ground state. Test: `escape_followed_by_non_ascii_text_prints_the_text`, including byte-by-byte input. |
| LOW-12 | ✅ ⚠ behavior change | IL/DL inside the scroll region now do a carriage return (VT510, xterm, kitty, Ghostty). The old test `inserting_and_deleting_lines_preserve_the_cursor_column` encoded the previous Alacritty-era behavior. It's renamed to `…_return_to_the_left_margin` with updated assertions; please review. |
| LOW-13 | ✅ ⚠ behavior change | OSC 7 `file://` paths are percent-decoded via a shared `terminal_engine::percent_decode`, used by both the engine dispatch and `osc_intercept`. Malformed escapes or non-UTF-8 drop the event (engine) or fall back to the raw URL (intercept). The old intercept test `parse_osc_7_preserves_percent_escaped_paths` was renamed and now expects `/tmp/a b`. Hostname is still ignored. Test: `osc7_file_urls_are_percent_decoded`. |
| LOW-14 | ✅ | The clipboard permission dialog quotes the program-supplied name as "A terminal application calling itself “…”", strips control characters, and truncates to 64 chars (MIME list to 256). |
| LOW-1 | ✅ | The Unix writer polls the PTY master *and* a nonblocking wake socket. `WriterHandle::wake()` (resize, close, replies) always writes a byte to it, even when a channel Wake is already pending. The poll timeout is now a 1 s backstop instead of 10 ms, so a blocked writer no longer wakes ~100×/s. Poll/resize helpers moved to `transport/unix/master.rs` for the line budget. Existing resize-under-backpressure tests pass. |
| COR-1 | ✅ ⚠ behavior change | Unix + Windows writers only start a protocol reply when no normal write is partially written (`offset == 0`), so replies never split a key sequence or bracketed paste. The test was renamed to `protocol_reply_waits_for_a_partial_write_to_finish` and expects `normal-onereplynormal-two`. Windows is not compiled here. |
| COR-3 | ✅ ⚠ behavior change | A discarded oversized OSC/DCS/APC now ends at `ESC c` (RIS, what `reset` sends) or `ESC [` (any CSI), and the escape is reprocessed. Legitimate oversized payloads (base64 graphics/clipboard, sixel) never contain `ESC [`, while text after a stray `ESC P` usually does, so `cat` of such a file recovers at its first color or cursor sequence. Normal-size strings were already interrupted by any ESC. Other embedded escapes (`ESC ]`, `ESC P`, `ESC ESC`, `ESC` + C0) stay isolated until ST/CAN/SUB (or BEL for OSC). The sync-update scanner shares the parser, so an end marker after an oversized string now commits. Tests: `csi_recovers_from_an_oversized_string`, `full_reset_recovers_from_an_oversized_string`. ⚠ Updated pinned tests: `oversized_strings_discard_embedded_escapes_until_termination` (input uses non-CSI escapes), `scanner_tracks_large_string_limits_without_allocating_payloads` (all introducers now print `END`), and `oversized_string_embedded_marker_is_discarded_until_st`, renamed `oversized_string_isolates_escapes_until_a_csi_end_marker`. No time bound on discard mode was added. |
| CPU-6 | ✅ | Core: the "is this placement the image's prototype" lookup is hoisted out of the per-placeholder-cell loops (`kitty_graphics.rs` snapshot and `placement.rs` origin resolution), going from O(P²·C) to O(P·(P+C)). Desktop: `terminal_generation` is removed from `KittyGraphicsRenderCacheKey`, so plain text output no longer re-snapshots graphics every frame; the engine bumps the graphics revision for scroll, view and placeholder changes. ⚠ Worth a quick visual check with kitty images and Unicode placeholders while output scrolls. |
| LOW-5 | ✅ | `Row::clear` drops packed contents via `Row::discard_packed` (reusing a decoded cache's allocation) instead of decoding them only to blank them. |
| LOW-6 | ✅ | `PackedCells::encode` shrinks recycled scalar buffers above 2× the need and style/extra runs above 2·len + 4, so a single run no longer keeps capacity 4+ from a wider row. |
| LOW-18 | ✅ | `transport::belongs_to_parent_terminal` is a shared denylist (TMUX/TMUX_PANE/STY/WINDOW, ZELLIJ*, KITTY_*, ITERM_*, TERM_SESSION_ID, WT_*, ALACRITTY_*, WEZTERM_*, GHOSTTY_*, VTE_VERSION, KONSOLE_*, TERMINAL_EMULATOR, COLUMNS/LINES, `VSCODE_*`). It filters inherited variables on Unix and Windows, and explicit overrides still win. Test: `child_environment_drops_the_parent_terminals_identity`. |
| LOW-20 | ✅ | Unix transport constants (fcntl, signals, poll bits, FIONREAD, TIOCSWINSZ, IUTF8) now come from `libc`. `close_range` is limited to `target_os = "linux"` (Android uses the scan). ⚠ Only macOS was compiled locally; Linux/Android/Windows need CI. |
| LOW-7 | ✅ | `mark_scroll_damage` returns early once the last pending scroll for the same region and direction already covers the whole region, so the dirty-row rotate no longer runs per line during a flood. |
| LOW-8 | ✅ | Leaving the 1049 alternate screen frees it (entry clears it anyway); 47/1047 keep their contents. `clear_scrollback`/`reset` shrink the history deque, and `trim_history` shrinks it when capacity exceeds 2× the limit. Reflow (`ReflowWindow`, `reflow_primary`, `wrap_logical`) moved to `grid/reflow.rs` to keep `grid.rs` under the line budget. Tests: `leaving_the_1049_alternate_screen_releases_it`, `clearing_and_trimming_history_release_its_capacity`. |
| LOW-10 | ✅ | The parser keeps the first three bytes of each string (also in scanner mode) to choose its limit: `ESC _ G` gets the APC limit, `OSC 52;` gets `MAX_CLIPBOARD_OSC_BYTES` (8 MiB of base64, about 6 MiB of text), and everything else stays at 64 KiB. Large buffers are still released after the string ends. Replaces the `apc_graphics` flag. Test: `clipboard_osc_has_its_own_larger_limit`. |
| CPU-4 | ✅ | `Row::reflow_in_place`: a hard-terminated row that fits the new width (not the cursor row, not continuing a wrapped line, no leading spacer, no width-recomputed combining cell, default clear background) keeps its form, and packed rows stay packed with only `cols` updated. Rewrapped lines reuse one logical buffer (`drain`), and consumed or evicted source rows are recycled as output rows (pool of 8; oversized buffers dropped). Rows that land on the live screen are made dense. Measured (8738 history rows, 120↔119 cols, best of 20): short lines 7.1 ms → 0.16 ms; all-wrapped lines 10.7 → 9.2 ms (they still decode). Reflow code and tests moved to `grid/reflow.rs` / `grid/reflow/tests.rs`. ⚠ `width_resize_defers_history_compaction_to_idle_steps` now uses wrapped lines, since fitting rows no longer unpack. Test: `width_resize_keeps_fitting_history_rows_packed`. |
| MEM-7 | 🟡 partial | Engine: `StoredImage.anonymous` marks images sent with neither `i` nor `I`. `release_unplaced_anonymous_images` frees them once no placement references them; it runs at the end of `remove_orphaned_relative_placements` (which every clear/scroll/delete/overflow path already calls) and in `reset` (RIS). An `anonymous_images` counter makes it free when there are none. New `forget_image` keeps `stored_bytes`/`insertion_order` in sync. Images with an id or number keep kitty's semantics (they stay until deleted or evicted). Desktop: `KittyImageCache::get` drops older generations of the same image id on insert, instead of letting retransmissions pile up to the 256-entry / 128 MiB LRU limit (not unit-tested; needs a GPUI app). Not done: the CPU-side BGRA copy (kept on purpose: GPUI's `RenderImage` owns it for atlas re-upload) and clearing on tab switch (kept on purpose, since it frees memory for hidden tabs). Test: `id_less_images_are_freed_with_their_last_placement`. |
| LOW-9 | ✅ | ICH/DCH (and so IRM printing) no longer run `repair_wide` over the whole row. `repair_wide_at` checks only the disturbed columns: the last column after an insert, and the cursor column plus the cell before the new blanks after a delete. `edited_occupied` keeps the occupied bound tight instead of resetting it to the full width. The whole-row `repair_wide` stays only for alternate-screen resize. The invariant fuzz test now runs 4 seeds with insert mode toggled; mutation checks confirmed each repair is needed. Not benchmarked: the row rotate is still O(cols) per inserted character. Test: `editing_a_wrapped_row_drops_its_leading_wide_spacer_and_edge_glyph`. |
| CPU-3 | ✅ | Image decode runs outside the engine lock. `finish_upload` is split: `KittyUpload::prepare` (new `kitty_graphics/upload.rs`: base64 for single-chunk uploads, `t=f`/`t=t`/`t=s` reads, zlib, PNG/normalize; no state) and `KittyGraphicsState::commit_upload` (ids, quota, placement, reply). `Engine::feed_deferring_uploads` sets a defer flag around `parser.advance`, so a completed upload is stashed and `pause_requested` stops the parser right after its ST. `Shared::feed` (`runtime/custom_backend/lock.rs`) takes the upload, drops the lock, runs `prepare`, re-locks, commits, then feeds the rest, so replies and screen effects keep byte order. `Engine::feed` (embedders), hydration and `stop_synchronized_update` replay stay inline and atomic, and an untaken upload is committed before more output. Probe (release, 2048×2048 noisy PNG, 12 MiB escape, UI thread locks every 1 ms): worst UI lock wait 36 ms → 0.01 ms, feed time unchanged at 41 ms. The decode helpers moved out of `kitty_graphics.rs` (1577 → 1437 lines), so it's off the file-size allowlist. Tests: `deferred_uploads_pause_the_parser_and_commit_in_stream_order` (single and chunked), `deferred_upload_failures_reply_at_commit`, `synchronized_updates_apply_uploads_atomically`, `an_untaken_deferred_upload_commits_before_more_output`, `image_uploads_decoded_outside_the_lock_keep_stream_order`. Base64 of intermediate chunks (`m=1`) is still decoded under the lock, which is fine because chunks are small. |
| LOW-2 | ✅ | Mouse motion is coalesced at the source: Move/Drag reports go through the new `Terminal::write_mouse_motion` → `Transport::write_motion_owned` (`WriteClass::Motion` on Unix, `reserve_motion` on Windows), which may fill only half the writer backlog (bytes and entries) and is dropped silently when that half is full, so keys, pastes and clicks always have room. Other input that can't be queued sets a shared flag and emits one new `TerminalEvent::InputDropped` per drain (FFI kind 14, `TERMY_FFI_EVENT_INPUT_DROPPED`); the desktop shows a "Input was dropped: the program isn't reading it" warning toast at most every 5 s (not unit-tested; needs a GPUI app). Tests: `mouse_motion_fills_only_half_the_backlog_and_leaves_room_for_keys`, `unqueueable_input_reports_one_dropped_input_event`. |
| LOW-4 | ✅ | History is budgeted by actual heap bytes: `HISTORY_BYTE_BUDGET = Size::MAX_CELLS × size_of::<Cell>()` (about 24 MiB, the same memory the old cell cap allowed for dense rows), measured by `Row::heap_bytes`. New `grid/history.rs` keeps `Grid.history_bytes` in sync on every push/pop/clear/compaction; `enforce_history_bytes` packs pending rows before evicting the oldest. The width-based cap is gone, so `bounded_history` is just `MAX_HISTORY_ROWS`. Reflow tracks bytes in `ReflowWindow` and packs the oldest rewrapped rows (above the last screen) before evicting, so a wide or transient resize keeps the requested scrollback. Tests: `widening_packs_rewrapped_history_instead_of_evicting_it`, `a_transient_widen_keeps_the_requested_scrollback`; the grid fuzz invariant checks `history_bytes` against a recount and the budget. |
| CPU-7 | ✅ | New `Terminal::with_render_damage(|damage, cells| …)` takes render damage and reads the cells it names under one backend lock. `TerminalDamageCells` offers `visit(spans, …)`, `line_bounds()` and `display_offset()` (local engine and tmux remote backends). Desktop `update_pane_render_cache` now plans the strategy, replays scrolls and patches spans inside that call. A planned or fallback full rebuild runs after the lock is released (it re-takes damage via `render_read(true)`). A display-offset change between `scroll_state` and the read now forces a full rebuild instead of silently skipping cells. Removed the desktop's `take_render_damage_snapshot`/`for_each_damage_cell` wrappers, `from_damage`, and `finalized_cache_update_strategy` with its 2 tests (desktop count 900 → 898). The generation-checked `visit_viewport_ranges_*` APIs stay for embedders. Core viewport reads moved to `runtime/custom_backend/viewport.rs` (custom_backend.rs 1478 → 1353 lines). Tests: `render_damage_reads_stay_coherent_while_output_streams` replays damage against a concurrent feeder; a mutant that releases the lock between take and visit fails it 3/3. `core_incremental_scroll_damage_matches_fresh_snapshots` uses the new call. Not measured in the GPUI app: by construction the stale-generation fallback can no longer happen on this path. ⚠ Worth a quick visual check of scrolling output, selection and search highlights. |
| MEM-6 | ✅ ⚠ API change | `Cell` is 32 → 24 B, guarded by `const _: () = assert!(size_of::<Style>() == 10 && size_of::<Cell>() == 24)`. `underline_color` moved from `Style` into `CellExtra`; it's read via `Cell::underline_color()` and set on the pen by `set_underline_color` (SGR 58/59, cleared by SGR 0, kept across OSC 8 link changes). `Style.attributes` u16 → u8 (exactly the 8 attributes in use), with its constants retyped. `Cell` is `repr(C)` (extra, style, flags, character), so every field is naturally aligned with no padding and the print loops store a cell in four aligned writes; `Cell.flags` stays `u16` for that reason. `Color` stores `[u8; 4]` (same API, now `Hash`), so `Style` is 10 bytes at 1-byte alignment without `repr(packed)`. Wide-character spacers come from `Cell::wide_spacer()` and keep the underline color, so decorations still span both columns. The combining cache keys on underline color too. Packed history scalars are 8 → 4 B (`Scalar(u32)`: char + flags in the top byte); the packing budget (`cols × size_of::<Cell>() / 2`) shrinks by the same 4 B per column, so packing frequency is unchanged. That's 25% less dense screen/history memory and half the per-cell scalar cost of packed rows. Cells with SGR 58 now share the pen's `Arc<CellExtra>` (one atomic increment per printed cell, like hyperlinks). Fixed the `constants.rs` scrollback comment. ⚠ Public `terminal_engine` API: `Style.underline_color` removed, `Style.attributes` and its constants are now `u8`, and `CellExtra` has a new field (struct literals need `..CellExtra::default()`). `crates/wasm` was updated. Tests: `underline_color_follows_text_into_spacers_marks_links_and_history` (spacer, combining, OSC 8, SGR 0, packed history, DECRQSS); the malformed-SGR test also compares underline colors; `cell_layout_and_screen_dimensions_stay_bounded` asserts exactly 24 B. Follow-up: packed style runs are `(u32, Style)` (24 → 16 B) and a row's first run reserves exactly one slot instead of four (test `plain_packed_rows_store_one_tight_style_run`). |
| LOW-22 | ✅ | Windows close order now matches Unix: `ClosePseudoConsole` first (conhost sends CTRL_CLOSE_EVENT so PowerShell can save history), then `wait_for_exit(CLOSE_GRACE_MS = 250)`, and `TerminateProcess` only if the shell is still alive. Not compiled on Windows locally (type-checked with `cargo clippy -p termy_core --all-targets --target x86_64-pc-windows-gnu`); CI must cover it. |
| LOW-24 | ✅ ⚠ API change | Node addon: reads append to a shared buffer and at most one queued flush marker is pending, so output is coalesced while JS is busy (a 2 MB burst arrives in under 500 callbacks instead of about 2000). The reader blocks once 1 MiB is buffered, which is the backpressure (`pause()` no longer blocks the transport callback), the exit always follows the buffered output, and nothing (not even a flush queued before `pause()`) is delivered while paused. `NativePty.write` returns `false` on a full backlog instead of throwing; `terminal.ts` splits writes into 1 MiB chunks and retries queued chunks every 10 ms, like node-pty. ⚠ The native `write` signature changed (`boolean`). README and `website/.../web/pty.mdx` updated. Tests: three new vitest cases (coalescing with a busy consumer, a 10 MiB raw-mode write, exit after output while paused). |
| LOW-21 | 📝 documented | Kept as is: `readable_byte_count` now documents that FIONREAD is only a hint (0 on macOS, line-discipline buffer on Linux) and why the exit drain is still correct. |
| LOW-23 | ⏸ deferred | Folding the writer into the reader's poll loop and a shared reaper is a large transport refactor; better as its own PR. |
| CI perf | ✅ | The paired facade gate (≥ 0.95× main) failed on the first push. Causes and fixes: (1) rustc laid out the 24 B cell with `Style` at offset 12, so every style copy was an unaligned 8-byte store; `Cell` is now `repr(C)` with no padding. (2) `write_ascii` used `clone_from(&pen)`, which reloaded the pen per cell; it now builds plain cells directly. (3) `scalar_properties`/`ordinary_base` stopped inlining once the Indic ranges were added; the Indic table is out of line and both are inlined again. (4) `put_cell` built a blank cell on every call and the wide spacer through a stack temporary; both are now built only where used. (5) `Engine::feed` read the clock on every call, about half the cost of a one-byte PTY read, though only synchronized updates use it; the time is now read lazily. (6) The deferred upload is boxed, so the per-feed checks don't move a large empty `Option`. Local result on the CI script (32 MiB × 8 pairs): every case ≥ 0.97×, fragmented 1.23×. The REP bound test now feeds 200 repeats (it timed out on loaded debug runners). |
| LOW-19 | ✅ | The child clears its blocked-signal mask (`sigprocmask`) in `reset_child_process_state`. |

### Throughput check (release, 200×50, 200k lines, best of 5, `main` → branch)

| Workload | main | branch |
|---|---|---|
| plain short lines | 16.7 ms | 17.9 ms (noise to +7%; lock slicing) |
| short lines + `\e[K` | 68.6 ms | 46.4 ms |
| colored grep-style lines + `\e[K` | 87.3 ms | 66.0 ms |

## How to work this list

- Take one item (or a tight group) per branch/PR. Each item has an ID; reference it in commits/PRs.
- Line numbers are as of `797d0f87` and may have drifted. Locate code by the function or symbol named in the item.
- Add a regression test for every fix, in the existing `*_tests.rs` / `tests.rs` next to the code.
- Some existing tests encode current behavior on purpose, e.g. `oversized_strings_discard_embedded_escapes_until_termination` and `protocol_reply_preempts_at_a_bounded_partial_write_boundary`. Update them deliberately and say so in the PR.
- Commands, from `CONTRIBUTING.md`:
  - `cargo test -p termy_core`
  - `just check`
  - `just check-boundaries`
  - `just check-file-sizes` (files have a line budget; split instead of growing big files)
  - `just validate` before large PRs
- Run `cargo fmt` and clippy (workspace lints are strict). Regenerate config docs (`just generate-config-doc`) if you add config keys.

## Priority order

| ID | Sev | Area | One-line summary |
|---|---|---|---|
| SEC-1 | High | runtime | OSC 52 clipboard **read** answered without consent |
| SEC-2 | High | transport/unix | fds leak into child shells; concurrent spawns share PTY fds across tabs |
| CPU-1 | High | dispatch | `CSI n b` (REP) amplification freezes the UI under the engine lock |
| CPU-2 | Med-High | transport + runtime | Reader thread starves the UI of the engine mutex (unfair lock, 1 KiB reads on macOS) |
| MEM-1 | High | grid | `CSI K` marks the full row as occupied: 4× history memory, 3× CPU |
| MEM-2 | Med | dispatch | OSC 8 hyperlinks bypass the scrollback memory bound |
| MEM-3 | Med-High | parser + graphics | 256 MiB APC cap for any APC; graphics payload copied about 3.7× |
| MEM-4 | Med | kitty_graphics | zlib inflate capped at 128 MiB, not at the declared image size |
| CPU-3 | Med | runtime + graphics | Image decode/zlib/PNG/file reads run while holding the engine lock |
| COR-1 | Med | transport | Protocol replies spliced into the middle of pastes and key sequences |
| COR-2 | Med-Low | transport | Reply-queue overflow SIGKILLs the shell's process group |
| MEM-5 | Med-High | grid | Combining-mark floods: 256 B/cell, quadratic append |
| CPU-4 | Med | grid | Resize/reflow decodes all of history; about 62k allocations per 1-column resize |
| CPU-5 | Med | grid/print | RTL/Indic/Thai/Nerd Font chars miss the print fast path (15–20× ASCII cost) |
| MEM-6 | Med | types | Cell is 32 B; can be 24 B |
| CPU-6 | Med | kitty_graphics + desktop | Unicode-placeholder snapshot is O(placements × cells) and runs every frame |
| CPU-7 | Med | runtime + desktop | Damage/cell-read race forces full-grid rebuilds on 10–35% of frames |
| COR-3 | Med | parser | Oversized OSC/DCS swallows all later output, including RIS |
| COR-4 | Low-Med | sync | Synchronized output can be extended forever |
| MEM-7 | Low-Med | kitty_graphics | Images not freed when their placements go away; desktop keeps an extra BGRA copy |
| MEM-8 | Low-Med | kitty_graphics | Abandoned chunked uploads never expire and break the next upload |
| … | Low | various | See the "Low" section |

---

## High

### SEC-1: OSC 52 clipboard read without consent
- **Where:**
  - `crates/core/src/runtime/custom_backend.rs:399-402`: `engine::Event::Clipboard` with `data == "?"` queues `PendingEvent::ClipboardLoad`.
  - `custom_backend.rs:859-870`: the reply.
  - `crates/desktop_app/src/terminal_view/backend.rs:148-153`: `load_clipboard`.
- **Problem:** `ESC]52;c;?BEL` returns the system clipboard base64-encoded to the PTY, with no prompt and no config. This applies to remote SSH hosts, background tabs and the mux host bridge. It also undermines the OSC 5522 permission prompt, which reads the same clipboard.
- **Related:** the reply echoes the attacker-chosen `selection` field verbatim (`custom_backend.rs:867`, up to about 64 KiB, may contain C1 code points such as U+009B). ESC injection is not possible because control bytes are stripped inside OSC (verified).
- **Fix:**
  - Add a config key, e.g. `osc52 = copy-only | ask | allow`, defaulting to `copy-only` or `ask`.
  - Route reads through the same permission prompt as OSC 5522.
  - Validate `selection` against `[cpqs0-7]*` (xterm semantics) before echoing it.
- **Test:** feed `\e]52;c;?\a` and assert that no reply is produced under the default config.

### SEC-2: fd leak into child processes / cross-tab PTY fd sharing (measured)
- **Where:** `crates/core/src/terminal_engine/transport/unix.rs:690-747`. The `forkpty` call and the child branch are at about 695-727; `fcntl(master, F_SETFD, FD_CLOEXEC)` is at about 747.
- **Problem:**
  1. The child never closes fds ≥3 before `execve`, so every fd the GUI opened without `O_CLOEXEC` leaks into the shell and its descendants (e.g. a tmux server). Evidence: the live Termy `zsh` held fds 6–9 open read-write on `com.apple.metal/.../libraries.data` and `functions.data`.
  2. The master gets `FD_CLOEXEC` only after `forkpty` returns and after the parent waits for exec. `forkpty` also briefly holds the slave fd in the parent. Any concurrent fork/exec inherits them: another Termy spawn, or any of the about 16 `Command::new` call sites. With 8 threads × 3 rounds of concurrent `Pty::spawn`, 24 of 24 children held foreign `/dev/ptmx` fds (up to 4 each) and `/dev/ttysNNN`. A process in tab B can then read and inject into tab A, and keeps A's PTY alive.
- **Fix:**
  - In the child after fork, using only async-signal-safe calls, close all fds ≥3 except the child-status socket:
    - Linux: `close_range(3, ~0, 0)` or `closefrom`.
    - macOS: loop to `getdtablesize()`, or switch macOS to `posix_spawn` with `POSIX_SPAWN_CLOEXEC_DEFAULT | POSIX_SPAWN_SETSID`.
  - Open the PTY with `posix_openpt(O_RDWR | O_NOCTTY | O_CLOEXEC)` and a CLOEXEC slave instead of `forkpty`. At minimum, set `FD_CLOEXEC` immediately after `forkpty` and serialize spawns behind a global mutex.
- **Test:** spawn `ls -l /dev/fd` (macOS) or `/proc/self/fd` (Linux) and assert that only 0, 1 and 2 are open. Spawn concurrently from several threads and assert that no foreign ptmx fds appear.

### CPU-1: REP (`CSI n b`) amplification (measured)
- **Where:** `crates/core/src/terminal_engine/dispatch.rs:603-609`, which loops `self.grid.put_char(character)` `count` times (up to 65,535). The engine mutex is held for a whole read in `runtime/custom_backend.rs:175` (`Shared::feed`), and render/drain take the same mutex.
- **Measured:** 16 KB of `\e[65535b` took 1.05 s after ASCII, 1.57 s after CJK and 7.2 s after a regional indicator (grapheme segmentation on every call). One 32 KiB read can freeze the UI for about 15 s.
- **Fix:**
  - Clamp `count` to `cols * rows + cols` (anything more just overwrites itself).
  - For ASCII, bulk-write through the `write_ascii` fast path using a stack buffer.
  - If `last_printed` extends graphemes (regional indicator, ZWJ, VS15/16, combining), cap or ignore REP.
  - Also see CPU-2 about releasing the lock between parse batches.
- **Test:** time `\e[65535b` × N stays bounded; the screen content matches xterm for small counts.

### CPU-2: Reader thread starves the UI of the engine lock (measured on a model)
- **Where:** `transport/unix.rs:849-852` calls `on_output` once per read. That goes to `runtime/custom_backend.rs:171-176`, where `Shared::feed` locks the `state` mutex, also takes a second mutex in `schedule_maintenance`, and bumps the generation.
- **Problem:**
  - On macOS a PTY read returns at most 1024 bytes (measured during a 30 MB `cat`: max 1024, mean about 1006 bytes per callback), so the 32 KiB buffer is never filled and per-call overhead dominates.
  - `std::sync::Mutex` is not fair, so the reader re-takes it microseconds after releasing it.
  - In a model (real PTY, `std::Mutex`, UI locking every 8 ms), UI lock wait was:

    | Engine cost | Median | p99 | Max |
    |---|---|---|---|
    | 14 ns/byte (Termy `yes`) | 3.0 ms | 18.5 ms | |
    | 20 ns/byte | 8.6 ms | 55 ms | |
    | 40 ns/byte | | 174 ms | 293 ms |

  - A simple "UI waiting" flag that makes the reader yield after unlock brought p99 to about 20 µs with no throughput loss.
- **Fix:**
  - Use fair hand-off: `parking_lot::Mutex` with `unlock_fair`, or an Alacritty-style `FairMutex` lease.
  - Decouple reads from parsing: when `try_lock` fails, keep reading into a larger accumulation buffer (up to about 1 MiB), then parse that batch under one lock. Batching alone doesn't help on macOS; the decoupling is what matters.
  - Release the lock between about 4 KiB parse batches (this also helps CPU-1 and CPU-3).
- **Test/bench:** run `cat` on a large file while measuring UI-side lock wait (p99 should be well under one frame).

### MEM-1: `CSI K` (erase to end of line) marks the full row occupied (measured)
- **Where:**
  - `crates/core/src/terminal_engine/grid.rs:594` (`clear_wide_at`), called from `erase_range` at `grid.rs:706-708`.
  - Effects show up in `Row::clear` (`grid.rs:49-55`) and in packing (`grid/row.rs:153-157`, `178`).
- **Problem:** EL-to-end pushes the row's `occupied` to full width instead of lowering it, so every row that ever received `\e[K` is fully re-blanked on recycle and packed at full width. `\e[K` is everywhere: grep/gcc color output, readline/zsh redraws, `\r…\e[K` progress bars.
- **Measured** at 200 cols with short lines:
  - Packed history grows from 2.4 MB to 9.7 MB (4×).
  - 200k lines take 107 ms with `\e[K` vs 36 ms with a same-length no-op sequence (about 3×).
- **Fix:** if the erase reaches the row end and the bg matches the row's last clear bg, set `occupied = min(occupied, start_after_wide_fixup)` and only write cells below the old `occupied`. Only raise `occupied` when the bg differs.
- **Test:** after `"abc\e[K"` the row's occupied is 3, and the packed size equals that of a row containing only `"abc"`.

---

## Medium

### MEM-2: OSC 8 hyperlinks bypass the scrollback memory bound (measured)
- **Where:** `terminal_engine/dispatch.rs:760-778`.
- **Problem:** each OSC 8 allocates a fresh `Arc<Hyperlink>` with a URI of up to 64 KiB, and cells reference it. 3,000 links × 60 KB retained 172 MiB. The worst case is about 5 GiB at default limits (82k cells) and about 128 GiB at the 2M-cell maximum, which contradicts the README's "bounded history memory".
- **Fix:** cap the URI and `id` length (kitty uses 2 KiB), and intern links in a bounded table keyed by `(id, uri)` that evicts once no cell references the link.

### MEM-3: APC buffering and graphics payload copies (measured)
- **Where:**
  - `terminal_engine/parser.rs:12`: `MAX_APC_BYTES` = 256 MiB for *any* APC.
  - `terminal_engine/mod.rs:108`.
  - `parser.rs:440-460`: power-of-two growth.
  - `terminal_engine/graphics.rs:197`: `body.to_vec()` full copy, despite a comment in `KittyGraphicsCommand::parse` claiming the parser's allocation is kept.
  - `kitty_graphics.rs:427-446`: base64 is decoded before the size check.
  - `media/image.rs:33`: `Vec` → `Arc<[u8]>` copies again.
- **Measured:**
  - An unterminated `ESC _Z…` holds 256 MiB.
  - A 129 MiB payload reserves 256 MiB because of power-of-two rounding.
  - A 100 MiB graphics payload peaks at 303 MiB.
  - A 128 MiB image in one APC peaks at 473 MiB; in 4 KiB chunks, 260 MiB.
  - A rejected payload of about 143 MiB peaks at 529 MiB.
  - A discarded APC over 256 MiB still costs 260 MiB, and no error reply is sent.
  - There is no cross-pane budget: up to 128 MiB stored plus 128 MiB pending per pane.
- **Fix:**
  - Use the 64 KiB limit unless the first payload byte is `G`. Consider lowering the `G` limit to a few MiB, since the spec uses 4 KiB chunks.
  - Pass the APC buffer by value (`mem::take`) or decode from the borrowed slice.
  - Reject early when `encoded.len() / 4 * 3` exceeds the remaining quota.
  - Store `Arc<Vec<u8>>` to avoid the final copy.
  - Drop power-of-two rounding near the cap.
  - Send an error reply when a payload is discarded.

### MEM-4: zlib (`o=z`) inflate capped at 128 MiB, not at the declared image size (measured)
- **Where:**
  - `kitty_graphics.rs:895-900` and `1383-1393`.
  - The size check only happens later, at `kitty_graphics/image.rs:53`.
  - `t=f` and `t=s` follow the same pattern: they read the full length before validating.
- **Measured:** a 169 KiB message inflates to 128 MiB (about 256 MiB transient) in 23 ms, then fails the size check because it declares `s=1,v=1`.
- **Fix:** for `f=24/32`, cap inflation and file/shm reads at `s*v*channels + 1` (kitty's approach).

### CPU-3: Heavy image work runs under the engine lock (measured)
- **Where:** `runtime/custom_backend.rs:175-176` → `State::feed` → `apply_graphics`.
- **Problem:** base64, zlib, PNG decode and `t=f` file reads all run on the PTY reader thread while holding the lock, and the UI takes that lock about 5× per frame per pane. Measured: the worst single locked feed was 79–94 ms, and each zlib bomb was 23 ms. A `t=f` path on a slow or network filesystem blocks for the whole read.
- **Fix:** parse the command, release the lock, decode, then re-lock to commit (with ordering preserved), or use a decode worker that applies results in order.

### COR-1: Protocol replies spliced into pastes and key sequences
- **Where:**
  - `transport/unix.rs:1093-1101`, `transport/windows.rs:1059-1064`.
  - The test `protocol_reply_preempts_at_a_bounded_partial_write_boundary` asserts `"noreplyrmal-one"`, so the current behavior is intentional.
- **Problem:**
  - The reply lane preempts at any partial-write boundary.
  - The macOS raw-mode input queue is about 1022 bytes (measured), so any paste over about 1 KiB into a TUI is written in pieces.
  - A bracketed paste is one framed entry (`desktop_app/.../interaction/input.rs:658`), so a DSR/CPR, DA or OSC 11 reply generated mid-paste lands inside `ESC[200~…ESC[201~`. It can also land inside a key's escape sequence.
- **Fix:** only switch lanes at entry boundaries (offset == 0), or use strict FIFO like xterm/Ghostty. Update the test.

### COR-2: Reply-queue overflow SIGKILLs the shell
- **Where:** `transport/unix.rs:853-855`, `1331-1343`, `924-934`; `transport/windows.rs:1017-1029`.
- **Problem:** with 256 pending replies and a full input queue, the code runs `kill(-pid, SIGKILL)` on the process group.
- **Scenario:** `cat` of a binary/asciicast/log containing many queries while the foreground program isn't reading. The input queue fills in raw mode, or in canonical mode after typeahead plus Enter (blocks at about 1 KiB, measured). A hostile file can kill the user's shell.
- **Fix:** coalesce or deduplicate identical DA/DSR replies, drop the oldest on overflow, and log. Never kill.

### MEM-5: Combining-mark floods (measured)
- **Where:** `terminal_engine/types.rs:5` (`MAX_COMBINING_BYTES = 256`), `grid/combining.rs:113-123`.
- **Problem:** up to 256 B of marks per cell, and every added mark builds a new `String` + `Arc` that copies the existing marks (quadratic per cell).
- **Measured** with 128 random marks per cell:
  - 337 B retained per cell, about 10× a plain cell.
  - 40 MiB for a default 120×1000 pane; 335 MiB at max config (20000 rows × 52 cols).
  - Throughput about 11 MiB/s vs 394 MiB/s for ASCII.
- **Fix:**
  - Cap at about 32–64 B (the longest standard emoji ZWJ suffix is about 31 B).
  - Append in place via `Arc::make_mut` when the extra isn't shared.
  - Don't cache long values.

### CPU-4: Resize/reflow cost (measured)
- **Where:** `grid.rs:1272-1371`; the hot spots are at about `1299`, `1320-1328` and `1364`.
- **Problem:**
  - Every packed history row is fully decoded.
  - Each logical line builds a fresh `Vec` that grows by doubling (a filter iterator gives no size hint).
  - Every output row is newly allocated.
  - The whole history stays unpacked until idle compaction.
- **Measured:** a 1-column resize with about 8.7k history rows at 120 cols takes 8.7 ms, about 62k allocations and about 105 MB allocated; live heap grows from 7 MB to 33 MB. This repeats on every drag event, for every pane.
- **Fix:**
  - Reuse one line buffer (`drain`, `reserve(len)` up front).
  - Recycle the consumed source rows' buffers as output rows.
  - Fast path: rows that aren't soft-wrapped and fit the new width (and aren't the cursor row) keep their packed form with only a width update.

### CPU-5: Scripts that miss the print fast path (measured)
- **Where:** `terminal_engine/grid/print.rs:24-37` (`ordinary_base`), `91`, `98`, `148-150`.
- **Problem:** characters outside `ordinary_base` zero a 264 B stack buffer and run `graphemes(true)` on the previous character plus this one, for every character.
- **Measured** cost per character:

  | Text | ns/char |
  |---|---|
  | Hebrew | 35.9 |
  | Arabic | 36.5 |
  | Devanagari | 50.6 |
  | Thai | 44.1 |
  | Nerd Font icons (PUA plane 15) | 38.0 |
  | Cyrillic | 10.5 |
  | CJK | 13.8 |
  | ASCII | 2.4 |

- **Fix:** add these ranges to `ordinary_base`; the existing exhaustive tests will check that it stays correct.
  - Hebrew letters `U+05D0–05F2`.
  - Arabic letters, excluding the Prepend characters `U+0600–0605`, `U+06DD`, `U+0890–0891`, `U+08E2`.
  - Thai consonants.
  - PUA planes `U+F0000–10FFFD`.

  For Indic, first make virama end the fast path the way ZWJ does (Unicode's conjunct rule, GB9c), then add consonants. Use a small buffer when the previous cell has no marks.

### MEM-6: Cell size 32 B → 24 B
- **Where:** `terminal_engine/types.rs:47-54`, `89-95`; `grid/row.rs:8-12`.
- **Problem:**
  - Style is 16 B with 15 used. `underline_color` (4 B, set only by the rare SGR 58) sits on every cell.
  - `attributes` uses 8 of 16 bits, and `flags` 3 of 16.
  - The packed scalar is 8 B (`char` + `u16` + 2 B padding).
  - Each style run is 24 B, and a single run still allocates room for 4.
- **Fix:**
  - Move `underline_color` into `CellExtra` and fold underline style and cell flags into `attributes`. That gives a 12 B style and a 24 B cell: 25% less memory for the screen and unpacked history (up to 1M cells per pane).
  - Pack the 3 flag bits into the `char`'s unused high bits so a packed scalar is 4 B.
  - Avoid over-allocating single style runs.
  - Fix the stale comment at `crates/core/src/config_core/constants.rs:37`, which says about 24 B per cell.
  - Add a `const _: () = assert!(size_of::<Cell>() == 24);` guard.

### CPU-6: Unicode-placeholder snapshot cost (measured)
- **Where:**
  - `kitty_graphics.rs:627-640` and `kitty_graphics/placement.rs:73-96`: for each placeholder cell with placement id 0, the code scans all placements backwards.
  - Desktop: the graphics cache key includes `terminal_generation` (`crates/desktop_app/src/terminal_view/render.rs:3140-3147`), so a new snapshot is taken every output frame, on the UI thread, under the lock.
- **Measured:** 0.68 ms with 0 extra placements, 2.7 ms with 100, 121 ms with 4095.
- **Fix:**
  - Precompute the latest virtual placement per `(screen, image)` once per snapshot, in a HashMap.
  - Drop `terminal_generation` from the desktop key; the engine's graphics revision already covers the cases that matter.

### CPU-7: Damage/cell-read race forces full rebuilds (measured)
- **Where:**
  - `runtime/custom_backend.rs:1088-1091` rejects a stale generation.
  - The desktop then falls back to `render_read(true)`, a full-grid copy (`render.rs:1113-1116`, `terminal_view/mod.rs:949`).
- **Measured:** with 0.5 ms between taking damage and reading cells, the fallback hit 10% of frames at 1 KiB per 4 ms, 23% at 4 KiB per 2 ms, and 28–35% at 32 KiB per ms.
- **Fix:** one locked call that takes the damage and visits the dirty spans atomically.

### COR-3: Oversized OSC/DCS strings swallow everything after them (measured)
- **Where:** `terminal_engine/parser.rs:253-259`; the intended behavior is pinned by the test `oversized_strings_discard_embedded_escapes_until_termination`.
- **Problem:** once a string exceeds 64 KiB, `ESC x` no longer terminates it. `cat` of a file with a stray `ESC P` followed by more than 64 KiB of text hides all later output, and `reset` (RIS, `ESC c`) does nothing. DCS only recovers on ST/CAN/SUB; OSC also on BEL.
- **Fix:** at minimum, let `ESC c` and `ESC [` terminate a discarded DCS/APC string, or bound how long discard mode lasts.

### COR-4: Synchronized output can be extended forever (measured)
- **Where:** `terminal_engine/sync.rs:65-67`, `118`.
- **Problem:** every `?2026h` resets the 150 ms deadline. Re-sending it every 100 ms kept the screen frozen for over 600 ms, and it would stay frozen until the 2 MiB cap. Replies and events are held back for the same time.
- **Fix:** add a hard ceiling measured from the first begin-sync (e.g. 1 s total).

### MEM-7: Image data not freed when its placements go away (measured)
- **Where:** `kitty_graphics.rs:690-696` (`reset`); quota/eviction at `1264-1301`.
- **Problem:**
  - Images uploaded with no id (`a=T` without `i`/`I`) can't be referenced again, yet they survive scroll-off, screen clears and RIS. One could be re-placed by its internal id after `ESC c`.
  - Up to 128 MiB per pane stays resident until a later upload forces eviction. Kitty frees images with no remaining placements in these cases.
  - Desktop: the texture cache (`crates/desktop_app/.../kitty_images.rs:83`) isn't pruned against live placements. It keeps a second, CPU-side BGRA copy next to the engine RGBA and the GPU texture, and is cleared on tab switch (`tabs/sizing.rs:16`), so every image is re-converted on the UI thread.
- **Fix:**
  - Free unreferenced no-id images when their last placement goes away and on clear/RIS.
  - Prune the desktop cache against live placements, and drop the CPU copy once uploaded to the GPU.
  - Don't clear the cache on tab switch.

### MEM-8: Abandoned chunked uploads never expire (measured)
- **Where:** `kitty_graphics.rs:449-466`.
- **Problem:** an `m=1` upload that never finishes (e.g. the image tool is killed with Ctrl-C) keeps up to 128 MiB indefinitely. The next unrelated upload then fails with `EINVAL: continuation contains unsupported control data`, and its first chunk is lost.
- **Fix:** if a new command carries transmission keys (`a`, `i`, `I`, `f`), drop the stale upload and start the new one. Optionally add a timeout.

---

## Low

### LOW-1: Writer wakes about 100×/s while input is blocked (measured)
- **Where:** `transport/unix.rs:67`, `1119-1131`, `1159-1170`.
- **Problem:** the writer loops write → `EAGAIN` → `poll(POLLOUT, 10ms)` while the child isn't reading (typeahead, a paste during a long build). That is about 90–100 wakeups per second per terminal; there is no busy spin.
- **Fix:** add a wake fd to the poll set (the reader already uses a `UnixStream` pair) and poll with timeout `-1`.

### LOW-2: Input silently dropped when the write backlog is full
- **Where:** `runtime/custom_backend.rs:723`, `730` (only log a warning).
- **Problem:**
  - A paste over 8 MiB (one framed entry) is rejected whole.
  - Any input arriving while 8 MiB or 4096 entries are pending is discarded.
  - Mouse-motion reports to a stalled app use up the entry budget, after which keystrokes are dropped.
- **Fix:** surface the error in the UI, apply UI-side backpressure, and coalesce mouse motion.

### LOW-3: DECAWM off (`CSI ?7l`) disables the ASCII fast path (measured)
- **Where:** `grid/print.rs:303`.
- **Problem:** every byte goes through `put_char`, 4–6× slower (321–424 ms vs 50–110 ms for 15 MB of short lines).
- **Fix:** with autowrap off, bulk-write `min(n, cols - col)` bytes, then write only the last byte of the remainder into the final column.

### LOW-4: Scrollback capacity depends on the current width, and trimming is permanent (measured)
- **Where:** `grid.rs:308-312`, `1196-1197`, `1290-1293`.
- **Problem:** the limit is `min(configured, 20000, 1M cells / cols)`, which assumes unpacked 32 B cells even though old history is packed.
  - Requesting 10000 at 120 cols keeps 8738 rows.
  - Widening to 400 cols trims to 2621; narrowing back leaves 2621, so 6117 rows are lost for good.
  - At 250 cols only 4194 rows are kept.
  - The default of 1000 rows is only affected above about 1048 cols.
- **Fix:** budget by actual bytes, or don't evict on a transient widen.

### LOW-5: Recycling a packed row decodes it just to wipe it (measured)
- **Where:** `grid.rs:48` (`make_dense` inside `Row::clear`), reached from `grid.rs:876` and `888`.
- **Problem:** in any burst over 4 KB that follows idle compaction, each of the first history-limit lines costs 1 allocation and 3 frees. 5000 lines at 200 cols take 1.93 ms instead of 1.36 ms (+42%).
- **Fix:** in `Row::clear`, if the row is packed, drop the packed data and resize the cell buffer to blanks without decoding.

### LOW-6: Reused packed buffers only grow (measured)
- **Where:** `grid/row.rs:29-39`, `174-178`.
- **Problem:** the `retain` reuse path never shrinks the evicted row's buffers, and the size check uses capacity. The same 2000 rows take 2648 KiB through reuse vs 1371 KiB with fresh compaction.
- **Fix:** shrink when capacity is more than 2× what's needed.

### LOW-7: Dirty-row rotate on every scrolled line (measured)
- **Where:** `grid.rs:418`, `421`.
- **Problem:** per-line cost is 102 ns at 100 rows, 165 ns at 400 rows and 605 ns at 2000 rows.
- **Fix:** skip the rotate once the pending scroll already covers the whole region (every entry is already fully dirty), or use a ring buffer with an offset.

### LOW-8: Memory never released
- **Where:** `grid.rs:999-1001`, `1061`, `1083-1085`.
- **Problem:**
  - The alt screen stays allocated for the life of the pane once used (e.g. 320 KB at 200×50). It could be freed when leaving 1049/1047, since 1049 clears it on entry anyway.
  - The history `VecDeque` capacity never shrinks after a clear or trim.

### LOW-9: Insert mode (IRM) does a full-row pass per character
- **Where:** `grid/print.rs:235-237`, `grid.rs:814-833`.
- **Problem:** each printed character rotates the row and runs `repair_wide` over it. Insert mode is rarely used.

### LOW-10: OSC 52 writes over about 48 KiB of text silently dropped
- **Where:** `parser.rs:11` (the generic 64 KiB string cap).
- **Problem:** large copies from nvim/tmux fail with no feedback.
- **Fix:** give OSC 52 its own larger cap (e.g. a few MiB) or stream-decode it.

### LOW-11: `ESC` followed by a non-ASCII byte eats the following text (measured)
- **Where:** `parser.rs:364`.
- **Problem:** `A ESC é!BC` renders as `AC`.
- **Fix:** on a byte ≥0x80 in the escape state, return to ground and reprocess the byte as text.

### LOW-12: IL/DL (`CSI L`/`CSI M`) don't move the cursor to column 1 (measured)
- **Where:** `grid.rs:940-951`.
- **Problem:** the cursor stays in its column; xterm, kitty and Ghostty move it to column 1.

### LOW-13: OSC 7 not percent-decoded; hostname ignored
- **Where:** `dispatch.rs:737-743`.
- **Problem:** `My%20Dir`-style paths from vte.sh and fish end up in `last_prompt_cwd` and in persisted session data.
- **Fix:** percent-decode, and ignore the cwd (or mark it remote) if the host isn't local.

### LOW-14: OSC 5522 permission dialog shows a program-chosen `name`
- **Where:** `crates/desktop_app/src/terminal_view/backend.rs:162`.
- **Problem:** the name is up to 256 bytes, newlines allowed, so a program can impersonate something trusted.
- **Fix:** strip control characters, truncate, and label the name as program-supplied.

### LOW-15: kitty `t=f` reads any regular local file (measured)
- **Where:** `kitty_graphics.rs:1202-1219`, `1395-1423`.
- **Problem:** there is no deny-list for `/proc`, `/sys` or `/dev` (other than `/dev/shm`), unlike kitty's `is_ok_to_read_image_file`. Error replies reveal to a remote program whether a file exists and how big it is: `/etc/hosts` gave a size-mismatch error, a missing file gave `ENOENT`, and an existing one gave `OK` with `S=4`.
- **Fix:** add the deny-list, and return the same generic error for every failure.

### LOW-16: kitty `t=t` delete check uses the raw path string
- **Where:** `kitty_graphics.rs:1471-1485`.
- **Problem:** `/tmp/<dir containing tty-graphics-protocol>/../victim` passes the check if such a directory exists.
- **Fix:** canonicalize, require the marker in the resolved *file name*, and require the parent to be a temp root.

### LOW-17: kitty `t=s` unlinks the named shm object even when rejecting the request
- **Where:** `media/shared_memory.rs:37`, where the cleanup guard is created before validation.
- **Problem:** a remote program can unlink any POSIX shm object the user can open, just by naming it.
- **Fix:** only create the unlink guard after validation succeeds.

### LOW-18: Environment leakage into shells (plausible)
- **Where:** `transport/unix/launch.rs:22-46`, Windows `transport/windows/launch.rs:357`.
- **Problem:** the shell inherits the full parent environment; the code can add overrides but never remove variables. When Termy is launched from another terminal or an IDE, `TMUX`, `TMUX_PANE`, `STY`, `ZELLIJ*`, `KITTY_WINDOW_ID`, `ITERM_SESSION_ID`, `WT_SESSION`, `VSCODE_*`, `GHOSTTY_RESOURCES_DIR` and `COLUMNS`/`LINES` reach the shell.
- **Fix:** strip a denylist before applying overrides.

### LOW-19: Signal mask not reset in the child (latent)
- **Where:** `transport/unix.rs:1356-1375`.
- **Problem:** handlers are reset for seven signals, but the blocked mask is never cleared, unlike `std::process::Command`. Termy blocks no signals today (live zsh shows `BLOCKED=0`), so nothing is broken yet.
- **Fix:** call `sigprocmask(SIG_SETMASK, ∅)` and reset all signal dispositions in the child.

### LOW-20: Hard-coded Linux constants are wrong on some architectures
- **Where:** `transport/unix.rs:29-113`.
- **Problem:** x86/ARM values are used for all Linux targets. ppc64le has different `TIOCSWINSZ` (`0x80087467`) and `FIONREAD` (`0x4004667F`); mips has `O_NONBLOCK = 0x80` and `SIGCHLD = 18`.
- **Fix:** use the `libc` crate, or gate on `target_arch`.

### LOW-21: Exit-drain `FIONREAD` is always 0 on macOS
- **Where:** `transport/unix.rs:1292`, `1345`.
- **Problem:** `FIONREAD` on the master returns 0 on macOS even with 700 bytes pending (measured), so the "buffered at exit" snapshot is a no-op there and the drain relies only on its 25 ms budget. Harmless today: macOS makes the exiting child wait in `close()` until its output is read (verified), so nothing is lost. On Linux it only counts the 4 KiB line-discipline buffer.

### LOW-22: Windows: tab close kills the shell immediately
- **Where:** `transport/windows.rs:970`, `1129-1143`; `windows/resources.rs:51`.
- **Problem:** `TerminateProcess` runs before `ClosePseudoConsole` with no grace period, so PowerShell can't save history. Unix gives the shell 250 ms after SIGHUP. Otherwise Windows handle hygiene looked clean.

### LOW-23: Per-terminal thread count
- **Problem:** on Unix each terminal has reader + writer + watcher threads, an escalation thread on every drop, and the runtime maintenance thread.
- **Fix:**
  - Fold the writer into the reader's poll loop (`POLLOUT` + wake fd; also fixes LOW-1).
  - Use one shared reaper: kqueue `EVFILT_PROC` on macOS, pidfd on Linux.

### LOW-24: Node addon overhead (`crates/pty/src/lib.rs:131-138`)
- **Problem:**
  - One threadsafe-function call plus a copy per read (at most 1 KiB on macOS), so about 1000 JS callbacks per MB.
  - `pause()` blocks inside the transport callback, so the exit event waits for `resume()`.
  - `write` throws on a full backlog, which node-pty never does.
- **Fix:** coalesce chunks while the TSFN queue is non-empty, and make backpressure non-throwing.

---

## Verified OK (don't re-audit)
- **Parser:**
  - CSI params saturate at u16; the limits of 32 params, 8 subparams and 2 intermediates are enforced.
  - C0 controls execute inside CSI, and CAN/SUB cancel.
  - UTF-8 errors are replaced per maximal subpart, and split chunks give identical results.
  - Throughput: about 340 MiB/s ASCII, 190 MiB/s CJK, 220 MiB/s SGR-heavy. 4 KiB vs 64 KiB chunks made no measurable difference. No per-byte allocation, locking or hashing in the hot path.
- **Bounded stacks and queues:**
  - Title stack 64, keyboard stack 32; saved modes are limited to the known set.
  - Event queue 1024; reply queue 64 KiB in the engine and 2 MiB in runtime/transport.
  - Write queue 8 MiB / 4096 entries.
- **Query replies:** DECRQSS, DECRQM and XTGETTCAP never echo raw program input (XTGETTCAP validates hex). Title reporting is not implemented.
- **Transport:**
  - No unbounded reader→parser queue: parsing runs on the reader thread, so the PTY applies backpressure.
  - EINTR is retried on `poll`, `read`, `ioctl` and `waitpid`.
  - EIO (Linux) and EOF (macOS) are handled.
  - No zombies: the watcher always reaps the child.
  - The reader poll has no timeout, and resizes are coalesced and ordered before later input.
  - Windows ConPTY handles are not inherited and are cleaned up.
- **Runtime:**
  - No idle wakeups: maintenance waits on a condvar.
  - Scrollback compaction always makes progress.
  - The sync timeout fires only while sync is active.
  - The desktop normally reads only damaged spans.
- **Graphics:**
  - `w*h` is computed in u64 and capped at 32768 per side and 128 MiB.
  - Animation frames are capped at 4096 and counted against the quota; off-screen animations don't schedule wakeups.
  - An APNG first-frame mismatch is rejected by png 0.18.
- **Grid:** about 240k randomized fuzz sessions (debug assertions + overflow checks) found no panics. Wide and combining characters at the margin, 1-column screens, resize with wide chars on the alt screen, scroll regions and origin mode, ICH/DCH beside wide halves, and negative history indices were all correct.
