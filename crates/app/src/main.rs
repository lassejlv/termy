#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod config;
mod find;
mod interaction;
mod keyboard_input;
mod performance;

mod resize_cadence;
mod session;
mod shortcuts;

use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use arboard::Clipboard;
use config::{AppConfig, prepare_config_file};
use engine::pty::{PtyCommand, pty_size};
use engine::{
    DynamicColor, MouseButton, MouseEvent, MouseEventKind, MousePointerShape, SearchDirection,
    SearchOptions, SelectionPoint, Terminal, TerminalEvent,
};
use find::FindState;
use interaction::{
    ClickTracker, MouseRoute, ScrollAccumulator, clear_mouse_state_on_focus_loss, motion_button,
    mouse_button_route, mouse_route, repeat_mouse_report, selected_text_for_clipboard,
};
use keyboard_input::{KeyboardState, map_key, map_modifiers};
use mux::{Client as MuxClient, DAEMON_ARGUMENT, TabRestore, TerminalSize};
use render::{MetalRenderer, RenderStatus, SearchInput};
use resize_cadence::ResizeFrameCadence;
use session::{MAX_TABS, StoredTab, directory_from_osc7, dynamic_color_index};
use shortcuts::{Shortcut, shortcut_for_character};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::{
        ElementState, Ime, KeyEvent, Modifiers as WinitModifiers, MouseButton as WinitMouseButton,
        MouseScrollDelta, WindowEvent,
    },
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, NamedKey},
    window::{CursorIcon, UserAttentionType, Window, WindowAttributes},
};

#[cfg(target_os = "macos")]
use winit::platform::macos::{
    ActiveEventLoopExtMacOS, OptionAsAlt, WindowAttributesExtMacOS, WindowExtMacOS,
};

#[derive(Clone, Debug)]
enum AppEvent {
    MultiplexerWake,
}

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const LIVE_RESIZE_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const NATIVE_TAB_GROUP: &str = "com.tmon.app.tabs";
#[cfg(target_os = "macos")]
const MACOS_OPTION_AS_ALT: OptionAsAlt = OptionAsAlt::None;

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let first_argument = arguments.next();
    if first_argument.as_deref() == Some(OsStr::new(DAEMON_ARGUMENT)) {
        let socket_path = arguments
            .next()
            .context("missing multiplexer socket path")?;
        if arguments.next().is_some() {
            bail!("unexpected arguments after multiplexer socket path");
        }
        return mux::serve(PathBuf::from(socket_path).as_path());
    }
    if first_argument.as_deref() == Some(OsStr::new(performance::BENCHMARK_ARGUMENT)) {
        return performance::run(arguments.collect());
    }

    let config = AppConfig::load()?;
    let command = command_from_arguments(&config)?;
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("creating macOS event loop")?;
    let proxy = event_loop.create_proxy();
    let mut application = Application::new(command, proxy, config);
    event_loop.run_app(&mut application).context("running Tmon")
}

fn terminal_window_attributes() -> WindowAttributes {
    let attributes = Window::default_attributes()
        .with_title("Tmon")
        .with_inner_size(LogicalSize::new(1000.0, 640.0))
        .with_min_inner_size(LogicalSize::new(320.0, 180.0));
    #[cfg(target_os = "macos")]
    {
        // Preserve macOS layout characters from either Option key. Tmon classifies an
        // unchanged Option chord as terminal Alt later, after Winit has retained the layout text.
        // A shared identifier asks AppKit to use its native window tab group.
        attributes
            .with_option_as_alt(MACOS_OPTION_AS_ALT)
            .with_tabbing_identifier(NATIVE_TAB_GROUP)
    }
    #[cfg(not(target_os = "macos"))]
    {
        attributes
    }
}

#[allow(clippy::struct_excessive_bools)]
struct Application {
    command: PtyCommand,
    config: AppConfig,
    proxy: EventLoopProxy<AppEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<MetalRenderer>,
    terminal: Option<Terminal>,
    multiplexer: Option<MuxClient>,
    modifiers: WinitModifiers,
    keyboard_state: KeyboardState,
    cursor_position: (f64, f64),
    pressed_mouse_button: Option<MouseButton>,
    selection_dragging: bool,
    click_tracker: ClickTracker,
    terminal_pointer_shape: MousePointerShape,
    find: FindState,
    terminal_title: String,
    current_directory: Option<PathBuf>,
    dynamic_colors: [Option<[u8; 3]>; 3],
    active_tab_id: u64,
    active_tab_index: usize,
    inactive_tabs: Vec<StoredTab>,
    pixel_scroll: ScrollAccumulator,
    line_scroll: ScrollAccumulator,
    clipboard: Option<Clipboard>,
    grid_dimensions: (usize, usize),
    terminal_size: Option<TerminalSize>,
    terminal_dirty: bool,
    force_full_frame: bool,
    pending_surface_size: Option<PhysicalSize<u32>>,
    pending_scale_factor: Option<f64>,
    resize_frames: ResizeFrameCadence,
    resize_grid_dirty: bool,
    cursor_blink_deadline: Instant,
    window_focused: bool,
}

