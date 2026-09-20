//! Store PDF bytes once, while preserving each document's version history.
use anyhow::Result;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn initialize(db: &Connection) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS ApprovalPdfContent (
        Hash TEXT PRIMARY KEY, Content BLOB NOT NULL);",
    )?;
    let columns = tx
        .prepare("PRAGMA table_info(ApprovalDocumentVersions)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|c| c == "ContentHash") {
        tx.execute("ALTER TABLE ApprovalDocumentVersions ADD COLUMN ContentHash TEXT REFERENCES ApprovalPdfContent(Hash)", [])?;
    }
    let columns = tx
        .prepare("PRAGMA table_info(ApprovalDocuments)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|c| c == "CurrentVersionId") {
        tx.execute(
            "ALTER TABLE ApprovalDocuments ADD COLUMN CurrentVersionId INTEGER",
            [],
        )?;
        tx.execute("UPDATE ApprovalDocuments SET CurrentVersionId = (SELECT Id FROM ApprovalDocumentVersions v WHERE v.DocumentId = ApprovalDocuments.Id ORDER BY LastSeen DESC, Id DESC LIMIT 1)", [])?;
    }
    // Migrate one blob at a time; do not load an entire archive into memory.
    let ids = tx
        .prepare("SELECT Id FROM ApprovalDocumentVersions WHERE ContentHash IS NULL")?
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in ids {
        let bytes: Vec<u8> = tx.query_row(
            "SELECT Content FROM ApprovalDocumentVersions WHERE Id = ?1",
            [id],
            |r| r.get(0),
        )?;
        let hash = content_hash(&bytes);
        tx.execute(
            "INSERT OR IGNORE INTO ApprovalPdfContent VALUES (?1, ?2)",
            params![hash, bytes],
        )?;
        // Keep the legacy NOT NULL column for schema compatibility, without a second copy.
        tx.execute(
            "UPDATE ApprovalDocumentVersions SET ContentHash = ?2, Content = X'' WHERE Id = ?1",
            params![id, hash],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn read_pdf(db: &Connection, version: i64) -> Result<Vec<u8>> {
    Ok(db.query_row(
        "SELECT coalesce(b.Content, v.Content) FROM ApprovalDocumentVersions v
        LEFT JOIN ApprovalPdfContent b ON b.Hash = v.ContentHash WHERE v.Id = ?1",
        [version],
        |r| r.get(0),
    )?)
}

#[derive(Debug, serde::Serialize)]
pub struct StorageStats {
    pub document_versions: i64,
    pub unique_pdfs: i64,
    pub pdf_bytes: i64,
    pub version_pdf_bytes: i64,
    pub analysis_bytes: i64,
    pub database_allocated_bytes: i64,
    pub database_reusable_bytes: i64,
}

pub fn stats(db: &Connection) -> Result<StorageStats> {
    let scalar = |sql: &str| -> rusqlite::Result<i64> { db.query_row(sql, [], |r| r.get(0)) };
    let page_size = scalar("PRAGMA page_size")?;
    Ok(StorageStats {
        document_versions: scalar("SELECT count(*) FROM ApprovalDocumentVersions")?,
        unique_pdfs: scalar("SELECT count(*) FROM ApprovalPdfContent")?,
        pdf_bytes: scalar("SELECT coalesce(sum(length(Content)), 0) FROM ApprovalPdfContent")?,
        version_pdf_bytes: scalar("SELECT coalesce(sum(length(coalesce(b.Content, v.Content))), 0) FROM ApprovalDocumentVersions v LEFT JOIN ApprovalPdfContent b ON b.Hash = v.ContentHash")?,
        analysis_bytes: scalar("SELECT coalesce(sum(coalesce(length(PagesJson),0)+coalesce(length(SummaryJson),0)+coalesce(length(RawResponse),0)+coalesce(length(TraceJson),0)),0) FROM ConditionsSummaries")?,
        database_allocated_bytes: scalar("PRAGMA page_count")? * page_size,
        database_reusable_bytes: scalar("PRAGMA freelist_count")? * page_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_duplicate_legacy_pdfs_without_losing_ids_or_bytes() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE ApprovalDocuments(Id INTEGER PRIMARY KEY);
            INSERT INTO ApprovalDocuments VALUES(1),(2);
            CREATE TABLE ApprovalDocumentVersions(Id INTEGER PRIMARY KEY, DocumentId INTEGER, FirstSeen INTEGER, LastSeen INTEGER, Content BLOB NOT NULL);
            INSERT INTO ApprovalDocumentVersions VALUES(5,1,10,30,X'25504446'),(6,2,20,20,X'25504446'),(7,1,25,25,X'2550444632');").unwrap();
        initialize(&db).unwrap();
        initialize(&db).unwrap();
        assert_eq!(read_pdf(&db, 5).unwrap(), b"%PDF");
        assert_eq!(read_pdf(&db, 6).unwrap(), b"%PDF");
        assert_eq!(read_pdf(&db, 7).unwrap(), b"%PDF2");
        assert_eq!(
            db.query_row("SELECT count(*) FROM ApprovalPdfContent", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT sum(length(Content)) FROM ApprovalDocumentVersions",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT CurrentVersionId FROM ApprovalDocuments WHERE Id=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            5
        );
    }
}
