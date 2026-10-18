//! Rust side of the WinUI 2 tab strip (`native/windows/WinUiTabStrip.cpp`).
//!
//! The strip is a Microsoft.UI.Xaml `TabView` in a XAML Island: a child HWND
//! over the tab lane of a Termy window. Termy keeps its own tab model; this
//! wrapper pushes tabs, the lane rectangle and colors in (skipping pushes
//! that change nothing) and hands the strip's clicks out as
//! [`TabStripEvent`]s on a channel.

use std::ffi::c_void;
use std::ptr::NonNull;

use windows::Win32::Foundation::HWND;
use windows::core::HRESULT;

#[repr(C)]
struct RawEvent {
    kind: u32,
    tab_id: u64,
    index: i32,
    x: f32,
    y: f32,
    text: *const u16,
    text_len: u32,
}

#[repr(C)]
struct RawTab {
    id: u64,
    title: *const u16,
    title_len: u32,
}

/// Strip colors as `0xAARRGGBB`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabStripTheme {
    pub dark: i32,
    pub strip_background: u32,
    pub selected_background: u32,
    pub hover_background: u32,
    pub pressed_background: u32,
    pub selected_foreground: u32,
    pub foreground: u32,
}

type RawCallback = unsafe extern "C" fn(context: *mut c_void, event: *const RawEvent);

unsafe extern "C" {
    fn termy_winui_tab_strip_create(
        parent: *mut c_void,
        callback: RawCallback,
        context: *mut c_void,
        out_strip: *mut *mut c_void,
    ) -> HRESULT;
    fn termy_winui_tab_strip_destroy(strip: *mut c_void);
    fn termy_winui_tab_strip_set_items(
        strip: *mut c_void,
        items: *const RawTab,
        count: u32,
        selected: i32,
    );
    fn termy_winui_tab_strip_set_lane(
        strip: *mut c_void,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        visible: i32,
    );
    fn termy_winui_tab_strip_set_theme(strip: *mut c_void, theme: *const TabStripTheme);
    fn termy_winui_tab_strip_set_menu_button(strip: *mut c_void, visible: i32);
    fn termy_winui_tab_strip_begin_rename(strip: *mut c_void, tab_id: u64);
}

/// A click on the strip. Positions are in DIPs (GPUI logical pixels) from the
/// strip's top-left corner, except `DroppedOutside`, which is in physical
/// screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum TabStripEvent {
    Select {
        tab_id: u64,
    },
    Close {
        tab_id: u64,
    },
    NewTab,
    NewTabMenu {
        x: f32,
        y: f32,
    },
    ContextMenu {
        tab_id: u64,
        x: f32,
        y: f32,
    },
    Reorder {
        tab_id: u64,
        index: usize,
    },
    DroppedOutside {
        tab_id: u64,
        screen_x: f32,
        screen_y: f32,
    },
    /// Any press on the strip, before the event it may lead to.
    PointerDown,
    /// An in-place rename ([`WinUiTabStrip::begin_rename`]) finished.
    Rename {
        tab_id: u64,
        title: String,
    },
    RenameCancelled {
        tab_id: u64,
    },
}

impl TabStripEvent {
    fn from_raw(event: &RawEvent) -> Option<Self> {
        let tab_id = event.tab_id;
        Some(match event.kind {
            1 => Self::Select { tab_id },
            2 => Self::Close { tab_id },
            3 => Self::NewTab,
            4 => Self::NewTabMenu {
                x: event.x,
                y: event.y,
            },
            5 => Self::ContextMenu {
                tab_id,
                x: event.x,
                y: event.y,
            },
            6 => Self::Reorder {
                tab_id,
                index: usize::try_from(event.index).ok()?,
            },
            7 => Self::DroppedOutside {
                tab_id,
                screen_x: event.x,
                screen_y: event.y,
            },
            8 => Self::PointerDown,
            9 => Self::Rename {
                tab_id,
                title: if event.text.is_null() {
                    String::new()
                } else {
                    // SAFETY: the strip passes `text_len` UTF-16 units that
                    // stay valid for the duration of the callback.
                    String::from_utf16_lossy(unsafe {
                        std::slice::from_raw_parts(event.text, event.text_len as usize)
                    })
                },
            },
            10 => Self::RenameCancelled { tab_id },
            _ => return None,
        })
    }
}

