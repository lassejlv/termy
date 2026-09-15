mod file_manager;

pub use file_manager::{OPEN_TAB_HERE_LABEL, register_open_tab_here};

#[cfg(target_os = "macos")]
use dispatch2::run_on_main;
#[cfg(target_os = "macos")]
use objc2::{
    DeclaredClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::AnyObject, sel,
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSAlert, NSAlertSecondButtonReturn, NSAlertThirdButtonReturn, NSApplication, NSEvent, NSImage,
    NSMenu, NSMenuItem, NSView,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{MainThreadMarker, NSData, NSPoint, NSString};
#[cfg(target_os = "macos")]
use std::cell::Cell;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicI32, Ordering};

#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, POINT};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetForegroundWindow, IDNO, IDYES,
    MB_ICONINFORMATION, MB_OK, MB_YESNO, MB_YESNOCANCEL, MF_GRAYED, MF_SEPARATOR, MF_STRING,
    MessageBoxW, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
};

#[cfg(target_os = "windows")]
fn wide_string(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
struct MenuGuard(windows::Win32::UI::WindowsAndMessaging::HMENU);

#[cfg(target_os = "windows")]
impl Drop for MenuGuard {
    fn drop(&mut self) {
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    Copy,
    CopyImage,
    Paste,
    OpenSearch,
    CopyBufferPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabContextMenuAction {
    Rename,
    Pin,
    Unpin,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardPermission {
    Deny,
    AllowOnce,
    AllowAlways,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeContextMenuAnchor {
    pub native_view: usize,
    pub x: f64,
    pub y: f64,
}

const CONTEXT_MENU_COPY_ID: i32 = 1;
const CONTEXT_MENU_PASTE_ID: i32 = 2;
const CONTEXT_MENU_OPEN_SEARCH_ID: i32 = 3;
const CONTEXT_MENU_COPY_BUFFER_POSITION_ID: i32 = 4;
const CONTEXT_MENU_COPY_IMAGE_ID: i32 = 5;
const TAB_CONTEXT_MENU_PIN_ID: i32 = 101;
const TAB_CONTEXT_MENU_UNPIN_ID: i32 = 102;
const TAB_CONTEXT_MENU_RENAME_ID: i32 = 103;
const TAB_CONTEXT_MENU_CLOSE_ID: i32 = 104;
#[cfg(target_os = "macos")]
static CONTEXT_MENU_SELECTION: AtomicI32 = AtomicI32::new(0);

fn context_menu_action_for_id(action_id: i32) -> Option<ContextMenuAction> {
    match action_id {
        CONTEXT_MENU_COPY_ID => Some(ContextMenuAction::Copy),
        CONTEXT_MENU_COPY_IMAGE_ID => Some(ContextMenuAction::CopyImage),
        CONTEXT_MENU_PASTE_ID => Some(ContextMenuAction::Paste),
        CONTEXT_MENU_OPEN_SEARCH_ID => Some(ContextMenuAction::OpenSearch),
        CONTEXT_MENU_COPY_BUFFER_POSITION_ID => Some(ContextMenuAction::CopyBufferPosition),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSMenuItem))]
    #[name = "TermyContextMenuItem"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Cell<i32>]
    struct TermyContextMenuItem;

    impl TermyContextMenuItem {
        #[unsafe(method(fireContextMenuAction:))]
        fn fire_context_menu_action(&self, _sender: Option<&AnyObject>) {
            CONTEXT_MENU_SELECTION.store(self.ivars().get(), Ordering::Relaxed);
        }
    }
);

#[cfg(target_os = "macos")]
impl TermyContextMenuItem {
    fn new_with_action_id(
        mtm: MainThreadMarker,
        title: &str,
        action_id: i32,
        enabled: bool,
    ) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(Cell::new(action_id));
        let title = NSString::from_str(title);
        let key_equivalent = NSString::from_str("");
        let item: Retained<Self> = unsafe {
            msg_send![
                super(this),
                initWithTitle: &*title,
                action: Some(sel!(fireContextMenuAction:)),
                keyEquivalent: &*key_equivalent
            ]
        };
        unsafe {
            item.setTarget(Some(&item));
        }
        item.setEnabled(enabled);
        item
    }
}

#[cfg(target_os = "macos")]
fn pop_up_menu_at_anchor(menu: &NSMenu, anchor: Option<NativeContextMenuAnchor>) -> bool {
    if let Some(anchor) = anchor {
        let view_ptr = anchor.native_view as *mut NSView;
        if let Some(view_ptr) = std::ptr::NonNull::new(view_ptr) {
            let view = unsafe { view_ptr.as_ref() };
            let bounds = view.bounds();
            let x = anchor.x.clamp(0.0, bounds.size.width);
            let y = (bounds.size.height - anchor.y).clamp(0.0, bounds.size.height);
            return menu.popUpMenuPositioningItem_atLocation_inView(
                None,
                NSPoint::new(x, y),
                Some(view),
            );
        }
    }

    let location: NSPoint = NSEvent::mouseLocation();
    menu.popUpMenuPositioningItem_atLocation_inView(None, location, None)
}

#[cfg(target_os = "linux")]
fn has_command(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Read the system icon for any file or app bundle as PNG bytes.
/// Returns `None` on non-macOS or if icon retrieval fails.
///
/// # Safety
/// Must be called from the main thread.
pub fn app_icon_png_for_path(path: &str, size_px: f64) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
        use objc2_foundation::{NSDictionary, NSSize};

        let _ = unsafe { MainThreadMarker::new_unchecked() };
        let ns_path = NSString::from_str(path);
        let workspace = NSWorkspace::sharedWorkspace();
        let image = workspace.iconForFile(&ns_path);
        image.setSize(NSSize::new(size_px, size_px));
        let tiff_data = image.TIFFRepresentation()?;
        let bitmap_rep = NSBitmapImageRep::imageRepWithData(&tiff_data)?;
        let properties = NSDictionary::new();
        let png_data = unsafe {
            bitmap_rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
        }?;
        let bytes = unsafe { png_data.as_bytes_unchecked() };
        Some(bytes.to_vec())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, size_px);
        None
    }
}

