use cocoa::{
    appkit::{NSEventType, NSWindow, NSWindowCollectionBehavior},
    base::{BOOL, NO, YES, id, nil},
};
use gpui::Window;
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Class, Object, Sel},
    sel, sel_impl,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{fmt, sync::OnceLock};

static NON_DRAGGABLE_CONTENT_VIEW_CLASS: OnceLock<usize> = OnceLock::new();

unsafe extern "C" {
    fn object_getClass(obj: *mut Object) -> *const Class;
    fn object_setClass(obj: *mut Object, cls: *const Class) -> *const Class;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    static NSAccessibilityTextAreaRole: id;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NativeTitlebarDragError {
    WindowHandle,
    NonAppKitHandle,
    MissingView,
    MissingWindow,
    MissingViewClass,
    ClassRegistration,
    FirstResponder,
    PanelVisibility,
    MissingMouseDownEvent,
}

impl fmt::Display for NativeTitlebarDragError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowHandle => write!(f, "Failed to access the macOS window handle."),
            Self::NonAppKitHandle => write!(
                f,
                "macOS titlebar drag bridge requires an AppKit window handle.",
            ),
            Self::MissingView => write!(f, "macOS titlebar drag bridge requires a live NSView."),
            Self::MissingWindow => {
                write!(f, "macOS titlebar drag bridge requires a live NSWindow.")
            }
            Self::MissingViewClass => write!(
                f,
                "macOS titlebar drag bridge could not read the NSView class."
            ),
            Self::ClassRegistration => write!(
                f,
                "macOS titlebar drag bridge failed to register its NSView subclass.",
            ),
            Self::FirstResponder => write!(
                f,
                "macOS terminal content view could not become the first responder.",
            ),
            Self::PanelVisibility => write!(
                f,
                "macOS benchmark window does not support persistent panel visibility.",
            ),
            Self::MissingMouseDownEvent => write!(
                f,
                "macOS titlebar dragging requires this window's left mouse-down event.",
            ),
        }
    }
}

/// Hand a hit-tested titlebar press to AppKit while its original event is current.
pub(crate) fn start_titlebar_window_drag(window: &Window) -> Result<(), NativeTitlebarDragError> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|_| NativeTitlebarDragError::WindowHandle)?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err(NativeTitlebarDragError::NonAppKitHandle);
    };
    let ns_view = handle.ns_view.as_ptr().cast::<Object>();

    // SAFETY: GPUI calls this on the main thread during mouse-down dispatch.
    // The borrowed window keeps its NSView/NSWindow alive, and NSApplication
    // owns the current event throughout this synchronous call.
    unsafe {
        let ns_window: id = msg_send![ns_view, window];
        if ns_window == nil {
            return Err(NativeTitlebarDragError::MissingWindow);
        }
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let event: id = msg_send![app, currentEvent];
        if event == nil {
            return Err(NativeTitlebarDragError::MissingMouseDownEvent);
        }
        let event_type: usize = msg_send![event, type];
        let event_window: id = msg_send![event, window];
        if event_type != NSEventType::NSLeftMouseDown as usize || event_window != ns_window {
            return Err(NativeTitlebarDragError::MissingMouseDownEvent);
        }
        // Unlike GPUI 0.2.2's macOS start_window_move (a no-op), this hands
        // movement to Window Server and returns immediately.
        let _: () = msg_send![ns_window, performWindowDragWithEvent: event];
    }
    Ok(())
}

pub(crate) fn keep_benchmark_panel_visible_when_inactive(
    window: &Window,
) -> Result<(), NativeTitlebarDragError> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|_| NativeTitlebarDragError::WindowHandle)?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err(NativeTitlebarDragError::NonAppKitHandle);
    };

    let ns_view = handle.ns_view.as_ptr().cast::<Object>();
    if ns_view.is_null() {
        return Err(NativeTitlebarDragError::MissingView);
    }

    let ns_window: id = unsafe { msg_send![ns_view, window] };
    if ns_window == nil {
        return Err(NativeTitlebarDragError::MissingWindow);
    }

    let supports_visibility: BOOL =
        unsafe { msg_send![ns_window, respondsToSelector: sel!(setHidesOnDeactivate:)] };
    if supports_visibility != YES {
        return Err(NativeTitlebarDragError::PanelVisibility);
    }

    // GPUI implements floating windows with NSPanel. AppKit hides panels when
    // their app resigns active by default and confines them to their launch
    // Space, which makes xctrace record only the first burst of frames. Keep
    // this benchmark-only panel composited across focus and Space changes.
    unsafe {
        let _: () = msg_send![ns_window, setHidesOnDeactivate: NO];
        let collection_behavior = NSWindow::collectionBehavior(ns_window)
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary;
        NSWindow::setCollectionBehavior_(ns_window, collection_behavior);
        let _: () = msg_send![ns_window, orderFrontRegardless];
    }
    Ok(())
}

