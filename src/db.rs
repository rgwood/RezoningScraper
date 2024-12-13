use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::Project;

pub struct Token {
    pub expiration: DateTime<Utc>,
    pub jwt: String,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    #[allow(dead_code)]
    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let db = Database { conn };
        db.initialize_schema()?;
        Ok(db)
    }

    pub fn new_from_file(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let db = Database { conn };
        db.initialize_schema()?;
        Ok(db)
    }

    pub fn initialize_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS Projects(
                Id TEXT PRIMARY KEY NOT NULL,
                Serialized TEXT NOT NULL,
                Tweeted INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS TokenCache(
                Expiration INTEGER NOT NULL,
                Token TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS
                Cache(
                  Key TEXT PRIMARY KEY,
                  Expiration INTEGER NOT NULL,
                  Value TEXT NOT NULL
                );",
            [],
        )?;

        Ok(())
    }

    pub fn contains_project(&self, id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM Projects WHERE Id = ?",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn get_project(&self, id: &str) -> Result<Project> {
        let json: String = self.conn.query_row(
            "SELECT Serialized FROM Projects WHERE Id = ?",
            params![id],
            |row| row.get(0),
        )?;

        Ok(serde_json::from_str(&json)?)
    }

    pub fn upsert_project(&self, project: &Project) -> Result<()> {
        let json = serde_json::to_string(project)?;

        self.conn.execute(
            "INSERT INTO Projects(Id, Serialized) VALUES(?1, ?2)
             ON CONFLICT(Id) DO UPDATE SET Serialized = excluded.Serialized",
            params![project.id, json],
        )?;

        Ok(())
    }

    pub fn get_token(&self) -> Result<Option<Token>> {
        let result = self.conn.query_row(
            "SELECT Expiration, Token FROM TokenCache LIMIT 1",
            [],
            |row| {
                let timestamp: i64 = row.get(0)?;
                let jwt: String = row.get(1)?;
                Ok((timestamp, jwt))
            },
        );

        match result {
            Ok((timestamp, jwt)) => Ok(Some(Token {
                expiration: DateTime::from_timestamp(timestamp / 1000, 0).unwrap_or_default(),
                jwt,
            })),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_token(&mut self, token: &Token) -> Result<()> {
        let transaction = self.conn.transaction()?;

        transaction.execute("DELETE FROM TokenCache", [])?;

        transaction.execute(
            "INSERT INTO TokenCache(Expiration, Token) VALUES(?1, ?2)",
            params![token.expiration.timestamp_millis(), token.jwt,],
        )?;

        transaction.commit()?;
        Ok(())
    }

    pub fn get_cached_response(&self, url: &str) -> Result<Option<String>> {
        let now = Utc::now().timestamp();

        let result = self.conn.query_row(
            "SELECT Value FROM Cache WHERE Key = ? AND Expiration > ?",
            params![url, now],
            |row| row.get::<_, String>(0),
        );

        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn cache_response(&self, url: &str, value: &str) -> Result<()> {
        let expiration = Utc::now().timestamp() + 3600; // 1 hour from now

        self.conn.execute(
            "INSERT INTO Cache(Key, Expiration, Value) VALUES(?1, ?2, ?3)
             ON CONFLICT(Key) DO UPDATE SET Expiration = excluded.Expiration, Value = excluded.Value",
            params![url, expiration, value],
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_initialize_db() -> Result<()> {
        let _db = Database::new_in_memory()?;
        Ok(())
    }

    #[test]
    fn test_contains_works() -> Result<()> {
        let db = Database::new_in_memory()?;

        assert!(!db.contains_project("foo")?);

        let project = Project {
            id: "foo".to_string(),
            project_type: "".to_string(),
            attributes: Default::default(),
            relationships: Default::default(),
            links: Default::default(),
        };

        db.upsert_project(&project)?;
        assert!(db.contains_project("foo")?);

        Ok(())
    }

    #[test]
    fn test_upsert_works() -> Result<()> {
        let db = Database::new_in_memory()?;

        let project1 = Project {
            id: "foo".to_string(),
            project_type: "first".to_string(),
            attributes: Default::default(),
            relationships: Default::default(),
            links: Default::default(),
        };

        db.upsert_project(&project1)?;
        let retrieved = db.get_project("foo")?;
        assert_eq!(retrieved.project_type, "first");

        let project2 = Project {
            project_type: "second".to_string(),
            ..project1
        };

        db.upsert_project(&project2)?;
        let retrieved = db.get_project("foo")?;
        assert_eq!(retrieved.project_type, "second");

        Ok(())
    }

    #[test]
    fn test_token_works() -> Result<()> {
        let mut db = Database::new_in_memory()?;

        assert!(db.get_token()?.is_none());

        let token = Token {
            expiration: Utc::now(),
            jwt: "foo".to_string(),
        };

        db.set_token(&token)?;
        let retrieved = db.get_token()?.unwrap();

        assert_eq!(token.jwt, retrieved.jwt);
        assert!((token.expiration.timestamp() - retrieved.expiration.timestamp()).abs() <= 1);

        Ok(())
    }
}
