use super::*;

impl SettingsWindow {
    fn update_default_terminal(&mut self, set_default: bool, cx: &mut Context<Self>) {
        if self.default_terminal_busy {
            return;
        }
        self.default_terminal_busy = true;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = smol::unblock(move || {
                if set_default {
                    crate::default_terminal::set_default()?;
                }
                Ok(crate::default_terminal::is_default())
            })
            .await;
            let _ = view.update(cx, |view, cx| {
                view.default_terminal_state = Some(result);
                view.default_terminal_busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_default_terminal_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.default_terminal_state.is_none() {
            self.update_default_terminal(false, cx);
        }
        let is_default = matches!(self.default_terminal_state, Some(Ok(true)));
        let busy = self.default_terminal_busy;
        let description = match &self.default_terminal_state {
            Some(Err(error)) => error.clone(),
            Some(Ok(true)) => {
                "Termy is your default terminal. Apps may have their own terminal preference."
                    .into()
            }
            _ => "Use Termy when macOS requests the default terminal.".into(),
        };
        let accent = self.accent();
        let row = div()
            .w_full()
            .flex()
            .items_center()
            .gap_4()
            .py(px(CARD_ROW_PADDING_Y))
            .px(px(CARD_ROW_PADDING_X))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.text_primary())
                            .child("Default terminal"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.text_muted())
                            .child(description),
                    ),
            )
            .child(
                div()
                    .id("set-default-terminal")
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .rounded(px(SETTINGS_BUTTON_RADIUS))
                    .text_sm()
                    .text_color(accent)
                    .child(if busy {
                        "Checking…"
                    } else if is_default {
                        "Default"
                    } else {
                        "Set as default"
                    })
                    .when(!busy && !is_default, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.update_default_terminal(true, cx);
                            }))
                    }),
            )
            .into_any_element();
        let row = self.wrap_setting_with_scroll_anchor("default_terminal", row);
        self.render_settings_group("System integration", vec![row])
    }
}
