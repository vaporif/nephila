use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

use crate::id::{AgentId, CheckpointId, EntryId, InterruptId, ObjectiveId};

macro_rules! impl_uuid_sql {
    ($($ty:ident),+) => {$(
        impl ToSql for $ty {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.0.to_string()))
            }
        }

        impl FromSql for $ty {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let s = value.as_str()?;
                let uuid = uuid::Uuid::parse_str(s)
                    .map_err(|e| FromSqlError::Other(Box::new(e)))?;
                Ok(Self(uuid))
            }
        }
    )+};
}

impl_uuid_sql!(AgentId, EntryId, ObjectiveId, CheckpointId, InterruptId);