#[cfg(target_os = "macos")]
fn image_from_png_data(png_data: &[u8]) -> Option<Retained<NSImage>> {
    use objc2::AnyThread;

    let data = NSData::with_bytes(png_data);
    NSImage::initWithData(NSImage::alloc(), &data)
}

#[cfg(target_os = "macos")]
fn current_app_bundle_path() -> Option<std::path::PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    exe_path
        .ancestors()
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("app"))
        .map(std::path::Path::to_path_buf)
}

/// Set the macOS dock icon from PNG data. No-op on other platforms.
///
/// # Safety
/// Must be called from the main thread (e.g. inside `application.run()`).
pub fn set_dock_icon_from_png(png_data: &[u8]) {
    #[cfg(target_os = "macos")]
    {
        if let Some(image) = image_from_png_data(png_data) {
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            let app = NSApplication::sharedApplication(mtm);
            unsafe {
                app.setApplicationIconImage(Some(&image));
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = png_data;
    }
}

/// Persist a custom icon on the current `.app` bundle. Returns false when the
/// process is not running from a bundle or macOS refuses the file icon update.
///
/// # Safety
/// Must be called from the main thread.
pub fn set_current_app_bundle_file_icon_from_png(png_data: &[u8]) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(image) = image_from_png_data(png_data) else {
            return false;
        };
        let Some(bundle_path) = current_app_bundle_path() else {
            return false;
        };
        let Some(bundle_path) = bundle_path.to_str() else {
            return false;
        };
        let path = NSString::from_str(bundle_path);
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        workspace.setIcon_forFile_options(
            Some(&image),
            &path,
            objc2_app_kit::NSWorkspaceIconCreationOptions::empty(),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = png_data;
        false
    }
}

/// Clear a custom file icon from the current `.app` bundle so macOS uses the
/// bundled default icon again.
///
/// # Safety
/// Must be called from the main thread.
pub fn clear_current_app_bundle_file_icon() -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(bundle_path) = current_app_bundle_path() else {
            return false;
        };
        let Some(bundle_path) = bundle_path.to_str() else {
            return false;
        };
        let path = NSString::from_str(bundle_path);
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        workspace.setIcon_forFile_options(
            None,
            &path,
            objc2_app_kit::NSWorkspaceIconCreationOptions::empty(),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Whether macOS "Reduce transparency" (System Settings → Accessibility →
/// Display) is enabled. When it is on, AppKit flattens `NSVisualEffectView`
/// vibrancy, so window-background blur cannot render and callers should fall
/// back to plain transparency. Always `false` on non-macOS platforms.
pub fn macos_reduce_transparency() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn show_alert(title: &str, message: &str) {
    #[cfg(target_os = "macos")]
    {
        run_on_main(|mtm| {
            let alert = NSAlert::new(mtm);
            let ns_title = NSString::from_str(title);
            let ns_message = NSString::from_str(message);
            let ok = NSString::from_str("OK");

            alert.setMessageText(&ns_title);
            alert.setInformativeText(&ns_message);
            let _ = alert.addButtonWithTitle(&ok);
            let _ = alert.runModal();
        });
    }

    #[cfg(target_os = "linux")]
    {
        if has_command("zenity") {
            let _ = Command::new("zenity")
                .args(["--info", "--title", title, "--text", message])
                .status();
        } else if has_command("kdialog") {
            let _ = Command::new("kdialog")
                .args(["--msgbox", message, "--title", title])
                .status();
        } else {
            eprintln!("[native_sdk] show_alert: {title}: {message}");
        }
    }

    #[cfg(target_os = "windows")]
    {
        let wide_title = wide_string(title);
        let wide_message = wide_string(message);
        unsafe {
            MessageBoxW(
                None,
                windows::core::PCWSTR(wide_message.as_ptr()),
                windows::core::PCWSTR(wide_title.as_ptr()),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        eprintln!("[native_sdk] show_alert: {title}: {message}");
    }
}

pub fn show_copy_paste_context_menu(
    buffer_position_label: Option<String>,
    can_copy: bool,
    can_copy_image: bool,
    can_paste: bool,
    anchor: Option<NativeContextMenuAnchor>,
) -> Option<ContextMenuAction> {
    #[cfg(target_os = "macos")]
    {
        fn show_copy_paste_context_menu_on_main(
            mtm: MainThreadMarker,
            buffer_position_label: Option<String>,
            can_copy: bool,
            can_copy_image: bool,
            can_paste: bool,
            anchor: Option<NativeContextMenuAnchor>,
        ) -> Option<ContextMenuAction> {
            let app = NSApplication::sharedApplication(mtm);
            let _current_event = app.currentEvent()?;

            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);

            if let Some(buffer_position_label) = buffer_position_label.as_ref() {
                let buffer_position_item =
                    TermyContextMenuItem::new_with_action_id(mtm, buffer_position_label, 0, false);
                menu.addItem(&buffer_position_item);
            }

            let copy_item = TermyContextMenuItem::new_with_action_id(
                mtm,
                "Copy",
                CONTEXT_MENU_COPY_ID,
                can_copy,
            );
            let copy_image_item = can_copy_image.then(|| {
                TermyContextMenuItem::new_with_action_id(
                    mtm,
                    "Copy Image",
                    CONTEXT_MENU_COPY_IMAGE_ID,
                    true,
                )
            });
            let paste_item = TermyContextMenuItem::new_with_action_id(
                mtm,
                "Paste",
                CONTEXT_MENU_PASTE_ID,
                can_paste,
            );
            let copy_buffer_position_item = TermyContextMenuItem::new_with_action_id(
                mtm,
                "Copy Buffer Position",
                CONTEXT_MENU_COPY_BUFFER_POSITION_ID,
                buffer_position_label.is_some(),
            );
            let open_search_item = TermyContextMenuItem::new_with_action_id(
                mtm,
                "Open Search",
                CONTEXT_MENU_OPEN_SEARCH_ID,
                true,
            );

            menu.addItem(&copy_item);
            if let Some(copy_image_item) = copy_image_item.as_ref() {
                menu.addItem(copy_image_item);
            }
            menu.addItem(&paste_item);
            menu.addItem(&open_search_item);
            menu.addItem(&copy_buffer_position_item);

            CONTEXT_MENU_SELECTION.store(0, Ordering::Relaxed);
            let _ = pop_up_menu_at_anchor(&menu, anchor);

            context_menu_action_for_id(CONTEXT_MENU_SELECTION.swap(0, Ordering::Relaxed))
        }

        if let Some(mtm) = MainThreadMarker::new() {
            show_copy_paste_context_menu_on_main(
                mtm,
                buffer_position_label,
                can_copy,
                can_copy_image,
                can_paste,
                anchor,
            )
        } else {
            run_on_main(|mtm| {
                show_copy_paste_context_menu_on_main(
                    mtm,
                    buffer_position_label,
                    can_copy,
                    can_copy_image,
                    can_paste,
                    anchor,
                )
            })
        }
    }

    #[cfg(target_os = "windows")]
    {
        let _ = anchor;
        let menu = unsafe { CreatePopupMenu().ok()? };
        let _menu_guard = MenuGuard(menu);

        let has_buffer_position = buffer_position_label.is_some();
        if let Some(buffer_position_label) = buffer_position_label.as_ref() {
            let buffer_position_title = wide_string(buffer_position_label);
            unsafe {
                AppendMenuW(
                    menu,
                    MF_STRING | MF_GRAYED,
                    0,
                    windows::core::PCWSTR(buffer_position_title.as_ptr()),
                )
                .ok()?;
            }
        }

        let copy_title = wide_string("Copy");
        let copy_image_title = wide_string("Copy Image");
        let paste_title = wide_string("Paste");
        let open_search_title = wide_string("Open Search");
        let copy_buffer_position_title = wide_string("Copy Buffer Position");
        let copy_flags = if can_copy {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        let paste_flags = if can_paste {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        let copy_buffer_position_flags = if has_buffer_position {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };

        unsafe {
            AppendMenuW(
                menu,
                copy_flags,
                CONTEXT_MENU_COPY_ID as usize,
                windows::core::PCWSTR(copy_title.as_ptr()),
            )
            .ok()?;
            if can_copy_image {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    CONTEXT_MENU_COPY_IMAGE_ID as usize,
                    windows::core::PCWSTR(copy_image_title.as_ptr()),
                )
                .ok()?;
            }
            AppendMenuW(
                menu,
                paste_flags,
                CONTEXT_MENU_PASTE_ID as usize,
                windows::core::PCWSTR(paste_title.as_ptr()),
            )
            .ok()?;
            AppendMenuW(
                menu,
                MF_STRING,
                CONTEXT_MENU_OPEN_SEARCH_ID as usize,
                windows::core::PCWSTR(open_search_title.as_ptr()),
            )
            .ok()?;
            AppendMenuW(
                menu,
                copy_buffer_position_flags,
                CONTEXT_MENU_COPY_BUFFER_POSITION_ID as usize,
                windows::core::PCWSTR(copy_buffer_position_title.as_ptr()),
            )
            .ok()?;
        }

        let mut cursor = POINT::default();
        unsafe {
            GetCursorPos(&mut cursor).ok()?;
        }
        let owner: HWND = unsafe { GetForegroundWindow() };
        let result = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                cursor.x,
                cursor.y,
                Some(0),
                owner,
                None,
            )
            .0
        };

        return context_menu_action_for_id(result);
    }

    #[cfg(any(
        target_os = "linux",
        not(any(target_os = "macos", target_os = "windows"))
    ))]
    {
        let _ = (
            buffer_position_label,
            can_copy,
            can_copy_image,
            can_paste,
            anchor,
        );
        None
    }
}

