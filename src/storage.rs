//! Persistent browser data. This module deliberately stores navigation metadata
//! only; response bodies, credentials, and cookies are never written here.
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, params};
use thiserror::Error;

use crate::dom::Document;

pub trait DocumentStore {
    fn get(&self, url: &str) -> Option<&Document>;
    fn put(&mut self, document: Document);
}

#[derive(Debug, Default)]
pub struct MemoryStore {
    documents: HashMap<String, Document>,
}

impl DocumentStore for MemoryStore {
    fn get(&self, url: &str) -> Option<&Document> {
        self.documents.get(url)
    }
    fn put(&mut self, document: Document) {
        self.documents.insert(document.url.clone(), document);
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("SQLite storage error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type StorageResult<T> = std::result::Result<T, StorageError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub visited_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSettings {
    pub preferred_theme: String,
    pub default_homepage: String,
    pub request_timeout_seconds: u64,
    pub user_agent: String,
    pub mouse_support: bool,
    pub browser_engine_preference: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            preferred_theme: "auto".into(),
            default_homepage: "https://example.com/".into(),
            request_timeout_seconds: 30,
            user_agent: concat!("WebMorph/", env!("CARGO_PKG_VERSION")).into(),
            mouse_support: true,
            browser_engine_preference: "http".into(),
        }
    }
}

pub struct Storage {
    connection: Connection,
    private_session: bool,
}

