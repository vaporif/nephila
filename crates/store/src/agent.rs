use crate::SqliteStore;
use crate::util::parse_rfc3339;
use chrono::Utc;
use nephila_core::agent::{Agent, AgentConfigSnapshot, AgentState, SpawnOrigin};
use nephila_core::directive::Directive;
use nephila_core::id::{AgentId, CheckpointId};
use nephila_core::store::AgentStore;
use std::path::PathBuf;

/// JSON-encode `last_config_snapshot` for SQL storage. Returns `None` so the
/// column stays NULL when the agent has no snapshot yet.
fn encode_snapshot(snap: Option<&AgentConfigSnapshot>) -> Option<String> {
    snap.and_then(|s| serde_json::to_string(s).ok())
}

impl AgentStore for SqliteStore {
    async fn register(&self, agent: Agent) -> nephila_core::Result<()> {
        self.writer
            .execute(move |conn| {
                let (origin_type, spawned_by, source_cp) = match &agent.origin {
                    SpawnOrigin::Operator => ("operator", None, None),
                    SpawnOrigin::Agent(id) => ("agent", Some(*id), None),
                    SpawnOrigin::Fork {
                        source_agent_id,
                        source_checkpoint_id,
                    } => ("fork", Some(*source_agent_id), Some(*source_checkpoint_id)),
                };

                let snapshot_json = encode_snapshot(agent.last_config_snapshot.as_ref());
                conn.execute(
                    "INSERT INTO agents (id, state, directive, directory, objective_id, checkpoint_id, restore_checkpoint_id, spawned_by, spawn_origin_type, source_checkpoint_id, injected_message, session_id, last_config_snapshot, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    rusqlite::params![
                        agent.id,
                        agent.state.to_string(),
                        agent.directive.to_string(),
                        agent.directory.to_string_lossy().to_string(),
                        agent.objective_id,
                        agent.checkpoint_id,
                        agent.restore_checkpoint_id,
                        spawned_by,
                        origin_type,
                        source_cp,
                        agent.injected_message,
                        agent.session_id,
                        snapshot_json,
                        agent.created_at.to_rfc3339(),
                        agent.updated_at.to_rfc3339(),
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn get(&self, id: AgentId) -> nephila_core::Result<Option<Agent>> {
        let pool = self.read_pool.clone();
        let record = tokio::task::spawn_blocking(move || -> nephila_core::Result<Option<Agent>> {
            let conn = pool
                .acquire_guarded()
                .map_err(|e| nephila_core::NephilaError::Storage(format!("read pool: {e}")))?;
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, state, directory, objective_id, checkpoint_id, spawned_by, injected_message, created_at, updated_at, directive, session_id, spawn_origin_type, source_checkpoint_id, restore_checkpoint_id, last_config_snapshot
                     FROM agents WHERE id = ?1",
                )
                .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?;
            match stmt.query_row(rusqlite::params![id], row_to_agent) {
                Ok(agent) => Ok(Some(agent)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(nephila_core::NephilaError::Storage(e.to_string())),
            }
        })
        .await
        .map_err(|e| nephila_core::NephilaError::Storage(format!("join: {e}")))??;
        Ok(record)
    }

    async fn list(&self) -> nephila_core::Result<Vec<Agent>> {
        let pool = self.read_pool.clone();
        let records = tokio::task::spawn_blocking(move || -> nephila_core::Result<Vec<Agent>> {
            let conn = pool
                .acquire_guarded()
                .map_err(|e| nephila_core::NephilaError::Storage(format!("read pool: {e}")))?;
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, state, directory, objective_id, checkpoint_id, spawned_by, injected_message, created_at, updated_at, directive, session_id, spawn_origin_type, source_checkpoint_id, restore_checkpoint_id, last_config_snapshot
                     FROM agents ORDER BY created_at",
                )
                .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?;
            let rows = stmt
                .query_map([], row_to_agent)
                .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?;
            Ok(rows)
        })
        .await
        .map_err(|e| nephila_core::NephilaError::Storage(format!("join: {e}")))??;
        Ok(records)
    }

    async fn save(&self, agent: &Agent) -> nephila_core::Result<()> {
        let id = agent.id;
        let state = agent.state.to_string();
        let directive = agent.directive.to_string();
        let session_id = agent.session_id.clone();
        let checkpoint_id = agent.checkpoint_id;
        let restore_checkpoint_id = agent.restore_checkpoint_id;
        let injected_message = agent.injected_message.clone();
        let snapshot_json = encode_snapshot(agent.last_config_snapshot.as_ref());
        let now = agent.updated_at.to_rfc3339();

        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE agents SET state = ?1, directive = ?2, session_id = ?3, checkpoint_id = ?4, restore_checkpoint_id = ?5, injected_message = ?6, last_config_snapshot = ?7, updated_at = ?8 WHERE id = ?9",
                    rusqlite::params![state, directive, session_id, checkpoint_id, restore_checkpoint_id, injected_message, snapshot_json, now, id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::AgentNotFound(id))?;
        Ok(())
    }

    async fn update_state(&self, id: AgentId, state: AgentState) -> nephila_core::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE agents SET state = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![state.to_string(), now, id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::AgentNotFound(id))?;
        Ok(())
    }

    async fn set_directive(&self, id: AgentId, directive: Directive) -> nephila_core::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE agents SET directive = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![directive.to_string(), now, id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::AgentNotFound(id))?;
        Ok(())
    }

    async fn get_directive(&self, id: AgentId) -> nephila_core::Result<Directive> {
        let pool = self.read_pool.clone();
        let directive =
            tokio::task::spawn_blocking(move || -> nephila_core::Result<Option<Directive>> {
                let conn = pool
                    .acquire_guarded()
                    .map_err(|e| nephila_core::NephilaError::Storage(format!("read pool: {e}")))?;
                let mut stmt = conn
                    .prepare_cached("SELECT directive FROM agents WHERE id = ?1")
                    .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?;
                match stmt.query_row(rusqlite::params![id], |row| {
                    let s: String = row.get(0)?;
                    s.parse::<Directive>().map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })
                }) {
                    Ok(d) => Ok(Some(d)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(nephila_core::NephilaError::Storage(e.to_string())),
                }
            })
            .await
            .map_err(|e| nephila_core::NephilaError::Storage(format!("join: {e}")))??;
        directive.ok_or(nephila_core::NephilaError::AgentNotFound(id))
    }

