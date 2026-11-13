use anyhow::Result;
use rusqlite::{params, Connection};
use std::ops::{Deref, DerefMut};

use crate::models::Project;

pub struct Database {
    conn: Connection,
}

impl Deref for Database {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl DerefMut for Database {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
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
        // EngagementHQ-era projects (up to 2026-11-10). Kept read-only for history;
        // nothing writes to this table any more.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS Projects(
                Id TEXT PRIMARY KEY NOT NULL,
                Serialized TEXT NOT NULL,
                Tweeted INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;

        // Projects from the Hive-based Shape Your City site (2026-11-10 onward).
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS HiveProjects(
                Id TEXT PRIMARY KEY NOT NULL,
                Serialized TEXT NOT NULL,
                FirstSeen INTEGER NOT NULL DEFAULT (unixepoch()),
                LastSeen INTEGER NOT NULL DEFAULT (unixepoch())
            )",
            [],
        )?;

        // Old and new IDs for the same project page (matched by URL slug).
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS LegacyProjectLinks(
                HiveProjectId TEXT PRIMARY KEY NOT NULL,
                LegacyProjectId TEXT NOT NULL,
                MatchedOn TEXT NOT NULL
            )",
            [],
        )?;

        // TokenCache and Cache held EngagementHQ API state; unused since the migration.
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

        crate::approvals::initialize_schema(&self.conn)?;
        crate::conditions::initialize_schema(&self.conn)?;
        crate::approval_posts::initialize(&self.conn)?;

        Ok(())
    }

    pub fn contains_project(&self, id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM HiveProjects WHERE Id = ?",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    #[allow(dead_code)]
    pub fn get_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare("SELECT Serialized FROM HiveProjects")?;
        let rows = stmt.query_map([], |row| row.get(0))?;

        let mut projects = Vec::new();
        for json in rows {
            let json: String = json?;
            projects.push(serde_json::from_str(&json)?);
        }

        Ok(projects)
    }

    pub fn get_projects_by_id(&self) -> Result<std::collections::HashMap<String, Project>> {
        Ok(self
            .get_projects()?
            .into_iter()
            .map(|project| (project.id.clone(), project))
            .collect())
    }

    /// The EngagementHQ-era project with this URL slug, if there was one.
    pub fn legacy_project_id(&self, permalink: &str) -> Result<Option<String>> {
        if permalink.is_empty() {
            return Ok(None);
        }
        Ok(self.conn.query_row(
            "SELECT min(Id) FROM Projects
                 WHERE json_extract(Serialized, '$.attributes.permalink') = ?1",
            [permalink],
            |row| row.get(0),
        )?)
    }

    /// Record which EngagementHQ project each new-site project replaced, when the
    /// URL slug matches. Returns how many new links were recorded.
    pub fn link_legacy_projects(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO LegacyProjectLinks(HiveProjectId, LegacyProjectId, MatchedOn)
             SELECT h.Id, min(p.Id), 'permalink'
             FROM HiveProjects h
             JOIN Projects p
               ON json_extract(p.Serialized, '$.attributes.permalink')
                = json_extract(h.Serialized, '$.attributes.permalink')
             WHERE json_extract(h.Serialized, '$.attributes.permalink') <> ''
             GROUP BY h.Id",
            [],
        )?)
    }

    pub fn get_project(&self, id: &str) -> Result<Project> {
        let json: String = self.conn.query_row(
            "SELECT Serialized FROM HiveProjects WHERE Id = ?",
            params![id],
            |row| row.get(0),
        )?;

        Ok(serde_json::from_str(&json)?)
    }

    pub fn upsert_projects(&mut self, projects: &[Project]) -> Result<()> {
        let transaction = self.conn.transaction()?;

        for project in projects {
            let json = serde_json::to_string(project)?;
            transaction.execute(
                "INSERT INTO HiveProjects(Id, Serialized) VALUES(?1, ?2)
                 ON CONFLICT(Id) DO UPDATE SET Serialized = excluded.Serialized,
                    LastSeen = unixepoch()",
                params![project.id, json],
            )?;
        }

        transaction.commit()?;
        Ok(())
    }

    pub fn is_empty(&self) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM HiveProjects", [], |row| row.get(0))?;
        Ok(count == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_db() -> Result<()> {
        let _db = Database::new_in_memory()?;
        Ok(())
    }

    #[test]
    fn test_contains_works() -> Result<()> {
        let mut db = Database::new_in_memory()?;

        assert!(!db.contains_project("foo")?);

        let project = Project {
            id: "foo".to_string(),
            project_type: "".to_string(),
            attributes: Default::default(),
            relationships: Default::default(),
            links: Default::default(),
        };

        db.upsert_projects(&[project])?;
        assert!(db.contains_project("foo")?);

        Ok(())
    }

    #[test]
    fn test_upsert_works() -> Result<()> {
        let mut db = Database::new_in_memory()?;

        let project1 = Project {
            id: "foo".to_string(),
            project_type: "first".to_string(),
            attributes: Default::default(),
            relationships: Default::default(),
            links: Default::default(),
        };

        db.upsert_projects(std::slice::from_ref(&project1))?;
        let retrieved = db.get_project("foo")?;
        assert_eq!(retrieved.project_type, "first");

        let project2 = Project {
            project_type: "second".to_string(),
            ..project1
        };

        db.upsert_projects(std::slice::from_ref(&project2))?;
        let retrieved = db.get_project("foo")?;
        assert_eq!(retrieved.project_type, "second");

        Ok(())
    }

    #[test]
    fn legacy_projects_are_untouched_and_linked() -> Result<()> {
        let mut db = Database::new_in_memory()?;
        db.execute(
            "INSERT INTO Projects(Id, Serialized) VALUES('50711', ?1)",
            [r#"{"attributes":{"permalink":"6525-6575-oak"}}"#],
        )?;
        let mut project = Project {
            id: "hive-2008".to_string(),
            project_type: "hive-project".to_string(),
            attributes: Default::default(),
            relationships: Default::default(),
            links: Default::default(),
        };
        project.attributes.permalink = "6525-6575-oak".into();
        assert!(
            db.is_empty()?,
            "legacy rows do not count as new-site projects"
        );
        db.upsert_projects(&[project])?;
        assert_eq!(
            db.legacy_project_id("6525-6575-oak")?.as_deref(),
            Some("50711")
        );
        assert_eq!(db.legacy_project_id("brand-new-project")?, None);
        assert_eq!(db.legacy_project_id("")?, None);
        assert_eq!(db.link_legacy_projects()?, 1);
        assert_eq!(db.link_legacy_projects()?, 0);
        let legacy: String = db.query_row(
            "SELECT LegacyProjectId FROM LegacyProjectLinks WHERE HiveProjectId='hive-2008'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(legacy, "50711");
        let count: i64 = db.query_row("SELECT COUNT(*) FROM Projects", [], |r| r.get(0))?;
        assert_eq!(count, 1);
        Ok(())
    }
}