unsafe extern "C" fn forward_event(context: *mut c_void, event: *const RawEvent) {
    // SAFETY: `context` is the sender boxed in `WinUiTabStrip::create`, which
    // outlives the strip; `event` is valid for the duration of the call.
    let (sender, event) = unsafe { (&*(context as *const flume::Sender<TabStripEvent>), &*event) };
    if let Some(event) = TabStripEvent::from_raw(event) {
        let _ = sender.send(event);
    }
}

/// The lane the strip covers, in physical client pixels of the parent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabStripLane {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub struct WinUiTabStrip {
    handle: NonNull<c_void>,
    sender: NonNull<flume::Sender<TabStripEvent>>,
    items: Vec<(u64, String)>,
    selected: Option<usize>,
    lane: Option<(TabStripLane, bool)>,
    theme: Option<TabStripTheme>,
    menu_button: Option<bool>,
}

impl WinUiTabStrip {
    /// Creates a hidden strip over `parent`. Fails when the XAML runtime is
    /// unavailable, e.g. termy.exe runs without termy.manifest embedded.
    pub fn create(parent: HWND) -> Result<(Self, flume::Receiver<TabStripEvent>), HRESULT> {
        let (sender, receiver) = flume::unbounded();
        let sender = NonNull::from(Box::leak(Box::new(sender)));
        let mut handle = std::ptr::null_mut();
        // SAFETY: `parent` is a live window owned by this thread, and the
        // callback context stays valid until `Drop` destroys the strip.
        let result = unsafe {
            termy_winui_tab_strip_create(
                parent.0,
                forward_event,
                sender.as_ptr().cast(),
                &mut handle,
            )
        };
        let Some(handle) = NonNull::new(handle).filter(|_| result.is_ok()) else {
            // SAFETY: the strip was not created, so nothing else holds it.
            drop(unsafe { Box::from_raw(sender.as_ptr()) });
            return Err(if result.is_ok() {
                windows::Win32::Foundation::E_FAIL
            } else {
                result
            });
        };
        Ok((
            Self {
                handle,
                sender,
                items: Vec::new(),
                selected: None,
                lane: None,
                theme: None,
                menu_button: None,
            },
            receiver,
        ))
    }

