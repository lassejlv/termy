//! AppKit window tabs for `tab_style = native`.
//!
//! GPUI already groups windows that share a tabbing identifier, but it hides
//! AppKit's tab bar (Zed draws its own) and routes Show Next/Previous Tab to
//! its own controller. Native mode swaps those `GPUIWindow` methods back to
//! AppKit's and adds `newWindowForTab:` so the tab bar shows its "+" button.
//!
//! On macOS 26 and later the tab bar also moves up into the titlebar row,
//! beside the traffic lights, the way Ghostty's titlebar tabs do: AppKit has
//! no API for that, so it right-aligns the tab bar accessory and pins
//! AppKit's private titlebar views with constraints. When those views are
//! missing the tab bar simply stays below the title.

use cocoa::base::{BOOL, NO, YES, id, nil};
use cocoa::foundation::NSAutoreleasePool as _;
use cocoa::foundation::NSString;
use gpui_kit::Window;
use objc::{
    class, msg_send,
    runtime::{
        Class, Imp, Method, Object, Sel, class_addMethod, class_getInstanceMethod,
        method_setImplementation,
    },
    sel, sel_impl,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::OnceLock;

/// Shared by every Termy terminal window so AppKit offers to group them.
pub(crate) const TABBING_IDENTIFIER: &str = "dev.termy.terminal";

const NS_WINDOW_ABOVE: isize = 1;
const NS_WINDOW_BELOW: isize = -1;
const NS_LAYOUT_ATTRIBUTE_RIGHT: isize = 2;
const NS_WINDOW_TITLE_VISIBLE: isize = 0;
const NS_WINDOW_TITLE_HIDDEN: isize = 1;
const NS_WINDOW_TOOLBAR_STYLE_UNIFIED_COMPACT: isize = 4;
/// Room left of the tabs for the traffic lights.
const TITLEBAR_TABS_LEADING_INSET: f64 = 78.0;

/// Receives the `NSWindow` (as an address) whose tab bar "+" was clicked.
static NEW_TAB_REQUESTS: OnceLock<flume::Sender<usize>> = OnceLock::new();

/// Restore AppKit's tab behavior on GPUI windows. Later calls are no-ops.
/// Returns the receiver for tab bar "+" clicks on the first call only.
pub(crate) fn install() -> Option<flume::Receiver<usize>> {
    let (tx, rx) = flume::unbounded();
    NEW_TAB_REQUESTS.set(tx).ok()?;

    let Some(class) = Class::get("GPUIWindow") else {
        log::error!("GPUIWindow class is missing; native tabs keep GPUI's hidden tab bar");
        return Some(rx);
    };
    let class = std::ptr::from_ref::<Class>(class).cast_mut();
    // SAFETY: Runs once on the main thread after GPUI registered its window
    // class. Each replacement has the same signature as the method it
    // replaces, and every type encoding matches `(self, _cmd, id) -> void`.
    unsafe {
        replace_method(
            class,
            sel!(addTitlebarAccessoryViewController:),
            add_titlebar_accessory_view_controller as extern "C" fn(&Object, Sel, id),
        );
        replace_method(
            class,
            sel!(selectNextTab:),
            select_next_tab as extern "C" fn(&Object, Sel, id),
        );
        replace_method(
            class,
            sel!(selectPreviousTab:),
            select_previous_tab as extern "C" fn(&Object, Sel, id),
        );
        let added: BOOL = class_addMethod(
            class,
            sel!(newWindowForTab:),
            std::mem::transmute::<extern "C" fn(&Object, Sel, id), Imp>(new_window_for_tab),
            c"v@:@".as_ptr(),
        );
        if added == NO {
            log::warn!("GPUIWindow already implements newWindowForTab:");
        }
    }
    Some(rx)
}

unsafe fn replace_method(class: *mut Class, selector: Sel, imp: extern "C" fn(&Object, Sel, id)) {
    // SAFETY: Upheld by `install`.
    unsafe {
        let method = class_getInstanceMethod(class, selector);
        if method.is_null() {
            log::warn!("GPUIWindow has no {} to replace", selector.name());
            return;
        }
        method_setImplementation(
            method as *mut Method,
            std::mem::transmute::<extern "C" fn(&Object, Sel, id), Imp>(imp),
        );
    }
}

extern "C" fn add_titlebar_accessory_view_controller(this: &Object, _: Sel, controller: id) {
    let titlebar_tabs = titlebar_tabs_supported();
    // SAFETY: Forwards an AppKit call to NSWindow's own implementation. The
    // layout must change before AppKit adds the accessory, or AppKit asserts.
    unsafe {
        if titlebar_tabs {
            let _: () = msg_send![controller, setLayoutAttribute: NS_LAYOUT_ATTRIBUTE_RIGHT];
        }
        let _: () = msg_send![super(this, class!(NSWindow)), addTitlebarAccessoryViewController: controller];
    }
    if !titlebar_tabs {
        return;
    }
    // GPUI windows only get one titlebar accessory: the tab bar. Its view
    // hierarchy is complete on the next tick.
    // SAFETY: Both are live objects AppKit just handed us on the main thread.
    let (window, controller) = unsafe {
        (
            Retained::retain(std::ptr::from_ref::<Object>(this).cast_mut()),
            Retained::retain(controller),
        )
    };
    on_main_queue(move || {
        // SAFETY: Both objects are retained and messaged on the main thread.
        unsafe {
            if !place_tab_bar_in_titlebar(controller.id()) {
                log::warn!(
                    "AppKit's titlebar layout changed; native tabs stay below the title ({})",
                    view_ancestry(controller.id())
                );
            }
            refresh_title_visibility(window.id());
        }
    });
}

#[repr(C)]
struct OperatingSystemVersion {
    major: isize,
    minor: isize,
    patch: isize,
}

fn titlebar_tabs_supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let version = OperatingSystemVersion {
            major: 26,
            minor: 0,
            patch: 0,
        };
        // SAFETY: Plain NSProcessInfo query.
        unsafe {
            let info: id = msg_send![class!(NSProcessInfo), processInfo];
            let at_least: BOOL = msg_send![info, isOperatingSystemAtLeastVersion: version];
            at_least == YES
        }
    })
}

