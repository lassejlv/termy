// WinUI 2 tab strip for `tab_style = native` on Windows.
//
// Hosts a Microsoft.UI.Xaml TabView (the control Windows Terminal uses) in a
// XAML Island: a child HWND covering the tab lane of Termy's titlebar. Rust
// owns the tab model. It pushes tabs, bounds and colors in through the C API
// below, and the strip reports clicks back through a callback without
// changing its own items; the next push applies the result.
//
// No C++ exception crosses the C API: every entry point catches and returns
// an HRESULT (or nothing, for best-effort updates).

#include <windows.h>
#include <unknwn.h>
#undef GetCurrentTime

#include <cmath>
#include <cstdint>
#include <string>
#include <vector>

#include <windows.ui.xaml.hosting.desktopwindowxamlsource.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.System.h>
#include <winrt/Windows.UI.h>
#include <winrt/Windows.UI.Input.h>
#include <winrt/Windows.UI.Text.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Controls.Primitives.h>
#include <winrt/Windows.UI.Xaml.Hosting.h>
#include <winrt/Windows.UI.Xaml.Input.h>
#include <winrt/Windows.UI.Xaml.Interop.h>
#include <winrt/Windows.UI.Xaml.Markup.h>
#include <winrt/Windows.UI.Xaml.Media.h>
#include <winrt/Microsoft.UI.Xaml.Controls.h>
#include <winrt/Microsoft.UI.Xaml.XamlTypeInfo.h>

namespace wf = winrt::Windows::Foundation;
namespace wux = winrt::Windows::UI::Xaml;
namespace wuxc = winrt::Windows::UI::Xaml::Controls;
namespace muxc = winrt::Microsoft::UI::Xaml::Controls;

extern "C" {

enum TermyWinUiTabEventKind : uint32_t {
    TERMY_WINUI_TAB_SELECT = 1,
    TERMY_WINUI_TAB_CLOSE = 2,
    TERMY_WINUI_TAB_NEW = 3,
    TERMY_WINUI_TAB_NEW_MENU = 4,
    TERMY_WINUI_TAB_CONTEXT_MENU = 5,
    TERMY_WINUI_TAB_REORDER = 6,
    TERMY_WINUI_TAB_DROPPED_OUTSIDE = 7,
    // Any press on the strip; Termy closes its open menus.
    TERMY_WINUI_TAB_POINTER_DOWN = 8,
    // An inline rename finished: `text` holds the new title.
    TERMY_WINUI_TAB_RENAME = 9,
    TERMY_WINUI_TAB_RENAME_CANCEL = 10,
};

// `x`/`y` are in DIPs relative to the strip's top-left corner. `text` is
// valid for the duration of the callback.
struct TermyWinUiTabEvent {
    uint32_t kind;
    uint64_t tab_id;
    int32_t index;
    float x;
    float y;
    const uint16_t* text;
    uint32_t text_len;
};

typedef void (*TermyWinUiTabCallback)(void* context, const TermyWinUiTabEvent* event);

struct TermyWinUiTab {
    uint64_t id;
    const uint16_t* title;
    uint32_t title_len;
};

// Colors are 0xAARRGGBB.
struct TermyWinUiTabTheme {
    int32_t dark;
    uint32_t strip_background;
    uint32_t selected_background;
    uint32_t hover_background;
    uint32_t pressed_background;
    uint32_t selected_foreground;
    uint32_t foreground;
};

}  // extern "C"

namespace {

// Hands Microsoft.UI.Xaml's type information to the XAML runtime, which is
// what lets XamlControlsResources and TabView's templates load.
struct XamlApp : wux::ApplicationT<XamlApp, wux::Markup::IXamlMetadataProvider> {
    winrt::Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider provider;