pub(crate) fn disable_automatic_content_view_window_drag(
    window: &Window,
) -> Result<(), NativeTitlebarDragError> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|_| NativeTitlebarDragError::WindowHandle)?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err(NativeTitlebarDragError::NonAppKitHandle);
    };

    let ns_view = handle.ns_view.as_ptr().cast::<Object>();
    if ns_view.is_null() {
        return Err(NativeTitlebarDragError::MissingView);
    }

    unsafe { disable_automatic_content_view_window_drag_for_view(ns_view) }
}

unsafe fn disable_automatic_content_view_window_drag_for_view(
    ns_view: *mut Object,
) -> Result<(), NativeTitlebarDragError> {
    let ns_window: id = unsafe { msg_send![ns_view, window] };
    if ns_window == nil {
        return Err(NativeTitlebarDragError::MissingWindow);
    }

    unsafe {
        let _: () = msg_send![ns_window, setMovableByWindowBackground: NO];
    }

    let current_class = unsafe { object_getClass(ns_view) };
    if current_class.is_null() {
        return Err(NativeTitlebarDragError::MissingViewClass);
    }

    let non_draggable_class = non_draggable_content_view_class(current_class)?;
    if current_class != non_draggable_class {
        unsafe {
            object_setClass(ns_view, non_draggable_class);
        }
    }

    let became_first_responder: BOOL = unsafe { msg_send![ns_window, makeFirstResponder: ns_view] };
    if became_first_responder != YES {
        return Err(NativeTitlebarDragError::FirstResponder);
    }

    Ok(())
}

fn non_draggable_content_view_class(
    superclass: *const Class,
) -> Result<*const Class, NativeTitlebarDragError> {
    let superclass =
        unsafe { superclass.as_ref() }.ok_or(NativeTitlebarDragError::MissingViewClass)?;
    let class = *NON_DRAGGABLE_CONTENT_VIEW_CLASS.get_or_init(|| unsafe {
        let Some(mut decl) = ClassDecl::new("TermyNonDraggableGPUIView", superclass) else {
            return 0;
        };
        decl.add_method(
            sel!(mouseDownCanMoveWindow),
            mouse_down_can_move_window as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(acceptsFirstResponder),
            accepts_first_responder as extern "C" fn(&Object, Sel) -> BOOL,
        );
        // The pinned GPUI revision predates its AccessKit integration, so its
        // custom NSTextInputClient view is otherwise invisible to macOS
        // accessibility clients. Voice input apps then see the window itself
        // as focused and refuse to insert the transcript.
        decl.add_method(
            sel!(isAccessibilityElement),
            is_accessibility_element as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(accessibilityRole),
            accessibility_role as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(isAccessibilityFocused),
            is_accessibility_focused as extern "C" fn(&Object, Sel) -> BOOL,
        );
        std::ptr::from_ref::<Class>(decl.register()) as usize
    });

    if class == 0 {
        Err(NativeTitlebarDragError::ClassRegistration)
    } else {
        Ok(class as *const Class)
    }
}

extern "C" fn mouse_down_can_move_window(_this: &Object, _sel: Sel) -> BOOL {
    NO
}

extern "C" fn accepts_first_responder(_this: &Object, _sel: Sel) -> BOOL {
    YES
}

extern "C" fn is_accessibility_element(_this: &Object, _sel: Sel) -> BOOL {
    YES
}

extern "C" fn accessibility_role(_this: &Object, _sel: Sel) -> id {
    unsafe { NSAccessibilityTextAreaRole }
}

extern "C" fn is_accessibility_focused(this: &Object, _sel: Sel) -> BOOL {
    unsafe {
        let window: id = msg_send![this, window];
        if window == nil {
            return NO;
        }

        let first_responder: id = msg_send![window, firstResponder];
        if std::ptr::eq(first_responder.cast_const(), std::ptr::from_ref(this)) {
            YES
        } else {
            NO
        }
    }
}