unsafe fn class_name_is(view: id, name: &str) -> bool {
    // SAFETY: Upheld by the callers.
    unsafe { class_name(view) == name }
}

unsafe fn class_name(view: id) -> String {
    // SAFETY: Upheld by the callers.
    unsafe {
        let class_name: id = msg_send![view, className];
        let utf8: *const std::ffi::c_char = msg_send![class_name, UTF8String];
        if utf8.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(utf8)
            .to_string_lossy()
            .into_owned()
    }
}

/// The class names from the accessory view up to the window, for the log
/// line when AppKit's titlebar hierarchy no longer matches.
unsafe fn view_ancestry(controller: id) -> String {
    // SAFETY: Upheld by the callers.
    unsafe {
        let mut names = Vec::new();
        let mut current: id = msg_send![controller, view];
        while current != nil {
            names.push(class_name(current));
            current = msg_send![current, superview];
        }
        names.join(" < ")
    }
}

unsafe fn ancestor_named(view: id, name: &str) -> Option<id> {
    // SAFETY: Upheld by the callers.
    unsafe {
        let mut current: id = msg_send![view, superview];
        while current != nil {
            if class_name_is(current, name) {
                return Some(current);
            }
            current = msg_send![current, superview];
        }
        None
    }
}

unsafe fn descendant_named(view: id, name: &str) -> Option<id> {
    // SAFETY: Upheld by the callers.
    unsafe {
        let subviews: id = msg_send![view, subviews];
        let count: usize = msg_send![subviews, count];
        for index in 0..count {
            let subview: id = msg_send![subviews, objectAtIndex: index];
            if class_name_is(subview, name) {
                return Some(subview);
            }
            if let Some(found) = descendant_named(subview, name) {
                return Some(found);
            }
        }
        None
    }
}

#[derive(Clone, Copy)]
enum Anchor {
    Left,
    Right,
    Top,
    Height,
}

unsafe fn anchor(view: id, anchor: Anchor) -> id {
    // SAFETY: Upheld by the callers.
    unsafe {
        match anchor {
            Anchor::Left => msg_send![view, leftAnchor],
            Anchor::Right => msg_send![view, rightAnchor],
            Anchor::Top => msg_send![view, topAnchor],
            Anchor::Height => msg_send![view, heightAnchor],
        }
    }
}

