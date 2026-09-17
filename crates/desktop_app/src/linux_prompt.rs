//! Modal replacement for GPUI 0.2.2's non-native Linux prompt fallback.

use gpui::{
    App, AppContext, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, PromptButton, PromptHandle, PromptLevel,
    PromptResponse, Render, RenderablePromptHandle, StatefulInteractiveElement, Styled, Window,
    div, px, rgb, rgba,
};

pub(crate) fn render_prompt(
    _level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    buttons: &[PromptButton],
    handle: PromptHandle,
    window: &mut Window,
    cx: &mut App,
) -> RenderablePromptHandle {
    let view = cx.new(|cx| ModalPrompt {
        message: message.to_owned(),
        detail: detail.map(str::to_owned),
        buttons: buttons.to_vec(),
        selected: 0,
        focus: cx.focus_handle(),
    });
    handle.with_view(view, window, cx)
}

struct ModalPrompt {
    message: String,
    detail: Option<String>,
    buttons: Vec<PromptButton>,
    selected: usize,
    focus: FocusHandle,
}

impl ModalPrompt {
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Consume unhandled keys too: they belong to the modal, never the PTY.
        window.prevent_default();
        cx.stop_propagation();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || self.buttons.is_empty() {
            return;
        }
        match event.keystroke.key.as_str() {
            "enter" | "space" => cx.emit(PromptResponse(self.selected)),
            "escape" => {
                if let Some(cancel) = self
                    .buttons
                    .iter()
                    .position(|button| matches!(button, PromptButton::Cancel(_)))
                {
                    cx.emit(PromptResponse(cancel));
                }
            }
            "tab" | "left" | "right" | "up" | "down" => {
                let backwards = event.keystroke.key == "left"
                    || event.keystroke.key == "up"
                    || (event.keystroke.key == "tab" && modifiers.shift);
                let count = self.buttons.len();
                self.selected = (self.selected + if backwards { count - 1 } else { 1 }) % count;
                cx.notify();
            }
            _ => {}
        }
    }
}

impl EventEmitter<PromptResponse> for ModalPrompt {}

impl Focusable for ModalPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ModalPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut overlay = div()
            .id("linux-prompt")
            .track_focus(&self.focus)
            .key_context("TermyPrompt")
            .size_full()
            .occlude()
            .bg(rgba(0x00000080))
            .cursor_default()
            .flex()
            .items_center()
            .justify_center()
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(|_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.focus.focus(window);
                cx.stop_propagation();
            }))
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        overlay
            .interactivity()
            .on_any_mouse_up(|_, _, cx| cx.stop_propagation());
        overlay.child(
            div()
                .w(px(420.0))
                .max_w_full()
                .p_4()
                .rounded_lg()
                .bg(rgb(0xffffff))
                .text_color(rgb(0x202020))
                .text_sm()
                .child(div().mb_2().child(self.message.clone()))
                .children(self.detail.clone().map(|detail| div().mb_2().child(detail)))
                .children(self.buttons.iter().enumerate().map(|(index, button)| {
                    div()
                        .id(index)
                        .debug_selector(move || format!("prompt-button-{index}"))
                        .mt_2()
                        .p_2()
                        .border_1()
                        .border_color(if self.selected == index {
                            rgb(0x2563eb)
                        } else {
                            rgb(0xb0b0b0)
                        })
                        .rounded_sm()
                        .cursor_pointer()
                        .child(button.label().clone())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(PromptResponse(index));
                        }))
                })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point};

    struct TerminalUnderlay {
        focus: FocusHandle,
        mouse_downs: usize,
        keys: usize,
    }

    impl Render for TerminalUnderlay {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .key_context("Terminal")
                .on_any_mouse_down(cx.listener(|this, _, window, _| {
                    this.mouse_downs += 1;
                    this.focus.focus(window);
                }))
                .on_key_down(cx.listener(|this, _, _, _| this.keys += 1))
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Entity<TerminalUnderlay>, &mut VisualTestContext) {
        cx.update(|cx| cx.set_prompt_builder(render_prompt));
        cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            TerminalUnderlay {
                focus,
                mouse_downs: 0,
                keys: 0,
            }
        })
    }

    #[gpui::test]
    fn keyboard_cancels_prompt_without_reaching_terminal_and_restores_focus(
        cx: &mut TestAppContext,
    ) {
        let (terminal, cx) = setup(cx);
        let mut answer = cx.update(|window, cx| {
            window.prompt(
                PromptLevel::Warning,
                "Quit Termy?",
                None,
                &["Quit", "Cancel"],
                cx,
            )
        });
        cx.simulate_keystrokes("a escape");
        cx.run_until_parked();
        assert_eq!(answer.try_recv().unwrap(), Some(1));
        terminal.read_with(cx, |terminal, _| assert_eq!(terminal.keys, 0));
        cx.simulate_keystrokes("a");
        terminal.read_with(cx, |terminal, _| assert_eq!(terminal.keys, 1));
    }

    #[gpui::test]
    fn mouse_is_modal_and_both_buttons_resolve_the_prompt(cx: &mut TestAppContext) {
        let (terminal, cx) = setup(cx);
        for index in 0..2 {
            let mut answer = cx.update(|window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    "Quit Termy?",
                    Some("A process is running."),
                    &["Quit", "Cancel"],
                    cx,
                )
            });
            cx.run_until_parked();
            cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::default());
            terminal.read_with(cx, |terminal, _| assert_eq!(terminal.mouse_downs, 0));
            cx.simulate_keystrokes("a");
            terminal.read_with(cx, |terminal, _| assert_eq!(terminal.keys, 0));
            assert_eq!(answer.try_recv().unwrap(), None);
            let selector = if index == 0 {
                "prompt-button-0"
            } else {
                "prompt-button-1"
            };
            let bounds = cx.debug_bounds(selector).expect("visible prompt button");
            cx.simulate_click(bounds.center(), Modifiers::default());
            cx.run_until_parked();
            assert_eq!(answer.try_recv().unwrap(), Some(index));
            terminal.read_with(cx, |terminal, _| assert_eq!(terminal.mouse_downs, 0));
        }
        cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::default());
        terminal.read_with(cx, |terminal, _| assert_eq!(terminal.mouse_downs, 1));
    }

    #[gpui::test]
    fn keyboard_can_confirm_or_navigate_to_cancel(cx: &mut TestAppContext) {
        let (_, cx) = setup(cx);
        for (keys, expected) in [
            ("enter", 0),
            ("tab enter", 1),
            ("shift-tab space", 1),
            ("right left enter", 0),
        ] {
            let mut answer = cx.update(|window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    "Quit Termy?",
                    None,
                    &["Quit", "Cancel"],
                    cx,
                )
            });
            cx.simulate_keystrokes(keys);
            cx.run_until_parked();
            assert_eq!(answer.try_recv().unwrap(), Some(expected), "{keys}");
        }
    }
}
