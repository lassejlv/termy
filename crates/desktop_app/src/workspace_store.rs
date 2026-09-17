//! SQLite-backed store for workspace / tab / pane session state and named
//! layouts. A fresh database imports legacy `native-tabs.json` once and reads
//! it no further.
//!
//! sqlx is async-first while the app runs on gpui/smol, so the store owns a
//! small single-threaded tokio runtime and exposes blocking methods. Callers
//! already run persistence work off the UI thread (debounced write threads),
//! or accept a fast blocking call (startup restore, quit-time sync).

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::path::Path;

pub(crate) const WORKSPACE_STORE_FILE: &str = "workspaces.db";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredPane {
    /// Live multiplexer layout only; not stored in the restart-snapshot database.
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    pub(crate) left: u16,
    pub(crate) top: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) buffer: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredTab {
    #[serde(default)]
    pub(crate) zoomed: bool,
    #[serde(default)]
    pub(crate) presentation: Option<StoredTabPresentation>,
    pub(crate) pinned: bool,
    pub(crate) manual_title: Option<String>,
    pub(crate) active_pane: usize,
    /// Pane split layout tree, serialized as JSON in the same shape the
    /// legacy file format used.
    pub(crate) layout_tree_json: Option<String>,
    pub(crate) panes: Vec<StoredPane>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredWorkspace {
    pub(crate) name: String,
    pub(crate) pinned: bool,
    pub(crate) active_tab: usize,
    pub(crate) tabs: Vec<StoredTab>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredSession {
    pub(crate) workspaces: Vec<StoredWorkspace>,
    pub(crate) active_workspace: usize,
}

/// UI title/status snapshot for live sessions; terminal events refresh it on attach.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredTabPresentation {
    pub(crate) title: String,
    pub(crate) explicit_title: Option<String>,
    pub(crate) explicit_title_is_prediction: bool,
    pub(crate) shell_title: Option<String>,
    pub(crate) current_command: Option<String>,
    pub(crate) last_prompt_cwd: Option<String>,
    pub(crate) running_process: bool,
}

pub(crate) struct WorkspaceStore {
    runtime: tokio::runtime::Runtime,
    pool: SqlitePool,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    active_tab INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS tabs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0,
    manual_title TEXT,
    active_pane INTEGER NOT NULL DEFAULT 0,
    layout_tree TEXT
);
CREATE TABLE IF NOT EXISTS panes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tab_id INTEGER NOT NULL REFERENCES tabs(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    pane_left INTEGER NOT NULL,
    pane_top INTEGER NOT NULL,
    pane_width INTEGER NOT NULL,
    pane_height INTEGER NOT NULL,
    buffer TEXT
);
CREATE INDEX IF NOT EXISTS idx_tabs_workspace ON tabs(workspace_id, position);
CREATE INDEX IF NOT EXISTS idx_panes_tab ON panes(tab_id, position);
CREATE TABLE IF NOT EXISTS named_layouts (
    name TEXT PRIMARY KEY COLLATE NOCASE,
    snapshot TEXT NOT NULL
);
";

const STORE_VERSION: &str = "1";

fn store_error(context: &str, error: sqlx::Error) -> String {
    format!("{context}: {error}")
}

async fn table_has_column(pool: &SqlitePool, table: &str, column: &str) -> Result<bool, String> {
    let columns = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .map_err(|error| store_error("Failed to inspect workspace store schema", error))?;
    Ok(columns
        .iter()
        .any(|row| row.get::<String, _>("name") == column))
}

async fn ensure_workspace_columns(pool: &SqlitePool) -> Result<(), String> {
    if !table_has_column(pool, "workspaces", "pinned").await? {
        sqlx::query("ALTER TABLE workspaces ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await
            .map_err(|error| store_error("Failed to migrate workspace pinned state", error))?;
    }
    Ok(())
}

impl WorkspaceStore {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("Failed to start workspace store runtime: {error}"))?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = runtime.block_on(async {
            let pool = SqlitePoolOptions::new()
                // A single connection serializes writers and keeps the WAL
                // file from accumulating concurrent-writer contention.
                .max_connections(1)
                .connect_with(options)
                .await
                .map_err(|error| store_error("Failed to open workspace store", error))?;
            sqlx::raw_sql(SCHEMA)
                .execute(&pool)
                .await
                .map_err(|error| store_error("Failed to initialize workspace store", error))?;
            ensure_workspace_columns(&pool).await?;
            Ok::<_, String>(pool)
        })?;
        Ok(Self { runtime, pool })
    }

    /// `true` until the first successful `save_session` / migration marks the
    /// database as initialized. Used to trigger the one-time JSON import.
    pub(crate) fn is_fresh(&self) -> Result<bool, String> {
        self.runtime.block_on(async {
            let row = sqlx::query("SELECT value FROM meta WHERE key = 'version'")
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| store_error("Failed to read workspace store version", error))?;
            Ok(row.is_none())
        })
    }

    pub(crate) fn mark_initialized(&self) -> Result<(), String> {
        self.runtime.block_on(async {
            sqlx::query("INSERT OR REPLACE INTO meta(key, value) VALUES('version', ?1)")
                .bind(STORE_VERSION)
                .execute(&self.pool)
                .await
                .map_err(|error| store_error("Failed to mark workspace store", error))?;
            Ok(())
        })
    }

    pub(crate) fn save_session(&self, session: &StoredSession) -> Result<(), String> {
        self.runtime.block_on(async {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|error| store_error("Failed to begin session write", error))?;
            sqlx::raw_sql("DELETE FROM panes; DELETE FROM tabs; DELETE FROM workspaces;")
                .execute(&mut *tx)
                .await
                .map_err(|error| store_error("Failed to clear previous session", error))?;

            for (workspace_position, workspace) in session.workspaces.iter().enumerate() {
                let workspace_id = sqlx::query(
                    "INSERT INTO workspaces(position, name, pinned, active_tab) VALUES(?1, ?2, ?3, ?4)",
                )
                .bind(workspace_position as i64)
                .bind(workspace.name.as_str())
                .bind(workspace.pinned)
                .bind(workspace.active_tab as i64)
                .execute(&mut *tx)
                .await
                .map_err(|error| store_error("Failed to write workspace", error))?
                .last_insert_rowid();

                for (tab_position, tab) in workspace.tabs.iter().enumerate() {
                    let tab_id = sqlx::query(
                        "INSERT INTO tabs(workspace_id, position, pinned, manual_title, \
                         active_pane, layout_tree) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    )
                    .bind(workspace_id)
                    .bind(tab_position as i64)
                    .bind(tab.pinned)
                    .bind(tab.manual_title.as_deref())
                    .bind(tab.active_pane as i64)
                    .bind(tab.layout_tree_json.as_deref())
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| store_error("Failed to write tab", error))?
                    .last_insert_rowid();

                    for (pane_position, pane) in tab.panes.iter().enumerate() {
                        sqlx::query(
                            "INSERT INTO panes(tab_id, position, pane_left, pane_top, \
                             pane_width, pane_height, buffer) \
                             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        )
                        .bind(tab_id)
                        .bind(pane_position as i64)
                        .bind(i64::from(pane.left))
                        .bind(i64::from(pane.top))
                        .bind(i64::from(pane.width))
                        .bind(i64::from(pane.height))
                        .bind(pane.buffer.as_deref())
                        .execute(&mut *tx)
                        .await
                        .map_err(|error| store_error("Failed to write pane", error))?;
                    }
                }
            }

            sqlx::query("INSERT OR REPLACE INTO meta(key, value) VALUES('active_workspace', ?1)")
                .bind(session.active_workspace as i64)
                .execute(&mut *tx)
                .await
                .map_err(|error| store_error("Failed to write active workspace", error))?;
            sqlx::query("INSERT OR REPLACE INTO meta(key, value) VALUES('version', ?1)")
                .bind(STORE_VERSION)
                .execute(&mut *tx)
                .await
                .map_err(|error| store_error("Failed to write store version", error))?;

            tx.commit()
                .await
                .map_err(|error| store_error("Failed to commit session write", error))
        })
    }

    pub(crate) fn load_session(&self) -> Result<Option<StoredSession>, String> {
        self.runtime.block_on(async {
            let workspace_rows = sqlx::query(
                "SELECT id, name, pinned, active_tab FROM workspaces ORDER BY position",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|error| store_error("Failed to read workspaces", error))?;
            if workspace_rows.is_empty() {
                return Ok(None);
            }

            let tab_rows = sqlx::query(
                "SELECT id, workspace_id, pinned, manual_title, active_pane, layout_tree \
                 FROM tabs ORDER BY workspace_id, position",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|error| store_error("Failed to read tabs", error))?;
            let pane_rows = sqlx::query(
                "SELECT tab_id, pane_left, pane_top, pane_width, pane_height, buffer \
                 FROM panes ORDER BY tab_id, position",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|error| store_error("Failed to read panes", error))?;

            let mut panes_by_tab: HashMap<i64, Vec<StoredPane>> = HashMap::new();
            for row in pane_rows {
                let tab_id: i64 = row.get("tab_id");
                panes_by_tab.entry(tab_id).or_default().push(StoredPane {
                    session_id: None,
                    left: clamp_cell(row.get::<i64, _>("pane_left")),
                    top: clamp_cell(row.get::<i64, _>("pane_top")),
                    width: clamp_cell(row.get::<i64, _>("pane_width")).max(1),
                    height: clamp_cell(row.get::<i64, _>("pane_height")).max(1),
                    buffer: row.get("buffer"),
                });
            }

            let mut tabs_by_workspace: HashMap<i64, Vec<StoredTab>> = HashMap::new();
            for row in tab_rows {
                let tab_id: i64 = row.get("id");
                let workspace_id: i64 = row.get("workspace_id");
                let panes = panes_by_tab.remove(&tab_id).unwrap_or_default();
                if panes.is_empty() {
                    continue;
                }
                tabs_by_workspace
                    .entry(workspace_id)
                    .or_default()
                    .push(StoredTab {
                        zoomed: false,
                        presentation: None,
                        pinned: row.get("pinned"),
                        manual_title: row.get("manual_title"),
                        active_pane: row.get::<i64, _>("active_pane").max(0) as usize,
                        layout_tree_json: row.get("layout_tree"),
                        panes,
                    });
            }

            let mut workspaces = Vec::with_capacity(workspace_rows.len());
            for row in workspace_rows {
                let workspace_id: i64 = row.get("id");
                let tabs = tabs_by_workspace.remove(&workspace_id).unwrap_or_default();
                let active_tab = if tabs.is_empty() {
                    0
                } else {
                    (row.get::<i64, _>("active_tab").max(0) as usize)
                        .min(tabs.len().saturating_sub(1))
                };
                workspaces.push(StoredWorkspace {
                    name: row.get("name"),
                    pinned: row.get("pinned"),
                    active_tab,
                    tabs,
                });
            }
            if workspaces.is_empty() {
                return Ok(None);
            }

            let active_workspace =
                sqlx::query("SELECT value FROM meta WHERE key = 'active_workspace'")
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|error| store_error("Failed to read active workspace", error))?
                    .and_then(|row| row.get::<String, _>("value").parse::<usize>().ok())
                    .unwrap_or(0)
                    .min(workspaces.len() - 1);

            Ok(Some(StoredSession {
                workspaces,
                active_workspace,
            }))
        })
    }

    pub(crate) fn clear_session(&self) -> Result<(), String> {
        self.runtime.block_on(async {
            sqlx::raw_sql("DELETE FROM panes; DELETE FROM tabs; DELETE FROM workspaces;")
                .execute(&self.pool)
                .await
                .map_err(|error| store_error("Failed to clear session", error))?;
            Ok(())
        })
    }

    pub(crate) fn strip_session_buffers(&self) -> Result<(), String> {
        self.runtime.block_on(async {
            sqlx::query("UPDATE panes SET buffer = NULL")
                .execute(&self.pool)
                .await
                .map_err(|error| store_error("Failed to strip session buffers", error))?;
            Ok(())
        })
    }

    pub(crate) fn named_layout_names(&self) -> Result<Vec<String>, String> {
        self.runtime.block_on(async {
            let rows = sqlx::query("SELECT name FROM named_layouts ORDER BY name COLLATE NOCASE")
                .fetch_all(&self.pool)
                .await
                .map_err(|error| store_error("Failed to read saved layouts", error))?;
            Ok(rows.into_iter().map(|row| row.get("name")).collect())
        })
    }

    pub(crate) fn all_named_layouts(&self) -> Result<Vec<(String, String)>, String> {
        self.runtime.block_on(async {
            let rows = sqlx::query(
                "SELECT name, snapshot FROM named_layouts ORDER BY name COLLATE NOCASE",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|error| store_error("Failed to read saved layouts", error))?;
            Ok(rows
                .into_iter()
                .map(|row| (row.get("name"), row.get("snapshot")))
                .collect())
        })
    }

    /// Look up a layout by case-insensitive name; returns the stored
    /// (canonical) name plus the snapshot JSON.
    pub(crate) fn named_layout(&self, name: &str) -> Result<Option<(String, String)>, String> {
        self.runtime.block_on(async {
            let row = sqlx::query("SELECT name, snapshot FROM named_layouts WHERE name = ?1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| store_error("Failed to read saved layout", error))?;
            Ok(row.map(|row| (row.get("name"), row.get("snapshot"))))
        })
    }

    /// Insert or replace a layout. The stored name takes the caller's casing.
    pub(crate) fn upsert_named_layout(&self, name: &str, snapshot: &str) -> Result<(), String> {
        self.runtime.block_on(async {
            let updated =
                sqlx::query("UPDATE named_layouts SET name = ?1, snapshot = ?2 WHERE name = ?1")
                    .bind(name)
                    .bind(snapshot)
                    .execute(&self.pool)
                    .await
                    .map_err(|error| store_error("Failed to update saved layout", error))?
                    .rows_affected();
            if updated == 0 {
                sqlx::query("INSERT INTO named_layouts(name, snapshot) VALUES(?1, ?2)")
                    .bind(name)
                    .bind(snapshot)
                    .execute(&self.pool)
                    .await
                    .map_err(|error| store_error("Failed to save layout", error))?;
            }
            Ok(())
        })
    }

    /// Update a layout's snapshot only when it already exists (autosave).
    pub(crate) fn update_named_layout_if_exists(
        &self,
        name: &str,
        snapshot: &str,
    ) -> Result<(), String> {
        self.runtime.block_on(async {
            sqlx::query("UPDATE named_layouts SET snapshot = ?2 WHERE name = ?1")
                .bind(name)
                .bind(snapshot)
                .execute(&self.pool)
                .await
                .map_err(|error| store_error("Failed to autosave layout", error))?;
            Ok(())
        })
    }

    pub(crate) fn rename_named_layout(
        &self,
        current_name: &str,
        next_name: &str,
    ) -> Result<(), String> {
        self.runtime.block_on(async {
            let conflict =
                sqlx::query("SELECT 1 AS hit FROM named_layouts WHERE name = ?2 AND name != ?1")
                    .bind(current_name)
                    .bind(next_name)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|error| store_error("Failed to check saved layout name", error))?;
            if conflict.is_some() {
                return Err(format!(
                    "A saved layout named \"{next_name}\" already exists"
                ));
            }
            let updated = sqlx::query("UPDATE named_layouts SET name = ?2 WHERE name = ?1")
                .bind(current_name)
                .bind(next_name)
                .execute(&self.pool)
                .await
                .map_err(|error| store_error("Failed to rename saved layout", error))?
                .rows_affected();
            if updated == 0 {
                return Err(format!("Saved layout \"{current_name}\" was not found"));
            }
            Ok(())
        })
    }

    pub(crate) fn delete_named_layout(&self, name: &str) -> Result<bool, String> {
        self.runtime.block_on(async {
            let deleted = sqlx::query("DELETE FROM named_layouts WHERE name = ?1")
                .bind(name)
                .execute(&self.pool)
                .await
                .map_err(|error| store_error("Failed to delete saved layout", error))?
                .rows_affected();
            Ok(deleted > 0)
        })
    }
}