    async fn set_injected_message(
        &self,
        id: AgentId,
        message: Option<String>,
    ) -> nephila_core::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE agents SET injected_message = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![message, now, id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::AgentNotFound(id))?;
        Ok(())
    }

    async fn set_checkpoint_id(
        &self,
        id: AgentId,
        checkpoint_id: CheckpointId,
    ) -> nephila_core::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE agents SET checkpoint_id = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![checkpoint_id, now, id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::AgentNotFound(id))?;
        Ok(())
    }

    async fn set_restore_checkpoint(
        &self,
        id: AgentId,
        checkpoint_id: Option<CheckpointId>,
    ) -> nephila_core::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE agents SET restore_checkpoint_id = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![checkpoint_id, now, id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| nephila_core::NephilaError::AgentNotFound(id))?;
        Ok(())
    }
}

impl SqliteStore {
    /// Returns agents whose state is in an "active phase" — i.e.
    /// `Starting`, `Active`, `Paused`, or `Suspending`. Used by
    /// `SessionRegistry::on_startup` to decide which agents to resume after
    /// an orchestrator restart.
    pub async fn list_agents_in_active_phase(&self) -> nephila_core::Result<Vec<Agent>> {
        let pool = self.read_pool.clone();
        let records = tokio::task::spawn_blocking(move || -> nephila_core::Result<Vec<Agent>> {
            let conn = pool
                .acquire_guarded()
                .map_err(|e| nephila_core::NephilaError::Storage(format!("read pool: {e}")))?;
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, state, directory, objective_id, checkpoint_id, spawned_by, injected_message, created_at, updated_at, directive, session_id, spawn_origin_type, source_checkpoint_id, restore_checkpoint_id, last_config_snapshot
                     FROM agents
                     WHERE state IN ('starting', 'active', 'paused', 'suspending')
                     ORDER BY created_at",
                )
                .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?;
            let rows = stmt
                .query_map([], row_to_agent)
                .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?;
            Ok(rows)
        })
        .await
        .map_err(|e| nephila_core::NephilaError::Storage(format!("join: {e}")))??;
        Ok(records)
    }
}

