use crate::SqliteStore;
use crate::util::parse_rfc3339;
use chrono::Utc;
use meridian_core::agent::{Agent, AgentState, SpawnOrigin};
use meridian_core::directive::Directive;
use meridian_core::id::{AgentId, CheckpointId};
use meridian_core::store::AgentStore;
use std::path::PathBuf;

impl AgentStore for SqliteStore {
    async fn register(&self, agent: Agent) -> meridian_core::Result<()> {
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

                conn.execute(
                    "INSERT INTO agents (id, state, directive, directory, objective_id, checkpoint_id, restore_checkpoint_id, spawned_by, spawn_origin_type, source_checkpoint_id, injected_message, session_id, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                        agent.created_at.to_rfc3339(),
                        agent.updated_at.to_rfc3339(),
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn get(&self, id: AgentId) -> meridian_core::Result<Option<Agent>> {
        let record = self
            .writer
            .execute(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, state, directory, objective_id, checkpoint_id, spawned_by, injected_message, created_at, updated_at, directive, session_id, spawn_origin_type, source_checkpoint_id, restore_checkpoint_id
                     FROM agents WHERE id = ?1",
                )?;
                let result = stmt.query_row(rusqlite::params![id], row_to_agent);
                match result {
                    Ok(agent) => Ok(Some(agent)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e),
                }
            })
            .await?;
        Ok(record)
    }

    async fn list(&self) -> meridian_core::Result<Vec<Agent>> {
        let records = self
            .writer
            .execute(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, state, directory, objective_id, checkpoint_id, spawned_by, injected_message, created_at, updated_at, directive, session_id, spawn_origin_type, source_checkpoint_id, restore_checkpoint_id
                     FROM agents ORDER BY created_at",
                )?;
                let rows = stmt
                    .query_map([], row_to_agent)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(records)
    }

    async fn save(&self, agent: &Agent) -> meridian_core::Result<()> {
        let id = agent.id;
        let state = agent.state.to_string();
        let directive = agent.directive.to_string();
        let session_id = agent.session_id.clone();
        let checkpoint_id = agent.checkpoint_id;
        let restore_checkpoint_id = agent.restore_checkpoint_id;
        let injected_message = agent.injected_message.clone();
        let now = agent.updated_at.to_rfc3339();

        self.writer
            .execute(move |conn| {
                let rows = conn.execute(
                    "UPDATE agents SET state = ?1, directive = ?2, session_id = ?3, checkpoint_id = ?4, restore_checkpoint_id = ?5, injected_message = ?6, updated_at = ?7 WHERE id = ?8",
                    rusqlite::params![state, directive, session_id, checkpoint_id, restore_checkpoint_id, injected_message, now, id],
                )?;
                if rows == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(())
            })
            .await
            .map_err(|_| meridian_core::MeridianError::AgentNotFound(id))?;
        Ok(())
    }

    async fn update_state(&self, id: AgentId, state: AgentState) -> meridian_core::Result<()> {
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
            .map_err(|_| meridian_core::MeridianError::AgentNotFound(id))?;
        Ok(())
    }

    async fn set_directive(&self, id: AgentId, directive: Directive) -> meridian_core::Result<()> {
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
            .map_err(|_| meridian_core::MeridianError::AgentNotFound(id))?;
        Ok(())
    }

    async fn get_directive(&self, id: AgentId) -> meridian_core::Result<Directive> {
        let directive = self
            .writer
            .execute(move |conn| {
                match conn.query_row(
                    "SELECT directive FROM agents WHERE id = ?1",
                    rusqlite::params![id],
                    |row| {
                        let s: String = row.get(0)?;
                        Ok(s.parse::<Directive>().unwrap_or(Directive::Continue))
                    },
                ) {
                    Ok(d) => Ok(Some(d)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e),
                }
            })
            .await?;
        directive.ok_or(meridian_core::MeridianError::AgentNotFound(id))
    }

    async fn set_injected_message(
        &self,
        id: AgentId,
        message: Option<String>,
    ) -> meridian_core::Result<()> {
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
            .map_err(|_| meridian_core::MeridianError::AgentNotFound(id))?;
        Ok(())
    }

    async fn set_checkpoint_id(
        &self,
        id: AgentId,
        checkpoint_id: CheckpointId,
    ) -> meridian_core::Result<()> {
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
            .map_err(|_| meridian_core::MeridianError::AgentNotFound(id))?;
        Ok(())
    }

    async fn set_restore_checkpoint(
        &self,
        id: AgentId,
        checkpoint_id: Option<CheckpointId>,
    ) -> meridian_core::Result<()> {
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
            .map_err(|_| meridian_core::MeridianError::AgentNotFound(id))?;
        Ok(())
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
                    let src_agent = spawned_by.expect("fork must have spawned_by");
                    let src_cp_id = source_cp
                        .and_then(|s| s.parse::<uuid::Uuid>().ok())
                        .map(CheckpointId)
                        .expect("fork must have source_checkpoint_id");
                    SpawnOrigin::Fork {
                        source_agent_id: src_agent,
                        source_checkpoint_id: src_cp_id,
                    }
                }
                "agent" => {
                    SpawnOrigin::Agent(spawned_by.expect("agent origin must have spawned_by"))
                }
                _ => SpawnOrigin::Operator,
            }
        },
        children: Vec::new(),
        injected_message,
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
            meridian_core::ObjectiveId::new(),
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
            meridian_core::ObjectiveId::new(),
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
            meridian_core::ObjectiveId::new(),
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
