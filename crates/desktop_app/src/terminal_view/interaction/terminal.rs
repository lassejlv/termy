use super::*;

impl DeferredTerminalResize {
    fn is_ready_for(&self, signature: TerminalResizeSignature) -> bool {
        matches!(self, Self::Ready { signature: ready } if *ready == signature)
    }

    fn schedule(&mut self, signature: TerminalResizeSignature, deadline: Instant) -> bool {
        if matches!(self, Self::Pending { signature: pending, .. } if *pending == signature) {
            return false;
        }
        let timer_running = matches!(self, Self::Pending { .. });
        *self = Self::Pending {
            signature,
            deadline,
        };
        !timer_running
    }

    #[cfg(test)]
    fn mark_ready(&mut self, signature: TerminalResizeSignature, deadline: Instant) -> bool {
        if *self
            != (Self::Pending {
                signature,
                deadline,
            })
        {
            return false;
        }
        *self = Self::Ready { signature };
        true
    }
}

impl TerminalView {
    const TERMINAL_RESIZE_FINGERPRINT_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const TERMINAL_RESIZE_FINGERPRINT_PRIME: u64 = 0x0000_0100_0000_01b3;

    fn mix_terminal_resize_fingerprint(fingerprint: &mut u64, value: u64) {
        *fingerprint ^= value;
        *fingerprint = fingerprint.wrapping_mul(Self::TERMINAL_RESIZE_FINGERPRINT_PRIME);
    }