impl Application {
    fn new(command: PtyCommand, proxy: EventLoopProxy<AppEvent>, config: AppConfig) -> Self {
        let now = Instant::now();
        let current_directory = command.working_directory.clone();
        Self {
            command,
            config,
            proxy,
            window: None,
            renderer: None,
            terminal: None,
            multiplexer: None,
            modifiers: WinitModifiers::default(),
            keyboard_state: KeyboardState::default(),
            cursor_position: (0.0, 0.0),
            pressed_mouse_button: None,
            selection_dragging: false,
            click_tracker: ClickTracker::default(),
            terminal_pointer_shape: MousePointerShape::Text,
            find: FindState::default(),
            terminal_title: "Tmon".to_owned(),
            current_directory,
            dynamic_colors: [None; 3],
            active_tab_id: 1,
            active_tab_index: 0,
            inactive_tabs: Vec::new(),
            pixel_scroll: ScrollAccumulator::default(),
            line_scroll: ScrollAccumulator::default(),
            clipboard: Clipboard::new().ok(),
            grid_dimensions: (0, 0),
            terminal_size: None,
            terminal_dirty: false,
            force_full_frame: false,
            pending_surface_size: None,
            pending_scale_factor: None,
            resize_frames: ResizeFrameCadence::new(now, LIVE_RESIZE_FRAME_INTERVAL),
            resize_grid_dirty: false,
            cursor_blink_deadline: now + CURSOR_BLINK_INTERVAL,
            window_focused: true,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        #[cfg(target_os = "macos")]
        event_loop.set_allows_automatic_window_tabbing(true);
        let window = Arc::new(
            event_loop
                .create_window(terminal_window_attributes())
                .context("creating Tmon window")?,
        );
        window.set_ime_allowed(true);
        window.set_cursor(CursorIcon::Text);
        let renderer = pollster::block_on(MetalRenderer::new(
            Arc::clone(&window),
            event_loop,
            self.config.renderer_config(),
        ))?;
        let (columns, rows) = renderer.grid_dimensions();
        let metrics = renderer.cell_metrics();
        let size = pty_size(columns, rows, metrics.width, metrics.height);
        let socket_path = mux::default_socket_path()?;
        let executable = std::env::current_exe().context("locating the Tmon executable")?;
        let mut multiplexer = MuxClient::connect_or_spawn(&socket_path, &executable)?;
        let mut restore = multiplexer.attach(
            &self.command,
            TerminalSize::from(size),
            self.config.scrollback_limit,
            self.config.inactive_scrollback_limit,
        )?;
        restore.tabs.sort_by_key(|tab| tab.index);
        if restore.tabs.is_empty() {
            bail!("multiplexer restored no tabs");
        }
        let mut restored_tabs = Vec::with_capacity(restore.tabs.len());
        for (position, tab) in restore.tabs.into_iter().enumerate() {
            let tab_window = if position == 0 {
                Arc::clone(&window)
            } else {
                Arc::new(
                    event_loop
                        .create_window(terminal_window_attributes().with_title(tab.title.clone()))
                        .context("restoring native terminal tab")?,
                )
            };
            tab_window.set_ime_allowed(true);
            tab_window.set_cursor(CursorIcon::Text);
            restored_tabs.push(Self::restore_tab(tab, tab_window)?);
        }
        let active_position = restored_tabs
            .iter()
            .position(|tab| tab.id == restore.active_tab_id)
            .context("multiplexer active tab is missing")?;
        let active_tab = restored_tabs.remove(active_position);
        let active_window = Arc::clone(&active_tab.window);

        self.grid_dimensions = (columns, rows);
        self.renderer = Some(renderer);
        self.multiplexer = Some(multiplexer);
        self.inactive_tabs = restored_tabs;
        self.install_tab(active_tab)?;
        active_window.focus_window();
        let proxy = self.proxy.clone();
        self.multiplexer
            .as_ref()
            .expect("multiplexer was just installed")
            .watch(move || {
                let _ = proxy.send_event(AppEvent::MultiplexerWake);
            })?;
        Ok(())
    }

    fn restore_tab(tab: TabRestore, window: Arc<Window>) -> Result<StoredTab> {
        Ok(StoredTab {
            id: tab.id,
            index: tab.index,
            window,
            terminal: Terminal::from_snapshot(&tab.terminal_snapshot)?,
            title: tab.title,
            pointer_shape: tab.pointer_shape,
            dynamic_colors: tab.dynamic_colors,
            current_directory: tab.current_directory,
        })
    }

    fn send(&mut self, bytes: &[u8]) {
        if let Some(multiplexer) = &mut self.multiplexer
            && let Err(error) = multiplexer.write(self.active_tab_id, bytes)
        {
            eprintln!("Tmon multiplexer write failed: {error:#}");
        }
    }

    fn process_terminal_output(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        terminal.feed(bytes);
        let events = terminal.drain_events();
        self.terminal_dirty = true;
        if self.find.is_active() {
            self.find.invalidate_match();
            self.refresh_search_input();
        }
        self.handle_terminal_events(events);
    }

    fn flush_terminal_frame(&mut self) {
        let now = Instant::now();
        if self.resize_frames.deadline().is_some() {
            if !self.resize_frames.take_due(now) {
                return;
            }
            self.apply_pending_resize();
            if std::mem::take(&mut self.resize_grid_dirty) {
                self.terminal_dirty = true;
                self.force_full_frame = true;
            }
        }
        if !self.terminal_dirty {
            return;
        }
        if self
            .renderer
            .as_ref()
            .is_some_and(MetalRenderer::is_occluded)
        {
            return;
        }
        self.terminal_dirty = false;
        let force_full = std::mem::take(&mut self.force_full_frame);
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        let update = terminal.frame_update(force_full);
        if update.has_damage() {
            self.cursor_blink_deadline = now + CURSOR_BLINK_INTERVAL;
        }
        if let Some(renderer) = &mut self.renderer {
            renderer.apply_frame(&update);
        }
    }

    fn apply_pending_resize(&mut self) {
        let scale_factor = self.pending_scale_factor.take();
        let surface_size = self.pending_surface_size.take();
        if scale_factor.is_none() && surface_size.is_none() {
            return;
        }
        if let Some(renderer) = &mut self.renderer {
            if let Some(scale_factor) = scale_factor {
                renderer.set_scale_factor(scale_factor);
            }
            if let Some(size) = surface_size {
                renderer.resize_surface(size);
            }
        }
        if self.sync_current_grid_geometry() {
            self.resize_grid_dirty = true;
            self.terminal_dirty = true;
        }
    }

    fn queue_surface_resize(&mut self, size: PhysicalSize<u32>) {
        self.pending_surface_size = Some(size);
        // Keep the emulator and child PTY correct for every distinct cell geometry. The surface
        // reconfiguration, retained-grid rebuild, and presentation remain paced to one per frame.
        if size.width > 0 && size.height > 0 {
            let geometry = self.renderer.as_ref().map(|renderer| {
                let dimensions = renderer.grid_dimensions_for_size(size);
                let metrics = renderer.cell_metrics();
                (dimensions, metrics)
            });
            if let Some((dimensions, metrics)) = geometry
                && self.sync_grid_geometry(dimensions, metrics.width, metrics.height)
            {
                self.resize_grid_dirty = true;
                self.terminal_dirty = true;
            }
        }
        self.resize_frames.queue(Instant::now());
    }

    fn feed_inactive_output(&mut self, position: usize, bytes: &[u8]) {
        let events = {
            let Some(tab) = self.inactive_tabs.get_mut(position) else {
                return;
            };
            tab.terminal.feed(bytes);
            tab.terminal.drain_events()
        };
        self.handle_inactive_terminal_events(position, events);
    }

    fn drain_multiplexer(&mut self) {
        let Some(multiplexer) = &mut self.multiplexer else {
            return;
        };
        let batch = match multiplexer.drain() {
            Ok(batch) => batch,
            Err(error) => {
                eprintln!("Tmon multiplexer drain failed: {error:#}");
                return;
            }
        };
        for tab in batch.resynchronized_tabs {
            if let Err(error) = self.resynchronize_tab(tab) {
                eprintln!("Tmon could not resynchronize a tab: {error:#}");
            }
        }
        for output in batch.outputs {
            if output.tab_id == self.active_tab_id {
                self.process_terminal_output(&output.bytes);
            } else if let Some(position) = self
                .inactive_tabs
                .iter()
                .position(|tab| tab.id == output.tab_id)
            {
                self.feed_inactive_output(position, &output.bytes);
            }
        }
    }

    fn resynchronize_tab(&mut self, restore: TabRestore) -> Result<()> {
        let terminal = Terminal::from_snapshot(&restore.terminal_snapshot)?;
        if restore.id == self.active_tab_id {
            self.terminal = Some(terminal);
            self.terminal_title = restore.title;
            self.terminal_pointer_shape = restore.pointer_shape;
            self.dynamic_colors = restore.dynamic_colors;
            self.current_directory = restore.current_directory;
            self.force_full_frame = true;
            self.terminal_dirty = true;
            if let Some(renderer) = &mut self.renderer {
                apply_dynamic_colors(renderer, self.dynamic_colors);
            }
            self.update_cursor_icon();
            self.refresh_window_title();
        } else if let Some(tab) = self
            .inactive_tabs
            .iter_mut()
            .find(|tab| tab.id == restore.id)
        {
            tab.terminal = terminal;
            tab.title = restore.title;
            tab.pointer_shape = restore.pointer_shape;
            tab.dynamic_colors = restore.dynamic_colors;
            tab.current_directory = restore.current_directory;
            tab.window.set_title(&tab.title);
        }
        Ok(())
    }

    fn handle_inactive_terminal_events(&mut self, position: usize, events: Vec<TerminalEvent>) {
        for event in events {
            match event {
                TerminalEvent::Bell => {
                    if let Some(tab) = self.inactive_tabs.get(position) {
                        tab.window
                            .request_user_attention(Some(UserAttentionType::Informational));
                    }
                }
                TerminalEvent::Title(title) => {
                    if let Some(tab) = self.inactive_tabs.get_mut(position) {
                        tab.title = if title.is_empty() {
                            "Tmon".to_owned()
                        } else {
                            title
                        };
                        tab.window.set_title(&tab.title);
                    }
                }
                TerminalEvent::ResetTitle => {
                    if let Some(tab) = self.inactive_tabs.get_mut(position) {
                        "Tmon".clone_into(&mut tab.title);
                        tab.window.set_title(&tab.title);
                    }
                }
                TerminalEvent::CurrentDirectory(uri) => {
                    if let Some(tab) = self.inactive_tabs.get_mut(position) {
                        tab.current_directory = directory_from_osc7(&uri);
                    }
                }
                TerminalEvent::ClipboardStore { text, .. } => {
                    if let Some(clipboard) = &mut self.clipboard {
                        let _ = clipboard.set_text(text);
                    }
                }
                TerminalEvent::MousePointerShape(shape) => {
                    if let Some(tab) = self.inactive_tabs.get_mut(position) {
                        tab.pointer_shape = shape;
                    }
                }
                TerminalEvent::SetDynamicColor { target, color } => {
                    if let Some(tab) = self.inactive_tabs.get_mut(position) {
                        tab.dynamic_colors[dynamic_color_index(target)] = Some(color);
                    }
                }
                TerminalEvent::ResetDynamicColor { target } => {
                    if let Some(tab) = self.inactive_tabs.get_mut(position) {
                        tab.dynamic_colors[dynamic_color_index(target)] = None;
                    }
                }
                // The daemon owns the authoritative emulator and sends protocol replies once.
                TerminalEvent::Reply(_) => {}
            }
        }
    }

    fn handle_terminal_events(&mut self, events: Vec<TerminalEvent>) {
        for event in events {
            match event {
                TerminalEvent::Bell => {
                    if let Some(window) = &self.window {
                        window.request_user_attention(Some(UserAttentionType::Critical));
                    }
                }
                TerminalEvent::Title(title) => {
                    self.terminal_title = if title.is_empty() {
                        "Tmon".to_owned()
                    } else {
                        title
                    };
                    self.refresh_window_title();
                }
                TerminalEvent::ResetTitle => {
                    "Tmon".clone_into(&mut self.terminal_title);
                    self.refresh_window_title();
                }
                TerminalEvent::CurrentDirectory(uri) => {
                    self.current_directory = directory_from_osc7(&uri);
                }
                TerminalEvent::ClipboardStore { text, .. } => {
                    if let Some(clipboard) = &mut self.clipboard {
                        let _ = clipboard.set_text(text);
                    }
                }
                TerminalEvent::MousePointerShape(shape) => {
                    self.terminal_pointer_shape = shape;
                    self.update_cursor_icon();
                }
                TerminalEvent::SetDynamicColor { target, color } => {
                    self.dynamic_colors[dynamic_color_index(target)] = Some(color);
                    if let Some(renderer) = &mut self.renderer {
                        renderer.set_dynamic_color(target, color);
                    }
                }
                TerminalEvent::ResetDynamicColor { target } => {
                    self.dynamic_colors[dynamic_color_index(target)] = None;
                    if let Some(renderer) = &mut self.renderer {
                        renderer.reset_dynamic_color(target);
                    }
                }
                // The daemon owns the authoritative emulator and sends protocol replies once.
                TerminalEvent::Reply(_) => {}
            }
        }
    }

    fn sync_current_grid_geometry(&mut self) -> bool {
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let dimensions = renderer.grid_dimensions();
        let metrics = renderer.cell_metrics();
        self.sync_grid_geometry(dimensions, metrics.width, metrics.height)
    }

    fn sync_grid_geometry(
        &mut self,
        dimensions: (usize, usize),
        cell_width: f32,
        cell_height: f32,
    ) -> bool {
        let size = pty_size(dimensions.0, dimensions.1, cell_width, cell_height);
        let terminal_size = TerminalSize::from(size);
        self.grid_dimensions = dimensions;
        let mut active_changed = false;
        if let Some(terminal) = &mut self.terminal {
            active_changed = terminal.dimensions() != dimensions;
            if active_changed {
                terminal.resize(dimensions.0, dimensions.1);
            }
            terminal.set_pixel_size(u32::from(size.pixel_width), u32::from(size.pixel_height));
        }
        for tab in &mut self.inactive_tabs {
            if tab.terminal.dimensions() != dimensions {
                tab.terminal.resize(dimensions.0, dimensions.1);
            }
            tab.terminal
                .set_pixel_size(u32::from(size.pixel_width), u32::from(size.pixel_height));
        }
        if self.terminal_size != Some(terminal_size)
            && let Some(multiplexer) = &mut self.multiplexer
        {
            match multiplexer.resize_all(terminal_size) {
                Ok(()) => self.terminal_size = Some(terminal_size),
                Err(error) => eprintln!("Tmon multiplexer resize failed: {error:#}"),
            }
        }
        active_changed
    }

    fn sync_grid_size(&mut self) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let dimensions = self
            .pending_surface_size
            .filter(|size| size.width > 0 && size.height > 0)
            .map_or_else(
                || renderer.grid_dimensions(),
                |size| renderer.grid_dimensions_for_size(size),
            );
        let metrics = renderer.cell_metrics();
        if !self.sync_grid_geometry(dimensions, metrics.width, metrics.height) {
            return;
        }
        if self.pending_surface_size.is_some() || self.pending_scale_factor.is_some() {
            self.resize_grid_dirty = true;
            self.terminal_dirty = true;
            self.resize_frames.queue(Instant::now());
            return;
        }
        self.resize_frames.cancel(Instant::now());
        if let (Some(terminal), Some(renderer)) = (&mut self.terminal, &mut self.renderer) {
            renderer.apply_frame(&terminal.frame_update(true));
            self.terminal_dirty = false;
            self.force_full_frame = false;
        }
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        if self.handle_shortcut(event_loop, event) {
            return;
        }
        if self.find.is_active() {
            self.handle_find_key(event);
            return;
        }
        let Some(mapped) = map_key(event, self.modifiers, &mut self.keyboard_state) else {
            return;
        };
        if let Some(terminal) = &mut self.terminal {
            let selection_cleared = terminal.clear_selection();
            let bytes = terminal.encode_key(&mapped);
            if selection_cleared {
                self.terminal_dirty = true;
            }
            self.send(&bytes);
        }
    }