pub fn show_tab_context_menu(
    pinned: bool,
    anchor: Option<NativeContextMenuAnchor>,
) -> Option<TabContextMenuAction> {
    #[cfg(target_os = "macos")]
    {
        fn show_tab_context_menu_on_main(
            mtm: MainThreadMarker,
            pinned: bool,
            anchor: Option<NativeContextMenuAnchor>,
        ) -> Option<TabContextMenuAction> {
            let app = NSApplication::sharedApplication(mtm);
            let _current_event = app.currentEvent()?;

            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);

            // Rename Tab
            let rename_item = TermyContextMenuItem::new_with_action_id(
                mtm,
                "Rename Tab",
                TAB_CONTEXT_MENU_RENAME_ID,
                true,
            );
            menu.addItem(&rename_item);

            // Pin/Unpin Tab
            let (pin_title, pin_action_id) = if pinned {
                ("Unpin Tab", TAB_CONTEXT_MENU_UNPIN_ID)
            } else {
                ("Pin Tab", TAB_CONTEXT_MENU_PIN_ID)
            };
            let pin_item =
                TermyContextMenuItem::new_with_action_id(mtm, pin_title, pin_action_id, true);
            menu.addItem(&pin_item);

            // Separator
            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // Close Tab
            let close_item = TermyContextMenuItem::new_with_action_id(
                mtm,
                "Close Tab",
                TAB_CONTEXT_MENU_CLOSE_ID,
                true,
            );
            menu.addItem(&close_item);

            CONTEXT_MENU_SELECTION.store(0, Ordering::Relaxed);
            let _ = pop_up_menu_at_anchor(&menu, anchor);

            match CONTEXT_MENU_SELECTION.swap(0, Ordering::Relaxed) {
                TAB_CONTEXT_MENU_RENAME_ID => Some(TabContextMenuAction::Rename),
                TAB_CONTEXT_MENU_PIN_ID => Some(TabContextMenuAction::Pin),
                TAB_CONTEXT_MENU_UNPIN_ID => Some(TabContextMenuAction::Unpin),
                TAB_CONTEXT_MENU_CLOSE_ID => Some(TabContextMenuAction::Close),
                _ => None,
            }
        }

        if let Some(mtm) = MainThreadMarker::new() {
            show_tab_context_menu_on_main(mtm, pinned, anchor)
        } else {
            run_on_main(|mtm| show_tab_context_menu_on_main(mtm, pinned, anchor))
        }
    }

    #[cfg(target_os = "windows")]
    {
        let _ = anchor;
        let menu = unsafe { CreatePopupMenu().ok()? };
        let _menu_guard = MenuGuard(menu);

        // Rename Tab
        let rename_title = wide_string("Rename Tab");
        unsafe {
            AppendMenuW(
                menu,
                MF_STRING,
                TAB_CONTEXT_MENU_RENAME_ID as usize,
                windows::core::PCWSTR(rename_title.as_ptr()),
            )
            .ok()?;
        }

        // Pin/Unpin Tab
        let (pin_title, pin_action_id) = if pinned {
            (wide_string("Unpin Tab"), TAB_CONTEXT_MENU_UNPIN_ID)
        } else {
            (wide_string("Pin Tab"), TAB_CONTEXT_MENU_PIN_ID)
        };
        unsafe {
            AppendMenuW(
                menu,
                MF_STRING,
                pin_action_id as usize,
                windows::core::PCWSTR(pin_title.as_ptr()),
            )
            .ok()?;
        }

        // Separator
        unsafe {
            AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null()).ok()?;
        }

        // Close Tab
        let close_title = wide_string("Close Tab");
        unsafe {
            AppendMenuW(
                menu,
                MF_STRING,
                TAB_CONTEXT_MENU_CLOSE_ID as usize,
                windows::core::PCWSTR(close_title.as_ptr()),
            )
            .ok()?;
        }

        let mut cursor = POINT::default();
        unsafe {
            GetCursorPos(&mut cursor).ok()?;
        }
        let owner: HWND = unsafe { GetForegroundWindow() };
        let result = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                cursor.x,
                cursor.y,
                Some(0),
                owner,
                None,
            )
            .0
        };

        return match result {
            TAB_CONTEXT_MENU_RENAME_ID => Some(TabContextMenuAction::Rename),
            TAB_CONTEXT_MENU_PIN_ID => Some(TabContextMenuAction::Pin),
            TAB_CONTEXT_MENU_UNPIN_ID => Some(TabContextMenuAction::Unpin),
            TAB_CONTEXT_MENU_CLOSE_ID => Some(TabContextMenuAction::Close),
            _ => None,
        };
    }

    #[cfg(any(
        target_os = "linux",
        not(any(target_os = "macos", target_os = "windows"))
    ))]
    {
        let _ = (pinned, anchor);
        None
    }
}

