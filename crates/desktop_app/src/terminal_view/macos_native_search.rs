//! AppKit find bar for native tab windows: an `NSSearchField` with the match
//! counter, Case/Regex toggles and match navigation on a glass panel in the
//! terminal's top-right corner.
//!
//! The bar only mirrors the GPUI search state. `search_input` stays the query
//! of record: AppKit edits arrive as [`NativeSearchEvent`]s, and changes made
//! on the GPUI side (a selection prefill, Cmd-F, the menu) come back through
//! [`NativeSearchBar::apply`].
//!
//! Like the native tabs, every AppKit change runs on the main queue after the
//! current GPUI update: moving the first responder makes AppKit call back
//! into GPUI right away, which GPUI cannot take mid-update.

use super::search::SearchEditCommand;
use cocoa::{
    base::{BOOL, NO, YES, id, nil},
    foundation::{NSArray, NSAutoreleasePool as _, NSString},
};
use flume::{Receiver, Sender};
use gpui_kit::Window;
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Class, Object, Sel},
    sel, sel_impl,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ffi::{CStr, c_void},
    ptr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

const DELEGATE_STATE_IVAR: &str = "termyNativeSearchState";
static DELEGATE_CLASS: OnceLock<usize> = OnceLock::new();

const PANEL_WIDTH: f64 = 460.0;
const PANEL_HEIGHT: f64 = 40.0;
const PANEL_INSET: f64 = 10.0;
const GLASS_CORNER_RADIUS: f64 = 14.0;
const MATERIAL_CORNER_RADIUS: f64 = 10.0;

const MATCH_CASE_SEGMENT: isize = 0;
const REGEX_SEGMENT: isize = 1;
const NEXT_SEGMENT: isize = 0;

const NS_WINDOW_ABOVE: isize = 1;
const NS_SEGMENT_SWITCH_TRACKING_SELECT_ANY: usize = 1;
const NS_SEGMENT_SWITCH_TRACKING_MOMENTARY: usize = 2;
const NS_USER_INTERFACE_LAYOUT_ORIENTATION_HORIZONTAL: isize = 0;
const NS_LAYOUT_ATTRIBUTE_CENTER_Y: isize = 10;
const NS_LAYOUT_CONSTRAINT_ORIENTATION_HORIZONTAL: isize = 0;
const NS_LAYOUT_PRIORITY_DEFAULT_LOW: f32 = 250.0;
const NS_LAYOUT_PRIORITY_DEFAULT_HIGH: f32 = 750.0;
const NS_EVENT_MODIFIER_FLAG_SHIFT: usize = 1 << 17;
const NS_VISUAL_EFFECT_MATERIAL_POPOVER: isize = 6;
const NS_VISUAL_EFFECT_BLENDING_MODE_WITHIN_WINDOW: isize = 1;
const NS_VISUAL_EFFECT_STATE_ACTIVE: isize = 1;

/// What the user did in the AppKit bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NativeSearchEvent {
    QueryChanged(String),
    Next,
    Previous,
    Close,
    Options { case_sensitive: bool, regex: bool },
}

/// What the AppKit bar should show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NativeSearchBarState {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) status: Option<String>,
    pub(crate) invalid_pattern: bool,
    pub(crate) case_sensitive: bool,
    pub(crate) regex: bool,
    /// Bumped to move keyboard focus into the field with the query selected.
    pub(crate) focus_request: u64,
}

pub(crate) struct NativeSearchBar {
    views: BarViews,
    applied: NativeSearchBarState,
}

