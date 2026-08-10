use super::state::{CommandPaletteItem, CommandPaletteItemKind, CommandPaletteState};
use termy_terminal_ui::{TmuxSessionSummary, TmuxSocketTarget};

const TMUX_SESSION_ACTIVE_HINT: &str = "active session";
const TMUX_SESSION_NAME_REQUIRED_HINT: &str = "name required";
const TMUX_SESSION_NAME_UNCHANGED_HINT: &str = "unchanged";
const TMUX_SOCKET_DEFAULT_LABEL: &str = "default";
const TMUX_SOCKET_DEDICATED_LABEL: &str = "termy";
const TMUX_DETACH_CURRENT_TITLE: &str = "Detach Current Session";
const TMUX_OPEN_RENAME_MODE_TITLE: &str = "Rename Session…";
const TMUX_OPEN_KILL_MODE_TITLE: &str = "Kill Session…";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TmuxSessionStatusHint {
    ActiveSession,
    NameRequired,
    NameUnchanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TmuxSessionRow {
    pub(super) summary: TmuxSessionSummary,
    pub(super) socket_target: TmuxSocketTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum TmuxSessionIntent {
    AttachOrSwitch,
    RenameSelect,
    RenameInput,
    Kill,
}

impl CommandPaletteItem {
    fn tmux_socket_label(socket_target: &TmuxSocketTarget) -> String {
        match socket_target {
            TmuxSocketTarget::Default => TMUX_SOCKET_DEFAULT_LABEL.to_string(),
            TmuxSocketTarget::DedicatedTermy => TMUX_SOCKET_DEDICATED_LABEL.to_string(),
            TmuxSocketTarget::Named(name) => name.clone(),
        }
    }

    fn tmux_session_title(row: &TmuxSessionRow) -> String {
        let socket_label = Self::tmux_socket_label(&row.socket_target);
        format!(
            "{}  ({} window{}, {} attached)  [{socket_label}]",
            row.summary.name,
            row.summary.window_count,
            if row.summary.window_count == 1 {
                ""
            } else {
                "s"
            },
            row.summary.attached_clients
        )
    }

    pub(super) fn tmux_session_attach_or_switch(row: &TmuxSessionRow) -> Self {
        let title = Self::tmux_session_title(row);
        let socket_label = Self::tmux_socket_label(&row.socket_target);
        let keywords = format!(
            "tmux attach switch session {} socket {}",
            row.summary.name.replace('-', " "),
            socket_label.replace('-', " ")
        );
        Self {
            title,
            keywords,
            enabled: true,
            status_hint: None,
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::TmuxSessionAttachOrSwitch {
                session_name: row.summary.name.clone(),
                socket_target: row.socket_target.clone(),
            },
        }
    }

    pub(super) fn tmux_session_rename_select(
        row: &TmuxSessionRow,
        active_session_name: Option<&str>,
    ) -> Self {
        let title = Self::tmux_session_title(row);
        let socket_label = Self::tmux_socket_label(&row.socket_target);
        let keywords = format!(
            "tmux rename session {} socket {}",
            row.summary.name.replace('-', " "),
            socket_label.replace('-', " ")
        );
        let is_active = active_session_name.is_some_and(|active| row.summary.name == active);
        Self {
            title,
            keywords,
            enabled: !is_active,
            status_hint: is_active.then(|| TMUX_SESSION_ACTIVE_HINT.to_string()),
            tmux_status_hint: is_active.then_some(TmuxSessionStatusHint::ActiveSession),
            kind: CommandPaletteItemKind::TmuxSessionRenameSelect {
                session_name: row.summary.name.clone(),
                socket_target: row.socket_target.clone(),
            },
        }
    }

    pub(super) fn tmux_session_rename_apply(
        current_session_name: &str,
        next_session_name: &str,
        socket_target: &TmuxSocketTarget,
    ) -> Self {
        let next_session_name = next_session_name.trim().to_string();
        let mut enabled = true;
        let mut status_hint = None;
        let mut tmux_status_hint = None;
        if next_session_name.is_empty() {
            enabled = false;
            status_hint = Some(TMUX_SESSION_NAME_REQUIRED_HINT.to_string());
            tmux_status_hint = Some(TmuxSessionStatusHint::NameRequired);
        } else if current_session_name.eq_ignore_ascii_case(&next_session_name) {
            enabled = false;
            status_hint = Some(TMUX_SESSION_NAME_UNCHANGED_HINT.to_string());
            tmux_status_hint = Some(TmuxSessionStatusHint::NameUnchanged);
        }

        let rendered_next_name = if next_session_name.is_empty() {
            "<new name>"
        } else {
            next_session_name.as_str()
        };

        Self {
            title: format!("Rename \"{current_session_name}\" -> \"{rendered_next_name}\""),
            keywords: format!(
                "tmux rename session {} {}",
                current_session_name.replace('-', " "),
                next_session_name.replace('-', " ")
            ),
            enabled,
            status_hint,
            tmux_status_hint,
            kind: CommandPaletteItemKind::TmuxSessionRenameApply {
                current_session_name: current_session_name.to_string(),
                next_session_name,
                socket_target: socket_target.clone(),
            },
        }
    }

    pub(super) fn tmux_session_kill(
        row: &TmuxSessionRow,
        active_session_name: Option<&str>,
    ) -> Self {
        let title = Self::tmux_session_title(row);
        let socket_label = Self::tmux_socket_label(&row.socket_target);
        let keywords = format!(
            "tmux kill close session {} socket {}",
            row.summary.name.replace('-', " "),
            socket_label.replace('-', " ")
        );
        let is_active = active_session_name.is_some_and(|active| row.summary.name == active);
        Self {
            title,
            keywords,
            enabled: !is_active,
            status_hint: is_active.then(|| TMUX_SESSION_ACTIVE_HINT.to_string()),
            tmux_status_hint: is_active.then_some(TmuxSessionStatusHint::ActiveSession),
            kind: CommandPaletteItemKind::TmuxSessionKill {
                session_name: row.summary.name.clone(),
                socket_target: row.socket_target.clone(),
            },
        }
    }

    pub(super) fn tmux_session_create_and_attach(
        session_name: &str,
        socket_target: &TmuxSocketTarget,
    ) -> Self {
        let session_name = session_name.trim().to_string();
        Self {
            title: format!("Create tmux Session \"{session_name}\""),
            keywords: format!(
                "tmux attach create switch session {}",
                session_name.replace('-', " ")
            ),
            enabled: true,
            status_hint: None,
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::TmuxSessionCreateAndAttach {
                session_name,
                socket_target: socket_target.clone(),
            },
        }
    }

    pub(super) fn tmux_session_detach_current() -> Self {
        Self {
            title: TMUX_DETACH_CURRENT_TITLE.to_string(),
            keywords: "tmux detach current session".to_string(),
            enabled: true,
            status_hint: None,
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::TmuxSessionDetachCurrent,
        }
    }

    pub(super) fn tmux_session_open_rename_mode() -> Self {
        Self {
            title: TMUX_OPEN_RENAME_MODE_TITLE.to_string(),
            keywords: "tmux rename session".to_string(),
            enabled: true,
            status_hint: None,
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::TmuxSessionOpenRenameMode,
        }
    }

    pub(super) fn tmux_session_open_kill_mode() -> Self {
        Self {
            title: TMUX_OPEN_KILL_MODE_TITLE.to_string(),
            keywords: "tmux kill session".to_string(),
            enabled: true,
            status_hint: None,
            tmux_status_hint: None,
            kind: CommandPaletteItemKind::TmuxSessionOpenKillMode,
        }
    }
}

impl CommandPaletteState {
    pub(super) fn tmux_session_intent(&self) -> TmuxSessionIntent {
        self.tmux_session_intent
    }

    pub(super) fn set_tmux_session_intent(&mut self, intent: TmuxSessionIntent) {
        self.tmux_session_intent = intent;
        if intent != TmuxSessionIntent::RenameInput {
            self.tmux_rename_source_session = None;
            self.tmux_rename_source_socket = None;
        }
    }

    pub(super) fn begin_tmux_session_rename(
        &mut self,
        session_name: &str,
        socket_target: TmuxSocketTarget,
    ) {
        self.tmux_session_intent = TmuxSessionIntent::RenameInput;
        self.tmux_rename_source_session = Some(session_name.to_string());
        self.tmux_rename_source_socket = Some(socket_target);
        self.input_mut().clear();
    }

    pub(super) fn back_from_tmux_rename_input(&mut self) -> bool {
        if self.tmux_session_intent != TmuxSessionIntent::RenameInput {
            return false;
        }
        self.tmux_session_intent = TmuxSessionIntent::RenameSelect;
        self.tmux_rename_source_session = None;
        self.tmux_rename_source_socket = None;
        self.input_mut().clear();
        true
    }

    pub(super) fn tmux_rename_source_session(&self) -> Option<&str> {
        self.tmux_rename_source_session.as_deref()
    }

    pub(super) fn set_tmux_session_rows(
        &mut self,
        mut rows: Vec<TmuxSessionRow>,
        create_socket_target: TmuxSocketTarget,
    ) {
        rows.sort_unstable_by(|left, right| {
            left.summary.name.cmp(&right.summary.name).then_with(|| {
                CommandPaletteItem::tmux_socket_label(&left.socket_target)
                    .cmp(&CommandPaletteItem::tmux_socket_label(&right.socket_target))
            })
        });
        rows.dedup_by(|left, right| {
            left.summary.name == right.summary.name && left.socket_target == right.socket_target
        });
        self.tmux_create_socket_target = create_socket_target;
        self.tmux_session_rows = rows;
    }

    pub(super) fn tmux_session_items_for_query(
        &self,
        query: &str,
        active_session_name: Option<&str>,
        allow_detach: bool,
    ) -> Vec<CommandPaletteItem> {
        match self.tmux_session_intent {
            TmuxSessionIntent::AttachOrSwitch => {
                let mut items = self
                    .tmux_session_rows
                    .iter()
                    .map(CommandPaletteItem::tmux_session_attach_or_switch)
                    .collect::<Vec<_>>();

                let query = query.trim();
                if query.is_empty() {
                    let mut utility_items = Vec::new();
                    if allow_detach && active_session_name.is_some() {
                        utility_items.push(CommandPaletteItem::tmux_session_detach_current());
                    }
                    if !self.tmux_session_rows.is_empty() {
                        utility_items.push(CommandPaletteItem::tmux_session_open_rename_mode());
                        utility_items.push(CommandPaletteItem::tmux_session_open_kill_mode());
                    }
                    utility_items.extend(items);
                    return utility_items;
                }

                let exact_match = self.tmux_session_rows.iter().any(|row| {
                    row.summary.name.eq_ignore_ascii_case(query)
                        && row.socket_target == self.tmux_create_socket_target
                });
                if !exact_match {
                    items.push(CommandPaletteItem::tmux_session_create_and_attach(
                        query,
                        &self.tmux_create_socket_target,
                    ));
                }

                items
            }
            TmuxSessionIntent::RenameSelect => self
                .tmux_session_rows
                .iter()
                .map(|row| CommandPaletteItem::tmux_session_rename_select(row, active_session_name))
                .collect(),
            TmuxSessionIntent::RenameInput => {
                let Some(current_session_name) = self.tmux_rename_source_session() else {
                    return Vec::new();
                };
                let Some(socket_target) = self.tmux_rename_source_socket.as_ref() else {
                    return Vec::new();
                };
                vec![CommandPaletteItem::tmux_session_rename_apply(
                    current_session_name,
                    query,
                    socket_target,
                )]
            }
            TmuxSessionIntent::Kill => self
                .tmux_session_rows
                .iter()
                .map(|row| CommandPaletteItem::tmux_session_kill(row, active_session_name))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_view::command_palette::state::CommandPaletteState;

    fn tmux_row(name: &str, socket_target: TmuxSocketTarget) -> TmuxSessionRow {
        TmuxSessionRow {
            summary: TmuxSessionSummary {
                name: name.to_string(),
                id: format!("${name}"),
                window_count: 1,
                attached_clients: 0,
            },
            socket_target,
        }
    }

    #[test]
    fn tmux_session_items_add_create_row_only_when_query_has_no_exact_match() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![TmuxSessionRow {
                summary: TmuxSessionSummary {
                    name: "work".to_string(),
                    id: "$1".to_string(),
                    window_count: 2,
                    attached_clients: 1,
                },
                socket_target: TmuxSocketTarget::Default,
            }],
            TmuxSocketTarget::Default,
        );

        let with_exact = state.tmux_session_items_for_query("work", None, true);
        assert_eq!(with_exact.len(), 1);
        assert!(matches!(
            with_exact[0].kind,
            CommandPaletteItemKind::TmuxSessionAttachOrSwitch { .. }
        ));

        let with_new = state.tmux_session_items_for_query("new-session", None, true);
        assert_eq!(with_new.len(), 2);
        assert!(matches!(
            with_new[0].kind,
            CommandPaletteItemKind::TmuxSessionAttachOrSwitch { .. }
        ));
        assert!(matches!(
            with_new[1].kind,
            CommandPaletteItemKind::TmuxSessionCreateAndAttach { .. }
        ));
    }

    #[test]
    fn tmux_session_items_show_create_row_when_match_exists_on_non_create_socket() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![tmux_row("work", TmuxSocketTarget::Default)],
            TmuxSocketTarget::DedicatedTermy,
        );

        let items = state.tmux_session_items_for_query("work", None, true);
        assert_eq!(items.len(), 2);
        assert!(matches!(
            items[0].kind,
            CommandPaletteItemKind::TmuxSessionAttachOrSwitch { .. }
        ));
        assert!(matches!(
            items[1].kind,
            CommandPaletteItemKind::TmuxSessionCreateAndAttach { .. }
        ));
    }

    #[test]
    fn tmux_session_items_hide_create_row_when_match_exists_on_create_socket() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![
                tmux_row("work", TmuxSocketTarget::Default),
                tmux_row("work", TmuxSocketTarget::DedicatedTermy),
            ],
            TmuxSocketTarget::DedicatedTermy,
        );

        let items = state.tmux_session_items_for_query("work", None, true);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| matches!(
            item.kind,
            CommandPaletteItemKind::TmuxSessionAttachOrSwitch { .. }
        )));
    }

    #[test]
    fn tmux_rename_select_disables_active_session_row() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_intent(TmuxSessionIntent::RenameSelect);
        state.set_tmux_session_rows(
            vec![
                TmuxSessionRow {
                    summary: TmuxSessionSummary {
                        name: "work".to_string(),
                        id: "$1".to_string(),
                        window_count: 2,
                        attached_clients: 1,
                    },
                    socket_target: TmuxSocketTarget::Default,
                },
                TmuxSessionRow {
                    summary: TmuxSessionSummary {
                        name: "sandbox".to_string(),
                        id: "$2".to_string(),
                        window_count: 1,
                        attached_clients: 0,
                    },
                    socket_target: TmuxSocketTarget::Default,
                },
            ],
            TmuxSocketTarget::Default,
        );

        let items = state.tmux_session_items_for_query("", Some("work"), true);
        let active = items
            .iter()
            .find(|item| item.title.starts_with("work"))
            .expect("missing active session row");
        let inactive = items
            .iter()
            .find(|item| item.title.starts_with("sandbox"))
            .expect("missing inactive session row");

        assert!(!active.enabled);
        assert_eq!(active.status_hint.as_deref(), Some("active session"));
        assert!(inactive.enabled);
        assert_eq!(inactive.status_hint, None);
    }

    #[test]
    fn tmux_rename_input_builds_single_row_and_requires_non_empty_new_name() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![TmuxSessionRow {
                summary: TmuxSessionSummary {
                    name: "work".to_string(),
                    id: "$1".to_string(),
                    window_count: 1,
                    attached_clients: 1,
                },
                socket_target: TmuxSocketTarget::Default,
            }],
            TmuxSocketTarget::Default,
        );
        state.begin_tmux_session_rename("work", TmuxSocketTarget::Default);

        let empty = state.tmux_session_items_for_query("   ", Some("work"), true);
        assert_eq!(empty.len(), 1);
        assert!(!empty[0].enabled);
        assert_eq!(empty[0].status_hint.as_deref(), Some("name required"));

        let same = state.tmux_session_items_for_query("work", Some("work"), true);
        assert_eq!(same.len(), 1);
        assert!(!same[0].enabled);
        assert_eq!(same[0].status_hint.as_deref(), Some("unchanged"));

        let valid = state.tmux_session_items_for_query("next", Some("work"), true);
        assert_eq!(valid.len(), 1);
        assert!(valid[0].enabled);
        assert_eq!(valid[0].status_hint, None);
        match &valid[0].kind {
            CommandPaletteItemKind::TmuxSessionRenameApply { socket_target, .. } => {
                assert_eq!(socket_target, &TmuxSocketTarget::Default);
            }
            _ => panic!("expected rename apply row"),
        }
    }

    #[test]
    fn tmux_session_rows_keep_same_session_name_across_different_sockets() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![
                tmux_row("work", TmuxSocketTarget::DedicatedTermy),
                tmux_row("work", TmuxSocketTarget::Default),
            ],
            TmuxSocketTarget::DedicatedTermy,
        );

        let items = state.tmux_session_items_for_query("", None, true);
        let attach_rows = items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    CommandPaletteItemKind::TmuxSessionAttachOrSwitch { .. }
                )
            })
            .count();
        assert_eq!(attach_rows, 2);
    }

    #[test]
    fn tmux_session_rows_dedup_exact_duplicates_only() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![
                tmux_row("work", TmuxSocketTarget::Default),
                tmux_row("work", TmuxSocketTarget::Default),
                tmux_row("work", TmuxSocketTarget::DedicatedTermy),
            ],
            TmuxSocketTarget::Default,
        );

        let items = state.tmux_session_items_for_query("", None, true);
        let attach_rows = items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    CommandPaletteItemKind::TmuxSessionAttachOrSwitch { .. }
                )
            })
            .count();
        assert_eq!(attach_rows, 2);
    }

    #[test]
    fn tmux_session_create_row_uses_configured_create_socket_target() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(Vec::new(), TmuxSocketTarget::DedicatedTermy);
        let items = state.tmux_session_items_for_query("new-session", None, true);
        assert_eq!(items.len(), 1);
        let create_socket_target = match &items[0].kind {
            CommandPaletteItemKind::TmuxSessionCreateAndAttach { socket_target, .. } => {
                Some(socket_target)
            }
            _ => None,
        }
        .expect("expected create-and-attach row");
        assert_eq!(create_socket_target, &TmuxSocketTarget::DedicatedTermy);
    }

    #[test]
    fn back_from_tmux_rename_input_resets_query_and_source_session() {
        let mut state = CommandPaletteState::new(false);
        state.begin_tmux_session_rename("work", TmuxSocketTarget::DedicatedTermy);
        state.input_mut().set_text("next".to_string());

        assert!(state.back_from_tmux_rename_input());
        assert_eq!(state.tmux_session_intent(), TmuxSessionIntent::RenameSelect);
        assert!(state.tmux_rename_source_session().is_none());
        assert!(state.input().text().is_empty());
    }

    #[test]
    fn tmux_attach_intent_empty_query_prepends_utility_rows() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![tmux_row("work", TmuxSocketTarget::Default)],
            TmuxSocketTarget::Default,
        );

        let items = state.tmux_session_items_for_query("", Some("work"), true);
        assert!(matches!(
            items.first().map(|item| &item.kind),
            Some(CommandPaletteItemKind::TmuxSessionDetachCurrent)
        ));
        assert!(matches!(
            items.get(1).map(|item| &item.kind),
            Some(CommandPaletteItemKind::TmuxSessionOpenRenameMode)
        ));
        assert!(matches!(
            items.get(2).map(|item| &item.kind),
            Some(CommandPaletteItemKind::TmuxSessionOpenKillMode)
        ));
    }

    #[test]
    fn tmux_attach_intent_hides_detach_utility_when_exclusive() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![tmux_row("work", TmuxSocketTarget::Default)],
            TmuxSocketTarget::Default,
        );

        let items = state.tmux_session_items_for_query("", Some("work"), false);
        assert!(
            !items
                .iter()
                .any(|item| matches!(item.kind, CommandPaletteItemKind::TmuxSessionDetachCurrent))
        );
        assert!(matches!(
            items.first().map(|item| &item.kind),
            Some(CommandPaletteItemKind::TmuxSessionOpenRenameMode)
        ));
    }

    #[test]
    fn tmux_attach_intent_hides_detach_utility_when_runtime_is_native() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![tmux_row("work", TmuxSocketTarget::Default)],
            TmuxSocketTarget::Default,
        );

        let items = state.tmux_session_items_for_query("", None, true);
        assert!(
            !items
                .iter()
                .any(|item| matches!(item.kind, CommandPaletteItemKind::TmuxSessionDetachCurrent))
        );
        assert!(
            items
                .iter()
                .any(|item| matches!(item.kind, CommandPaletteItemKind::TmuxSessionOpenRenameMode))
        );
        assert!(
            items
                .iter()
                .any(|item| matches!(item.kind, CommandPaletteItemKind::TmuxSessionOpenKillMode))
        );
    }

    #[test]
    fn tmux_attach_intent_hides_utility_rows_when_query_is_non_empty() {
        let mut state = CommandPaletteState::new(false);
        state.set_tmux_session_rows(
            vec![tmux_row("work", TmuxSocketTarget::Default)],
            TmuxSocketTarget::Default,
        );

        let items = state.tmux_session_items_for_query("wo", Some("work"), true);
        assert!(!items.iter().any(|item| matches!(
            item.kind,
            CommandPaletteItemKind::TmuxSessionDetachCurrent
                | CommandPaletteItemKind::TmuxSessionOpenRenameMode
                | CommandPaletteItemKind::TmuxSessionOpenKillMode
        )));
    }
}
