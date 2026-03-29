use rusqlite::Connection;
use std::sync::mpsc;
use tokio::sync::oneshot;

type BoxedCommand = Box<dyn FnOnce(&Connection) + Send>;

struct WriterCommand {
    func: BoxedCommand,
}

#[derive(Clone)]
pub struct WriterHandle {
    tx: mpsc::Sender<WriterCommand>,
}

impl WriterHandle {
    pub fn new(conn: Connection) -> Self {
        let (tx, rx) = mpsc::channel::<WriterCommand>();
        std::thread::spawn(move || {
            while let Ok(cmd) = rx.recv() {
                (cmd.func)(&conn);
            }
        });
        Self { tx }
    }

    pub async fn execute<F, R>(&self, func: F) -> Result<R, crate::StoreError>
    where
        F: FnOnce(&Connection) -> Result<R, rusqlite::Error> + Send + 'static,
        R: Send + 'static,
    {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = WriterCommand {
            func: Box::new(move |conn| {
                let result = func(conn);
                let _ = resp_tx.send(result);
            }),
        };
        self.tx
            .send(cmd)
            .map_err(|_| crate::StoreError::WriterClosed)?;
        resp_rx
            .await
            .map_err(|_| crate::StoreError::WriterClosed)?
            .map_err(crate::StoreError::Sqlite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    #[tokio::test]
    async fn insert_and_query_back() {
        schema::register_sqlite_vec();
        let conn = Connection::open_in_memory().unwrap();
        schema::init_db(&conn).unwrap();
        schema::init_vec_tables(&conn, 384).unwrap();
        let writer = WriterHandle::new(conn);

        // FK constraint: hitl_tracking requires an agent
        writer
            .execute(|conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO agents (id, state, directive, directory, objective_id, created_at, updated_at)
                     VALUES ('agent-1', 'starting', 'continue', '/tmp', 'obj-1', ?1, ?2)",
                    rusqlite::params![&now, &now],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        writer
            .execute(|conn| {
                conn.execute(
                    "INSERT INTO hitl_tracking (agent_id, question_hash, ask_count) VALUES (?1, ?2, ?3)",
                    rusqlite::params!["agent-1", 12345i64, 1],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let count: u32 = writer
            .execute(|conn| {
                conn.query_row(
                    "SELECT ask_count FROM hitl_tracking WHERE agent_id = ?1 AND question_hash = ?2",
                    rusqlite::params!["agent-1", 12345i64],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();

        assert_eq!(count, 1);
    }
}