    fn handle_shortcut(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) -> bool {
        if !self.modifiers.state().super_key() {
            return false;
        }
        let Key::Character(character) = event.logical_key.as_ref() else {
            return false;
        };
        let Some(shortcut) = shortcut_for_character(character) else {
            return false;
        };
        // Consume the matching release too. Otherwise an application using Kitty event-type
        // reporting receives a release for a shortcut press that Tmon handled locally.
        if !event.state.is_pressed() {
            return true;
        }
        match shortcut {
            Shortcut::Quit => event_loop.exit(),
            Shortcut::NewTab => self.new_tab(event_loop),
            Shortcut::CloseTab => {
                if let Some(window_id) = self.window.as_ref().map(|window| window.id()) {
                    self.close_native_tab(event_loop, window_id);
                }
            }
            Shortcut::PreviousTab => self.cycle_tab(-1),
            Shortcut::NextTab => self.cycle_tab(1),
            Shortcut::SelectTab(index) => self.switch_to_tab(index),
            Shortcut::OpenConfig => open_config(),
            Shortcut::Find => self.activate_find(),
            Shortcut::NextMatch => {
                if !self.find.is_active() {
                    self.find.activate();
                }
                let direction = if self.modifiers.state().shift_key() {
                    SearchDirection::Backward
                } else {
                    SearchDirection::Forward
                };
                self.search_find(direction);
            }
            Shortcut::Paste => {
                if self.find.is_active() {
                    if let Some(clipboard) = &mut self.clipboard
                        && let Ok(text) = clipboard.get_text()
                    {
                        self.append_find_text(&text);
                    }
                } else if let (Some(clipboard), Some(terminal)) =
                    (&mut self.clipboard, &mut self.terminal)
                    && let Ok(text) = clipboard.get_text()
                {
                    let selection_cleared = terminal.clear_selection();
                    let bytes = terminal.encode_paste(&text);
                    if selection_cleared {
                        self.terminal_dirty = true;
                    }
                    self.send(&bytes);
                }
            }
            Shortcut::ZoomIn => self.change_font_size(1.0),
            Shortcut::ZoomOut => self.change_font_size(-1.0),
            Shortcut::ResetFontSize => self.reset_font_size(),
            Shortcut::Copy => {
                if let (Some(clipboard), Some(terminal)) = (&mut self.clipboard, &self.terminal)
                    && let Some(text) = selected_text_for_clipboard(terminal)
                {
                    let _ = clipboard.set_text(text);
                }
            }
        }
        true
    }

