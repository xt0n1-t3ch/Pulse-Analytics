use super::models::AccountSnapshot;
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

#[derive(Clone)]
pub(super) struct AccountRecord {
    pub id: String,
    pub provider: String,
    pub origin: String,
    pub source_path: PathBuf,
    pub label: String,
    pub removed: bool,
}

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRecord> {
    Ok(AccountRecord {
        id: row.get(0)?,
        provider: row.get(1)?,
        origin: row.get(2)?,
        source_path: PathBuf::from(row.get::<_, String>(3)?),
        label: row.get(4)?,
        removed: row.get(5)?,
    })
}

pub(super) fn records() -> Result<Vec<AccountRecord>> {
    crate::db::with_connection(|conn| {
        let mut stmt = conn.prepare("SELECT id, provider, origin, source_path, label, removed FROM accounts WHERE removed=0 ORDER BY provider,created_at,id")?;
        Ok(stmt
            .query_map([], from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

pub(super) fn record(id: &str) -> Result<AccountRecord> {
    validate_id(id)?;
    crate::db::with_connection(|conn| {
        Ok(conn.query_row("SELECT id, provider, origin, source_path, label, removed FROM accounts WHERE id=?1 AND removed=0", [id], from_row)?)
    })
}

pub(super) fn validate_id(id: &str) -> Result<()> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Invalid account identifier");
    }
    Ok(())
}

pub(super) fn new_id() -> Result<String> {
    crate::db::with_connection(|conn| {
        Ok(conn.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?)
    })
}

pub(super) fn add(record: &AccountRecord, source_key: &str, explicit: bool) -> Result<String> {
    crate::db::with_connection(|conn| add_into(conn, record, source_key, explicit))
}

fn add_into(
    conn: &Connection,
    record: &AccountRecord,
    source_key: &str,
    explicit: bool,
) -> Result<String> {
    let existing: Option<(String, bool)> = conn
        .query_row(
            "SELECT id,removed FROM accounts WHERE source_key=?1",
            [source_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((id, removed)) = existing {
        if explicit {
            conn.execute(
                "UPDATE accounts SET removed=0,label=?2 WHERE id=?1",
                params![id, record.label],
            )?;
        }
        let _ = removed;
        return Ok(id);
    }
    conn.execute("INSERT INTO accounts(id,provider,origin,source_key,source_path,label,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![record.id,record.provider,record.origin,source_key,record.source_path.to_string_lossy(),record.label,chrono::Utc::now().to_rfc3339()])?;
    Ok(record.id.clone())
}

pub(super) fn remove(id: &str) -> Result<()> {
    validate_id(id)?;
    crate::db::with_connection(|conn| {
        conn.execute("UPDATE accounts SET removed=1 WHERE id=?1", [id])?;
        conn.execute("DELETE FROM account_snapshots WHERE account_id=?1", [id])?;
        Ok(())
    })
}

pub(super) fn snapshots() -> Result<Vec<AccountSnapshot>> {
    let records = records()?;
    crate::db::with_connection(|conn| {
        records
            .iter()
            .map(|record| {
                let payload: Option<String> = conn
                    .query_row(
                        "SELECT payload FROM account_snapshots WHERE account_id=?1",
                        [&record.id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let mut snapshot = payload
                    .and_then(|value| serde_json::from_str::<AccountSnapshot>(&value).ok())
                    .unwrap_or_else(|| AccountSnapshot::pending(record));
                snapshot.label = record.label.clone();
                if snapshot.status == "connected"
                    && snapshot
                        .expires_at
                        .as_deref()
                        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                        .is_none_or(|expires| expires < chrono::Utc::now())
                {
                    snapshot.status = "stale".into();
                }
                Ok(snapshot)
            })
            .collect()
    })
}

pub(super) fn save(snapshot: &AccountSnapshot) -> Result<()> {
    let payload = serde_json::to_string(snapshot)?;
    crate::db::with_connection(|conn| {
        conn.execute("INSERT INTO account_snapshots(account_id,payload,updated_at) SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND removed=0) ON CONFLICT(account_id) DO UPDATE SET payload=excluded.payload,updated_at=excluded.updated_at",
            params![snapshot.id,payload,chrono::Utc::now().to_rfc3339()])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removed_local_account_is_not_rediscovered_until_explicitly_added() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE accounts(id TEXT PRIMARY KEY,provider TEXT,origin TEXT,source_key TEXT UNIQUE,source_path TEXT,label TEXT,removed INTEGER DEFAULT 0,created_at TEXT);").unwrap();
        let record = AccountRecord {
            id: "a".repeat(32),
            provider: "codex".into(),
            origin: "local".into(),
            source_path: PathBuf::from("profile"),
            label: "First".into(),
            removed: false,
        };
        let first = add_into(&conn, &record, "local:codex:profile", false).unwrap();
        conn.execute("UPDATE accounts SET removed=1 WHERE id=?1", [&first])
            .unwrap();
        assert_eq!(
            first,
            add_into(&conn, &record, "local:codex:profile", false).unwrap()
        );
        assert!(
            conn.query_row("SELECT removed FROM accounts", [], |row| row
                .get::<_, bool>(0))
                .unwrap()
        );
        add_into(&conn, &record, "local:codex:profile", true).unwrap();
        assert!(
            !conn
                .query_row("SELECT removed FROM accounts", [], |row| row
                    .get::<_, bool>(0))
                .unwrap()
        );
    }
}