fn row_to_agent(row: &rusqlite::Row) -> Result<Agent, rusqlite::Error> {
    let state_str: String = row.get(1)?;
    let dir_str: String = row.get(2)?;
    let injected_message: Option<String> = row.get(6)?;
    let created_str: String = row.get(7)?;
    let updated_str: String = row.get(8)?;
    let directive_str: String = row
        .get::<_, Option<String>>(9)?
        .unwrap_or_else(|| "continue".to_string());
    let session_id: Option<String> = row.get(10)?;
    let snapshot_json: Option<String> = row.get(14)?;
    let last_config_snapshot: Option<AgentConfigSnapshot> = match snapshot_json {
        Some(s) => serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(14, rusqlite::types::Type::Text, Box::new(e))
        })?,
        None => None,
    };

    Ok(Agent {
        id: row.get(0)?,
        state: state_str
            .parse::<AgentState>()
            .unwrap_or(AgentState::Failed),
        directive: directive_str
            .parse::<Directive>()
            .unwrap_or(Directive::Continue),
        session_id,
        directory: PathBuf::from(dir_str),
        objective_id: row.get(3)?,
        checkpoint_id: row.get(4)?,
        restore_checkpoint_id: row.get(13)?,
        origin: {
            let origin_type: String = row
                .get::<_, Option<String>>(11)?
                .unwrap_or_else(|| "operator".to_string());
            let spawned_by: Option<AgentId> = row.get(5)?;
            let source_cp: Option<String> = row.get(12)?;
            match origin_type.as_str() {
                "fork" => {
                    let src_agent = spawned_by.ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            "fork origin missing spawned_by".into(),
                        )
                    })?;
                    let src_cp_id = source_cp
                        .as_deref()
                        .ok_or_else(|| {
                            rusqlite::Error::FromSqlConversionFailure(
                                12,
                                rusqlite::types::Type::Text,
                                "fork origin missing source_checkpoint_id".into(),
                            )
                        })?
                        .parse::<uuid::Uuid>()
                        .map(CheckpointId)
                        .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                12,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?;
                    SpawnOrigin::Fork {
                        source_agent_id: src_agent,
                        source_checkpoint_id: src_cp_id,
                    }
                }
                "agent" => SpawnOrigin::Agent(spawned_by.ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        "agent origin missing spawned_by".into(),
                    )
                })?),
                _ => SpawnOrigin::Operator,
            }
        },
        children: Vec::new(),
        injected_message,
        last_config_snapshot,
        created_at: parse_rfc3339(&created_str)?,
        updated_at: parse_rfc3339(&updated_str)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;
    use std::path::PathBuf;

    fn make_agent() -> Agent {
        Agent::new(
            AgentId::new(),
            nephila_core::ObjectiveId::new(),
            PathBuf::from("/tmp/test"),
            SpawnOrigin::Operator,
            None,
        )
    }

    #[tokio::test]
    async fn register_and_get() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent = make_agent();
        let id = agent.id;

        store.register(agent.clone()).await.unwrap();
        let fetched = store.get(id).await.unwrap().unwrap();

        assert_eq!(fetched.id, id);
        assert_eq!(fetched.state, AgentState::Starting);
        assert_eq!(fetched.directory, PathBuf::from("/tmp/test"));
        assert!(matches!(fetched.origin, SpawnOrigin::Operator));
    }

    #[tokio::test]
    async fn update_state() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent = make_agent();
        let id = agent.id;

        store.register(agent).await.unwrap();
        store.update_state(id, AgentState::Active).await.unwrap();

        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.state, AgentState::Active);
    }

    #[tokio::test]
    async fn directive_roundtrip() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent = make_agent();
        let id = agent.id;

        store.register(agent).await.unwrap();

        let d = store.get_directive(id).await.unwrap();
        assert_eq!(d, Directive::Continue);

        store.set_directive(id, Directive::Pause).await.unwrap();
        let d = store.get_directive(id).await.unwrap();
        assert_eq!(d, Directive::Pause);
    }

    #[tokio::test]
    async fn update_state_nonexistent_agent_errors() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let fake_id = AgentId::new();
        let result = store.update_state(fake_id, AgentState::Active).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let fake_id = AgentId::new();
        let result = store.get(fake_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_empty() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agents = store.list().await.unwrap();
        assert!(agents.is_empty());
    }

    /// `last_config_snapshot` survives an SQL round-trip through
    /// `register` → `get`. Regression guard for an earlier hardcoded `None`
    /// in `row_to_agent` that defeated per-agent persistence.
    #[tokio::test]
    async fn last_config_snapshot_round_trips_through_register_get() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let mut agent = make_agent();
        let snap = AgentConfigSnapshot {
            working_dir: PathBuf::from("/agent/work"),
            mcp_endpoint: "http://stub:1234".into(),
            permission_mode: "bypassPermissions".into(),
            claude_binary: PathBuf::from("/usr/local/bin/claude"),
        };
        agent.last_config_snapshot = Some(snap.clone());
        let id = agent.id;
        store.register(agent).await.unwrap();
        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.last_config_snapshot.as_ref(), Some(&snap));
    }

    /// `save` updates the snapshot field on an existing row.
    #[tokio::test]
    async fn last_config_snapshot_updated_via_save() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent = make_agent();
        let id = agent.id;
        store.register(agent).await.unwrap();

        let mut fetched = store.get(id).await.unwrap().unwrap();
        assert!(fetched.last_config_snapshot.is_none());

        let snap = AgentConfigSnapshot {
            working_dir: PathBuf::from("/agent/work"),
            mcp_endpoint: "http://stub".into(),
            permission_mode: "bypassPermissions".into(),
            claude_binary: PathBuf::from("/bin/claude"),
        };
        fetched.last_config_snapshot = Some(snap.clone());
        store.save(&fetched).await.unwrap();

        let after = store.get(id).await.unwrap().unwrap();
        assert_eq!(after.last_config_snapshot.as_ref(), Some(&snap));
    }

    /// Snapshot also round-trips through `list_agents_in_active_phase`.
    #[tokio::test]
    async fn last_config_snapshot_preserved_in_active_phase_listing() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let mut agent = make_agent();
        let snap = AgentConfigSnapshot {
            working_dir: PathBuf::from("/x"),
            mcp_endpoint: "http://x".into(),
            permission_mode: "bypassPermissions".into(),
            claude_binary: PathBuf::from("/x/claude"),
        };
        agent.last_config_snapshot = Some(snap.clone());
        let id = agent.id;
        store.register(agent).await.unwrap();
        store.update_state(id, AgentState::Active).await.unwrap();

        let agents = store.list_agents_in_active_phase().await.unwrap();
        let found = agents.iter().find(|a| a.id == id).expect("active agent");
        assert_eq!(found.last_config_snapshot.as_ref(), Some(&snap));
    }

    #[tokio::test]
    async fn list_agents_in_active_phase_filters_terminal_states() {
        let store = SqliteStore::open_in_memory(384).unwrap();

        let active = make_agent();
        let active_id = active.id;
        store.register(active.clone()).await.unwrap();
        store
            .update_state(active_id, AgentState::Active)
            .await
            .unwrap();

        let exited = make_agent();
        let exited_id = exited.id;
        store.register(exited).await.unwrap();
        store
            .update_state(exited_id, AgentState::Exited)
            .await
            .unwrap();

        let starting = make_agent();
        let starting_id = starting.id;
        store.register(starting).await.unwrap();

        let agents = store.list_agents_in_active_phase().await.unwrap();
        let ids: Vec<_> = agents.iter().map(|a| a.id).collect();
        assert!(ids.contains(&active_id));
        assert!(ids.contains(&starting_id));
        assert!(!ids.contains(&exited_id));
    }

    #[tokio::test]
    async fn save_persists_all_fields() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let mut agent = make_agent();
        let id = agent.id;

        store.register(agent.clone()).await.unwrap();

        agent.state = AgentState::Active;
        agent.directive = Directive::Pause;
        agent.session_id = Some("sess-1".to_string());

        store.save(&agent).await.unwrap();

        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.state, AgentState::Active);
        assert_eq!(fetched.directive, Directive::Pause);
        assert_eq!(fetched.session_id, Some("sess-1".to_string()));
    }

    async fn create_checkpoint(store: &SqliteStore, agent_id: AgentId) -> CheckpointId {
        let cp_id = CheckpointId::new();
        store
            .writer
            .execute(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO checkpoints (id, agent_id, channels, l2_namespace, created_at) VALUES (?1, ?2, '{}', 'general', ?3)",
                    rusqlite::params![cp_id, agent_id, now],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        cp_id
    }

    #[tokio::test]
    async fn set_checkpoint_id_roundtrip() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent = make_agent();
        let id = agent.id;
        store.register(agent).await.unwrap();

        let cp_id = create_checkpoint(&store, id).await;
        store.set_checkpoint_id(id, cp_id).await.unwrap();

        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.checkpoint_id, Some(cp_id));
    }

    #[tokio::test]
    async fn set_restore_checkpoint_roundtrip() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let agent = make_agent();
        let id = agent.id;
        store.register(agent).await.unwrap();

        let cp_id = create_checkpoint(&store, id).await;
        store.set_restore_checkpoint(id, Some(cp_id)).await.unwrap();

        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.restore_checkpoint_id, Some(cp_id));

        store.set_restore_checkpoint(id, None).await.unwrap();
        let fetched = store.get(id).await.unwrap().unwrap();
        assert!(fetched.restore_checkpoint_id.is_none());
    }

    #[tokio::test]
    async fn fork_origin_roundtrip() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let source_agent_id = AgentId::new();
        let source_cp_id = CheckpointId::new();

        let agent = Agent::new(
            AgentId::new(),
            nephila_core::ObjectiveId::new(),
            PathBuf::from("/tmp/fork"),
            SpawnOrigin::Fork {
                source_agent_id,
                source_checkpoint_id: source_cp_id,
            },
            None,
        );
        let id = agent.id;
        store.register(agent).await.unwrap();

        let fetched = store.get(id).await.unwrap().unwrap();
        match fetched.origin {
            SpawnOrigin::Fork {
                source_agent_id: sa,
                source_checkpoint_id: sc,
            } => {
                assert_eq!(sa, source_agent_id);
                assert_eq!(sc, source_cp_id);
            }
            other => panic!("expected Fork, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_origin_roundtrip() {
        let store = SqliteStore::open_in_memory(384).unwrap();
        let parent_id = AgentId::new();

        let agent = Agent::new(
            AgentId::new(),
            nephila_core::ObjectiveId::new(),
            PathBuf::from("/tmp/child"),
            SpawnOrigin::Agent(parent_id),
            None,
        );
        let id = agent.id;
        store.register(agent).await.unwrap();

        let fetched = store.get(id).await.unwrap().unwrap();
        assert!(matches!(fetched.origin, SpawnOrigin::Agent(p) if p == parent_id));
    }
}
