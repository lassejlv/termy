use super::*;
use termy_ssh_core::{
    SecretUpdate, SshAuthentication, SshAuthenticationType, SshHost, SshHostInput,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SshFormField {
    DisplayName,
    Hostname,
    Port,
    Username,
    IdentityFile,
    Secret,
}

#[derive(Clone)]
pub(super) struct ActiveSshInput {
    pub(super) field: SshFormField,
    pub(super) state: TextInputState,
    selecting: bool,
}

impl ActiveSshInput {
    fn new(field: SshFormField, value: String) -> Self {
        Self {
            field,
            state: TextInputState::new(value),
            selecting: false,
        }
    }
}

#[derive(Clone)]
pub(super) struct SshHostForm {
    editing_id: Option<String>,
    display_name: String,
    hostname: String,
    port: String,
    username: String,
    authentication_type: SshAuthenticationType,
    original_authentication_type: SshAuthenticationType,
    identity_file: String,
    secret: String,
    original_secret_saved: bool,
    saved_secret: bool,
    clear_saved_secret: bool,
}

impl SshHostForm {
    fn new() -> Self {
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        Self {
            editing_id: None,
            display_name: String::new(),
            hostname: String::new(),
            port: "22".to_string(),
            username,
            authentication_type: SshAuthenticationType::Key,
            original_authentication_type: SshAuthenticationType::Key,
            identity_file: "~/.ssh/id_ed25519".to_string(),
            secret: String::new(),
            original_secret_saved: false,
            saved_secret: false,
            clear_saved_secret: false,
        }
    }

    fn from_host(host: &SshHost, secret_saved: bool) -> Self {
        let (authentication_type, identity_file) = match &host.authentication {
            SshAuthentication::Key { identity_file } => {
                (SshAuthenticationType::Key, identity_file.clone())
            }
            SshAuthentication::Password => (SshAuthenticationType::Password, String::new()),
        };
        Self {
            editing_id: Some(host.id.clone()),
            display_name: host.display_name.clone(),
            hostname: host.hostname.clone(),
            port: host.port.to_string(),
            username: host.username.clone(),
            authentication_type,
            original_authentication_type: authentication_type,
            identity_file,
            secret: String::new(),
            original_secret_saved: secret_saved,
            saved_secret: secret_saved,
            clear_saved_secret: false,
        }
    }

    fn value(&self, field: SshFormField) -> &str {
        match field {
            SshFormField::DisplayName => &self.display_name,
            SshFormField::Hostname => &self.hostname,
            SshFormField::Port => &self.port,
            SshFormField::Username => &self.username,
            SshFormField::IdentityFile => &self.identity_file,
            SshFormField::Secret => &self.secret,
        }
    }

    fn set_value(&mut self, field: SshFormField, value: String) {
        match field {
            SshFormField::DisplayName => self.display_name = value,
            SshFormField::Hostname => self.hostname = value,
            SshFormField::Port => self.port = value,
            SshFormField::Username => self.username = value,
            SshFormField::IdentityFile => self.identity_file = value,
            SshFormField::Secret => self.secret = value,
        }
    }
}

impl SettingsWindow {
    pub(super) fn refresh_ssh_hosts(&mut self) {
        match crate::ssh::load_hosts(self.config_path.as_deref()) {
            Ok(hosts) => {
                self.ssh_hosts = hosts;
                self.ssh_hosts_error = None;
            }
            Err(error) => {
                self.ssh_hosts.clear();
                self.ssh_hosts_error = Some(error);
            }
        }
    }

    fn begin_add_ssh_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ssh_input = None;
        self.ssh_form = Some(SshHostForm::new());
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn begin_edit_ssh_host(&mut self, host_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self
            .ssh_hosts
            .iter()
            .find(|host| host.id == host_id)
            .cloned()
        else {
            crate::ui::toast::error("That saved SSH host no longer exists");
            return;
        };
        let secret_saved = match crate::ssh::manager(self.config_path.as_deref())
            .and_then(|manager| manager.has_secret(&host.id, host.authentication.secret_kind()))
        {
            Ok(saved) => saved,
            Err(error) => {
                crate::ui::toast::warning(format!(
                    "Could not inspect this host's Keychain credential; non-secret settings can still be edited: {error}"
                ));
                false
            }
        };
        self.ssh_input = None;
        self.ssh_form = Some(SshHostForm::from_host(&host, secret_saved));
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn cancel_ssh_host_form(&mut self, cx: &mut Context<Self>) {
        self.ssh_input = None;
        self.ssh_form = None;
        cx.notify();
    }

    fn set_ssh_authentication_type(
        &mut self,
        authentication_type: SshAuthenticationType,
        cx: &mut Context<Self>,
    ) {
        self.sync_active_ssh_input();
        self.ssh_input = None;
        let Some(form) = self.ssh_form.as_mut() else {
            return;
        };
        if form.authentication_type == authentication_type {
            return;
        }
        form.authentication_type = authentication_type;
        form.secret.clear();
        form.clear_saved_secret = false;
        form.saved_secret =
            authentication_type == form.original_authentication_type && form.original_secret_saved;
        cx.notify();
    }

    fn clear_saved_ssh_secret(&mut self, cx: &mut Context<Self>) {
        self.sync_active_ssh_input();
        self.ssh_input = None;
        if let Some(form) = self.ssh_form.as_mut() {
            form.secret.clear();
            form.saved_secret = false;
            form.clear_saved_secret = true;
            cx.notify();
        }
    }

    fn save_ssh_host_form(&mut self, cx: &mut Context<Self>) {
        self.sync_active_ssh_input();
        let Some(form) = self.ssh_form.clone() else {
            return;
        };
        let port = match form.port.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                crate::ui::toast::error("Port must be between 1 and 65535");
                return;
            }
        };
        let authentication = match form.authentication_type {
            SshAuthenticationType::Key => SshAuthentication::Key {
                identity_file: form.identity_file.clone(),
            },
            SshAuthenticationType::Password => SshAuthentication::Password,
        };
        let input = SshHostInput {
            display_name: form.display_name,
            hostname: form.hostname,
            port,
            username: form.username,
            authentication,
        };
        if let Err(error) = input.validate() {
            crate::ui::toast::error(error);
            return;
        }
        let secret_update = if !form.secret.is_empty() {
            SecretUpdate::Set(form.secret)
        } else if form.clear_saved_secret {
            SecretUpdate::Clear
        } else {
            SecretUpdate::Keep
        };

        let mut manager = match crate::ssh::manager(self.config_path.as_deref()) {
            Ok(manager) => manager,
            Err(error) => {
                crate::ui::toast::error(error);
                return;
            }
        };
        let result = match form.editing_id.as_deref() {
            Some(host_id) => manager
                .update(host_id, input, secret_update)
                .map(|_| "SSH host updated"),
            None => manager
                .create(input, secret_update)
                .map(|_| "SSH host saved"),
        };
        match result {
            Ok(message) => {
                self.ssh_hosts = manager.hosts().to_vec();
                self.ssh_hosts_error = None;
                self.ssh_input = None;
                self.ssh_form = None;
                crate::ui::toast::success(message);
                cx.notify();
            }
            Err(error) => crate::ui::toast::error(error),
        }
    }

    fn confirm_delete_ssh_host(
        &mut self,
        host_id: String,
        display_name: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let title = "Delete SSH Host";
            let message = format!("Delete “{display_name}” and its saved Keychain credential?");
            if !termy_native_sdk::confirm(title, &message) {
                return;
            }
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.delete_ssh_host(&host_id, cx);
                })
            });
        })
        .detach();
    }

    fn delete_ssh_host(&mut self, host_id: &str, cx: &mut Context<Self>) {
        let mut manager = match crate::ssh::manager(self.config_path.as_deref()) {
            Ok(manager) => manager,
            Err(error) => {
                crate::ui::toast::error(error);
                return;
            }
        };
        match manager.delete(host_id) {
            Ok(_) => {
                self.ssh_hosts = manager.hosts().to_vec();
                self.ssh_hosts_error = None;
                if self
                    .ssh_form
                    .as_ref()
                    .and_then(|form| form.editing_id.as_deref())
                    == Some(host_id)
                {
                    self.ssh_form = None;
                    self.ssh_input = None;
                }
                crate::ui::toast::success("SSH host deleted");
                cx.notify();
            }
            Err(error) => crate::ui::toast::error(error),
        }
    }

    fn begin_ssh_input(
        &mut self,
        field: SshFormField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_active_ssh_input();
        let Some(value) = self
            .ssh_form
            .as_ref()
            .map(|form| form.value(field).to_string())
        else {
            return;
        };
        self.active_input = None;
        self.plugin_setting_input = None;
        self.blur_sidebar_search();
        self.theme_store_search_active = false;
        self.ssh_input = Some(ActiveSshInput::new(field, value));
        self.focus_handle.focus(window);
        cx.notify();
    }

    pub(super) fn sync_active_ssh_input(&mut self) {
        let Some((field, value)) = self
            .ssh_input
            .as_ref()
            .map(|input| (input.field, input.state.text().to_string()))
        else {
            return;
        };
        if let Some(form) = self.ssh_form.as_mut() {
            form.set_value(field, value);
        }
    }

    pub(super) fn handle_ssh_input_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        if Self::cmd_only(event.keystroke.modifiers)
            && event.keystroke.key.eq_ignore_ascii_case("a")
        {
            if let Some(input) = self.ssh_input.as_mut() {
                input.state.select_all();
            }
            cx.notify();
            return;
        }
        if Self::cmd_only(event.keystroke.modifiers)
            && event.keystroke.key.eq_ignore_ascii_case("v")
        {
            if let Some(clipboard_text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let filtered: String = clipboard_text
                    .chars()
                    .filter(|character| !matches!(character, '\n' | '\r'))
                    .collect();
                if let Some(input) = self.ssh_input.as_mut() {
                    input.state.replace_text_in_range(None, &filtered);
                }
                self.sync_active_ssh_input();
                cx.notify();
            }
            return;
        }

        match event.keystroke.key.as_str() {
            "enter" | "tab" => {
                self.sync_active_ssh_input();
                self.ssh_input = None;
            }
            "escape" => self.ssh_input = None,
            "backspace" => {
                if let Some(input) = self.ssh_input.as_mut() {
                    input.state.delete_backward();
                }
                self.sync_active_ssh_input();
            }
            "delete" => {
                if let Some(input) = self.ssh_input.as_mut() {
                    input.state.delete_forward();
                }
                self.sync_active_ssh_input();
            }
            "left" => {
                if let Some(input) = self.ssh_input.as_mut() {
                    input.state.move_left();
                }
            }
            "right" => {
                if let Some(input) = self.ssh_input.as_mut() {
                    input.state.move_right();
                }
            }
            "home" => {
                if let Some(input) = self.ssh_input.as_mut() {
                    input.state.move_to_start();
                }
            }
            "end" => {
                if let Some(input) = self.ssh_input.as_mut() {
                    input.state.move_to_end();
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn handle_ssh_input_mouse_down(
        &mut self,
        field: SshFormField,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if self
            .ssh_input
            .as_ref()
            .is_none_or(|input| input.field != field)
        {
            self.begin_ssh_input(field, window, cx);
        }
        if let Some(input) = self.ssh_input.as_mut() {
            let index = input.state.character_index_for_point(event.position);
            if event.modifiers.shift {
                input.state.select_to_utf16(index);
            } else if event.click_count >= 3 {
                input.state.select_all();
            } else if event.click_count == 2 {
                input.state.select_token_at_utf16(index);
            } else {
                input.state.set_cursor_utf16(index);
            }
            input.selecting = event.click_count == 1;
        }
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn handle_ssh_input_mouse_move(
        &mut self,
        field: SshFormField,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.ssh_input.as_mut() else {
            return;
        };
        if input.field != field || !input.selecting || !event.dragging() {
            return;
        }
        let index = input.state.character_index_for_point(event.position);
        input.state.select_to_utf16(index);
        cx.notify();
    }

    fn handle_ssh_input_mouse_up(&mut self, field: SshFormField, cx: &mut Context<Self>) {
        if let Some(input) = self.ssh_input.as_mut()
            && input.field == field
        {
            input.selecting = false;
            cx.notify();
        }
    }

    fn render_ssh_input(
        &self,
        field: SshFormField,
        placeholder: &'static str,
        secret: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = self
            .ssh_form
            .as_ref()
            .map_or_else(String::new, |form| form.value(field).to_string());
        let is_active = self
            .ssh_input
            .as_ref()
            .is_some_and(|input| input.field == field);
        let text_color = self.text_primary();
        let muted = self.text_muted();
        let border = if is_active {
            self.accent_with_alpha(0.75)
        } else {
            self.border_color()
        };
        let font = Font {
            family: self.config.ui_font_family.clone().into(),
            ..gpui::font("")
        };

        let content: AnyElement = if is_active {
            if secret {
                let mut hidden_text = text_color;
                hidden_text.a = 0.0;
                let mut hidden_selection = self.accent_with_alpha(0.3);
                hidden_selection.a = 0.0;
                let masked = self.ssh_input.as_ref().map_or_else(String::new, |input| {
                    if input.state.text().is_empty() {
                        String::new()
                    } else {
                        "••••••••".to_string()
                    }
                });
                div()
                    .relative()
                    .size_full()
                    .child(TextInputElement::new(
                        cx.entity(),
                        self.focus_handle.clone(),
                        font,
                        px(SETTINGS_INPUT_TEXT_SIZE),
                        hidden_text.into(),
                        hidden_selection.into(),
                        TextInputAlignment::Left,
                    ))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .size_full()
                            .flex()
                            .items_center()
                            .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                            .text_color(if masked.is_empty() { muted } else { text_color })
                            .child(if masked.is_empty() {
                                placeholder.to_string()
                            } else {
                                masked
                            }),
                    )
                    .into_any_element()
            } else {
                TextInputElement::new(
                    cx.entity(),
                    self.focus_handle.clone(),
                    font,
                    px(SETTINGS_INPUT_TEXT_SIZE),
                    text_color.into(),
                    self.accent_with_alpha(0.3).into(),
                    TextInputAlignment::Left,
                )
                .into_any_element()
            }
        } else {
            let display = if value.is_empty() {
                placeholder.to_string()
            } else if secret {
                "••••••••".to_string()
            } else {
                value
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                .text_color(if display == placeholder {
                    muted
                } else {
                    text_color
                })
                .child(display)
                .into_any_element()
        };

        div()
            .id(SharedString::from(format!("ssh-input-{field:?}")))
            .h(px(SETTINGS_CONTROL_HEIGHT))
            .w_full()
            .px(px(SETTINGS_CONTROL_INNER_PADDING))
            .rounded(px(SETTINGS_INPUT_RADIUS))
            .bg(self.bg_input())
            .border_1()
            .border_color(border)
            .cursor_text()
            .child(content)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.handle_ssh_input_mouse_down(field, event, window, cx);
                }),
            )
            .on_mouse_move(
                cx.listener(move |view, event: &MouseMoveEvent, _window, cx| {
                    view.handle_ssh_input_mouse_move(field, event, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, _event: &MouseUpEvent, _window, cx| {
                    view.handle_ssh_input_mouse_up(field, cx);
                }),
            )
            .into_any_element()
    }

    fn render_ssh_field(
        &self,
        field: SshFormField,
        label: &'static str,
        placeholder: &'static str,
        secret: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(self.text_secondary())
                    .child(label),
            )
            .child(self.render_ssh_input(field, placeholder, secret, cx))
            .into_any_element()
    }

    fn render_ssh_host_form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = self.ssh_form.clone() else {
            return div().into_any_element();
        };
        let is_editing = form.editing_id.is_some();
        let auth_type = form.authentication_type;
        let card_bg = self.bg_elevated();
        let border = self.card_border_color();
        let accent = self.accent_with_alpha(0.95);
        let accent_text = self.contrasting_text_for_fill(accent, card_bg);
        let hover_bg = self.bg_hover();

        let auth_button = |id: &'static str,
                           label: &'static str,
                           target: SshAuthenticationType,
                           selected: bool,
                           cx: &mut Context<Self>| {
            div()
                .id(id)
                .h(px(30.0))
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(SETTINGS_BUTTON_RADIUS))
                .cursor_pointer()
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .bg(if selected { accent } else { self.bg_input() })
                .text_color(if selected {
                    accent_text
                } else {
                    self.text_secondary()
                })
                .hover(
                    move |style| {
                        if selected { style } else { style.bg(hover_bg) }
                    },
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.set_ssh_authentication_type(target, cx);
                }))
                .child(label)
        };

        let mut fields = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(self.render_ssh_field(
                SshFormField::DisplayName,
                "Display name",
                "Production server",
                false,
                cx,
            ))
            .child(self.render_ssh_field(
                SshFormField::Hostname,
                "Hostname",
                "server.example.com",
                false,
                cx,
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap(px(12.0))
                    .child(self.render_ssh_field(
                        SshFormField::Username,
                        "Username",
                        "deploy",
                        false,
                        cx,
                    ))
                    .child(div().w(px(150.0)).child(self.render_ssh_field(
                        SshFormField::Port,
                        "Port",
                        "22",
                        false,
                        cx,
                    ))),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(self.text_secondary())
                            .child("Authentication"),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .gap(px(6.0))
                            .child(auth_button(
                                "ssh-auth-key",
                                "SSH key",
                                SshAuthenticationType::Key,
                                auth_type == SshAuthenticationType::Key,
                                cx,
                            ))
                            .child(auth_button(
                                "ssh-auth-password",
                                "Password",
                                SshAuthenticationType::Password,
                                auth_type == SshAuthenticationType::Password,
                                cx,
                            )),
                    ),
            );
        if auth_type == SshAuthenticationType::Key {
            fields = fields.child(self.render_ssh_field(
                SshFormField::IdentityFile,
                "Identity file",
                "~/.ssh/id_ed25519",
                false,
                cx,
            ));
        }
        let secret_label = if auth_type == SshAuthenticationType::Key {
            "Private-key passphrase (optional)"
        } else {
            "Password (optional)"
        };
        let secret_placeholder = if form.saved_secret {
            "Saved in Keychain — leave blank to keep"
        } else {
            "Leave blank to enter interactively"
        };
        fields = fields.child(self.render_ssh_field(
            SshFormField::Secret,
            secret_label,
            secret_placeholder,
            true,
            cx,
        ));
        if form.saved_secret {
            fields = fields.child(
                div()
                    .id("ssh-clear-saved-secret")
                    .ml_auto()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .text_color(self.text_muted())
                    .hover(|style| style.text_color(self.text_primary()))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.clear_saved_ssh_secret(cx);
                    }))
                    .child("Remove saved Keychain credential"),
            );
        }

        let cancel_button = div()
            .id("ssh-form-cancel")
            .h(px(30.0))
            .px(px(14.0))
            .flex()
            .items_center()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .bg(self.bg_input())
            .text_size(px(12.0))
            .text_color(self.text_secondary())
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg))
            .on_click(cx.listener(|view, _, _, cx| {
                view.cancel_ssh_host_form(cx);
            }))
            .child("Cancel");
        let save_button = div()
            .id("ssh-form-save")
            .h(px(30.0))
            .px(px(14.0))
            .flex()
            .items_center()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .bg(accent)
            .text_size(px(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(accent_text)
            .cursor_pointer()
            .on_click(cx.listener(|view, _, _, cx| {
                view.save_ssh_host_form(cx);
            }))
            .child(if is_editing {
                "Save changes"
            } else {
                "Add host"
            });

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .p(px(16.0))
            .rounded(px(SETTINGS_CARD_RADIUS))
            .bg(card_bg)
            .border_1()
            .border_color(border)
            .child(
                div()
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(self.text_primary())
                    .child(if is_editing {
                        "Edit SSH host"
                    } else {
                        "Add SSH host"
                    }),
            )
            .child(fields)
            .child(
                div()
                    .w_full()
                    .text_size(px(11.0))
                    .text_color(self.text_muted())
                    .child(
                        "Secrets are stored only in the system Keychain. Host keys continue to use OpenSSH known_hosts verification.",
                    ),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(cancel_button)
                    .child(save_button),
            )
            .into_any_element()
    }

    fn render_ssh_host_row(&self, host: SshHost, cx: &mut Context<Self>) -> AnyElement {
        let auth_label = match host.authentication {
            SshAuthentication::Key { .. } => "SSH key",
            SshAuthentication::Password => "Password",
        };
        let endpoint = format!("{}@{}:{}", host.username, host.hostname, host.port);
        let edit_id = host.id.clone();
        let delete_id = host.id.clone();
        let delete_name = host.display_name.clone();
        let hover_bg = self.bg_hover();
        let danger = Rgba {
            r: 0.88,
            g: 0.25,
            b: 0.25,
            a: 0.9,
        };

        div()
            .w_full()
            .min_h(px(62.0))
            .px(px(CARD_ROW_PADDING_X))
            .py(px(CARD_ROW_PADDING_Y))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(self.text_primary())
                            .child(host.display_name),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(self.text_muted())
                            .child(format!("{endpoint} · {auth_label}")),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("ssh-edit-{edit_id}")))
                            .h(px(28.0))
                            .px(px(10.0))
                            .flex()
                            .items_center()
                            .rounded(px(SETTINGS_BUTTON_RADIUS))
                            .bg(self.bg_input())
                            .text_size(px(11.0))
                            .text_color(self.text_secondary())
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover_bg))
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.begin_edit_ssh_host(&edit_id, window, cx);
                            }))
                            .child("Edit"),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("ssh-delete-{delete_id}")))
                            .h(px(28.0))
                            .px(px(10.0))
                            .flex()
                            .items_center()
                            .rounded(px(SETTINGS_BUTTON_RADIUS))
                            .text_size(px(11.0))
                            .text_color(danger)
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover_bg))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.confirm_delete_ssh_host(
                                    delete_id.clone(),
                                    delete_name.clone(),
                                    cx,
                                );
                            }))
                            .child("Delete"),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_ssh_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let add_button = div()
            .id("ssh-add-host")
            .h(px(30.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .bg(self.accent_with_alpha(0.95))
            .text_size(px(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(
                self.contrasting_text_for_fill(self.accent_with_alpha(0.95), self.bg_card()),
            )
            .cursor_pointer()
            .on_click(cx.listener(|view, _, window, cx| {
                view.begin_add_ssh_host(window, cx);
            }))
            .child("Add host");

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(CARD_GAP))
            .child(self.render_section_header(
                "SSH",
                "Manage remote terminal connections",
                SettingsSection::Ssh,
                cx,
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .when(self.ssh_form.is_none(), |row| row.child(add_button)),
            );

        if self.ssh_form.is_some() {
            section = section.child(self.render_ssh_host_form(cx));
        }
        if let Some(error) = self.ssh_hosts_error.clone() {
            section = section.child(
                div()
                    .w_full()
                    .p(px(14.0))
                    .rounded(px(SETTINGS_CARD_RADIUS))
                    .bg(self.bg_elevated())
                    .border_1()
                    .border_color(self.card_border_color())
                    .text_size(px(12.0))
                    .text_color(self.text_secondary())
                    .child(error),
            );
        } else if self.ssh_hosts.is_empty() {
            section = section.child(
                div()
                    .w_full()
                    .p(px(22.0))
                    .rounded(px(SETTINGS_CARD_RADIUS))
                    .bg(self.bg_elevated())
                    .border_1()
                    .border_color(self.card_border_color())
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0))
                    .text_align(TextAlign::Center)
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(self.text_primary())
                            .child("No saved SSH hosts"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(self.text_muted())
                            .child("Add a host, then connect from the new-tab menu."),
                    ),
            );
        } else {
            let rows = self
                .ssh_hosts
                .clone()
                .into_iter()
                .map(|host| self.render_ssh_host_row(host, cx))
                .collect();
            section = section.child(self.render_settings_group("Saved hosts", rows));
        }
        section
    }
}