impl Storage {
    /// Opens (and migrates) a database. If an existing file is malformed, it is
    /// moved aside and a fresh database is created so startup can recover.
    pub fn open(path: impl AsRef<Path>, private_session: bool) -> StorageResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let connection = match Connection::open(path) {
            Ok(connection) => connection,
            Err(error) if path.exists() => {
                quarantine_database(path)?;
                let _ = error;
                Connection::open(path)?
            }
            Err(error) => return Err(error.into()),
        };
        let mut storage = Self {
            connection,
            private_session,
        };
        if storage.migrate().is_err() && path.exists() {
            // A structurally corrupt database may open successfully but fail
            // during migration. Recreate only after preserving the old file.
            drop(storage);
            quarantine_database(path)?;
            storage = Self {
                connection: Connection::open(path)?,
                private_session,
            };
            storage.migrate()?;
        }
        Ok(storage)
    }

    pub fn open_default(private_session: bool) -> StorageResult<Self> {
        Self::open(default_database_path(), private_session)
    }

    pub fn path() -> PathBuf {
        default_database_path()
    }

    pub fn is_private(&self) -> bool {
        self.private_session
    }

    fn migrate(&mut self) -> StorageResult<()> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 1 {
            let tx = self.connection.transaction()?;
            tx.execute_batch(
                "CREATE TABLE history (
                    id INTEGER PRIMARY KEY, url TEXT NOT NULL, title TEXT NOT NULL,
                    visited_at INTEGER NOT NULL
                 );
                 CREATE INDEX history_visited_at ON history(visited_at DESC);
                 CREATE TABLE bookmarks (
                    id INTEGER PRIMARY KEY, url TEXT NOT NULL UNIQUE, title TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                 );
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 PRAGMA user_version = 1;",
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn record_visit(&mut self, url: &str, title: &str) -> StorageResult<()> {
        if self.private_session {
            return Ok(());
        }
        self.connection.execute(
            "INSERT INTO history(url,title,visited_at) VALUES (?1,?2,?3)",
            params![url, title, timestamp()],
        )?;
        Ok(())
    }

    pub fn history(&self, limit: usize) -> StorageResult<Vec<HistoryEntry>> {
        if self.private_session {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT id,url,title,visited_at FROM history ORDER BY visited_at DESC,id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit.min(i64::MAX as usize) as i64], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                visited_at: row.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn search_history(&self, query: &str, limit: usize) -> StorageResult<Vec<HistoryEntry>> {
        if self.private_session {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", escape_like(query));
        let mut statement = self.connection.prepare(
            "SELECT id,url,title,visited_at FROM history WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\' ORDER BY visited_at DESC,id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![pattern, limit.min(i64::MAX as usize) as i64],
            |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    title: row.get(2)?,
                    visited_at: row.get(3)?,
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn delete_history_entry(&mut self, id: i64) -> StorageResult<bool> {
        if self.private_session {
            return Ok(false);
        }
        Ok(self
            .connection
            .execute("DELETE FROM history WHERE id=?1", [id])?
            > 0)
    }

    pub fn clear_history(&mut self) -> StorageResult<()> {
        if !self.private_session {
            self.connection.execute("DELETE FROM history", [])?;
        }
        Ok(())
    }

    pub fn add_bookmark(&mut self, url: &str, title: &str) -> StorageResult<()> {
        self.connection.execute(
            "INSERT INTO bookmarks(url,title,created_at) VALUES (?1,?2,?3) ON CONFLICT(url) DO UPDATE SET title=excluded.title",
            params![url, title, timestamp()],
        )?;
        Ok(())
    }

    pub fn remove_bookmark(&mut self, url: &str) -> StorageResult<bool> {
        Ok(self
            .connection
            .execute("DELETE FROM bookmarks WHERE url=?1", [url])?
            > 0)
    }

    pub fn bookmarks(&self) -> StorageResult<Vec<Bookmark>> {
        self.search_bookmarks("", usize::MAX)
    }

    pub fn search_bookmarks(&self, query: &str, limit: usize) -> StorageResult<Vec<Bookmark>> {
        let pattern = format!("%{}%", escape_like(query));
        let mut statement = self.connection.prepare(
            "SELECT id,url,title,created_at FROM bookmarks WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\' ORDER BY title COLLATE NOCASE,url LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![pattern, limit.min(i64::MAX as usize) as i64],
            |row| {
                Ok(Bookmark {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    title: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn settings(&self) -> StorageResult<AppSettings> {
        let mut settings = AppSettings::default();
        let mut statement = self.connection.prepare("SELECT key,value FROM settings")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (key, value) = row?;
            match key.as_str() {
                "preferred_theme" => settings.preferred_theme = value,
                "default_homepage" => settings.default_homepage = value,
                "request_timeout_seconds" => {
                    if let Ok(v) = value.parse() {
                        settings.request_timeout_seconds = v;
                    }
                }
                "user_agent" => settings.user_agent = value,
                "mouse_support" => settings.mouse_support = value == "true",
                "browser_engine_preference" => settings.browser_engine_preference = value,
                _ => {}
            }
        }
        Ok(settings)
    }

    pub fn save_settings(&mut self, settings: &AppSettings) -> StorageResult<()> {
        let tx = self.connection.transaction()?;
        for (key, value) in [
            ("preferred_theme", settings.preferred_theme.as_str()),
            ("default_homepage", settings.default_homepage.as_str()),
            ("request_timeout_seconds", ""),
            ("user_agent", settings.user_agent.as_str()),
            (
                "mouse_support",
                if settings.mouse_support {
                    "true"
                } else {
                    "false"
                },
            ),
            (
                "browser_engine_preference",
                settings.browser_engine_preference.as_str(),
            ),
        ] {
            let value = if key == "request_timeout_seconds" {
                settings.request_timeout_seconds.to_string()
            } else {
                value.to_owned()
            };
            tx.execute("INSERT INTO settings(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn default_database_path() -> PathBuf {
    dirs::data_local_dir()
        .map(|dir| dir.join("WebMorph").join("webmorph.sqlite3"))
        .unwrap_or_else(|| {
            dirs::config_dir()
                .map(|dir| dir.join("WebMorph").join("webmorph.sqlite3"))
                .unwrap_or_else(|| PathBuf::from("webmorph.sqlite3"))
        })
}

fn quarantine_database(path: &Path) -> StorageResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut backup = path.as_os_str().to_os_string();
    backup.push(format!(".corrupt-{}", timestamp()));
    fs::rename(path, PathBuf::from(backup))?;
    Ok(())
}

fn timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, DocumentStore, MemoryStore, Storage};
    use crate::dom::Document;
    use tempfile::tempdir;

    #[test]
    fn stores_documents_by_url() {
        let mut store = MemoryStore::default();
        store.put(Document::new("https://example.com", "Example"));
        assert_eq!(
            store
                .get("https://example.com")
                .map(|doc| doc.title.as_str()),
            Some("Example")
        );
    }

    #[test]
    fn persists_history_bookmarks_and_settings() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.sqlite3");
        {
            let mut db = Storage::open(&path, false).unwrap();
            db.record_visit("https://example.com", "Example").unwrap();
            db.record_visit("https://rust-lang.org", "Rust").unwrap();
            assert_eq!(db.search_history("rust", 10).unwrap()[0].title, "Rust");
            db.delete_history_entry(db.history(10).unwrap()[0].id)
                .unwrap();
            assert_eq!(db.history(10).unwrap().len(), 1);
            db.add_bookmark("https://example.com", "Example").unwrap();
            assert_eq!(db.search_bookmarks("exam", 5).unwrap().len(), 1);
            let mut settings = AppSettings::default();
            settings.preferred_theme = "dark".into();
            settings.request_timeout_seconds = 9;
            db.save_settings(&settings).unwrap();
        }
        let db = Storage::open(path, false).unwrap();
        assert_eq!(db.bookmarks().unwrap().len(), 1);
        assert_eq!(db.settings().unwrap().preferred_theme, "dark");
        assert_eq!(db.settings().unwrap().request_timeout_seconds, 9);
    }

    #[test]
    fn private_session_never_persists_or_exposes_history() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("private.sqlite3");
        let mut db = Storage::open(&path, true).unwrap();
        db.record_visit("https://secret.example", "secret").unwrap();
        assert!(db.history(10).unwrap().is_empty());
        drop(db);
        let db = Storage::open(path, false).unwrap();
        assert!(db.history(10).unwrap().is_empty());
    }

    #[test]
    fn corrupted_database_is_preserved_and_recreated() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("broken.sqlite3");
        std::fs::write(&path, b"not a database").unwrap();
        let db = Storage::open(&path, false).unwrap();
        assert!(db.history(10).unwrap().is_empty());
        assert!(std::fs::read_dir(dir.path()).unwrap().count() >= 2);
    }
}