    wux::Markup::IXamlType GetXamlType(wux::Interop::TypeName const& type) {
        return provider.GetXamlType(type);
    }
    wux::Markup::IXamlType GetXamlType(winrt::hstring const& name) {
        return provider.GetXamlType(name);
    }
    winrt::com_array<wux::Markup::XmlnsDefinition> GetXmlnsDefinitions() {
        return provider.GetXmlnsDefinitions();
    }
};

// One XAML runtime per process, on the UI thread, kept alive for its lifetime.
struct XamlRuntime {
    wux::Application app{nullptr};
    wux::Hosting::WindowsXamlManager manager{nullptr};
};

XamlRuntime* g_runtime = nullptr;
HRESULT g_init_result = S_FALSE;

// How far a press on the empty strip moves before it drags the window.
constexpr double kDragThreshold = 4.0;
constexpr wchar_t kTabGlyph[] = L"\uE756";      // CommandPrompt
constexpr wchar_t kChevronGlyph[] = L"\uE70D";  // ChevronDown

winrt::Windows::UI::Color ColorFromArgb(uint32_t argb) {
    return winrt::Windows::UI::ColorHelper::FromArgb(
        static_cast<uint8_t>(argb >> 24), static_cast<uint8_t>(argb >> 16),
        static_cast<uint8_t>(argb >> 8), static_cast<uint8_t>(argb));
}

wux::Media::SolidColorBrush Brush(uint32_t argb) {
    return wux::Media::SolidColorBrush(ColorFromArgb(argb));
}

winrt::hstring IconFontFamily() {
    // Segoe Fluent Icons is the Windows 11 icon face; Windows 10 has MDL2,
    // which carries the same code points.
    OSVERSIONINFOEXW version{sizeof(version)};
    version.dwBuildNumber = 22000;
    DWORDLONG mask = VerSetConditionMask(0, VER_BUILDNUMBER, VER_GREATER_EQUAL);
    bool windows11 = VerifyVersionInfoW(&version, VER_BUILDNUMBER, mask) != FALSE;
    return windows11 ? L"Segoe Fluent Icons" : L"Segoe MDL2 Assets";
}

struct TabStrip {
    HWND parent = nullptr;
    HWND island = nullptr;
    TermyWinUiTabCallback callback = nullptr;
    void* context = nullptr;

    wux::Hosting::DesktopWindowXamlSource source{nullptr};
    wuxc::Grid root{nullptr};
    muxc::TabView tabs{nullptr};
    wuxc::Button menu_button{nullptr};

    // Item ids in strip order, mirroring `tabs.TabItems()`.
    std::vector<uint64_t> ids;
    // Set while applying a push from Rust, so the strip's own change events
    // are not echoed back as user actions.
    bool syncing = false;
    bool visible = false;
    RECT lane{};
    // A left press on the empty strip, which turns into a window drag once
    // the pointer moves past kDragThreshold.
    bool drag_armed = false;
    wf::Point drag_origin{};
    // The tab being renamed in place, and its title before the edit.
    uint64_t renaming = 0;
    winrt::hstring rename_original;

    void Emit(TermyWinUiTabEvent const& event) const {
        if (callback) {
            callback(context, &event);
        }
    }

    void EmitAt(uint32_t kind, uint64_t tab_id, wf::Point point) const {
        Emit({kind, tab_id, -1, point.X, point.Y, nullptr, 0});
    }

    // Termy keeps keyboard focus: a click on the strip must not leave the
    // terminal without it.
    void ReturnFocus() const {
        if (parent && GetFocus() != parent) {
            SetFocus(parent);
        }
    }

    static uint64_t IdOf(wf::IInspectable const& item) {
        auto element = item.try_as<wux::FrameworkElement>();
        if (!element) {
            return 0;
        }
        return winrt::unbox_value_or<uint64_t>(element.Tag(), 0);
    }

    double Scale() const {
        UINT dpi = parent ? GetDpiForWindow(parent) : 96;
        return dpi ? dpi / 96.0 : 1.0;
    }

    // A tab's tooltip is a popup above Termy's window, so it would cover the
    // context menu Termy opens under the strip.
    void CloseToolTips() const {
        if (!root.XamlRoot()) {
            return;
        }
        auto popups = wux::Media::VisualTreeHelper::GetOpenPopupsForXamlRoot(root.XamlRoot());
        for (auto const& popup : popups) {
            if (popup.Child().try_as<wuxc::ToolTip>()) {
                popup.IsOpen(false);
            }
        }
    }

