//! Desktop toast state and queueing.

use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

static NEXT_TOAST_ID: AtomicU64 = AtomicU64::new(0);

fn next_toast_id() -> u64 {
    NEXT_TOAST_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
    Loading,
}

impl ToastKind {
    pub fn icon_path(self) -> Option<&'static str> {
        match self {
            Self::Info => Some("icons/command_palette/info.svg"),
            Self::Success => Some("icons/check.svg"),
            Self::Warning => Some("icons/alert.svg"),
            Self::Error => Some("icons/close.svg"),
            Self::Loading => None,
        }
    }
}

/// Duration of the fade-in animation in milliseconds
pub const TOAST_FADE_IN_MS: u64 = 140;
/// Duration of the fade-out animation in milliseconds
pub const TOAST_FADE_OUT_MS: u64 = 180;
/// How far a toast drops from above on enter / exits upward. Keep this small —
/// toasts are high-frequency and a long travel feels like a banner, not a chip.
pub const TOAST_SLIDE_PX: f32 = 14.0;
/// Approximate stacked-toast slot used to ease existing toasts down when a
/// newer one appears at the top of the stack.
pub const TOAST_STACK_SLOT_PX: f32 = 54.0;

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub message: String,
    pub action_label: Option<String>,
    pub created_at: Instant,
    pub paused_at: Option<Instant>,
    pub paused_total: Duration,
    pub duration: Duration,
    stack_shift_from: f32,
    stack_shift_started_at: Option<Instant>,
}

impl Toast {
    fn elapsed(&self) -> Duration {
        let now = Instant::now();
        let total = now.duration_since(self.created_at);
        let active_pause = self
            .paused_at
            .map(|paused_at| now.duration_since(paused_at))
            .unwrap_or_default();
        total.saturating_sub(self.paused_total + active_pause)
    }

    /// Returns animation progress from 0.0 to 1.0 for fade-in/fade-out
    /// 0.0 = fully transparent, 1.0 = fully visible
    pub fn opacity(&self) -> f32 {
        let elapsed = self.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;

        if elapsed_ms < TOAST_FADE_IN_MS {
            return ease_out_cubic(elapsed_ms as f32 / TOAST_FADE_IN_MS as f32);
        }

        let remaining = self.duration.saturating_sub(elapsed);
        let remaining_ms = remaining.as_millis() as u64;

        if remaining_ms < TOAST_FADE_OUT_MS {
            return 1.0 - ease_in_cubic(1.0 - remaining_ms as f32 / TOAST_FADE_OUT_MS as f32);
        }

        1.0
    }

    /// Vertical offset for the stacked top-center layout.
    /// Negative values move the toast up: enter from above, exit upward, and
    /// existing toasts start one slot higher so they ease down for a new one.
    pub fn slide_offset(&self) -> f32 {
        self.edge_slide_offset() + self.stack_shift_offset()
    }

    fn edge_slide_offset(&self) -> f32 {
        let elapsed = self.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;

        if elapsed_ms < TOAST_FADE_IN_MS {
            let progress = elapsed_ms as f32 / TOAST_FADE_IN_MS as f32;
            return -TOAST_SLIDE_PX * (1.0 - ease_out_cubic(progress));
        }

        let remaining_ms = self.duration.saturating_sub(elapsed).as_millis() as u64;
        if remaining_ms < TOAST_FADE_OUT_MS {
            let exit_progress = 1.0 - remaining_ms as f32 / TOAST_FADE_OUT_MS as f32;
            return -TOAST_SLIDE_PX * ease_in_cubic(exit_progress);
        }

        0.0
    }

    fn stack_shift_offset(&self) -> f32 {
        let Some(started_at) = self.stack_shift_started_at else {
            return 0.0;
        };
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        if elapsed_ms >= TOAST_FADE_IN_MS {
            return 0.0;
        }
        let progress = elapsed_ms as f32 / TOAST_FADE_IN_MS as f32;
        self.stack_shift_from * (1.0 - ease_out_cubic(progress))
    }

