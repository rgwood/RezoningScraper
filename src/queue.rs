use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;

const MAX_ATTEMPTS: i32 = 3;

#[derive(Debug, Clone)]
pub struct Queue<T> {
    name: String,
    _phantom: PhantomData<T>,
}

#[derive(Debug)]
pub struct QueueMessage<T> {
    pub id: i64,
    pub payload: T,
    pub attempts: i32,
    pub created_at: DateTime<Utc>,
    pub last_attempt: Option<DateTime<Utc>>,
}

impl<T> Queue<T>
where
    T: Serialize + DeserializeOwned,
{
    pub fn new(name: &str, conn: &Connection) -> Self {
        initialize(conn).unwrap();
        Queue {
            name: name.to_string(),
            _phantom: PhantomData,
        }
    }

    pub fn push(&self, conn: &Connection, item: T) -> Result<i64> {
        let msg = QueueMessage {
            id: 0, // Will be set by SQLite
            payload: item,
            attempts: 0,
            created_at: Utc::now(),
            last_attempt: None,
        };
        self.push_message(conn, &msg)
    }

    pub fn push_message(&self, conn: &Connection, msg: &QueueMessage<T>) -> Result<i64> {
        let payload = serde_json::to_string(&msg.payload)?;
        let now = msg.created_at.timestamp();
        let last_attempt = msg.last_attempt.map(|dt| dt.timestamp());

        conn.execute(
            "INSERT INTO Queue (queue_name, payload, attempts, created_at, last_attempt) 
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                self.name,
                payload,
                msg.attempts,
                now,
                last_attempt
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    pub fn pop(&self, conn: &mut Connection) -> Result<Option<QueueMessage<T>>> {
        let transaction = conn.transaction()?;
        let result = transaction.query_row(
            "SELECT id, payload, attempts, created_at, last_attempt 
             FROM Queue 
             WHERE queue_name = ?1 
             ORDER BY id ASC 
             LIMIT 1",
            params![self.name],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        ).optional()?;

        match result {
            Some((id, payload_str, attempts, created_at, last_attempt)) => {
                let payload: T = serde_json::from_str(&payload_str)?;
                let message = QueueMessage {
                    id,
                    payload,
                    attempts,
                    created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_default(),
                    last_attempt: last_attempt
                        .map(|ts| DateTime::from_timestamp(ts, 0).unwrap_or_default()),
                };

                transaction.execute(
                    "DELETE FROM Queue WHERE id = ? AND queue_name = ?",
                    params![message.id, self.name],
                )?;

                transaction.commit()?;
                Ok(Some(message))
            }
            None => Ok(None),
        }
    }

    pub fn push_to_dead_letter(&self, conn: &Connection, msg: &QueueMessage<T>, error: &str) -> Result<i64> {
        let payload = serde_json::to_string(&msg.payload)?;
        let now = Utc::now().timestamp();
        let created_at = msg.created_at.timestamp();
        let last_attempt = msg.last_attempt.map(|dt| dt.timestamp());

        conn.execute(
            "INSERT INTO DeadLetterQueue (
                queue_name, payload, attempts, created_at, last_attempt, moved_at, error_text
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                self.name,
                payload,
                msg.attempts,
                created_at,
                last_attempt,
                now,
                error
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }


    pub fn pop_from_dead_letter(&self, conn: &Connection) -> Result<Option<(QueueMessage<T>, String)>> {
        todo!("make this the same as pop but it also returns the error text")
    }

    pub fn remove(&self, conn: &Connection, msg_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM Queue WHERE id = ? AND queue_name = ?",
            params![msg_id, self.name],
        )?;
        Ok(())
    }

}

fn initialize(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS Queue (
            id INTEGER PRIMARY KEY,
            queue_name TEXT NOT NULL,
            payload TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            last_attempt INTEGER
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS DeadLetterQueue (
            id INTEGER PRIMARY KEY,
            queue_name TEXT NOT NULL,
            payload TEXT NOT NULL,
            attempts INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            last_attempt INTEGER,
            moved_at INTEGER NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_queue_name ON Queue(queue_name)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_dlq_queue_name ON DeadLetterQueue(queue_name)",
        [],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestMessage {
        content: String,
    }

    #[test]
    fn test_queue_operations() -> Result<()> {
        let conn = Connection::open_in_memory()?;

        let queue: Queue<TestMessage> = Queue::new("test_queue", &conn);
        
        // TODO add more tests

        Ok(())
    }
}