    muxc::TabViewItem MakeItem(uint64_t id, winrt::hstring const& title) {
        muxc::TabViewItem item;
        item.Tag(winrt::box_value(id));
        item.Header(winrt::box_value(title));
        item.IsTabStop(false);
        wuxc::ToolTipService::SetToolTip(item, winrt::box_value(title));
        muxc::FontIconSource icon;
        icon.Glyph(kTabGlyph);
        icon.FontFamily(wux::Media::FontFamily(IconFontFamily()));
        item.IconSource(icon);
        item.RightTapped([this](wf::IInspectable const& sender,
                                wux::Input::RightTappedRoutedEventArgs const& args) {
            args.Handled(true);
            CloseToolTips();
            EmitAt(TERMY_WINUI_TAB_CONTEXT_MENU, IdOf(sender), args.GetPosition(root));
        });
        return item;
    }

    void Build() {
        root = wuxc::Grid();
        root.AllowFocusOnInteraction(false);

        tabs = muxc::TabView();
        tabs.IsTabStop(false);
        tabs.AllowFocusOnInteraction(false);
        tabs.CanReorderTabs(true);
        tabs.CanDragTabs(true);
        tabs.AllowDropTabs(false);
        tabs.IsAddTabButtonVisible(true);
        tabs.TabWidthMode(muxc::TabViewWidthMode::Equal);
        tabs.VerticalAlignment(wux::VerticalAlignment::Bottom);
        tabs.HorizontalAlignment(wux::HorizontalAlignment::Stretch);
        // Tabs sit flush on the bottom edge of the titlebar row.
        tabs.Resources().Insert(winrt::box_value(L"TabViewHeaderPadding"),
                                winrt::box_value(wux::Thickness{0, 4, 0, 0}));

        menu_button = wuxc::Button();
        menu_button.IsTabStop(false);
        menu_button.AllowFocusOnInteraction(false);
        menu_button.Visibility(wux::Visibility::Collapsed);
        menu_button.VerticalAlignment(wux::VerticalAlignment::Center);
        menu_button.Width(24);
        menu_button.Height(24);
        menu_button.Padding(wux::Thickness{0, 0, 0, 0});
        menu_button.Margin(wux::Thickness{0, 0, 0, 0});
        menu_button.BorderThickness(wux::Thickness{0, 0, 0, 0});
        menu_button.Background(Brush(0x00000000));
        wuxc::FontIcon chevron;
        chevron.Glyph(kChevronGlyph);
        chevron.FontSize(9);
        chevron.FontFamily(wux::Media::FontFamily(IconFontFamily()));
        menu_button.Content(chevron);
        tabs.TabStripFooter(menu_button);

        tabs.SelectionChanged([this](auto&&, auto&&) {
            if (syncing) {
                return;
            }
            uint64_t id = IdOf(tabs.SelectedItem());
            if (id) {
                Emit({TERMY_WINUI_TAB_SELECT, id, -1, 0, 0, nullptr, 0});
            }
            ReturnFocus();
        });
        tabs.TabCloseRequested(
            [this](muxc::TabView const&, muxc::TabViewTabCloseRequestedEventArgs const& args) {
                Emit({TERMY_WINUI_TAB_CLOSE, IdOf(args.Tab()), -1, 0, 0, nullptr, 0});
                ReturnFocus();
            });
        tabs.AddTabButtonClick([this](auto&&, auto&&) {
            Emit({TERMY_WINUI_TAB_NEW, 0, -1, 0, 0, nullptr, 0});
            ReturnFocus();
        });
        menu_button.Click([this](auto&&, auto&&) {
            auto origin = menu_button.TransformToVisual(root).TransformPoint({0, 0});
            origin.Y = static_cast<float>(root.ActualHeight());
            EmitAt(TERMY_WINUI_TAB_NEW_MENU, 0, origin);
            ReturnFocus();
        });
        tabs.TabDragCompleted(
            [this](muxc::TabView const&, muxc::TabViewTabDragCompletedEventArgs const& args) {
                uint64_t id = IdOf(args.Tab());
                uint32_t index = 0;
                if (id && tabs.TabItems().IndexOf(args.Tab(), index)) {
                    Emit({TERMY_WINUI_TAB_REORDER, id, static_cast<int32_t>(index), 0, 0,
                          nullptr, 0});
                }
                ReturnFocus();
            });
        tabs.TabDroppedOutside(
            [this](muxc::TabView const&, muxc::TabViewTabDroppedOutsideEventArgs const& args) {
                POINT cursor{};
                GetCursorPos(&cursor);
                Emit({TERMY_WINUI_TAB_DROPPED_OUTSIDE, IdOf(args.Tab()), -1,
                      static_cast<float>(cursor.x), static_cast<float>(cursor.y), nullptr, 0});
            });

        // The strip's empty space is titlebar: it drags the window and a
        // double-click maximizes or restores it.
        root.AddHandler(
            wux::UIElement::PointerPressedEvent(),
            winrt::box_value(wux::Input::PointerEventHandler(
                [this](wf::IInspectable const&, wux::Input::PointerRoutedEventArgs const& args) {
                    Emit({TERMY_WINUI_TAB_POINTER_DOWN, 0, -1, 0, 0, nullptr, 0});
                    auto point = args.GetCurrentPoint(root);
                    drag_armed = point.Properties().IsLeftButtonPressed() &&
                                 !IsInteractive(args.OriginalSource());
                    drag_origin = point.Position();
                })),
            true);
        root.AddHandler(
            wux::UIElement::PointerMovedEvent(),
            winrt::box_value(wux::Input::PointerEventHandler(
                [this](wf::IInspectable const&, wux::Input::PointerRoutedEventArgs const& args) {
                    if (!drag_armed) {
                        return;
                    }
                    auto point = args.GetCurrentPoint(root);
                    if (!point.Properties().IsLeftButtonPressed()) {
                        drag_armed = false;
                        return;
                    }
                    auto position = point.Position();
                    if (std::abs(position.X - drag_origin.X) < kDragThreshold &&
                        std::abs(position.Y - drag_origin.Y) < kDragThreshold) {
                        return;
                    }
                    drag_armed = false;
                    ReleaseCapture();
                    PostMessageW(parent, WM_SYSCOMMAND, SC_MOVE | HTCAPTION, 0);
                })),
            true);
        root.AddHandler(
            wux::UIElement::PointerReleasedEvent(),
            winrt::box_value(wux::Input::PointerEventHandler(
                [this](auto&&, auto&&) { drag_armed = false; })),
            true);
        root.DoubleTapped([this](wf::IInspectable const&,
                                 wux::Input::DoubleTappedRoutedEventArgs const& args) {
            if (IsInteractive(args.OriginalSource())) {
                return;
            }
            PostMessageW(parent, WM_SYSCOMMAND, IsZoomed(parent) ? SC_RESTORE : SC_MAXIMIZE, 0);
        });

        root.Children().Append(tabs);
        source.Content(root);
    }