    fn activate_find(&mut self) {
        self.find.activate();
        if self.find.query().is_empty() {
            if let Some(terminal) = &mut self.terminal
                && terminal.reset_search()
            {
                self.terminal_dirty = true;
            }
            self.refresh_window_title();
            self.refresh_search_input();
        } else {
            self.search_find(SearchDirection::Backward);
        }
    }

    fn close_find(&mut self) {
        self.find.close();
        if let Some(terminal) = &mut self.terminal
            && terminal.reset_search()
        {
            self.terminal_dirty = true;
        }
        self.refresh_search_input();
    }

    fn append_find_text(&mut self, text: &str) {
        if self.find.push(text) {
            self.search_find(SearchDirection::Backward);
        }
    }

    fn search_find(&mut self, direction: SearchDirection) {
        let query = self.find.query().to_owned();
        let found = self.terminal.as_mut().and_then(|terminal| {
            terminal.search_with_options(
                &query,
                SearchOptions {
                    direction,
                    ..SearchOptions::default()
                },
            )
        });
        self.find.set_has_match(found.is_some());
        self.terminal_dirty = true;
        self.refresh_search_input();
    }

    fn handle_find_key(&mut self, event: &KeyEvent) {
        if !event.state.is_pressed() {
            return;
        }
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => self.close_find(),
            Key::Named(NamedKey::Enter) => {
                let direction = if self.modifiers.state().shift_key() {
                    SearchDirection::Backward
                } else {
                    SearchDirection::Forward
                };
                self.search_find(direction);
            }
            Key::Named(NamedKey::Backspace) => {
                if self.find.pop() {
                    self.search_find(SearchDirection::Backward);
                }
            }
            Key::Character(character)
                if !self.modifiers.state().super_key() && !self.modifiers.state().control_key() =>
            {
                self.append_find_text(event.text.as_deref().unwrap_or(character));
            }
            _ => {}
        }
    }

    fn refresh_window_title(&self) {
        if let Some(window) = &self.window {
            window.set_title(&self.terminal_title);
        }
    }

    fn refresh_search_input(&mut self) {
        let input = self.find.is_active().then(|| SearchInput {
            query: self.find.query().to_owned(),
            has_match: self.find.has_match(),
        });
        if let Some(renderer) = &mut self.renderer {
            renderer.set_search_input(input);
        }
    }

    fn take_active_tab(&mut self) -> Option<StoredTab> {
        if self.window_focused {
            let focus = self
                .terminal
                .as_mut()
                .and_then(|terminal| terminal.focus_changed(false));
            if let Some(bytes) = focus {
                self.send(&bytes);
            }
        }
        let mut terminal = self.terminal.take()?;
        let window = self
            .window
            .take()
            .expect("an initialized terminal always owns a native window");
        terminal.set_scrollback_limit(self.config.inactive_scrollback_limit);
        Some(StoredTab {
            id: self.active_tab_id,
            index: self.active_tab_index,
            window,
            terminal,
            title: std::mem::take(&mut self.terminal_title),
            pointer_shape: self.terminal_pointer_shape,
            dynamic_colors: self.dynamic_colors,
            current_directory: self.current_directory.take(),
        })
    }

    fn install_tab(&mut self, mut tab: StoredTab) -> Result<()> {
        if let Some(multiplexer) = &mut self.multiplexer {
            multiplexer.activate_tab(tab.id)?;
        }
        if let Some(renderer) = &mut self.renderer {
            renderer.retarget_window(Arc::clone(&tab.window))?;
        }
        tab.terminal
            .set_scrollback_limit(self.config.scrollback_limit);
        self.window = Some(tab.window);
        self.active_tab_id = tab.id;
        self.active_tab_index = tab.index;
        self.terminal_title = tab.title;
        self.terminal_pointer_shape = tab.pointer_shape;
        self.dynamic_colors = tab.dynamic_colors;
        self.current_directory = tab.current_directory;
        self.terminal = Some(tab.terminal);
        self.sync_current_grid_geometry();
        if self.window_focused {
            let focus = self
                .terminal
                .as_mut()
                .and_then(|terminal| terminal.focus_changed(true));
            if let Some(bytes) = focus {
                self.send(&bytes);
            }
        }
        if let (Some(renderer), Some(terminal)) = (&mut self.renderer, &mut self.terminal) {
            apply_dynamic_colors(renderer, self.dynamic_colors);
            renderer.apply_frame(&terminal.frame_update(true));
        }
        self.terminal_dirty = false;
        self.force_full_frame = false;
        self.selection_dragging = false;
        self.pressed_mouse_button = None;
        self.click_tracker.reset();
        self.update_cursor_icon();
        self.refresh_window_title();
        Ok(())
    }

    fn spawn_tab(&mut self, event_loop: &ActiveEventLoop, index: usize) -> Result<StoredTab> {
        let renderer = self
            .renderer
            .as_ref()
            .context("renderer is not initialized")?;
        let window = Arc::new(
            event_loop
                .create_window(terminal_window_attributes())
                .context("creating native terminal tab")?,
        );
        window.set_ime_allowed(true);
        window.set_cursor(CursorIcon::Text);
        let metrics = renderer.cell_metrics();
        let dimensions = renderer.grid_dimensions_for_size(window.inner_size());
        let size = pty_size(dimensions.0, dimensions.1, metrics.width, metrics.height);
        let mut command = self.command.clone();
        if self
            .current_directory
            .as_ref()
            .is_some_and(|directory| directory.is_dir())
        {
            command
                .working_directory
                .clone_from(&self.current_directory);
        }
        let restore = self
            .multiplexer
            .as_mut()
            .context("multiplexer is not initialized")?
            .new_tab(
                &command,
                TerminalSize::from(size),
                self.config.scrollback_limit,
            )?;
        if restore.index != index {
            bail!(
                "multiplexer returned tab index {}, expected {index}",
                restore.index
            );
        }
        Self::restore_tab(restore, window)
    }

    fn new_tab(&mut self, event_loop: &ActiveEventLoop) {
        let count = self.inactive_tabs.len() + 1;
        if count >= MAX_TABS {
            return;
        }
        let Ok(tab) = self.spawn_tab(event_loop, count) else {
            eprintln!("Tmon could not create a new tab");
            return;
        };
        let window = Arc::clone(&tab.window);
        self.close_find();
        if let Some(active) = self.take_active_tab() {
            self.inactive_tabs.push(active);
        }
        if let Err(error) = self.install_tab(tab) {
            eprintln!("Tmon could not activate the new tab: {error:#}");
            event_loop.exit();
            return;
        }
        window.focus_window();
        window.request_redraw();
    }

    fn switch_to_tab(&self, index: usize) {
        if let Some(window) = &self.window {
            window.select_tab_at_index(index);
        }
    }

    fn cycle_tab(&self, amount: isize) {
        let Some(window) = &self.window else {
            return;
        };
        if amount.is_negative() {
            window.select_previous_tab();
        } else {
            window.select_next_tab();
        }
    }

    fn activate_native_tab(&mut self, window_id: winit::window::WindowId) -> Result<bool> {
        if self
            .window
            .as_ref()
            .is_some_and(|window| window.id() == window_id)
        {
            return Ok(false);
        }
        let Some(position) = self
            .inactive_tabs
            .iter()
            .position(|tab| tab.window.id() == window_id)
        else {
            return Ok(false);
        };
        self.close_find();
        let target = self.inactive_tabs.swap_remove(position);
        if let Some(active) = self.take_active_tab() {
            self.inactive_tabs.push(active);
        }
        self.install_tab(target)?;
        Ok(true)
    }

    fn close_current_tab(&mut self, event_loop: &ActiveEventLoop) {
        if self.inactive_tabs.is_empty() {
            event_loop.exit();
            return;
        }
        self.close_find();
        let closed_index = self.active_tab_index;
        if let Some(multiplexer) = &mut self.multiplexer
            && let Err(error) = multiplexer.close_tab(self.active_tab_id)
        {
            eprintln!("Tmon could not close the multiplexer tab: {error:#}");
            return;
        }
        let closed_window = self.window.take();
        self.terminal.take();
        for tab in &mut self.inactive_tabs {
            if tab.index > closed_index {
                tab.index -= 1;
            }
        }
        let desired = closed_index.min(self.inactive_tabs.len() - 1);
        let position = self
            .inactive_tabs
            .iter()
            .position(|tab| tab.index == desired)
            .expect("remaining tab indexes stay contiguous");
        let target = self.inactive_tabs.swap_remove(position);
        if let Err(error) = self.install_tab(target) {
            eprintln!("Tmon could not activate a remaining native tab: {error:#}");
            event_loop.exit();
        }
        drop(closed_window);
    }

    fn close_native_tab(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
    ) {
        if self
            .window
            .as_ref()
            .is_some_and(|window| window.id() == window_id)
        {
            self.close_current_tab(event_loop);
            return;
        }
        let Some(position) = self
            .inactive_tabs
            .iter()
            .position(|tab| tab.window.id() == window_id)
        else {
            return;
        };
        let closed_index = self.inactive_tabs[position].index;
        let tab = self.inactive_tabs.swap_remove(position);
        if let Some(multiplexer) = &mut self.multiplexer
            && let Err(error) = multiplexer.close_tab(tab.id)
        {
            eprintln!("Tmon could not close the multiplexer tab: {error:#}");
            self.inactive_tabs.push(tab);
            return;
        }
        drop(tab);
        for tab in &mut self.inactive_tabs {
            if tab.index > closed_index {
                tab.index -= 1;
            }
        }
        if self.active_tab_index > closed_index {
            self.active_tab_index -= 1;
        }
    }

    fn change_font_size(&mut self, amount: f32) {
        if let Some(renderer) = &mut self.renderer {
            renderer.set_font_size(renderer.font_size() + amount);
        }
        self.sync_grid_size();
    }

    fn reset_font_size(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            renderer.set_font_size(self.config.font_size);
        }
        self.sync_grid_size();
    }

    fn mouse_position(&self, clamped: bool) -> Option<(usize, usize, usize, usize)> {
        let renderer = self.renderer.as_ref()?;
        let (column, row) = if clamped {
            renderer.closest_cell_at(self.cursor_position.0, self.cursor_position.1)?
        } else {
            renderer.cell_at(self.cursor_position.0, self.cursor_position.1)?
        };
        let (pixel_x, pixel_y) = if clamped {
            renderer.closest_terminal_pixel_at(self.cursor_position.0, self.cursor_position.1)?
        } else {
            renderer.terminal_pixel_at(self.cursor_position.0, self.cursor_position.1)?
        };
        Some((column, row, pixel_x, pixel_y))
    }

    fn handle_mouse_button(&mut self, button: WinitMouseButton, state: ElementState) {
        let Some(button) = map_mouse_button(button) else {
            return;
        };
        let completing_drag =
            !state.is_pressed() && button == MouseButton::Left && self.selection_dragging;
        let completing_application_click =
            !state.is_pressed() && self.pressed_mouse_button.is_some();
        let Some((column, row, pixel_x, pixel_y)) =
            self.mouse_position(completing_drag || completing_application_click)
        else {
            if !state.is_pressed() {
                self.selection_dragging = false;
                self.pressed_mouse_button = None;
            }
            return;
        };
        let modifiers = map_modifiers(self.modifiers);
        if state.is_pressed()
            && button == MouseButton::Left
            && modifiers.contains(engine::Modifiers::SUPER)
            && let Some(hyperlink) = self
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.hyperlink_at(SelectionPoint { column, row }))
        {
            self.selection_dragging = false;
            self.pressed_mouse_button = None;
            self.click_tracker.reset();
            open_hyperlink(&hyperlink);
            return;
        }
        let mode = self
            .terminal
            .as_ref()
            .map_or(engine::MouseTrackingMode::Disabled, |terminal| {
                terminal.mouse_tracking_mode()
            });
        if mouse_button_route(
            mode,
            modifiers,
            self.selection_dragging,
            self.pressed_mouse_button.is_some(),
            button,
        ) == MouseRoute::Selection
            && button == MouseButton::Left
        {
            if let Some(terminal) = &mut self.terminal {
                let point = SelectionPoint { column, row };
                let changed = if state.is_pressed() {
                    self.selection_dragging = true;
                    let mode = self.click_tracker.register(Instant::now(), point);
                    terminal.begin_selection_with_mode(point, mode)
                } else if self.selection_dragging {
                    self.selection_dragging = false;
                    terminal.update_selection(point)
                } else {
                    false
                };
                if changed {
                    self.terminal_dirty = true;
                }
            }
            self.pressed_mouse_button = None;
            return;
        }

        self.click_tracker.reset();
        self.selection_dragging = false;
        self.pressed_mouse_button = if state.is_pressed() {
            Some(button)
        } else {
            None
        };
        let event = MouseEvent {
            button,
            kind: if state.is_pressed() {
                MouseEventKind::Press
            } else {
                MouseEventKind::Release
            },
            column,
            row,
            pixel_x,
            pixel_y,
            modifiers,
        };
        if let Some(terminal) = &self.terminal
            && let Some(bytes) = terminal.encode_mouse(event)
        {
            self.send(&bytes);
        }
    }

    fn handle_mouse_motion(&mut self) {
        let Some((column, row, pixel_x, pixel_y)) = self.mouse_position(self.selection_dragging)
        else {
            return;
        };
        if self.selection_dragging {
            if let Some(terminal) = &mut self.terminal
                && terminal.update_selection(SelectionPoint { column, row })
            {
                self.terminal_dirty = true;
            }
            return;
        }

        let Some(terminal) = &self.terminal else {
            return;
        };
        let modifiers = map_modifiers(self.modifiers);
        let button = motion_button(self.pressed_mouse_button);
        if mouse_button_route(
            terminal.mouse_tracking_mode(),
            modifiers,
            false,
            self.pressed_mouse_button.is_some(),
            button,
        ) != MouseRoute::Application
        {
            return;
        }
        let event = MouseEvent {
            button,
            kind: MouseEventKind::Motion,
            column,
            row,
            pixel_x,
            pixel_y,
            modifiers,
        };
        if let Some(bytes) = terminal.encode_mouse(event) {
            self.send(&bytes);
        }
    }

    fn update_cursor_icon(&self) {
        let hyperlink = self
            .mouse_position(false)
            .and_then(|(column, row, _, _)| {
                self.terminal
                    .as_ref()
                    .and_then(|terminal| terminal.hyperlink_at(SelectionPoint { column, row }))
            })
            .is_some();
        if let Some(window) = &self.window {
            window.set_cursor(if hyperlink {
                CursorIcon::Pointer
            } else {
                map_pointer_shape(self.terminal_pointer_shape)
            });
        }
    }

    fn handle_scroll(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, lines) => {
                self.pixel_scroll.reset();
                self.line_scroll.consume(f64::from(lines), 1.0)
            }
            MouseScrollDelta::PixelDelta(position) => {
                self.line_scroll.reset();
                let height = self
                    .renderer
                    .as_ref()
                    .map_or(16.0, |renderer| f64::from(renderer.cell_metrics().height));
                self.pixel_scroll.consume(position.y, height)
            }
        };
        if lines == 0 {
            return;
        }
        let modifiers = map_modifiers(self.modifiers);
        let route = self
            .terminal
            .as_ref()
            .map_or(MouseRoute::Selection, |terminal| {
                mouse_route(terminal.mouse_tracking_mode(), modifiers)
            });
        if route == MouseRoute::Application {
            let Some((column, row, pixel_x, pixel_y)) = self.mouse_position(false) else {
                return;
            };
            let button = if lines > 0 {
                MouseButton::WheelUp
            } else {
                MouseButton::WheelDown
            };
            let event = MouseEvent {
                button,
                kind: MouseEventKind::Press,
                column,
                row,
                pixel_x,
                pixel_y,
                modifiers,
            };
            if let Some(terminal) = &self.terminal
                && let Some(bytes) = terminal.encode_mouse(event)
            {
                let reports = repeat_mouse_report(&bytes, lines.unsigned_abs());
                self.send(&reports);
            }
        } else if let Some(terminal) = &mut self.terminal
            && terminal.scroll_display(lines)
        {
            self.terminal_dirty = true;
        }
    }
}