impl NativeSearchBar {
    /// Build the (hidden) bar for the window's GPUI view. Returns `None` when
    /// the window has no AppKit view; the caller keeps Termy's own bar then.
    pub(crate) fn new(window: &Window) -> Option<(Self, Receiver<NativeSearchEvent>)> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        let host = handle.ns_view.as_ptr().cast::<Object>() as id;
        let (sender, receiver) = flume::unbounded();
        // SAFETY: Runs on the main thread while the borrowed window keeps
        // `host` alive. The new views are not in a window yet, so building
        // them cannot call back into GPUI.
        let views = unsafe { BarViews::build(host, sender) };
        Some((
            Self {
                views,
                applied: NativeSearchBarState::default(),
            },
            receiver,
        ))
    }

    /// Record a query that came from the field itself, so `apply` does not
    /// write it back and move the field's cursor.
    pub(crate) fn note_query(&mut self, query: &str) {
        query.clone_into(&mut self.applied.query);
    }

    pub(crate) fn apply(&mut self, state: NativeSearchBarState) {
        if state == self.applied {
            return;
        }
        let previous = std::mem::replace(&mut self.applied, state.clone());
        let views = self.views.clone();
        on_main_queue(move || {
            // SAFETY: The views are retained and messaged on the main thread.
            unsafe { views.apply(&previous, &state) }
        });
    }

    /// Give keyboard focus back to the terminal when the bar holds it, for
    /// another input (the command palette) that GPUI draws.
    pub(crate) fn release_focus(&self) {
        let views = self.views.clone();
        on_main_queue(move || {
            // SAFETY: The views are retained and messaged on the main thread.
            unsafe { views.return_focus_to_host() }
        });
    }

    /// Hand an edit shortcut to the field while it has keyboard focus. GPUI
    /// sees Cmd and Ctrl shortcuts first and would otherwise apply them to
    /// its own copy of the query.
    ///
    /// Runs right away, in order with the keys typed into the field: the
    /// field editor only reports back through the event channel, never into
    /// GPUI.
    pub(crate) fn forward_edit_command(&self, command: SearchEditCommand) -> bool {
        // SAFETY: The field is retained and messaged on the main thread.
        unsafe {
            let editor: id = msg_send![self.views.field.id(), currentEditor];
            if editor == nil {
                return false;
            }
            let _: BOOL = msg_send![editor, tryToPerform: responder_action(command) with: nil];
        }
        true
    }
}

/// The `NSResponder` action the field editor runs for an edit shortcut.
fn responder_action(command: SearchEditCommand) -> Sel {
    match command {
        SearchEditCommand::Copy => sel!(copy:),
        SearchEditCommand::Cut => sel!(cut:),
        SearchEditCommand::Undo => sel!(undo:),
        SearchEditCommand::Redo => sel!(redo:),
        SearchEditCommand::Paste => sel!(paste:),
        SearchEditCommand::SelectAll => sel!(selectAll:),
        SearchEditCommand::DeleteBackward => sel!(deleteBackward:),
        SearchEditCommand::DeleteForward => sel!(deleteForward:),
        SearchEditCommand::MoveLeft => sel!(moveLeft:),
        SearchEditCommand::MoveRight => sel!(moveRight:),
        SearchEditCommand::SelectLeft => sel!(moveLeftAndModifySelection:),
        SearchEditCommand::SelectRight => sel!(moveRightAndModifySelection:),
        SearchEditCommand::MoveToStart => sel!(moveToBeginningOfLine:),
        SearchEditCommand::MoveToEnd => sel!(moveToEndOfLine:),
        SearchEditCommand::DeleteWordBackward => sel!(deleteWordBackward:),
        SearchEditCommand::DeleteWordForward => sel!(deleteWordForward:),
        SearchEditCommand::DeleteToStart => sel!(deleteToBeginningOfLine:),
        SearchEditCommand::DeleteToEnd => sel!(deleteToEndOfLine:),
    }
}

impl Drop for NativeSearchBar {
    fn drop(&mut self) {
        let views = self.views.clone();
        on_main_queue(move || {
            // SAFETY: The views are retained and messaged on the main thread.
            unsafe { views.detach() }
        });
    }
}

/// The AppKit objects behind one bar.
#[derive(Clone)]
struct BarViews {
    host: Retained,
    panel: Retained,
    field: Retained,
    status: Retained,
    options: Retained,
    navigation: Retained,
    done: Retained,
    /// Controls only hold their target and delegate weakly.
    _delegate: Retained,
    /// Whether the panel is in the window yet. Set on the main queue once
    /// attaching succeeds, so a failed attach is retried on the next open.
    attached: Arc<AtomicBool>,
}