    // Whether `source` is part of a tab or a button, rather than the strip's
    // empty space.
    bool IsInteractive(wf::IInspectable const& source) const {
        auto node = source.try_as<wux::DependencyObject>();
        while (node && node != root) {
            if (node.try_as<muxc::TabViewItem>() ||
                node.try_as<wuxc::Primitives::ButtonBase>()) {
                return true;
            }
            node = wux::Media::VisualTreeHelper::GetParent(node);
        }
        return false;
    }

    void Layout() {
        if (!island) {
            return;
        }
        int width = lane.right - lane.left;
        int height = lane.bottom - lane.top;
        if (!visible || width <= 0 || height <= 0) {
            ShowWindow(island, SW_HIDE);
            return;
        }
        tabs.Height(height / Scale());
        SetWindowPos(island, HWND_TOP, lane.left, lane.top, width, height,
                     SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }

    // GPUI's message loop first sends each WM_KEYDOWN to its target as a
    // private WM_GPUI_KEYDOWN and drops the key when the reply is 0, which
    // any window GPUI does not own gives. While the island takes typing, a
    // message hook hands its keydowns to XAML directly and leaves GPUI a
    // WM_NULL. (Subclassing the island's input window instead makes
    // InputHost.dll fail fast.)
    void HookKeyboard() {
        if (g_keyboard_hook) {
            return;
        }
        g_keyboard_strip = this;
        g_keyboard_hook =
            SetWindowsHookExW(WH_GETMESSAGE, IslandKeyboardHook, nullptr, GetCurrentThreadId());
    }

    void UnhookKeyboard() {
        if (g_keyboard_strip != this) {
            return;
        }
        if (g_keyboard_hook) {
            UnhookWindowsHookEx(g_keyboard_hook);
        }
        g_keyboard_hook = nullptr;
        g_keyboard_strip = nullptr;
    }

    static inline HHOOK g_keyboard_hook = nullptr;
    static inline TabStrip* g_keyboard_strip = nullptr;

    static LRESULT CALLBACK IslandKeyboardHook(int code, WPARAM wparam, LPARAM lparam) {
        auto msg = reinterpret_cast<MSG*>(lparam);
        auto strip = g_keyboard_strip;
        if (code == HC_ACTION && wparam == PM_REMOVE && strip &&
            (msg->message == WM_KEYDOWN || msg->message == WM_SYSKEYDOWN) &&
            (msg->hwnd == strip->island || IsChild(strip->island, msg->hwnd))) {
            BOOL handled = FALSE;
            if (auto native = strip->source.try_as<IDesktopWindowXamlSourceNative2>()) {
                native->PreTranslateMessage(msg, &handled);
            }
            if (!handled) {
                TranslateMessage(msg);
                DispatchMessageW(msg);
            }
            msg->message = WM_NULL;
        }
        return CallNextHookEx(nullptr, code, wparam, lparam);
    }

    muxc::TabViewItem ItemOf(uint64_t id) const {
        for (size_t i = 0; i < ids.size(); ++i) {
            if (ids[i] == id) {
                return tabs.TabItems().GetAt(static_cast<uint32_t>(i)).as<muxc::TabViewItem>();
            }
        }
        return nullptr;
    }

    // Edits the tab's title in place, like Windows Terminal: Enter or
    // leaving the box keeps the text, Escape restores the old title.
    void BeginRename(uint64_t id) {
        if (renaming) {
            FinishRename(true);
        }
        auto item = ItemOf(id);
        if (!item) {
            return;
        }
        renaming = id;
        rename_original = winrt::unbox_value_or<winrt::hstring>(item.Header(), L"");
        wuxc::TextBox box;
        box.Text(rename_original);
        box.AllowFocusOnInteraction(true);
        box.MinWidth(0);
        box.Padding(wux::Thickness{4, 0, 4, 0});
        box.VerticalAlignment(wux::VerticalAlignment::Center);
        box.KeyDown([this](wf::IInspectable const&, wux::Input::KeyRoutedEventArgs const& args) {
            if (args.Key() == winrt::Windows::System::VirtualKey::Enter) {
                args.Handled(true);
                FinishRename(true);
            } else if (args.Key() == winrt::Windows::System::VirtualKey::Escape) {
                args.Handled(true);
                FinishRename(false);
            }
        });
        box.LostFocus([this](auto&&, auto&&) { FinishRename(true); });
        item.Header(box);
        HookKeyboard();
        // Keyboard input reaches the island only once it holds Win32 focus,
        // which NavigateFocus hands it.
        source.NavigateFocus(wux::Hosting::XamlSourceFocusNavigationRequest(
            wux::Hosting::XamlSourceFocusNavigationReason::Programmatic));
        // The box joins the visual tree on the next layout pass; focusing
        // it before then lands on the next focusable element ("+").
        box.Loaded([box](auto&&, auto&&) {
            box.Focus(wux::FocusState::Programmatic);
            box.SelectAll();
        });
    }

    void FinishRename(bool commit) {
        uint64_t id = renaming;
        if (!id) {
            return;
        }
        renaming = 0;
        UnhookKeyboard();
        winrt::hstring text = rename_original;
        if (auto item = ItemOf(id)) {
            if (auto box = item.Header().try_as<wuxc::TextBox>()) {
                text = box.Text();
            }
            item.Header(winrt::box_value(rename_original));
        }
        if (commit) {
            Emit({TERMY_WINUI_TAB_RENAME, id, -1, 0, 0,
                  reinterpret_cast<uint16_t const*>(text.c_str()), text.size()});
        } else {
            Emit({TERMY_WINUI_TAB_RENAME_CANCEL, id, -1, 0, 0, nullptr, 0});
        }
        ReturnFocus();
    }

    void SetItems(TermyWinUiTab const* items, uint32_t count, int32_t selected) {
        syncing = true;
        auto collection = tabs.TabItems();
        for (uint32_t i = 0; i < count; ++i) {
            uint64_t id = items[i].id;
            winrt::hstring title(reinterpret_cast<wchar_t const*>(items[i].title),
                                 items[i].title_len);
            if (i < ids.size() && ids[i] == id) {
                auto item = collection.GetAt(i).as<muxc::TabViewItem>();
                if (id == renaming) {
                    rename_original = title;
                } else if (winrt::unbox_value_or<winrt::hstring>(item.Header(), L"") != title) {
                    item.Header(winrt::box_value(title));
                    wuxc::ToolTipService::SetToolTip(item, winrt::box_value(title));
                }
                continue;
            }
            // Move the item here if it exists further on, otherwise make it.
            muxc::TabViewItem item{nullptr};
            for (size_t j = i + 1; j < ids.size(); ++j) {
                if (ids[j] == id) {
                    item = collection.GetAt(static_cast<uint32_t>(j)).as<muxc::TabViewItem>();
                    collection.RemoveAt(static_cast<uint32_t>(j));
                    ids.erase(ids.begin() + j);
                    break;
                }
            }
            if (!item) {
                item = MakeItem(id, title);
            } else {
                item.Header(winrt::box_value(title));
                wuxc::ToolTipService::SetToolTip(item, winrt::box_value(title));
            }
            collection.InsertAt(i, item);
            ids.insert(ids.begin() + i, id);
        }
        while (ids.size() > count) {
            collection.RemoveAt(static_cast<uint32_t>(ids.size() - 1));
            ids.pop_back();
        }
        if (selected >= 0 && static_cast<uint32_t>(selected) < count &&
            tabs.SelectedIndex() != selected) {
            tabs.SelectedIndex(selected);
        }
        syncing = false;
        Layout();
    }

    void SetTheme(TermyWinUiTabTheme const& theme) {
        auto resources = tabs.Resources();
        auto set = [&](wchar_t const* key, uint32_t argb) {
            resources.Insert(winrt::box_value(key), Brush(argb));
        };
        set(L"TabViewBackground", theme.strip_background);
        set(L"TabViewItemHeaderBackground", 0x00000000);
        set(L"TabViewItemHeaderBackgroundSelected", theme.selected_background);
        set(L"TabViewItemHeaderBackgroundPointerOver", theme.hover_background);
        set(L"TabViewItemHeaderBackgroundPressed", theme.pressed_background);
        set(L"TabViewItemHeaderForeground", theme.foreground);
        set(L"TabViewItemHeaderForegroundSelected", theme.selected_foreground);
        set(L"TabViewItemHeaderForegroundPointerOver", theme.selected_foreground);
        set(L"TabViewItemHeaderForegroundPressed", theme.selected_foreground);
        set(L"TabViewItemIconForeground", theme.foreground);
        set(L"TabViewItemIconForegroundSelected", theme.selected_foreground);
        set(L"TabViewItemIconForegroundPointerOver", theme.selected_foreground);
        set(L"TabViewItemIconForegroundPressed", theme.selected_foreground);
        set(L"TabViewItemHeaderCloseButtonForeground", theme.foreground);
        set(L"TabViewButtonForeground", theme.foreground);
        set(L"TabViewButtonBackgroundPointerOver", theme.hover_background);
        set(L"TabViewButtonBackgroundPressed", theme.pressed_background);
        set(L"TabViewButtonForegroundPointerOver", theme.selected_foreground);
        set(L"TabViewButtonForegroundPressed", theme.selected_foreground);
        root.Background(Brush(theme.strip_background));
        menu_button.Foreground(Brush(theme.foreground));

        // Theme resources resolve when a theme is applied, so flip through
        // the other theme to make the new brushes take effect.
        auto target = theme.dark ? wux::ElementTheme::Dark : wux::ElementTheme::Light;
        root.RequestedTheme(theme.dark ? wux::ElementTheme::Light : wux::ElementTheme::Dark);
        root.RequestedTheme(target);
    }
};

}  // namespace

extern "C" {

// Starts the XAML runtime on the calling (UI) thread. Idempotent; returns the
// first attempt's result. Fails when the exe manifest lacks the XAML Islands
// entries or Microsoft.UI.Xaml.dll is missing.
HRESULT termy_winui_initialize() {
    if (g_init_result != S_FALSE) {
        return g_init_result;
    }
    try {
        HRESULT com = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
        if (FAILED(com) && com != RPC_E_CHANGED_MODE) {
            winrt::throw_hresult(com);
        }
        auto runtime = new XamlRuntime();
        runtime->app = winrt::make<XamlApp>();
        runtime->manager = wux::Hosting::WindowsXamlManager::InitializeForCurrentThread();
        runtime->app.Resources().MergedDictionaries().Append(muxc::XamlControlsResources());
        runtime->app.UnhandledException(
            [](auto&&, wux::UnhandledExceptionEventArgs const& args) { args.Handled(true); });
        g_runtime = runtime;
        g_init_result = S_OK;
    } catch (...) {
        g_init_result = winrt::to_hresult();
        if (g_init_result == S_OK || g_init_result == S_FALSE) {
            g_init_result = E_FAIL;
        }
    }
    return g_init_result;
}

HRESULT termy_winui_tab_strip_create(HWND parent, TermyWinUiTabCallback callback, void* context,
                                     void** out_strip) {
    if (!out_strip) {
        return E_POINTER;
    }
    *out_strip = nullptr;
    HRESULT init = termy_winui_initialize();
    if (FAILED(init)) {
        return init;
    }
    TabStrip* strip = nullptr;
    try {
        strip = new TabStrip();
        strip->parent = parent;
        strip->callback = callback;
        strip->context = context;
        strip->source = wux::Hosting::DesktopWindowXamlSource();
        auto native = strip->source.as<IDesktopWindowXamlSourceNative>();
        winrt::check_hresult(native->AttachToWindow(parent));
        winrt::check_hresult(native->get_WindowHandle(&strip->island));
        strip->Build();
        ShowWindow(strip->island, SW_HIDE);
        *out_strip = strip;
        return S_OK;
    } catch (...) {
        HRESULT hr = winrt::to_hresult();
        if (strip) {
            try {
                if (strip->source) {
                    strip->source.Close();
                }
            } catch (...) {
            }
            delete strip;
        }
        return FAILED(hr) ? hr : E_FAIL;
    }
}

void termy_winui_tab_strip_destroy(void* handle) {
    auto strip = static_cast<TabStrip*>(handle);
    if (!strip) {
        return;
    }
    strip->callback = nullptr;
    strip->UnhookKeyboard();
    try {
        strip->source.Close();
    } catch (...) {
    }
    delete strip;
}

void termy_winui_tab_strip_set_items(void* handle, TermyWinUiTab const* items, uint32_t count,
                                     int32_t selected) {
    auto strip = static_cast<TabStrip*>(handle);
    if (!strip || (!items && count)) {
        return;
    }
    try {
        strip->SetItems(items, count, selected);
    } catch (...) {
        strip->syncing = false;
    }
}

// `x`/`y`/`width`/`height`: the tab lane in the parent's client area, in
// physical pixels. A zero-sized lane hides the strip.
void termy_winui_tab_strip_set_lane(void* handle, int32_t x, int32_t y, int32_t width,
                                    int32_t height, int32_t visible) {
    auto strip = static_cast<TabStrip*>(handle);
    if (!strip) {
        return;
    }
    RECT lane{x, y, x + width, y + height};
    bool show = visible != 0;
    if (EqualRect(&lane, &strip->lane) && show == strip->visible) {
        return;
    }
    strip->lane = lane;
    strip->visible = show;
    try {
        strip->Layout();
    } catch (...) {
    }
}

void termy_winui_tab_strip_set_theme(void* handle, TermyWinUiTabTheme const* theme) {
    auto strip = static_cast<TabStrip*>(handle);
    if (!strip || !theme) {
        return;
    }
    try {
        strip->SetTheme(*theme);
    } catch (...) {
    }
}

void termy_winui_tab_strip_begin_rename(void* handle, uint64_t tab_id) {
    auto strip = static_cast<TabStrip*>(handle);
    if (!strip) {
        return;
    }
    try {
        strip->BeginRename(tab_id);
    } catch (...) {
    }
}

void termy_winui_tab_strip_set_menu_button(void* handle, int32_t visible) {
    auto strip = static_cast<TabStrip*>(handle);
    if (!strip) {
        return;
    }
    try {
        strip->menu_button.Visibility(visible ? wux::Visibility::Visible
                                              : wux::Visibility::Collapsed);
        strip->Layout();
    } catch (...) {
    }
}

}  // extern "C"