fn clamp_cell(value: i64) -> u16 {
    value.clamp(0, i64::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (tempfile::TempDir, WorkspaceStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = WorkspaceStore::open(&dir.path().join(WORKSPACE_STORE_FILE)).expect("open");
        (dir, store)
    }

    fn sample_session() -> StoredSession {
        StoredSession {
            workspaces: vec![
                StoredWorkspace {
                    name: "Workspace 1".to_string(),
                    pinned: true,
                    active_tab: 1,
                    tabs: vec![
                        StoredTab {
                            zoomed: false,
                            presentation: None,
                            pinned: true,
                            manual_title: Some("build".to_string()),
                            active_pane: 0,
                            layout_tree_json: None,
                            panes: vec![StoredPane {
                                session_id: None,
                                left: 0,
                                top: 0,
                                width: 80,
                                height: 24,
                                buffer: Some("hello".to_string()),
                            }],
                        },
                        StoredTab {
                            zoomed: false,
                            presentation: None,
                            pinned: false,
                            manual_title: None,
                            active_pane: 1,
                            layout_tree_json: Some("{\"kind\":\"leaf\",\"pane\":0}".to_string()),
                            panes: vec![
                                StoredPane {
                                    session_id: None,
                                    left: 0,
                                    top: 0,
                                    width: 40,
                                    height: 24,
                                    buffer: None,
                                },
                                StoredPane {
                                    session_id: None,
                                    left: 40,
                                    top: 0,
                                    width: 40,
                                    height: 24,
                                    buffer: None,
                                },
                            ],
                        },
                    ],
                },
                StoredWorkspace {
                    name: "Workspace 2".to_string(),
                    pinned: false,
                    active_tab: 0,
                    tabs: vec![StoredTab {
                        zoomed: false,
                        presentation: None,
                        pinned: false,
                        manual_title: Some("Docs".to_string()),
                        active_pane: 0,
                        layout_tree_json: None,
                        panes: vec![StoredPane {
                            session_id: None,
                            left: 0,
                            top: 0,
                            width: 80,
                            height: 24,
                            buffer: None,
                        }],
                    }],
                },
            ],
            active_workspace: 1,
        }
    }

    #[test]
    fn session_round_trips_workspace_grouping() {
        let (_dir, store) = test_store();
        assert!(store.is_fresh().expect("fresh check"));
        assert_eq!(store.load_session().expect("load"), None);

        let session = sample_session();
        store.save_session(&session).expect("save");
        assert!(!store.is_fresh().expect("fresh check"));
        let loaded = store.load_session().expect("load").expect("session");
        assert_eq!(loaded, session);
    }

    #[test]
    fn session_round_trips_empty_workspace() {
        let (_dir, store) = test_store();
        let mut session = sample_session();
        session.workspaces.push(StoredWorkspace {
            name: "Empty".to_string(),
            pinned: true,
            active_tab: 0,
            tabs: Vec::new(),
        });
        session.active_workspace = 2;

        store.save_session(&session).expect("save");
        assert_eq!(store.load_session().expect("load"), Some(session));
    }

    #[test]
    fn save_session_replaces_previous_session() {
        let (_dir, store) = test_store();
        store.save_session(&sample_session()).expect("save");
        let mut next = sample_session();
        next.workspaces.truncate(1);
        next.active_workspace = 0;
        next.workspaces[0].name = "Only".to_string();
        store.save_session(&next).expect("save");
        assert_eq!(store.load_session().expect("load"), Some(next));
    }

    #[test]
    fn clear_session_removes_workspaces_but_keeps_layouts() {
        let (_dir, store) = test_store();
        store.save_session(&sample_session()).expect("save");
        store.upsert_named_layout("Main", "{}").expect("upsert");
        store.clear_session().expect("clear");
        assert_eq!(store.load_session().expect("load"), None);
        assert_eq!(
            store.named_layout_names().expect("names"),
            vec!["Main".to_string()]
        );
    }

    #[test]
    fn strip_session_buffers_clears_pane_buffers() {
        let (_dir, store) = test_store();
        store.save_session(&sample_session()).expect("save");
        store.strip_session_buffers().expect("strip");
        let loaded = store.load_session().expect("load").expect("session");
        assert!(
            loaded
                .workspaces
                .iter()
                .flat_map(|workspace| workspace.tabs.iter())
                .flat_map(|tab| tab.panes.iter())
                .all(|pane| pane.buffer.is_none())
        );
    }

    #[test]
    fn named_layouts_match_case_insensitively_and_rename_checks_conflicts() {
        let (_dir, store) = test_store();
        store
            .upsert_named_layout("Main", "{\"a\":1}")
            .expect("upsert");
        store
            .upsert_named_layout("MAIN", "{\"a\":2}")
            .expect("upsert");
        assert_eq!(
            store.named_layout("main").expect("get"),
            Some(("MAIN".to_string(), "{\"a\":2}".to_string()))
        );

        store.upsert_named_layout("Alt", "{}").expect("upsert");
        assert!(store.rename_named_layout("alt", "MAIN").is_err());
        store
            .rename_named_layout("main", "Primary")
            .expect("rename");
        assert_eq!(
            store.named_layout_names().expect("names"),
            vec!["Alt".to_string(), "Primary".to_string()]
        );
        assert!(store.delete_named_layout("primary").expect("delete"));
        assert!(!store.delete_named_layout("primary").expect("delete"));
    }
}