    pub fn set_items(&mut self, items: &[(u64, &str)], selected: Option<usize>) {
        let unchanged = self.selected == selected
            && self.items.len() == items.len()
            && self
                .items
                .iter()
                .zip(items)
                .all(|((id, title), (new_id, new_title))| id == new_id && title == new_title);
        if unchanged {
            return;
        }
        self.items = items
            .iter()
            .map(|(id, title)| (*id, (*title).to_string()))
            .collect();
        self.selected = selected;

        let titles: Vec<Vec<u16>> = items
            .iter()
            .map(|(_, title)| title.encode_utf16().collect())
            .collect();
        let raw: Vec<RawTab> = items
            .iter()
            .zip(&titles)
            .map(|((id, _), title)| RawTab {
                id: *id,
                title: title.as_ptr(),
                title_len: title.len() as u32,
            })
            .collect();
        let selected = selected
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1);
        // SAFETY: `raw` and the titles it points into live across the call.
        unsafe {
            termy_winui_tab_strip_set_items(
                self.handle.as_ptr(),
                raw.as_ptr(),
                raw.len() as u32,
                selected,
            );
        }
    }

    /// Makes the next `set_items` push even if the tabs look unchanged, for
    /// when the strip itself may have changed (a click Termy then refused).
    pub fn invalidate_items(&mut self) {
        self.items.clear();
        self.selected = None;
    }

    pub fn set_lane(&mut self, lane: TabStripLane, visible: bool) {
        if self.lane == Some((lane, visible)) {
            return;
        }
        self.lane = Some((lane, visible));
        // SAFETY: the handle is live until `Drop`.
        unsafe {
            termy_winui_tab_strip_set_lane(
                self.handle.as_ptr(),
                lane.x,
                lane.y,
                lane.width,
                lane.height,
                i32::from(visible),
            );
        }
    }

    pub fn hide(&mut self) {
        let lane = self.lane.map(|(lane, _)| lane).unwrap_or_default();
        self.set_lane(lane, false);
    }

    pub fn is_visible(&self) -> bool {
        self.lane.is_some_and(|(_, visible)| visible)
    }

    pub fn set_theme(&mut self, theme: TabStripTheme) {
        if self.theme == Some(theme) {
            return;
        }
        self.theme = Some(theme);
        // SAFETY: the handle is live until `Drop`; `theme` outlives the call.
        unsafe { termy_winui_tab_strip_set_theme(self.handle.as_ptr(), &theme) };
    }

    /// Turns the tab's title into a text box. The result comes back as
    /// [`TabStripEvent::Rename`] or [`TabStripEvent::RenameCancelled`].
    pub fn begin_rename(&mut self, tab_id: u64) {
        // SAFETY: the handle is live until `Drop`.
        unsafe { termy_winui_tab_strip_begin_rename(self.handle.as_ptr(), tab_id) };
    }

    pub fn set_menu_button(&mut self, visible: bool) {
        if self.menu_button == Some(visible) {
            return;
        }
        self.menu_button = Some(visible);
        // SAFETY: the handle is live until `Drop`.
        unsafe { termy_winui_tab_strip_set_menu_button(self.handle.as_ptr(), i32::from(visible)) };
    }
}

impl Drop for WinUiTabStrip {
    fn drop(&mut self) {
        // SAFETY: the strip stops calling back once destroyed, after which
        // nothing references the sender.
        unsafe {
            termy_winui_tab_strip_destroy(self.handle.as_ptr());
            drop(Box::from_raw(self.sender.as_ptr()));
        }
    }
}

/// `0xAARRGGBB` from a GPUI color.
pub fn argb(color: gpui_kit::Rgba) -> u32 {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    (channel(color.a) << 24) | (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argb_packs_channels_in_order() {
        let color = gpui_kit::Rgba {
            r: 1.0,
            g: 0.5,
            b: 0.0,
            a: 1.0,
        };
        assert_eq!(argb(color), 0xFFFF8000);
    }

    #[test]
    fn raw_events_map_to_tab_strip_events() {
        let raw = |kind, index| RawEvent {
            kind,
            tab_id: 7,
            index,
            x: 1.0,
            y: 2.0,
            text: std::ptr::null(),
            text_len: 0,
        };
        assert_eq!(
            TabStripEvent::from_raw(&raw(1, -1)),
            Some(TabStripEvent::Select { tab_id: 7 })
        );
        assert_eq!(
            TabStripEvent::from_raw(&raw(6, 3)),
            Some(TabStripEvent::Reorder {
                tab_id: 7,
                index: 3
            })
        );
        assert_eq!(TabStripEvent::from_raw(&raw(6, -1)), None);
        let title: Vec<u16> = "logs".encode_utf16().collect();
        let rename = RawEvent {
            text: title.as_ptr(),
            text_len: title.len() as u32,
            ..raw(9, -1)
        };
        assert_eq!(
            TabStripEvent::from_raw(&rename),
            Some(TabStripEvent::Rename {
                tab_id: 7,
                title: "logs".into()
            })
        );
        assert_eq!(TabStripEvent::from_raw(&raw(99, 0)), None);
    }
}