    fn is_stack_shifting(&self) -> bool {
        self.stack_shift_started_at.is_some_and(|started_at| {
            started_at.elapsed().as_millis() < u128::from(TOAST_FADE_IN_MS)
        })
    }
}

#[derive(Clone, Debug)]
pub struct ToastRequest {
    pub kind: ToastKind,
    pub message: String,
    pub duration: Duration,
}

#[derive(Default)]
pub struct ToastManager {
    active: Vec<Toast>,
}

impl ToastManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active(&self) -> &[Toast] {
        &self.active
    }

    pub fn push(&mut self, request: ToastRequest) -> u64 {
        let id = next_toast_id();
        self.insert_newest(Toast {
            id,
            kind: request.kind,
            message: request.message,
            action_label: None,
            created_at: Instant::now(),
            paused_at: None,
            paused_total: Duration::ZERO,
            duration: request.duration,
            stack_shift_from: 0.0,
            stack_shift_started_at: None,
        });
        id
    }

    fn insert_newest(&mut self, toast: Toast) {
        let now = Instant::now();
        for existing in &mut self.active {
            let current = existing.stack_shift_offset();
            existing.stack_shift_from = current - TOAST_STACK_SLOT_PX;
            existing.stack_shift_started_at = Some(now);
        }
        self.active.insert(0, toast);
    }

    pub fn dismiss(&mut self, id: u64) {
        self.active.retain(|toast| toast.id != id);
    }

    /// Tick with optional hovered toast ID - hovered toasts don't expire
    pub fn tick_with_hovered(&mut self, hovered_id: Option<u64>) {
        let now = Instant::now();
        for toast in &mut self.active {
            let is_hovered = hovered_id == Some(toast.id);
            match (is_hovered, toast.paused_at) {
                (true, None) => {
                    // Refresh lifetime when hovering begins so repeated hover keeps the toast alive.
                    toast.created_at = now - Duration::from_millis(TOAST_FADE_IN_MS);
                    toast.paused_total = Duration::ZERO;
                    toast.paused_at = Some(now);
                }
                (false, Some(paused_at)) => {
                    toast.paused_total += now.duration_since(paused_at);
                    toast.paused_at = None;
                }
                _ => {}
            }
        }

        self.active.retain(|toast| toast.elapsed() < toast.duration);
    }

    pub fn ingest_pending(&mut self) {
        for request in drain_pending() {
            self.push(request);
        }
        for request in drain_pending_with_id() {
            self.push_with_id(request);
        }
        for update in drain_pending_updates() {
            self.apply_update(update);
        }
        for dismiss in drain_pending_dismisses() {
            self.dismiss(dismiss.id);
        }
    }

    pub fn push_with_id(&mut self, request: ToastRequestWithId) {
        self.insert_newest(Toast {
            id: request.id,
            kind: request.kind,
            message: request.message,
            action_label: request.action_label,
            created_at: Instant::now(),
            paused_at: None,
            paused_total: Duration::ZERO,
            duration: request.duration,
            stack_shift_from: 0.0,
            stack_shift_started_at: None,
        });
    }

    pub fn apply_update(&mut self, update: ToastUpdate) {
        if let Some(toast) = self.active.iter_mut().find(|t| t.id == update.id) {
            toast.kind = update.kind;
            toast.message = update.message;
            // Reset duration to default for non-loading toasts
            if update.kind != ToastKind::Loading {
                toast.duration = DEFAULT_TOAST_DURATION;
                // Reset created_at so it starts fresh timing from now
                toast.created_at = Instant::now();
                toast.paused_at = None;
                toast.paused_total = Duration::ZERO;
            }
        }
    }

    /// Returns true if any toast is currently animating (fade in, fade out, stack, or spinner)
    pub fn is_animating(&self) -> bool {
        self.active.iter().any(|toast| {
            if toast.kind == ToastKind::Loading || toast.is_stack_shifting() {
                return true;
            }

            let elapsed = toast.elapsed();
            let elapsed_ms = elapsed.as_millis() as u64;
            let remaining_ms = toast.duration.saturating_sub(elapsed).as_millis() as u64;

            elapsed_ms < TOAST_FADE_IN_MS || remaining_ms < TOAST_FADE_OUT_MS
        })
    }
}

