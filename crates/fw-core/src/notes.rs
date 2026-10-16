//! Texto local opt-in, separado das métricas. Nenhum áudio ou contexto de janela.
use std::fs::{self, DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    pub ts: i64,
    pub text: String,
    pub language: String,
    pub words: u64,
}

// Debug é usado por envelopes IPC: nunca incluir o conteúdo.
impl std::fmt::Debug for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Note")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum NotesError {
    #[error("armazenamento de notas indisponível")]
    Io(#[from] std::io::Error),
    #[error("banco de notas indisponível")]
    Db(#[from] rusqlite::Error),
    #[error("armazenamento de notas inseguro")]
    Unsafe,
    #[error("Notas por voz desligado")]
    Disabled,
    #[error("esquema de notas mais novo que este binário")]
    Newer,
    #[error("nota acima do limite de armazenamento")]
    TooLong,
}

pub fn default_path() -> Option<PathBuf> {
    crate::history::default_path().map(|p| p.with_file_name("notes").join("notes.sqlite"))
}

/// Não abre/cria nada quando desligado; leitura e limpeza ausentes são no-op.
pub struct Notes {
    path: PathBuf,
}
impl Notes {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn open(&self, create: bool) -> Result<Option<Connection>, NotesError> {
        if !create && !self.path.try_exists()? {
            return Ok(None);
        }
        let dir = self.path.parent().ok_or(NotesError::Unsafe)?;
        if create {
            // Apenas o diretório exclusivo das notas fica 0700. Nunca chmod no XDG do dono.
            if let Some(parent) = dir.parent() {
                fs::create_dir_all(parent)?;
            }
            match DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        let meta = fs::symlink_metadata(dir)?;
        let uid = rustix::process::geteuid().as_raw();
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o777 != 0o700 {
            return Err(NotesError::Unsafe);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&self.path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.uid() != uid || meta.nlink() != 1 {
            return Err(NotesError::Unsafe);
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 1 {
            return Err(NotesError::Newer);
        }
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA secure_delete=ON; PRAGMA temp_store=MEMORY;",
        )?;
        if create {
            conn.execute_batch("CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, text TEXT NOT NULL, language TEXT NOT NULL, words INTEGER NOT NULL); PRAGMA user_version=1;")?;
        }
        Ok(Some(conn))
    }

    pub fn save(&self, enabled: bool, text: &str, language: &str) -> Result<(), NotesError> {
        if !enabled {
            return Err(NotesError::Disabled);
        }
        if text.len() > 32 * 1024 || serde_json::to_vec(text).map_or(true, |j| j.len() > 40 * 1024)
        {
            return Err(NotesError::TooLong);
        }
        let conn = self.open(true)?.ok_or(NotesError::Unsafe)?;
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO notes(ts,text,language,words) VALUES (?1,?2,?3,?4)",
            params![ts, text, language, text.split_whitespace().count() as i64],
        )?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Note>, NotesError> {
        self.list_recent(i64::MAX)
    }

    pub fn list_recent(&self, limit: i64) -> Result<Vec<Note>, NotesError> {
        let Some(conn) = self.open(false)? else {
            return Ok(vec![]);
        };
        let mut stmt = conn.prepare(
            "SELECT id,ts,text,language,words FROM notes ORDER BY ts DESC,id DESC LIMIT ?1",
        )?;
        Ok(stmt
            .query_map([limit], |r| {
                Ok(Note {
                    id: r.get::<_, i64>(0)? as u64,
                    ts: r.get(1)?,
                    text: r.get(2)?,
                    language: r.get(3)?,
                    words: r.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<Result<_, _>>()?)
    }
    pub fn get(&self, id: u64) -> Result<Option<Note>, NotesError> {
        let Some(conn) = self.open(false)? else {
            return Ok(None);
        };
        let mut stmt = conn.prepare("SELECT id,ts,text,language,words FROM notes WHERE id=?1")?;
        Ok(stmt
            .query_map([i64::try_from(id).map_err(|_| NotesError::Unsafe)?], |r| {
                Ok(Note {
                    id: r.get::<_, i64>(0)? as u64,
                    ts: r.get(1)?,
                    text: r.get(2)?,
                    language: r.get(3)?,
                    words: r.get::<_, i64>(4)? as u64,
                })
            })?
            .next()
            .transpose()?)
    }
    pub fn delete(&self, id: u64) -> Result<usize, NotesError> {
        let Some(conn) = self.open(false)? else {
            return Ok(0);
        };
        Ok(conn.execute(
            "DELETE FROM notes WHERE id=?1",
            [i64::try_from(id).map_err(|_| NotesError::Unsafe)?],
        )?)
    }
    pub fn clear(&self) -> Result<usize, NotesError> {
        let Some(conn) = self.open(false)? else {
            return Ok(0);
        };
        let n = conn.execute("DELETE FROM notes", [])?;
        // secure_delete (ligado em `open`) zera o texto; o resto da faxina
        // é best-effort e não falha a limpeza (`history::scrub`).
        crate::history::scrub(&conn);
        Ok(n)
    }
}