impl ApplicationHandler<AppEvent> for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(error) = self.initialize(event_loop)
        {
            eprintln!("Tmon failed to start: {error:#}");
            event_loop.exit();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        let AppEvent::MultiplexerWake = event;
        self.drain_multiplexer();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if matches!(&event, WindowEvent::Focused(true))
            && let Err(error) = self.activate_native_tab(window_id)
        {
            eprintln!("Tmon could not activate a native tab: {error:#}");
            event_loop.exit();
            return;
        }
        let is_active = self
            .window
            .as_ref()
            .is_some_and(|window| window.id() == window_id);
        if !is_active {
            if matches!(&event, WindowEvent::CloseRequested) {
                self.close_native_tab(event_loop, window_id);
            }
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.close_native_tab(event_loop, window_id),
            WindowEvent::Resized(size) => {
                self.queue_surface_resize(size);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.pending_scale_factor = Some(scale_factor);
                self.resize_frames.queue(Instant::now());
            }
            WindowEvent::RedrawRequested => {
                self.flush_terminal_frame();
                if self.resize_frames.deadline().is_some() {
                    return;
                }
                if let Some(renderer) = &mut self.renderer {
                    match renderer.render() {
                        Ok(RenderStatus::Retry) => {
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                        Ok(RenderStatus::Presented | RenderStatus::Occluded) => {}
                        Err(error) => eprintln!("Metal render failed: {error:#}"),
                    }
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.keyboard_state.modifiers_changed(modifiers);
                self.modifiers = modifiers;
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => self.handle_key(event_loop, &event),
            WindowEvent::Ime(Ime::Commit(text)) => {
                if text.is_empty() {
                    return;
                }
                if self.find.is_active() {
                    self.append_find_text(&text);
                } else if let Some(terminal) = &mut self.terminal {
                    let selection_cleared = terminal.clear_selection();
                    let bytes = terminal.encode_text(&text);
                    if selection_cleared {
                        self.terminal_dirty = true;
                    }
                    self.send(&bytes);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);
                self.handle_mouse_motion();
                self.update_cursor_icon();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(button, state);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_scroll(delta),
            WindowEvent::Focused(focused) => {
                self.window_focused = focused;
                if !focused {
                    self.modifiers = WinitModifiers::default();
                    self.keyboard_state.clear_held_modifiers();
                }
                if let Some(terminal) = &mut self.terminal
                    && let Some(bytes) = terminal.focus_changed(focused)
                {
                    self.send(&bytes);
                }
                clear_mouse_state_on_focus_loss(
                    focused,
                    &mut self.pressed_mouse_button,
                    &mut self.selection_dragging,
                );
                if !focused {
                    self.click_tracker.reset();
                }
            }
            WindowEvent::Occluded(occluded) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.set_occluded(occluded);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let now = Instant::now();
        let mut wake_deadline = self.resize_frames.deadline();
        if should_request_terminal_redraw(
            self.terminal_dirty,
            renderer.is_occluded(),
            wake_deadline,
            now,
        ) {
            renderer.window().request_redraw();
        }
        if renderer.cursor_should_blink() {
            if now >= self.cursor_blink_deadline {
                renderer.toggle_cursor_blink();
                self.cursor_blink_deadline = now + CURSOR_BLINK_INTERVAL;
            }
            wake_deadline = Some(
                wake_deadline.map_or(self.cursor_blink_deadline, |deadline| {
                    deadline.min(self.cursor_blink_deadline)
                }),
            );
        }
        event_loop
            .set_control_flow(wake_deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

fn should_request_terminal_redraw(
    terminal_dirty: bool,
    occluded: bool,
    resize_deadline: Option<Instant>,
    now: Instant,
) -> bool {
    terminal_dirty && !occluded && resize_deadline.is_none_or(|deadline| now >= deadline)
}

fn command_from_arguments(config: &AppConfig) -> Result<PtyCommand> {
    let mut arguments = std::env::args_os().skip(1);
    let working_directory = config
        .working_directory
        .clone()
        .map_or_else(std::env::current_dir, Ok)
        .context("reading current directory")?;
    if !working_directory.is_dir() {
        anyhow::bail!(
            "Tmon working directory does not exist or is not a directory: {}",
            working_directory.display()
        );
    }
    if let Some(program) = arguments.next() {
        Ok(PtyCommand::new(program)
            .with_arguments(arguments)
            .with_working_directory(working_directory))
    } else {
        let shell = config
            .shell
            .clone()
            .or_else(|| std::env::var_os("SHELL"))
            .unwrap_or_else(|| OsString::from("/bin/zsh"));
        Ok(PtyCommand::new(shell)
            .with_arguments(["-l"])
            .with_working_directory(working_directory))
    }
}

fn map_mouse_button(button: WinitMouseButton) -> Option<MouseButton> {
    match button {
        WinitMouseButton::Left => Some(MouseButton::Left),
        WinitMouseButton::Middle => Some(MouseButton::Middle),
        WinitMouseButton::Right => Some(MouseButton::Right),
        WinitMouseButton::Back | WinitMouseButton::Forward | WinitMouseButton::Other(_) => None,
    }
}

fn open_hyperlink(hyperlink: &str) {
    if !is_openable_hyperlink(hyperlink) {
        return;
    }
    #[cfg(target_os = "macos")]
    if let Err(error) = Command::new("/usr/bin/open").arg(hyperlink).spawn() {
        eprintln!("Tmon could not open hyperlink: {error}");
    }
}

fn open_config() {
    let path = match prepare_config_file() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Tmon could not prepare its config file: {error:#}");
            return;
        }
    };
    #[cfg(target_os = "macos")]
    if let Err(error) = Command::new("/usr/bin/open").arg(&path).spawn() {
        eprintln!("Tmon could not open config at {}: {error}", path.display());
    }
}

fn is_openable_hyperlink(hyperlink: &str) -> bool {
    let Some((scheme, _)) = hyperlink.split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto" | "file"
    )
}

fn apply_dynamic_colors(renderer: &mut MetalRenderer, colors: [Option<[u8; 3]>; 3]) {
    for (target, color) in [
        DynamicColor::Foreground,
        DynamicColor::Background,
        DynamicColor::Cursor,
    ]
    .into_iter()
    .zip(colors)
    {
        if let Some(color) = color {
            renderer.set_dynamic_color(target, color);
        } else {
            renderer.reset_dynamic_color(target);
        }
    }
}

const fn map_pointer_shape(shape: MousePointerShape) -> CursorIcon {
    match shape {
        MousePointerShape::Default => CursorIcon::Default,
        MousePointerShape::Pointer => CursorIcon::Pointer,
        MousePointerShape::Text => CursorIcon::Text,
        MousePointerShape::Crosshair => CursorIcon::Crosshair,
        MousePointerShape::Move => CursorIcon::Move,
        MousePointerShape::NotAllowed => CursorIcon::NotAllowed,
        MousePointerShape::Help => CursorIcon::Help,
        MousePointerShape::Progress => CursorIcon::Progress,
        MousePointerShape::Wait => CursorIcon::Wait,
        MousePointerShape::Cell => CursorIcon::Cell,
        MousePointerShape::VerticalText => CursorIcon::VerticalText,
        MousePointerShape::Alias => CursorIcon::Alias,
        MousePointerShape::Copy => CursorIcon::Copy,
        MousePointerShape::NoDrop => CursorIcon::NoDrop,
        MousePointerShape::Grab => CursorIcon::Grab,
        MousePointerShape::Grabbing => CursorIcon::Grabbing,
        MousePointerShape::EResize => CursorIcon::EResize,
        MousePointerShape::NResize => CursorIcon::NResize,
        MousePointerShape::NeResize => CursorIcon::NeResize,
        MousePointerShape::NwResize => CursorIcon::NwResize,
        MousePointerShape::SResize => CursorIcon::SResize,
        MousePointerShape::SeResize => CursorIcon::SeResize,
        MousePointerShape::SwResize => CursorIcon::SwResize,
        MousePointerShape::WResize => CursorIcon::WResize,
        MousePointerShape::EwResize => CursorIcon::EwResize,
        MousePointerShape::NsResize => CursorIcon::NsResize,
        MousePointerShape::NeswResize => CursorIcon::NeswResize,
        MousePointerShape::NwseResize => CursorIcon::NwseResize,
        MousePointerShape::ZoomIn => CursorIcon::ZoomIn,
        MousePointerShape::ZoomOut => CursorIcon::ZoomOut,
    }
}

#[cfg(test)]
mod app_tests {
    use std::time::{Duration, Instant};

    use super::{is_openable_hyperlink, should_request_terminal_redraw};

    #[cfg(target_os = "macos")]
    use super::{MACOS_OPTION_AS_ALT, OptionAsAlt};

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_preserves_layout_text_from_both_option_keys() {
        assert_eq!(MACOS_OPTION_AS_ALT, OptionAsAlt::None);
    }

    #[test]
    fn command_click_only_accepts_explicit_safe_hyperlink_schemes() {
        assert!(is_openable_hyperlink("https://example.com/path"));
        assert!(is_openable_hyperlink("MAILTO:user@example.com"));
        assert!(is_openable_hyperlink("file:///tmp/report.txt"));
        assert!(!is_openable_hyperlink("javascript:alert(1)"));
        assert!(!is_openable_hyperlink("-a Calculator"));
        assert!(!is_openable_hyperlink("relative/path"));
    }

    #[test]
    fn terminal_redraws_are_coalesced_until_the_next_display_opportunity() {
        let now = Instant::now();
        assert!(should_request_terminal_redraw(true, false, None, now));
        assert!(!should_request_terminal_redraw(false, false, None, now));
        assert!(!should_request_terminal_redraw(true, true, None, now));
        assert!(!should_request_terminal_redraw(
            true,
            false,
            Some(now + Duration::from_millis(8)),
            now,
        ));
        assert!(should_request_terminal_redraw(true, false, Some(now), now,));
    }
}