const DEFAULT_TOAST_DURATION: Duration = Duration::from_millis(3000);

static TOAST_QUEUE: OnceLock<Mutex<Vec<ToastRequest>>> = OnceLock::new();

fn queue() -> &'static Mutex<Vec<ToastRequest>> {
    TOAST_QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn enqueue_toast(kind: ToastKind, message: impl Into<String>, duration: Option<Duration>) {
    let request = ToastRequest {
        kind,
        message: message.into(),
        duration: duration.unwrap_or(DEFAULT_TOAST_DURATION),
    };

    let mut queue = queue().lock().expect("toast queue lock poisoned");
    queue.push(request);
}

pub fn drain_pending() -> Vec<ToastRequest> {
    let mut queue = queue().lock().expect("toast queue lock poisoned");
    std::mem::take(&mut *queue)
}

pub fn info(message: impl Into<String>) {
    enqueue_toast(ToastKind::Info, message, None);
}

pub fn success(message: impl Into<String>) {
    enqueue_toast(ToastKind::Success, message, None);
}

pub fn warning(message: impl Into<String>) {
    enqueue_toast(ToastKind::Warning, message, None);
}

pub fn error(message: impl Into<String>) {
    enqueue_toast(ToastKind::Error, message, None);
}

/// Show a loading toast (stays indefinitely until updated or dismissed)
pub fn loading(message: impl Into<String>) -> u64 {
    enqueue_toast_with_id(
        ToastKind::Loading,
        message,
        Some(Duration::from_secs(60 * 60)), // 1 hour (effectively indefinite)
    )
}

/// Enqueue a toast and return its ID for later updates
pub fn enqueue_toast_with_id(
    kind: ToastKind,
    message: impl Into<String>,
    duration: Option<Duration>,
) -> u64 {
    enqueue_actionable_toast_with_id(kind, message, duration, None)
}

/// Enqueue a toast with an action button label, returning its ID.
pub fn enqueue_actionable_toast_with_id(
    kind: ToastKind,
    message: impl Into<String>,
    duration: Option<Duration>,
    action_label: Option<String>,
) -> u64 {
    let id = next_toast_id();

    let request = ToastRequestWithId {
        id,
        kind,
        message: message.into(),
        action_label,
        duration: duration.unwrap_or(DEFAULT_TOAST_DURATION),
    };

    let mut queue = pending_with_id().lock().expect("toast queue lock poisoned");
    queue.push(request);
    id
}

/// Update an existing toast's kind and message
pub fn update_toast(id: u64, kind: ToastKind, message: impl Into<String>) {
    let update = ToastUpdate {
        id,
        kind,
        message: message.into(),
    };
    let mut queue = pending_updates()
        .lock()
        .expect("toast update queue lock poisoned");
    queue.push(update);
}

#[derive(Clone, Debug)]
pub struct ToastRequestWithId {
    pub id: u64,
    pub kind: ToastKind,
    pub message: String,
    pub action_label: Option<String>,
    pub duration: Duration,
}

#[derive(Clone, Debug)]
pub struct ToastUpdate {
    pub id: u64,
    pub kind: ToastKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ToastDismiss {
    pub id: u64,
}

static TOAST_QUEUE_WITH_ID: OnceLock<Mutex<Vec<ToastRequestWithId>>> = OnceLock::new();
static TOAST_UPDATE_QUEUE: OnceLock<Mutex<Vec<ToastUpdate>>> = OnceLock::new();
static TOAST_DISMISS_QUEUE: OnceLock<Mutex<Vec<ToastDismiss>>> = OnceLock::new();

fn pending_with_id() -> &'static Mutex<Vec<ToastRequestWithId>> {
    TOAST_QUEUE_WITH_ID.get_or_init(|| Mutex::new(Vec::new()))
}

fn pending_updates() -> &'static Mutex<Vec<ToastUpdate>> {
    TOAST_UPDATE_QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