pub fn confirm(title: &str, message: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        run_on_main(|mtm| {
            let alert = NSAlert::new(mtm);
            let ns_title = NSString::from_str(title);
            let ns_message = NSString::from_str(message);
            let cancel = NSString::from_str("Cancel");
            let ok = NSString::from_str("OK");

            alert.setMessageText(&ns_title);
            alert.setInformativeText(&ns_message);
            let _ = alert.addButtonWithTitle(&cancel);
            let _ = alert.addButtonWithTitle(&ok);

            let response = alert.runModal();
            response == NSAlertSecondButtonReturn
        })
    }

    #[cfg(target_os = "linux")]
    {
        if has_command("zenity") {
            Command::new("zenity")
                .args(["--question", "--title", title, "--text", message])
                .status()
                .is_ok_and(|s| s.success())
        } else if has_command("kdialog") {
            Command::new("kdialog")
                .args(["--yesno", message, "--title", title])
                .status()
                .is_ok_and(|s| s.success())
        } else {
            eprintln!("[native_sdk] confirm: {title}: {message}");
            false
        }
    }

    #[cfg(target_os = "windows")]
    {
        let wide_title = wide_string(title);
        let wide_message = wide_string(message);
        let result = unsafe {
            MessageBoxW(
                None,
                windows::core::PCWSTR(wide_message.as_ptr()),
                windows::core::PCWSTR(wide_title.as_ptr()),
                MB_YESNO | MB_ICONINFORMATION,
            )
        };
        result == IDYES
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        eprintln!("[native_sdk] confirm: {title}: {message}");
        false
    }
}