/// Constrain `view`'s anchor to the same anchor of `to`, offset by `constant`.
unsafe fn pin(view: id, to: id, edge: Anchor, constant: f64) {
    // SAFETY: Upheld by the callers; both views share the titlebar view.
    unsafe {
        let from = anchor(view, edge);
        let target = anchor(to, edge);
        let constraint: id = msg_send![from, constraintEqualToAnchor: target constant: constant];
        let _: () = msg_send![constraint, setActive: YES];
    }
}

/// Stretch the tab bar across the titlebar row, right of the traffic lights.
unsafe fn place_tab_bar_in_titlebar(controller: id) -> bool {
    // SAFETY: Upheld by the callers.
    unsafe {
        let accessory: id = msg_send![controller, view];
        if accessory == nil {
            return false;
        }
        // macOS 26 wraps accessories in a clip view; macOS 27 in a container.
        let Some(clip) = ancestor_named(accessory, "NSTitlebarAccessoryClipView")
            .or_else(|| ancestor_named(accessory, "NSTitlebarAccessoryContainerView"))
        else {
            return false;
        };
        let Some(titlebar) = ancestor_named(clip, "NSTitlebarView") else {
            return false;
        };
        let container = descendant_named(titlebar, "NSToolbarView").unwrap_or(titlebar);
        let _: () = msg_send![clip, setTranslatesAutoresizingMaskIntoConstraints: NO];
        let _: () = msg_send![accessory, setTranslatesAutoresizingMaskIntoConstraints: NO];
        pin(clip, container, Anchor::Left, TITLEBAR_TABS_LEADING_INSET);
        pin(clip, container, Anchor::Right, 0.0);
        pin(clip, container, Anchor::Top, 0.0);
        pin(clip, container, Anchor::Height, 0.0);
        pin(accessory, clip, Anchor::Left, 0.0);
        pin(accessory, clip, Anchor::Right, 0.0);
        pin(accessory, clip, Anchor::Top, 0.0);
        pin(accessory, clip, Anchor::Height, 0.0);
        let _: () = msg_send![clip, setNeedsLayout: YES];
        let _: () = msg_send![accessory, setNeedsLayout: YES];
        true
    }
}

/// With the tabs in the titlebar row, the title would sit under them, so it
/// only shows while the tab bar is hidden (a single tab).
unsafe fn refresh_title_visibility(window: id) {
    if !titlebar_tabs_supported() {
        return;
    }
    // SAFETY: Upheld by the callers.
    unsafe {
        for tab in tabbed_windows(window) {
            let group: id = msg_send![tab, tabGroup];
            let tab_bar_visible: BOOL = if group == nil {
                NO
            } else {
                msg_send![group, isTabBarVisible]
            };
            let visibility = if tab_bar_visible == YES {
                NS_WINDOW_TITLE_HIDDEN
            } else {
                NS_WINDOW_TITLE_VISIBLE
            };
            let _: () = msg_send![tab, setTitleVisibility: visibility];
        }
    }
}

extern "C" fn select_next_tab(this: &Object, _: Sel, sender: id) {
    // SAFETY: Forwards an AppKit call to NSWindow's own implementation.
    unsafe {
        let _: () = msg_send![super(this, class!(NSWindow)), selectNextTab: sender];
    }
}

extern "C" fn select_previous_tab(this: &Object, _: Sel, sender: id) {
    // SAFETY: Forwards an AppKit call to NSWindow's own implementation.
    unsafe {
        let _: () = msg_send![super(this, class!(NSWindow)), selectPreviousTab: sender];
    }
}

extern "C" fn new_window_for_tab(this: &Object, _: Sel, _sender: id) {
    if let Some(tx) = NEW_TAB_REQUESTS.get() {
        let _ = tx.send(std::ptr::from_ref::<Object>(this) as usize);
    }
}

/// The window's `NSWindow` address, used to match AppKit callbacks to GPUI
/// windows.
pub(crate) fn ns_window_address(window: &Window) -> Option<usize> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    let ns_view = handle.ns_view.as_ptr().cast::<Object>();
    // SAFETY: The borrowed GPUI window keeps its content view alive.
    let ns_window: id = unsafe { msg_send![ns_view, window] };
    (ns_window != nil).then_some(ns_window as usize)
}