fn pending_dismisses() -> &'static Mutex<Vec<ToastDismiss>> {
    TOAST_DISMISS_QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn drain_pending_with_id() -> Vec<ToastRequestWithId> {
    let mut queue = pending_with_id().lock().expect("toast queue lock poisoned");
    std::mem::take(&mut *queue)
}

pub fn drain_pending_updates() -> Vec<ToastUpdate> {
    let mut queue = pending_updates()
        .lock()
        .expect("toast update queue lock poisoned");
    std::mem::take(&mut *queue)
}

pub fn dismiss_toast(id: u64) {
    let mut queue = pending_dismisses()
        .lock()
        .expect("toast dismiss queue lock poisoned");
    queue.push(ToastDismiss { id });
}

pub fn drain_pending_dismisses() -> Vec<ToastDismiss> {
    let mut queue = pending_dismisses()
        .lock()
        .expect("toast dismiss queue lock poisoned");
    std::mem::take(&mut *queue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismissing_loading_toast_keeps_follow_up_toast() {
        let mut manager = ToastManager::new();
        let loading_id = loading("Running plugin");
        success("Plugin finished");
        dismiss_toast(loading_id);

        manager.ingest_pending();

        assert_eq!(manager.active().len(), 1);
        assert_eq!(manager.active()[0].message, "Plugin finished");
    }

    fn test_toast(created_at: Instant, duration: Duration) -> Toast {
        Toast {
            id: 1,
            kind: ToastKind::Info,
            message: String::from("hello"),
            action_label: None,
            created_at,
            paused_at: None,
            paused_total: Duration::ZERO,
            duration,
            stack_shift_from: 0.0,
            stack_shift_started_at: None,
        }
    }

    #[test]
    fn slide_offset_enters_from_above() {
        let toast = test_toast(Instant::now(), DEFAULT_TOAST_DURATION);
        let offset = toast.slide_offset();
        assert!(offset <= 0.0);
        assert!(offset >= -TOAST_SLIDE_PX);
    }

    #[test]
    fn slide_offset_settles_after_fade_in() {
        let toast = test_toast(
            Instant::now() - Duration::from_millis(TOAST_FADE_IN_MS + 16),
            DEFAULT_TOAST_DURATION,
        );
        assert_eq!(toast.slide_offset(), 0.0);
    }

    #[test]
    fn slide_offset_exits_upward() {
        let created_at = Instant::now()
            - (DEFAULT_TOAST_DURATION - Duration::from_millis(TOAST_FADE_OUT_MS / 2));
        let toast = test_toast(created_at, DEFAULT_TOAST_DURATION);
        let offset = toast.slide_offset();
        assert!(offset < 0.0);
        assert!(offset >= -TOAST_SLIDE_PX);
    }

    #[test]
    fn newest_toast_stacks_at_the_top_and_shifts_existing_down() {
        let mut manager = ToastManager::new();
        manager.push(ToastRequest {
            kind: ToastKind::Info,
            message: String::from("first"),
            duration: DEFAULT_TOAST_DURATION,
        });
        manager.push(ToastRequest {
            kind: ToastKind::Info,
            message: String::from("second"),
            duration: DEFAULT_TOAST_DURATION,
        });

        assert_eq!(manager.active()[0].message, "second");
        assert_eq!(manager.active()[1].message, "first");
        assert!(manager.active()[1].stack_shift_offset() < 0.0);
        assert!(manager.active()[1].stack_shift_offset() >= -TOAST_STACK_SLOT_PX);
        assert!(manager.is_animating());
    }

    #[test]
    fn kind_icons_cover_every_non_loading_variant() {
        assert_eq!(
            ToastKind::Info.icon_path(),
            Some("icons/command_palette/info.svg")
        );
        assert_eq!(ToastKind::Success.icon_path(), Some("icons/check.svg"));
        assert_eq!(ToastKind::Warning.icon_path(), Some("icons/alert.svg"));
        assert_eq!(ToastKind::Error.icon_path(), Some("icons/close.svg"));
        assert_eq!(ToastKind::Loading.icon_path(), None);
    }
}