pub fn request_clipboard_permission(
    title: &str,
    message: &str,
    can_remember: bool,
) -> ClipboardPermission {
    #[cfg(target_os = "macos")]
    {
        run_on_main(|mtm| {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(title));
            alert.setInformativeText(&NSString::from_str(message));
            let _ = alert.addButtonWithTitle(&NSString::from_str("Deny"));
            let _ = alert.addButtonWithTitle(&NSString::from_str("Allow Once"));
            if can_remember {
                let _ = alert.addButtonWithTitle(&NSString::from_str("Always Allow"));
            }
            let response = alert.runModal();
            if response == NSAlertSecondButtonReturn {
                ClipboardPermission::AllowOnce
            } else if can_remember && response == NSAlertThirdButtonReturn {
                ClipboardPermission::AllowAlways
            } else {
                ClipboardPermission::Deny
            }
        })
    }

    #[cfg(target_os = "linux")]
    {
        if has_command("zenity") {
            let mut command = Command::new("zenity");
            command.args([
                "--question",
                "--title",
                title,
                "--text",
                message,
                "--ok-label",
                "Allow Once",
                "--cancel-label",
                "Deny",
            ]);
            if can_remember {
                command.arg("--extra-button=Always Allow");
            }
            return match command.output() {
                Ok(output) if String::from_utf8_lossy(&output.stdout).trim() == "Always Allow" => {
                    ClipboardPermission::AllowAlways
                }
                Ok(output) if output.status.success() => ClipboardPermission::AllowOnce,
                _ => ClipboardPermission::Deny,
            };
        }
        if has_command("kdialog") {
            if can_remember {
                let status = Command::new("kdialog")
                    .args([
                        "--yesnocancel",
                        message,
                        "--title",
                        title,
                        "--yes-label",
                        "Allow Once",
                        "--no-label",
                        "Always Allow",
                        "--cancel-label",
                        "Deny",
                    ])
                    .status();
                return match status.ok().and_then(|status| status.code()) {
                    Some(0) => ClipboardPermission::AllowOnce,
                    Some(1) => ClipboardPermission::AllowAlways,
                    _ => ClipboardPermission::Deny,
                };
            }
            return if Command::new("kdialog")
                .args(["--yesno", message, "--title", title])
                .status()
                .is_ok_and(|status| status.success())
            {
                ClipboardPermission::AllowOnce
            } else {
                ClipboardPermission::Deny
            };
        }
        eprintln!("[native_sdk] clipboard permission: {title}: {message}");
        ClipboardPermission::Deny
    }

    #[cfg(target_os = "windows")]
    {
        let message = if can_remember {
            format!("{message}\n\nYes: Allow Once\nNo: Always Allow\nCancel: Deny")
        } else {
            format!("{message}\n\nYes: Allow Once\nNo: Deny")
        };
        let result = unsafe {
            MessageBoxW(
                None,
                windows::core::PCWSTR(wide_string(&message).as_ptr()),
                windows::core::PCWSTR(wide_string(title).as_ptr()),
                if can_remember {
                    MB_YESNOCANCEL | MB_ICONINFORMATION
                } else {
                    MB_YESNO | MB_ICONINFORMATION
                },
            )
        };
        if result == IDYES {
            ClipboardPermission::AllowOnce
        } else if can_remember && result == IDNO {
            ClipboardPermission::AllowAlways
        } else {
            ClipboardPermission::Deny
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (title, message, can_remember);
        ClipboardPermission::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_menu_ids_dispatch_to_their_actions() {
        assert_eq!(
            context_menu_action_for_id(CONTEXT_MENU_COPY_ID),
            Some(ContextMenuAction::Copy)
        );
        assert_eq!(
            context_menu_action_for_id(CONTEXT_MENU_COPY_IMAGE_ID),
            Some(ContextMenuAction::CopyImage)
        );
        assert_eq!(
            context_menu_action_for_id(CONTEXT_MENU_PASTE_ID),
            Some(ContextMenuAction::Paste)
        );
        assert_eq!(
            context_menu_action_for_id(CONTEXT_MENU_OPEN_SEARCH_ID),
            Some(ContextMenuAction::OpenSearch)
        );
        assert_eq!(
            context_menu_action_for_id(CONTEXT_MENU_COPY_BUFFER_POSITION_ID),
            Some(ContextMenuAction::CopyBufferPosition)
        );
        assert_eq!(context_menu_action_for_id(0), None);
    }
}
mod clipboard;

pub use clipboard::{
    NativeClipboardContent, NativeClipboardError, available_clipboard_formats,
    read_clipboard_formats, write_clipboard_contents,
};