    fn pane_layout_fingerprint(&self) -> u64 {
        let mut fingerprint = Self::TERMINAL_RESIZE_FINGERPRINT_OFFSET;
        Self::mix_terminal_resize_fingerprint(&mut fingerprint, self.session.tabs.len() as u64);
        for tab in &self.session.tabs {
            Self::mix_terminal_resize_fingerprint(&mut fingerprint, tab.id);
            Self::mix_terminal_resize_fingerprint(&mut fingerprint, tab.panes.len() as u64);
            for pane in &tab.panes {
                for byte in pane.id.bytes() {
                    Self::mix_terminal_resize_fingerprint(&mut fingerprint, u64::from(byte));
                }
                Self::mix_terminal_resize_fingerprint(&mut fingerprint, u64::from(pane.left));
                Self::mix_terminal_resize_fingerprint(&mut fingerprint, u64::from(pane.top));
                Self::mix_terminal_resize_fingerprint(&mut fingerprint, u64::from(pane.width));
                Self::mix_terminal_resize_fingerprint(&mut fingerprint, u64::from(pane.height));
                Self::mix_terminal_resize_fingerprint(
                    &mut fingerprint,
                    pane.pane_zoom_steps as u16 as u64,
                );
                Self::mix_terminal_resize_fingerprint(
                    &mut fingerprint,
                    u64::from(pane.last_alternate_screen.get()),
                );
            }
        }
        fingerprint
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal_resize_signature(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        cell_width: f32,
        cell_height: f32,
        sidebar_width: f32,
        content_top_inset: f32,
        runtime_kind: RuntimeKind,
    ) -> TerminalResizeSignature {
        TerminalResizeSignature {
            viewport_width_bits: viewport_width.to_bits(),
            viewport_height_bits: viewport_height.to_bits(),
            cell_width_bits: cell_width.to_bits(),
            cell_height_bits: cell_height.to_bits(),
            sidebar_width_bits: sidebar_width.to_bits(),
            content_top_inset_bits: content_top_inset.to_bits(),
            padding_x_bits: self.padding_x.to_bits(),
            padding_y_bits: self.padding_y.to_bits(),
            runtime_kind,
            active_tab_id: self
                .session
                .tabs
                .get(self.session.active_tab)
                .map(|tab| tab.id),
            pane_layout_fingerprint: self.pane_layout_fingerprint(),
        }
    }

    fn schedule_deferred_inactive_resize(
        &mut self,
        signature: TerminalResizeSignature,
        cx: &mut Context<Self>,
    ) {
        let delay = Duration::from_millis(INACTIVE_RESIZE_DEBOUNCE_MS);
        let deadline = Instant::now() + delay;
        if !self.deferred_inactive_resize.schedule(signature, deadline) {
            return;
        }

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut remaining = delay;
            loop {
                smol::Timer::after(remaining).await;
                let next_delay = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        let DeferredTerminalResize::Pending {
                            signature,
                            deadline,
                        } = view.deferred_inactive_resize
                        else {
                            return None;
                        };
                        let now = Instant::now();
                        if deadline > now {
                            return Some(deadline.duration_since(now));
                        }
                        view.deferred_inactive_resize = DeferredTerminalResize::Ready { signature };
                        cx.notify();
                        None
                    })
                });
                match next_delay {
                    Ok(Ok(Some(next_delay))) => remaining = next_delay,
                    _ => return,
                }
            }
        })
        .detach();
    }

    fn sync_active_alternate_screen_state(&self) {
        let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
            return;
        };
        for pane in &tab.panes {
            pane.sync_alternate_screen_state();
        }
    }

    fn font_size_cache_key(font_size: Pixels) -> u32 {
        let font_size_px: f32 = font_size.into();
        font_size_px.to_bits()
    }

    pub(in super::super) fn cached_cell_size_for_font_size(
        &self,
        font_size: Pixels,
    ) -> Option<Size<Pixels>> {
        self.cell_size_cache
            .get(&Self::font_size_cache_key(font_size))
            .copied()
    }

    fn fallback_cell_size() -> Size<Pixels> {
        let default = TerminalSize::default();
        Size {
            width: default.cell_width.into(),
            height: default.cell_height.into(),
        }
    }

    pub(in super::super) fn layout_cell_size(&self) -> Size<Pixels> {
        self.cached_cell_size_for_font_size(self.font_size)
            .unwrap_or_else(Self::fallback_cell_size)
    }

    fn pane_local_zoom_enabled_for_tab(&self, tab: &TerminalTab) -> bool {
        self.runtime_kind() == RuntimeKind::Native && tab.panes.len() > 1
    }

    fn clamped_font_size_for_zoom_steps(
        base_font_size: Pixels,
        zoom_steps: i16,
        use_local_zoom: bool,
    ) -> Pixels {
        let base_font_size: f32 = base_font_size.into();
        let effective_font_size = if use_local_zoom {
            base_font_size + (zoom_steps as f32 * ZOOM_STEP)
        } else {
            base_font_size
        };
        px(effective_font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE))
    }

    fn effective_font_size_for_zoom_steps(&self, zoom_steps: i16, use_local_zoom: bool) -> Pixels {
        Self::clamped_font_size_for_zoom_steps(self.font_size, zoom_steps, use_local_zoom)
    }

    pub(in super::super) fn effective_font_size_for_pane_in_tab(
        &self,
        tab: &TerminalTab,
        pane: &TerminalPane,
    ) -> Pixels {
        self.effective_font_size_for_zoom_steps(
            pane.pane_zoom_steps,
            self.pane_local_zoom_enabled_for_tab(tab),
        )
    }

    fn active_pane_uses_local_zoom(&self) -> bool {
        self.active_tab_ref()
            .is_some_and(|tab| self.pane_local_zoom_enabled_for_tab(tab))
    }

    fn adjust_active_pane_zoom_steps(&mut self, step_delta: i16, cx: &mut Context<Self>) {
        if !self.active_pane_uses_local_zoom() {
            return;
        }

        let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) else {
            return;
        };
        let Some(active_pane_index) = tab.active_pane_index() else {
            return;
        };
        let Some(active_pane) = tab.panes.get_mut(active_pane_index) else {
            return;
        };

        let current_steps = active_pane.pane_zoom_steps;
        let current_font_size =
            Self::clamped_font_size_for_zoom_steps(self.font_size, current_steps, true);
        let next_steps = current_steps.saturating_add(step_delta);
        let next_font_size =
            Self::clamped_font_size_for_zoom_steps(self.font_size, next_steps, true);
        if current_font_size == next_font_size {
            return;
        }

        active_pane.pane_zoom_steps = next_steps;
        self.clear_terminal_scrollbar_marker_cache();
        cx.notify();
    }

    fn reset_active_pane_zoom_steps(&mut self, cx: &mut Context<Self>) {
        if !self.active_pane_uses_local_zoom() {
            return;
        }

        let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) else {
            return;
        };
        let Some(active_pane_index) = tab.active_pane_index() else {
            return;
        };
        let Some(active_pane) = tab.panes.get_mut(active_pane_index) else {
            return;
        };
        if active_pane.pane_zoom_steps == 0 {
            return;
        }

        active_pane.pane_zoom_steps = 0;
        self.clear_terminal_scrollbar_marker_cache();
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal_grid_size_for_pane_count(
        pane_count: usize,
        viewport_width: f32,
        viewport_height: f32,
        sidebar_width: f32,
        content_top_inset: f32,
        padding_x: f32,
        padding_y: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> (u16, u16) {
        let (outer_padding_x, outer_padding_y) = if Self::uses_outer_terminal_padding(pane_count) {
            (padding_x, padding_y)
        } else {
            (0.0, 0.0)
        };
        let terminal_width =
            (viewport_width - sidebar_width - (outer_padding_x * 2.0)).max(cell_width * 2.0);
        let terminal_height =
            (viewport_height - content_top_inset - (outer_padding_y * 2.0)).max(cell_height);
        (
            Self::compute_terminal_cols(terminal_width, cell_width, false),
            Self::compute_terminal_rows(terminal_height, cell_height),
        )
    }

    fn repair_native_tab_active_pane_for_resize(tab: &mut TerminalTab) -> bool {
        if tab.panes.is_empty() {
            return false;
        }
        if tab.has_active_pane() {
            return true;
        }

        // Restored native layouts can carry a pane id that no longer exists.
        // Repair that invariant here so a resize never crashes the app.
        let fallback_id = tab
            .panes
            .first()
            .map(|pane| pane.id.clone())
            .expect("non-empty native tab must have a first pane");
        log::warn!(
            "native resize repaired stale active pane id '{}' for tab '{}'; falling back to '{}'",
            tab.active_pane_id,
            tab.window_id,
            fallback_id
        );
        tab.active_pane_id = fallback_id;
        true
    }

    pub(in super::super) fn execute_layout_command_action(
        &mut self,
        action: CommandAction,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            CommandAction::ZoomIn => {
                if self.active_pane_uses_local_zoom() {
                    self.adjust_active_pane_zoom_steps(1, cx);
                } else {
                    let current: f32 = self.font_size.into();
                    self.update_zoom(current + ZOOM_STEP, cx);
                }
                true
            }
            CommandAction::ZoomOut => {
                if self.active_pane_uses_local_zoom() {
                    self.adjust_active_pane_zoom_steps(-1, cx);
                } else {
                    let current: f32 = self.font_size.into();
                    self.update_zoom(current - ZOOM_STEP, cx);
                }
                true
            }
            CommandAction::ZoomReset => {
                if self.active_pane_uses_local_zoom() {
                    self.reset_active_pane_zoom_steps(cx);
                } else {
                    self.update_zoom(self.base_font_size, cx);
                }
                true
            }
            _ => false,
        }
    }

    pub(in super::super) fn update_zoom(&mut self, next_size: f32, cx: &mut Context<Self>) {
        let clamped = next_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        let current: f32 = self.font_size.into();
        if (current - clamped).abs() < f32::EPSILON {
            return;
        }

        self.font_size = px(clamped);
        self.cell_size_cache.clear();
        self.clear_tab_title_width_cache();
        cx.notify();
    }

    pub(in super::super) fn calculate_cell_size_for_font_size(
        &mut self,
        font_size: Pixels,
        window: &mut Window,
        _cx: &App,
    ) -> Size<Pixels> {
        if let Some(cell_size) = self.cached_cell_size_for_font_size(font_size) {
            return cell_size;
        }

        let font = Font {
            family: self.font_family.clone(),
            weight: FontWeight::NORMAL,
            ..gpui::font("")
        };

        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system
            .advance(font_id, font_size, 'M')
            .map_or(px(9.0), |advance| advance.width);

        let cell_height = font_size * self.line_height;

        let cell_size = Size {
            width: cell_width,
            height: cell_height,
        };
        self.cell_size_cache
            .insert(Self::font_size_cache_key(font_size), cell_size);
        cell_size
    }

    pub(in super::super) fn calculate_cell_size(
        &mut self,
        window: &mut Window,
        cx: &App,
    ) -> Size<Pixels> {
        self.calculate_cell_size_for_font_size(self.font_size, window, cx)
    }

    pub(in super::super) fn sync_terminal_size(
        &mut self,
        window: &mut Window,
        layout_cell_size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let viewport = window.viewport_size();
        let viewport_width: f32 = viewport.width.into();
        // The inspector pane reserves space at the bottom; size the PTY grids
        // against the remaining terminal area so content never hides under it.
        let viewport_height: f32 =
            Into::<f32>::into(viewport.height) - self.inspector_bottom_inset();
        let cell_width: f32 = layout_cell_size.width.into();
        let cell_height: f32 = layout_cell_size.height.into();

        if cell_width <= 0.0 || cell_height <= 0.0 {
            return;
        }

        // Entering/leaving a full-screen TUI changes its available grid even
        // when the window size is unchanged. Refresh before the resize cache.
        self.sync_active_alternate_screen_state();
        let sidebar_width = self.effective_sidebar_width();
        let content_top_inset = self.terminal_content_top_inset();
        let backend_mode = self.runtime_kind();
        let runtime_uses_tmux = matches!(backend_mode, RuntimeKind::Tmux);
        let active_pane_count = self
            .session
            .tabs
            .get(self.session.active_tab)
            .map_or(0, |tab| tab.panes.len());
        let total_sidebar_width = sidebar_width + self.workspace_sidebar_width();
        let resize_signature = self.terminal_resize_signature(
            viewport_width,
            viewport_height,
            cell_width,
            cell_height,
            total_sidebar_width,
            content_top_inset,
            backend_mode,
        );
        let apply_deferred_inactive_resize =
            self.deferred_inactive_resize.is_ready_for(resize_signature);
        if self.last_terminal_resize_signature == Some(resize_signature)
            && !apply_deferred_inactive_resize
        {
            return;
        }

        let (padding_x, padding_y) = self.effective_terminal_padding();
        let (cols, rows) = Self::terminal_grid_size_for_pane_count(
            active_pane_count,
            viewport_width,
            viewport_height,
            total_sidebar_width,
            content_top_inset,
            padding_x,
            padding_y,
            cell_width,
            cell_height,
        );

        match backend_mode {
            RuntimeKind::Tmux => {
                if (self.tmux_client_cols() != cols || self.tmux_client_rows() != rows)
                    && let Err(error) = self.sync_tmux_client_size(cols, rows)
                {
                    let now = Instant::now();
                    if self.should_emit_tmux_resize_error_toast(now) {
                        crate::ui::toast::error(format!("tmux resize failed: {error}"));
                    } else {
                        log::debug!("tmux resize failed (toast debounced): {error}");
                    }
                }
            }
            RuntimeKind::Native => {
                let mut tab_index = 0usize;
                while tab_index < self.session.tabs.len() {
                    let resize_tab =
                        tab_index == self.session.active_tab || apply_deferred_inactive_resize;
                    if !resize_tab {
                        tab_index += 1;
                        continue;
                    }
                    let (tab_id, pane_count, alternate_screen, should_sync) = {
                        let tab = &mut self.session.tabs[tab_index];
                        (
                            tab.id,
                            tab.panes.len(),
                            tab.single_pane_alternate_screen(),
                            Self::repair_native_tab_active_pane_for_resize(tab),
                        )
                    };
                    tab_index += 1;
                    if !should_sync {
                        continue;
                    }
                    let (padding_x, padding_y) = Self::terminal_padding_for_screen_mode(
                        self.padding_x,
                        self.padding_y,
                        alternate_screen,
                    );
                    let (cols, rows) = Self::terminal_grid_size_for_pane_count(
                        pane_count,
                        viewport_width,
                        viewport_height,
                        total_sidebar_width,
                        content_top_inset,
                        padding_x,
                        padding_y,
                        cell_width,
                        cell_height,
                    );
                    self.sync_native_tab_pane_geometry(tab_id, cols, rows);
                }
            }
        }

        // Throttle resize to avoid SIGWINCH flood during window drag.
        let can_resize = Self::can_apply_resize_at(now, self.last_resize_applied_at);
        let throttle_follow_up_delay =
            Self::resize_throttle_follow_up_delay(now, self.last_resize_applied_at);
        let mut needs_throttle_follow_up = false;

        for tab_index in 0..self.session.tabs.len() {
            if tab_index != self.session.active_tab && !apply_deferred_inactive_resize {
                continue;
            }
            let pane_count = self.session.tabs[tab_index].panes.len();
            let tab_uses_native_split_padding =
                Self::uses_native_split_content_padding(runtime_uses_tmux, pane_count);
            let (content_padding_x, content_padding_y) = if tab_uses_native_split_padding {
                (self.padding_x, self.padding_y)
            } else {
                (0.0, 0.0)
            };
            let use_local_zoom = backend_mode == RuntimeKind::Native && pane_count > 1;
            for pane_index in 0..pane_count {
                let pane_font_size = {
                    let pane = &self.session.tabs[tab_index].panes[pane_index];
                    self.effective_font_size_for_zoom_steps(pane.pane_zoom_steps, use_local_zoom)
                };
                let pane_cell_size = if backend_mode == RuntimeKind::Native {
                    self.calculate_cell_size_for_font_size(pane_font_size, window, cx)
                } else {
                    layout_cell_size
                };
                let pane = &self.session.tabs[tab_index].panes[pane_index];
                let mut pane_cols = pane.width.max(1);
                let mut pane_rows = pane.height.max(1);
                if content_padding_x > 0.0 || content_padding_y > 0.0 {
                    let pane_width_px = (f32::from(pane.width) * cell_width).max(cell_width);
                    let pane_height_px = (f32::from(pane.height) * cell_height).max(cell_height);
                    let pane_cell_width: f32 = pane_cell_size.width.into();
                    let pane_cell_height: f32 = pane_cell_size.height.into();
                    pane_cols = ((pane_width_px - (content_padding_x * 2.0)).max(pane_cell_width)
                        / pane_cell_width)
                        .floor()
                        .max(1.0) as u16;
                    pane_rows = ((pane_height_px - (content_padding_y * 2.0)).max(pane_cell_height)
                        / pane_cell_height)
                        .floor()
                        .max(1.0) as u16;
                }
                let next_size = TerminalSize {
                    cols: pane_cols,
                    rows: pane_rows,
                    cell_width: pane_cell_size.width.into(),
                    cell_height: pane_cell_size.height.into(),
                };
                let terminal = pane.terminal();
                let current = terminal.size();
                let needs_resize = current.cols != next_size.cols
                    || current.rows != next_size.rows
                    || current.cell_width != next_size.cell_width
                    || current.cell_height != next_size.cell_height;

                if needs_resize {
                    if can_resize {
                        terminal.resize(next_size);
                        self.last_resize_applied_at = Some(now);
                    } else {
                        needs_throttle_follow_up = true;
                    }
                }
            }
        }

        let final_resize_signature = self.terminal_resize_signature(
            viewport_width,
            viewport_height,
            cell_width,
            cell_height,
            total_sidebar_width,
            content_top_inset,
            backend_mode,
        );
        let has_inactive_terminal = self
            .session
            .tabs
            .iter()
            .enumerate()
            .any(|(tab_index, tab)| tab_index != self.session.active_tab && !tab.panes.is_empty());
        if apply_deferred_inactive_resize {
            self.deferred_inactive_resize = if needs_throttle_follow_up {
                DeferredTerminalResize::Ready {
                    signature: final_resize_signature,
                }
            } else {
                DeferredTerminalResize::Idle
            };
        } else if has_inactive_terminal {
            self.schedule_deferred_inactive_resize(final_resize_signature, cx);
        } else {
            self.deferred_inactive_resize = DeferredTerminalResize::Idle;
        }

        if needs_throttle_follow_up {
            self.last_terminal_resize_signature = None;
            self.schedule_resize_throttle_follow_up(throttle_follow_up_delay, cx);
        } else {
            self.last_terminal_resize_signature = Some(final_resize_signature);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_terminal() -> Terminal {
        Terminal::new_tmux(TerminalSize::default(), TerminalOptions::default())
    }

    fn test_pane(id: &str) -> TerminalPane {
        TerminalPane {
            id: id.to_string(),
            left: 0,
            top: 0,
            width: 1,
            height: 1,
            pane_zoom_steps: 0,
            degraded: false,
            tmux_mouse_mode: None,
            progress_state: ProgressState::default(),
            terminal: test_terminal(),
            render_cache: RefCell::new(TerminalPaneRenderCache::default()),
            last_alternate_screen: Cell::new(false),
            cached_element_ids: PaneCachedElementIds::new(id),
        }
    }

    fn test_resize_signature(active_tab_id: TabId) -> TerminalResizeSignature {
        TerminalResizeSignature {
            viewport_width_bits: 800.0_f32.to_bits(),
            viewport_height_bits: 600.0_f32.to_bits(),
            cell_width_bits: 10.0_f32.to_bits(),
            cell_height_bits: 20.0_f32.to_bits(),
            sidebar_width_bits: 0.0_f32.to_bits(),
            content_top_inset_bits: 32.0_f32.to_bits(),
            padding_x_bits: 8.0_f32.to_bits(),
            padding_y_bits: 8.0_f32.to_bits(),
            runtime_kind: RuntimeKind::Native,
            active_tab_id: Some(active_tab_id),
            pane_layout_fingerprint: 1,
        }
    }

    #[test]
    fn deferred_resize_keeps_one_deadline_and_rejects_stale_wakeups() {
        let first_signature = test_resize_signature(1);
        let second_signature = test_resize_signature(2);
        let first_deadline = Instant::now() + Duration::from_millis(120);
        let second_deadline = first_deadline + Duration::from_millis(16);
        let mut deferred = DeferredTerminalResize::Idle;

        assert!(deferred.schedule(first_signature, first_deadline));
        assert!(!deferred.schedule(first_signature, second_deadline));
        assert_eq!(
            deferred,
            DeferredTerminalResize::Pending {
                signature: first_signature,
                deadline: first_deadline,
            }
        );

        assert!(!deferred.schedule(second_signature, second_deadline));
        assert!(!deferred.mark_ready(first_signature, first_deadline));
        assert!(deferred.mark_ready(second_signature, second_deadline));
        assert!(deferred.is_ready_for(second_signature));
        assert!(!deferred.is_ready_for(first_signature));
    }

    #[test]
    fn terminal_grid_size_uses_outer_padding_only_for_single_pane_tabs() {
        let single_pane = TerminalView::terminal_grid_size_for_pane_count(
            1, 800.0, 600.0, 0.0, 32.0, 12.0, 8.0, 10.0, 20.0,
        );
        let split_pane = TerminalView::terminal_grid_size_for_pane_count(
            2, 800.0, 600.0, 0.0, 32.0, 12.0, 8.0, 10.0, 20.0,
        );

        assert_eq!(single_pane, (77, 27));
        assert_eq!(split_pane, (80, 28));
    }

    #[test]
    fn fullscreen_tui_preserves_configured_padding_on_entry_and_exit() {
        for native in [true, false] {
            let mut pane = test_pane("fullscreen-tui");
            if native {
                pane.terminal = Terminal::new_test_display(TerminalSize::default());
            }
            for (sequence, expected_padding, expected_size) in [
                (b"".as_slice(), (12.0, 8.0), (77, 27)),
                (b"\x1b[?1049h".as_slice(), (12.0, 8.0), (77, 27)),
                (b"\x1b[?1049l".as_slice(), (12.0, 8.0), (77, 27)),
            ] {
                pane.terminal().hydrate_output(sequence);
                pane.sync_alternate_screen_state();
                let padding = TerminalView::terminal_padding_for_screen_mode(
                    12.0,
                    8.0,
                    pane.last_alternate_screen.get(),
                );
                assert_eq!(padding, expected_padding);
                assert_eq!(
                    TerminalView::terminal_grid_size_for_pane_count(
                        1, 800.0, 600.0, 0.0, 32.0, padding.0, padding.1, 10.0, 20.0,
                    ),
                    expected_size,
                );
            }
        }
    }

    #[test]
    fn repair_native_tab_active_pane_for_resize_falls_back_to_first_pane() {
        let mut tab = TerminalTab {
            id: 1,
            window_id: "@native-1".to_string(),
            window_index: 0,
            panes: vec![test_pane("%native-1"), test_pane("%native-2")],
            active_pane_id: "%missing".to_string(),
            pinned: false,
            manual_title: None,
            explicit_title: None,
            explicit_title_is_prediction: false,
            shell_title: None,
            current_command: None,
            pending_command_title: None,
            pending_command_token: 0,
            last_prompt_cwd: None,
            title: DEFAULT_TAB_TITLE.to_string(),
            title_text_width: 0.0,
            sticky_title_width: 0.0,
            display_width: TAB_MIN_WIDTH,
            running_process: false,
            command_lifecycle: CommandLifecycle::default(),
        };

        assert!(TerminalView::repair_native_tab_active_pane_for_resize(
            &mut tab
        ));
        assert_eq!(tab.active_pane_id, "%native-1");
    }

    #[test]
    fn aggregate_progress_state_picks_highest_severity_across_panes() {
        let mut tab = TerminalTab {
            id: 1,
            window_id: "@native-1".to_string(),
            window_index: 0,
            panes: vec![
                test_pane("%native-1"),
                test_pane("%native-2"),
                test_pane("%native-3"),
            ],
            active_pane_id: "%native-1".to_string(),
            pinned: false,
            manual_title: None,
            explicit_title: None,
            explicit_title_is_prediction: false,
            shell_title: None,
            current_command: None,
            pending_command_title: None,
            pending_command_token: 0,
            last_prompt_cwd: None,
            title: DEFAULT_TAB_TITLE.to_string(),
            title_text_width: 0.0,
            sticky_title_width: 0.0,
            display_width: TAB_MIN_WIDTH,
            running_process: false,
            command_lifecycle: CommandLifecycle::default(),
        };

        assert_eq!(tab.aggregate_progress_state(), ProgressState::Clear);

        tab.panes[0].progress_state = ProgressState::InProgress(20);
        tab.panes[1].progress_state = ProgressState::InProgress(80);
        assert_eq!(
            tab.aggregate_progress_state(),
            ProgressState::InProgress(50)
        );

        tab.panes[2].progress_state = ProgressState::Indeterminate;
        assert_eq!(
            tab.aggregate_progress_state(),
            ProgressState::InProgress(50)
        );

        tab.panes[1].progress_state = ProgressState::Warning(80);
        assert_eq!(tab.aggregate_progress_state(), ProgressState::Warning(80));

        tab.panes[0].progress_state = ProgressState::Error(42);
        assert_eq!(tab.aggregate_progress_state(), ProgressState::Error(42));
    }

    #[test]
    fn aggregate_progress_state_falls_back_to_indeterminate() {
        let mut tab = TerminalTab {
            id: 1,
            window_id: "@native-1".to_string(),
            window_index: 0,
            panes: vec![test_pane("%native-1"), test_pane("%native-2")],
            active_pane_id: "%native-1".to_string(),
            pinned: false,
            manual_title: None,
            explicit_title: None,
            explicit_title_is_prediction: false,
            shell_title: None,
            current_command: None,
            pending_command_title: None,
            pending_command_token: 0,
            last_prompt_cwd: None,
            title: DEFAULT_TAB_TITLE.to_string(),
            title_text_width: 0.0,
            sticky_title_width: 0.0,
            display_width: TAB_MIN_WIDTH,
            running_process: false,
            command_lifecycle: CommandLifecycle::default(),
        };

        tab.panes[1].progress_state = ProgressState::Indeterminate;
        assert_eq!(tab.aggregate_progress_state(), ProgressState::Indeterminate);
    }

    #[test]
    fn sidebar_width_reduces_terminal_columns() {
        let viewport_width = 1280.0;
        let viewport_height = 800.0;
        let cell_width = 8.0;
        let cell_height = 16.0;
        let padding = 8.0;
        let content_top_inset = 32.0;
        let pane_count = 1;

        let (cols_no_sidebar, _) = TerminalView::terminal_grid_size_for_pane_count(
            pane_count,
            viewport_width,
            viewport_height,
            0.0,
            content_top_inset,
            padding,
            padding,
            cell_width,
            cell_height,
        );
        let (cols_with_sidebar, _) = TerminalView::terminal_grid_size_for_pane_count(
            pane_count,
            viewport_width,
            viewport_height,
            240.0,
            content_top_inset,
            padding,
            padding,
            cell_width,
            cell_height,
        );

        assert!(
            cols_with_sidebar < cols_no_sidebar,
            "non-zero sidebar must reduce column count (no_sidebar={cols_no_sidebar}, with_sidebar={cols_with_sidebar})"
        );
        assert_eq!(cols_no_sidebar - cols_with_sidebar, 30);
    }
}