/// A retained AppKit object that can wait for a deferred AppKit call.
///
/// Changing tabs, titles or colors makes AppKit call back into GPUI right
/// away (resize, key and move events). GPUI drops those callbacks while it is
/// mid-update ("RefCell already borrowed"), so every change here runs on the
/// main queue after the current update, like GPUI's own `activate`.
pub(crate) struct Retained(usize);

// SAFETY: The window is only messaged on the main queue; the retain keeps the
// object alive until then even if the window closes.
unsafe impl Send for Retained {}

impl Retained {
    pub(crate) fn of(window: &Window) -> Option<Self> {
        let ns_window = ns_window_address(window)? as id;
        // SAFETY: `ns_window` belongs to the live window borrowed above.
        Some(unsafe { Self::retain(ns_window) })
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

impl Drop for Retained {
    fn drop(&mut self) {
        // SAFETY: Balances the retain in `of`.
        unsafe {
            let _: () = msg_send![self.id(), release];
        }
    }
}

fn on_main_queue(work: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(work);
}

/// Add `tab` to `host`'s tab group, directly after (right of) or before it,
/// and select it.
pub(crate) fn add_tab(host: Retained, tab: Retained, after: bool) {
    on_main_queue(move || {
        // SAFETY: Both windows are retained and messaged on the main thread.
        unsafe {
            // GPUI may already have tabbed the window onto the main window when
            // the user prefers tabs; take it out so it lands beside `host`.
            detach_from_tab_group(tab.id());
            let ordered = if after {
                NS_WINDOW_ABOVE
            } else {
                NS_WINDOW_BELOW
            };
            let _: () = msg_send![host.id(), addTabbedWindow: tab.id() ordered: ordered];
            refresh_title_visibility(host.id());
        }
    });
}

unsafe fn detach_from_tab_group(window: id) {
    // SAFETY: Upheld by the callers.
    unsafe {
        let group: id = msg_send![window, tabGroup];
        if group == nil {
            return;
        }
        let windows: id = msg_send![group, windows];
        let count: usize = msg_send![windows, count];
        if count > 1 {
            let _: () = msg_send![group, removeWindow: window];
        }
    }
}

/// The windows in `window`'s tab group, in tab order. A window that is not
/// tabbed returns just itself.
unsafe fn tabbed_windows(window: id) -> Vec<id> {
    // SAFETY: Upheld by the callers.
    unsafe {
        let windows: id = msg_send![window, tabbedWindows];
        if windows == nil {
            return vec![window];
        }
        let count: usize = msg_send![windows, count];
        (0..count)
            .map(|index| msg_send![windows, objectAtIndex: index])
            .collect()
    }
}

unsafe fn select(window: id) {
    // SAFETY: Upheld by the callers.
    unsafe {
        let group: id = msg_send![window, tabGroup];
        if group != nil {
            let _: () = msg_send![group, setSelectedWindow: window];
        }
        let _: () = msg_send![window, makeKeyAndOrderFront: nil];
    }
}

/// Position of the window's tab and the number of tabs in its group.
pub(crate) fn tab_position(window: &Window) -> Option<(usize, usize)> {
    let ns_window = ns_window_address(window)? as id;
    // SAFETY: `ns_window` belongs to the live window borrowed above.
    let windows = unsafe { tabbed_windows(ns_window) };
    let index = windows
        .iter()
        .position(|candidate| *candidate == ns_window)?;
    Some((index, windows.len()))
}

/// Select the tab at `index` in the window's tab group. Returns false when
/// there is no such tab.
pub(crate) fn select_tab(window: &Window, index: usize) -> bool {
    let Some((current, count)) = tab_position(window) else {
        return false;
    };
    if index >= count {
        return false;
    }
    if index == current {
        return true;
    }
    let Some(window) = Retained::of(window) else {
        return false;
    };
    on_main_queue(move || {
        // SAFETY: The window is retained and messaged on the main thread.
        unsafe {
            if let Some(&target) = tabbed_windows(window.id()).get(index) {
                select(target);
            }
        }
    });
    true
}

/// Move the window's tab one slot left or right within its group.
pub(crate) fn move_tab(window: &Window, to_right: bool) {
    let Some(window) = Retained::of(window) else {
        return;
    };
    on_main_queue(move || {
        let ns_window = window.id();
        // SAFETY: The window is retained and messaged on the main thread.
        unsafe {
            let windows = tabbed_windows(ns_window);
            let Some(index) = windows.iter().position(|candidate| *candidate == ns_window) else {
                return;
            };
            let neighbor = if to_right {
                windows.get(index + 1)
            } else {
                index.checked_sub(1).and_then(|index| windows.get(index))
            };
            let Some(&neighbor) = neighbor else {
                return;
            };
            detach_from_tab_group(ns_window);
            let ordered = if to_right {
                NS_WINDOW_ABOVE
            } else {
                NS_WINDOW_BELOW
            };
            let _: () = msg_send![neighbor, addTabbedWindow: ns_window ordered: ordered];
            select(ns_window);
        }
    });
}

/// Show or hide the tab bar of the window's tab group.
pub(crate) fn toggle_tab_bar(window: &Window) {
    let Some(window) = Retained::of(window) else {
        return;
    };
    on_main_queue(move || {
        // SAFETY: The window is retained and messaged on the main thread.
        unsafe {
            let _: () = msg_send![window.id(), toggleTabBar: nil];
            refresh_title_visibility(window.id());
        }
    });
}

/// Title (the tab label) and titlebar color of a native tab window. The
/// titlebar turns transparent over the window background, which takes the
/// terminal background; the appearance keeps the title and tab bar legible.
pub(crate) fn set_window_chrome(window: &Window, title: String, background: [f64; 4], dark: bool) {
    let Some(window) = Retained::of(window) else {
        return;
    };
    on_main_queue(move || {
        let [red, green, blue, alpha] = background;
        // SAFETY: The window is retained and messaged on the main thread.
        unsafe {
            let ns_window = window.id();
            let title = NSString::alloc(nil).init_str(&title).autorelease();
            let _: () = msg_send![ns_window, setTitle: title];
            let _: () = msg_send![ns_window, setTitlebarAppearsTransparent: YES];
            let color: id = msg_send![
                class!(NSColor),
                colorWithSRGBRed: red green: green blue: blue alpha: alpha
            ];
            let _: () = msg_send![ns_window, setBackgroundColor: color];
            let name = NSString::alloc(nil)
                .init_str(if dark {
                    "NSAppearanceNameDarkAqua"
                } else {
                    "NSAppearanceNameAqua"
                })
                .autorelease();
            let appearance: id = msg_send![class!(NSAppearance), appearanceNamed: name];
            let _: () = msg_send![ns_window, setAppearance: appearance];
        }
    });
}

/// Give a new native tab window a compact toolbar: it makes the titlebar
/// row tall enough to hold the tab bar on macOS 26 and later.
pub(crate) fn prepare_titlebar_tabs(window: &Window) {
    if !titlebar_tabs_supported() {
        return;
    }
    let Some(window) = Retained::of(window) else {
        return;
    };
    on_main_queue(move || {
        // SAFETY: The window is retained and messaged on the main thread.
        unsafe {
            let identifier = NSString::alloc(nil).init_str("TermyTabs").autorelease();
            let toolbar: id = msg_send![class!(NSToolbar), alloc];
            let toolbar: id = msg_send![toolbar, initWithIdentifier: identifier];
            let _: () = msg_send![window.id(), setToolbar: toolbar];
            let _: () = msg_send![toolbar, release];
            let _: () =
                msg_send![window.id(), setToolbarStyle: NS_WINDOW_TOOLBAR_STYLE_UNIFIED_COMPACT];
            refresh_title_visibility(window.id());
        }
    });
}

/// Re-check which windows in the group should show their title, after tabs
/// were added or closed elsewhere.
pub(crate) fn refresh_titlebar_tabs(window: &Window) {
    if !titlebar_tabs_supported() {
        return;
    }
    let Some(window) = Retained::of(window) else {
        return;
    };
    on_main_queue(move || {
        // SAFETY: The window is retained and messaged on the main thread.
        unsafe { refresh_title_visibility(window.id()) }
    });
}

/// Let AppKit offer its tab menu items and automatic tabbing again. GPUI
/// turns this off whenever it opens a window without a tabbing identifier,
/// such as Settings.
pub(crate) fn allow_automatic_window_tabbing() {
    // SAFETY: Class method call on the main thread.
    unsafe {
        let _: () = msg_send![class!(NSWindow), setAllowsAutomaticWindowTabbing: YES];
    }
}