impl BarViews {
    /// # Safety
    /// Main thread only; `host` must be a live `NSView`.
    unsafe fn build(host: id, sender: Sender<NativeSearchEvent>) -> Self {
        // SAFETY: Upheld by the caller.
        unsafe {
            let delegate: id = msg_send![delegate_class(), new];
            let state = Box::into_raw(Box::new(DelegateState { sender })) as *mut c_void;
            (*delegate).set_ivar(DELEGATE_STATE_IVAR, state);

            let field: id = msg_send![class!(NSSearchField), new];
            let _: () = msg_send![field, setPlaceholderString: ns_string("Find in terminal")];
            let _: () = msg_send![field, setDelegate: delegate];
            // The clear button only reports through the action on some macOS
            // versions, not through `controlTextDidChange:`.
            let _: () = msg_send![field, setTarget: delegate];
            let _: () = msg_send![field, setAction: sel!(searchFieldChanged:)];
            let _: () = msg_send![field, setSendsSearchStringImmediately: YES];
            let _: () = msg_send![field, setContentHuggingPriority: NS_LAYOUT_PRIORITY_DEFAULT_LOW
                forOrientation: NS_LAYOUT_CONSTRAINT_ORIENTATION_HORIZONTAL];

            let status: id = msg_send![class!(NSTextField), labelWithString: ns_string("")];
            let font: id =
                msg_send![class!(NSFont), monospacedDigitSystemFontOfSize: 11.0f64 weight: 0.0f64];
            let _: () = msg_send![status, setFont: font];
            let _: () = msg_send![status, setTextColor: status_color(false)];
            let _: () = msg_send![status, setHidden: YES];

            let labels = NSArray::arrayWithObjects(nil, &[ns_string("Aa"), ns_string(".*")]);
            let options: id = msg_send![class!(NSSegmentedControl),
                segmentedControlWithLabels: labels
                trackingMode: NS_SEGMENT_SWITCH_TRACKING_SELECT_ANY
                target: delegate
                action: sel!(optionsChanged:)];
            let _: () = msg_send![options, setToolTip: ns_string("Match Case") forSegment: MATCH_CASE_SEGMENT];
            let _: () = msg_send![options, setToolTip: ns_string("Regular Expression") forSegment: REGEX_SEGMENT];

            // Matches run newest first, so "next" walks up the scrollback.
            let images = NSArray::arrayWithObjects(
                nil,
                &[
                    symbol("chevron.up", "Next Match"),
                    symbol("chevron.down", "Previous Match"),
                ],
            );
            let navigation: id = msg_send![class!(NSSegmentedControl),
                segmentedControlWithImages: images
                trackingMode: NS_SEGMENT_SWITCH_TRACKING_MOMENTARY
                target: delegate
                action: sel!(navigate:)];
            let _: () = msg_send![navigation, setToolTip: ns_string("Next Match (↵)") forSegment: NEXT_SEGMENT];
            let _: () = msg_send![navigation, setToolTip: ns_string("Previous Match (⇧↵)") forSegment: 1isize];

            let done: id = msg_send![class!(NSButton),
                buttonWithTitle: ns_string("Done")
                target: delegate
                action: sel!(done:)];

            for view in [status, options, navigation, done] {
                let _: () = msg_send![view, setContentHuggingPriority: NS_LAYOUT_PRIORITY_DEFAULT_HIGH
                    forOrientation: NS_LAYOUT_CONSTRAINT_ORIENTATION_HORIZONTAL];
            }

            let arranged =
                NSArray::arrayWithObjects(nil, &[field, status, options, navigation, done]);
            let stack: id = msg_send![class!(NSStackView), stackViewWithViews: arranged];
            let _: () =
                msg_send![stack, setOrientation: NS_USER_INTERFACE_LAYOUT_ORIENTATION_HORIZONTAL];
            let _: () = msg_send![stack, setAlignment: NS_LAYOUT_ATTRIBUTE_CENTER_Y];
            let _: () = msg_send![stack, setSpacing: 8.0f64];
            let _: () = msg_send![stack, setEdgeInsets: EdgeInsets {
                top: 6.0,
                left: 8.0,
                bottom: 6.0,
                right: 8.0,
            }];

            let panel = glass_panel(stack).unwrap_or_else(|| material_panel(stack));
            let _: () = msg_send![panel, setHidden: YES];

            Self {
                host: Retained::retain(host),
                panel: Retained::adopt(panel),
                field: Retained::adopt(field),
                status: Retained::retain(status),
                options: Retained::retain(options),
                navigation: Retained::retain(navigation),
                done: Retained::retain(done),
                _delegate: Retained::adopt(delegate),
                attached: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    /// # Safety
    /// Main thread only.
    unsafe fn apply(&self, previous: &NativeSearchBarState, state: &NativeSearchBarState) {
        // SAFETY: Upheld by the caller.
        unsafe {
            if !self.attached.load(Ordering::Relaxed) {
                if !state.open {
                    return;
                }
                if !self.attach() {
                    return;
                }
                self.attached.store(true, Ordering::Relaxed);
                // Nothing reached the views before, so apply everything.
                self.apply(&NativeSearchBarState::default(), state);
                return;
            }
            if state.query != previous.query {
                let _: () = msg_send![self.field.id(), setStringValue: ns_string(&state.query)];
            }
            if state.status != previous.status || state.invalid_pattern != previous.invalid_pattern
            {
                let label = state.status.as_deref().unwrap_or_default();
                let _: () = msg_send![self.status.id(), setStringValue: ns_string(label)];
                let _: () =
                    msg_send![self.status.id(), setTextColor: status_color(state.invalid_pattern)];
                let hidden = if state.status.is_some() { NO } else { YES };
                let _: () = msg_send![self.status.id(), setHidden: hidden];
            }
            if state.case_sensitive != previous.case_sensitive {
                let selected = if state.case_sensitive { YES } else { NO };
                let _: () = msg_send![self.options.id(), setSelected: selected forSegment: MATCH_CASE_SEGMENT];
            }
            if state.regex != previous.regex {
                let selected = if state.regex { YES } else { NO };
                let _: () =
                    msg_send![self.options.id(), setSelected: selected forSegment: REGEX_SEGMENT];
            }
            if state.open != previous.open {
                let hidden = if state.open { NO } else { YES };
                if !state.open {
                    self.return_focus_to_host();
                }
                let _: () = msg_send![self.panel.id(), setHidden: hidden];
            }
            if state.open && state.focus_request != previous.focus_request {
                let _: () = msg_send![self.field.id(), selectText: nil];
            }
        }
    }

    /// Float the panel over the GPUI view's top-right corner. Returns false
    /// when the GPUI view has left its window.
    ///
    /// # Safety
    /// Main thread only.
    unsafe fn attach(&self) -> bool {
        // SAFETY: Upheld by the caller.
        unsafe {
            let host = self.host.id();
            let panel = self.panel.id();
            let superview: id = msg_send![host, superview];
            if superview == nil {
                return false;
            }
            let _: () = msg_send![panel, setTranslatesAutoresizingMaskIntoConstraints: NO];
            let _: () = msg_send![superview, addSubview: panel positioned: NS_WINDOW_ABOVE relativeTo: host];

            let top: id = msg_send![panel, topAnchor];
            let host_top: id = msg_send![host, topAnchor];
            activate(msg_send![top, constraintEqualToAnchor: host_top constant: PANEL_INSET]);
            let right: id = msg_send![panel, rightAnchor];
            let host_right: id = msg_send![host, rightAnchor];
            activate(msg_send![right, constraintEqualToAnchor: host_right constant: -PANEL_INSET]);
            let left: id = msg_send![panel, leftAnchor];
            let host_left: id = msg_send![host, leftAnchor];
            activate(
                msg_send![left, constraintGreaterThanOrEqualToAnchor: host_left constant: PANEL_INSET],
            );
            let height: id = msg_send![panel, heightAnchor];
            activate(msg_send![height, constraintEqualToConstant: PANEL_HEIGHT]);
            // Narrow windows shrink the field rather than push the panel off
            // the left edge.
            let width: id = msg_send![panel, widthAnchor];
            let width: id = msg_send![width, constraintEqualToConstant: PANEL_WIDTH];
            let _: () = msg_send![width, setPriority: NS_LAYOUT_PRIORITY_DEFAULT_HIGH];
            activate(width);
            true
        }
    }

    /// Whether keyboard focus is anywhere in the panel: the field (through
    /// its field editor) or one of the buttons.
    ///
    /// # Safety
    /// Main thread only.
    unsafe fn panel_has_focus(&self) -> bool {
        // SAFETY: Upheld by the caller.
        unsafe {
            let editor: id = msg_send![self.field.id(), currentEditor];
            if editor != nil {
                return true;
            }
            let window: id = msg_send![self.host.id(), window];
            if window == nil {
                return false;
            }
            let responder: id = msg_send![window, firstResponder];
            if responder == nil {
                return false;
            }
            let is_view: BOOL = msg_send![responder, isKindOfClass: class!(NSView)];
            if is_view == NO {
                return false;
            }
            let inside: BOOL = msg_send![responder, isDescendantOf: self.panel.id()];
            inside == YES
        }
    }

    /// Make the GPUI view first responder again when the panel holds focus.
    /// GPUI never takes it back on its own: its view does not accept first
    /// responder on click.
    ///
    /// # Safety
    /// Main thread only.
    unsafe fn return_focus_to_host(&self) {
        // SAFETY: Upheld by the caller.
        unsafe {
            if !self.panel_has_focus() {
                return;
            }
            let window: id = msg_send![self.host.id(), window];
            if window != nil {
                let _: BOOL = msg_send![window, makeFirstResponder: self.host.id()];
            }
        }
    }

    /// Take the panel out of the window and drop the delegate's targets, so
    /// no control messages a delegate that is about to go away.
    ///
    /// # Safety
    /// Main thread only.
    unsafe fn detach(&self) {
        // SAFETY: Upheld by the caller.
        unsafe {
            self.return_focus_to_host();
            let _: () = msg_send![self.field.id(), setDelegate: nil];
            let _: () = msg_send![self.field.id(), setTarget: nil];
            for control in [&self.options, &self.navigation, &self.done] {
                let _: () = msg_send![control.id(), setTarget: nil];
            }
            let _: () = msg_send![self.panel.id(), removeFromSuperview];
        }
    }
}

#[repr(C)]
struct EdgeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}

/// Liquid Glass (`NSGlassEffectView`, macOS 26 and later).
unsafe fn glass_panel(content: id) -> Option<id> {
    let class = Class::get("NSGlassEffectView")?;
    // SAFETY: Upheld by `BarViews::build`.
    unsafe {
        let panel: id = msg_send![class, new];
        let _: () = msg_send![panel, setCornerRadius: GLASS_CORNER_RADIUS];
        let _: () = msg_send![panel, setContentView: content];
        Some(panel)
    }
}

/// A popover-material panel for macOS before 26.
unsafe fn material_panel(content: id) -> id {
    // SAFETY: Upheld by `BarViews::build`.
    unsafe {
        let panel: id = msg_send![class!(NSVisualEffectView), new];
        let _: () = msg_send![panel, setMaterial: NS_VISUAL_EFFECT_MATERIAL_POPOVER];
        let _: () = msg_send![panel, setBlendingMode: NS_VISUAL_EFFECT_BLENDING_MODE_WITHIN_WINDOW];
        let _: () = msg_send![panel, setState: NS_VISUAL_EFFECT_STATE_ACTIVE];
        let _: () = msg_send![panel, setWantsLayer: YES];
        let layer: id = msg_send![panel, layer];
        let _: () = msg_send![layer, setCornerRadius: MATERIAL_CORNER_RADIUS];
        let _: () = msg_send![layer, setMasksToBounds: YES];

        let _: () = msg_send![content, setTranslatesAutoresizingMaskIntoConstraints: NO];
        let _: () = msg_send![panel, addSubview: content];
        let left: id = msg_send![content, leftAnchor];
        let panel_left: id = msg_send![panel, leftAnchor];
        activate(msg_send![left, constraintEqualToAnchor: panel_left]);
        let right: id = msg_send![content, rightAnchor];
        let panel_right: id = msg_send![panel, rightAnchor];
        activate(msg_send![right, constraintEqualToAnchor: panel_right]);
        let top: id = msg_send![content, topAnchor];
        let panel_top: id = msg_send![panel, topAnchor];
        activate(msg_send![top, constraintEqualToAnchor: panel_top]);
        let bottom: id = msg_send![content, bottomAnchor];
        let panel_bottom: id = msg_send![panel, bottomAnchor];
        activate(msg_send![bottom, constraintEqualToAnchor: panel_bottom]);
        panel
    }
}

unsafe fn activate(constraint: id) {
    // SAFETY: Upheld by the callers.
    unsafe {
        let _: () = msg_send![constraint, setActive: YES];
    }
}

unsafe fn status_color(invalid: bool) -> id {
    // SAFETY: Class method calls on the main thread.
    unsafe {
        if invalid {
            msg_send![class!(NSColor), systemRedColor]
        } else {
            msg_send![class!(NSColor), secondaryLabelColor]
        }
    }
}

unsafe fn symbol(name: &str, description: &str) -> id {
    // SAFETY: Class method call on the main thread.
    unsafe {
        msg_send![class!(NSImage),
            imageWithSystemSymbolName: ns_string(name)
            accessibilityDescription: ns_string(description)]
    }
}

unsafe fn ns_string(value: &str) -> id {
    // SAFETY: Autoreleased into the caller's pool on the main thread.
    unsafe { NSString::alloc(nil).init_str(value).autorelease() }
}

unsafe fn rust_string(value: id) -> String {
    if value == nil {
        return String::new();
    }
    // SAFETY: `value` is a live NSString.
    unsafe {
        let utf8: *const std::ffi::c_char = msg_send![value, UTF8String];
        if utf8.is_null() {
            return String::new();
        }
        CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}

struct DelegateState {
    sender: Sender<NativeSearchEvent>,
}

fn delegate_class() -> *const Class {
    DELEGATE_CLASS
        .get_or_init(|| {
            let mut decl = ClassDecl::new("TermyNativeSearchBarDelegate", class!(NSObject))
                .expect("search bar delegate class should register once");
            decl.add_ivar::<*mut c_void>(DELEGATE_STATE_IVAR);

            // SAFETY: Each signature matches the AppKit selector it implements.
            unsafe {
                decl.add_method(sel!(dealloc), dealloc as extern "C" fn(&mut Object, Sel));
                decl.add_method(
                    sel!(controlTextDidChange:),
                    control_text_did_change as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    sel!(control:textView:doCommandBySelector:),
                    do_command_by_selector as extern "C" fn(&Object, Sel, id, id, Sel) -> BOOL,
                );
                decl.add_method(
                    sel!(searchFieldChanged:),
                    search_field_changed as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    sel!(optionsChanged:),
                    options_changed as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(sel!(navigate:), navigate as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(done:), done as extern "C" fn(&Object, Sel, id));
            }

            std::ptr::from_ref::<Class>(decl.register()) as usize
        })
        .to_owned() as *const Class
}

extern "C" fn dealloc(this: &mut Object, _: Sel) {
    // SAFETY: Frees the state boxed in `BarViews::build`, once.
    unsafe {
        let state = *this.get_ivar::<*mut c_void>(DELEGATE_STATE_IVAR);
        if !state.is_null() {
            drop(Box::from_raw(state.cast::<DelegateState>()));
            this.set_ivar(DELEGATE_STATE_IVAR, ptr::null_mut::<c_void>());
        }
        let _: () = msg_send![super(this, class!(NSObject)), dealloc];
    }
}

fn send(this: &Object, event: NativeSearchEvent) {
    // SAFETY: The state lives until `dealloc`.
    let state = unsafe { *this.get_ivar::<*mut c_void>(DELEGATE_STATE_IVAR) };
    if state.is_null() {
        return;
    }
    // SAFETY: Non-null states point at the `DelegateState` boxed in `build`.
    let state = unsafe { &*state.cast::<DelegateState>() };
    let _ = state.sender.send(event);
}

extern "C" fn control_text_did_change(this: &Object, _: Sel, notification: id) {
    // SAFETY: AppKit passes a live notification whose object is the field.
    let query = unsafe {
        let field: id = msg_send![notification, object];
        rust_string(msg_send![field, stringValue])
    };
    send(this, NativeSearchEvent::QueryChanged(query));
}

extern "C" fn search_field_changed(this: &Object, _: Sel, field: id) {
    // SAFETY: AppKit passes the live search field.
    let query = unsafe { rust_string(msg_send![field, stringValue]) };
    send(this, NativeSearchEvent::QueryChanged(query));
}

extern "C" fn do_command_by_selector(
    this: &Object,
    _: Sel,
    _control: id,
    _text_view: id,
    command: Sel,
) -> BOOL {
    let event = if command == sel!(insertNewline:) {
        if shift_pressed() {
            NativeSearchEvent::Previous
        } else {
            NativeSearchEvent::Next
        }
    } else if command == sel!(cancelOperation:) {
        NativeSearchEvent::Close
    } else if command == sel!(moveUp:) {
        NativeSearchEvent::Next
    } else if command == sel!(moveDown:) {
        NativeSearchEvent::Previous
    } else {
        return NO;
    };
    send(this, event);
    YES
}

fn shift_pressed() -> bool {
    // SAFETY: Reads the event AppKit is dispatching, on the main thread.
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let event: id = msg_send![app, currentEvent];
        if event == nil {
            return false;
        }
        let flags: usize = msg_send![event, modifierFlags];
        flags & NS_EVENT_MODIFIER_FLAG_SHIFT != 0
    }
}

extern "C" fn options_changed(this: &Object, _: Sel, sender: id) {
    // SAFETY: AppKit passes the live segmented control.
    let (case_sensitive, regex) = unsafe {
        let case_sensitive: BOOL = msg_send![sender, isSelectedForSegment: MATCH_CASE_SEGMENT];
        let regex: BOOL = msg_send![sender, isSelectedForSegment: REGEX_SEGMENT];
        (case_sensitive == YES, regex == YES)
    };
    send(
        this,
        NativeSearchEvent::Options {
            case_sensitive,
            regex,
        },
    );
}

extern "C" fn navigate(this: &Object, _: Sel, sender: id) {
    // SAFETY: AppKit passes the live segmented control.
    let segment: isize = unsafe { msg_send![sender, selectedSegment] };
    send(
        this,
        if segment == NEXT_SEGMENT {
            NativeSearchEvent::Next
        } else {
            NativeSearchEvent::Previous
        },
    );
}

extern "C" fn done(this: &Object, _: Sel, _sender: id) {
    send(this, NativeSearchEvent::Close);
}

/// A retained AppKit object that can travel to the main queue.
struct Retained(usize);

// SAFETY: The object is only messaged on the main thread; the retain keeps it
// alive until the last clone drops.
unsafe impl Send for Retained {}

impl Retained {
    /// Take ownership of a +1 reference (`new`, `alloc`/`init`).
    fn adopt(object: id) -> Self {
        Self(object as usize)
    }

    /// # Safety
    /// `object` must be a live Objective-C object.
    unsafe fn retain(object: id) -> Self {
        // SAFETY: Upheld by the caller.
        unsafe {
            let _: id = msg_send![object, retain];
        }
        Self(object as usize)
    }

    fn id(&self) -> id {
        self.0 as id
    }
}

impl Clone for Retained {
    fn clone(&self) -> Self {
        // SAFETY: `self` keeps the object alive.
        unsafe { Self::retain(self.id()) }
    }
}

impl Drop for Retained {
    fn drop(&mut self) {
        // SAFETY: Balances `adopt` or `retain`.
        unsafe {
            let _: () = msg_send![self.id(), release];
        }
    }
}

fn on_main_queue(work: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(work);
}
