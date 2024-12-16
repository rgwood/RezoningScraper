use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
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
    pub fn new(name: &str) -> Self {
        Queue {
            name: name.to_string(),
            _phantom: PhantomData,
        }
    }

    pub fn initialize(conn: &Connection) -> Result<()> {
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

    pub fn push(&self, conn: &Connection, item: &T) -> Result<i64> {
        let msg = QueueMessage {
            id: 0, // Will be set by SQLite
            payload: item.clone(),
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

    pub fn pop(&self, conn: &Connection) -> Result<Option<QueueMessage<T>>> {
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
        );

        match result {
            Ok((id, payload_str, attempts, created_at, last_attempt)) => {
                let payload: T = serde_json::from_str(&payload_str)?;
                Ok(Some(QueueMessage {
                    id,
                    payload,
                    attempts,
                    created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_default(),
                    last_attempt: last_attempt
                        .map(|ts| DateTime::from_timestamp(ts, 0).unwrap_or_default()),
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }?;

        // If we got a message, remove it from the queue
        if result.is_some() {
            transaction.execute(
                "DELETE FROM Queue WHERE id = ? AND queue_name = ?",
                params![result.as_ref().unwrap().id, self.name],
            )?;
        }

        transaction.commit()?;
        Ok(result)
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

    pub fn pop_from_dead_letter(&self, conn: &Connection) -> Result<Option<QueueMessage<T>>> {
        let transaction = conn.transaction()?;

        let result = transaction.query_row(
            "SELECT id, payload, attempts, created_at, last_attempt 
             FROM DeadLetterQueue 
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
        );

        let result = match result {
            Ok((id, payload_str, attempts, created_at, last_attempt)) => {
                let payload: T = serde_json::from_str(&payload_str)?;
                Ok(Some(QueueMessage {
                    id,
                    payload,
                    attempts,
                    created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_default(),
                    last_attempt: last_attempt
                        .map(|ts| DateTime::from_timestamp(ts, 0).unwrap_or_default()),
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }?;

        if result.is_some() {
            transaction.execute(
                "DELETE FROM DeadLetterQueue WHERE id = ? AND queue_name = ?",
                params![result.as_ref().unwrap().id, self.name],
            )?;
        }

        transaction.commit()?;
        Ok(result)
    }

    pub fn update_attempts(&self, conn: &Connection, msg: &QueueMessage<T>, error: Option<&str>) -> Result<()> {
        if msg.attempts + 1 >= MAX_ATTEMPTS {
            // Move to dead letter queue
            self.push_to_dead_letter(conn, msg, error.unwrap_or("Max attempts exceeded"))?;
        } else {
            // Create new message with incremented attempts
            let mut new_msg = msg.clone();
            new_msg.attempts += 1;
            new_msg.last_attempt = Some(Utc::now());
            self.push_message(conn, &new_msg)?;
        }
        Ok(())
    }

    pub fn remove(&self, conn: &Connection, msg_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM Queue WHERE id = ? AND queue_name = ?",
            params![msg_id, self.name],
        )?;
        Ok(())
    }

    pub fn get_dead_letter_messages(&self, conn: &Connection) -> Result<Vec<QueueMessage<T>>> {
        let mut stmt = conn.prepare(
            "SELECT id, payload, attempts, created_at, last_attempt 
             FROM DeadLetterQueue 
             WHERE queue_name = ?
             ORDER BY id ASC",
        )?;

        let messages = stmt
            .query_map(params![self.name], |row| {
                Ok(QueueMessage {
                    id: row.get(0)?,
                    payload: serde_json::from_str(&row.get::<_, String>(1)?).unwrap(),
                    attempts: row.get(2)?,
                    created_at: DateTime::from_timestamp(row.get::<_, i64>(3)?, 0).unwrap_or_default(),
                    last_attempt: row
                        .get::<_, Option<i64>>(4)?
                        .map(|ts| DateTime::from_timestamp(ts, 0).unwrap_or_default()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(messages)
    }
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
        Queue::<TestMessage>::initialize(&conn)?;

        let queue = Queue::new("test_queue");
        
        // Test push
        let msg = TestMessage {
            content: "test message".to_string(),
        };
        let id = queue.push(&conn, &msg)?;
        
        // Test pop
        let popped = queue.pop(&conn)?.unwrap();
        assert_eq!(popped.id, id);
        assert_eq!(popped.payload.content, msg.content);
        assert_eq!(popped.attempts, 0);
        
        // Test update attempts
        queue.update_attempts(&conn, id)?;
        let updated = queue.pop(&conn)?.unwrap();
        assert_eq!(updated.attempts, 1);
        
        // Test max attempts -> dead letter queue
        queue.update_attempts(&conn, id)?;
        queue.update_attempts(&conn, id)?;
        
        // Message should now be in dead letter queue
        assert!(queue.pop(&conn)?.is_none());
        let dead_letters = queue.get_dead_letter_messages(&conn)?;
        assert_eq!(dead_letters.len(), 1);
        assert_eq!(dead_letters[0].payload.content, msg.content);
        assert_eq!(dead_letters[0].attempts, 3);

        Ok(())
    }
}
