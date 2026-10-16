//! Histórico só de métricas: uma linha por ditado
//! com desfecho, durações, entrega e classe do app. **Sem texto e sem
//! título de janela**: o esquema nem tem coluna para isso.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use thiserror::Error;

use crate::pipeline::LastSummary;

/// Linhas mais velhas que isso saem ao abrir.
pub const RETENTION: Duration = Duration::from_secs(90 * 24 * 3600);

/// Migrações em ordem; `PRAGMA user_version` guarda quantas já rodaram.
/// Bancos do M5 (sem versão) já têm a tabela da v1: o `IF NOT EXISTS` os
/// adota sem perder linhas.
const MIGRATIONS: &[&str] = &[
    // v1 (M5): métricas por ditado, sem texto.
    "CREATE TABLE IF NOT EXISTS dictations (
        id INTEGER PRIMARY KEY,
        ts INTEGER NOT NULL,
        outcome TEXT NOT NULL,
        language TEXT,
        audio_ms INTEGER,
        speech_ms INTEGER,
        capture_start_ms INTEGER,
        asr_ms INTEGER,
        llm TEXT,
        llm_ms INTEGER,
        e2e_ms INTEGER,
        words INTEGER,
        delivery TEXT,
        delivery_reason TEXT,
        app TEXT
    );
    CREATE INDEX IF NOT EXISTS dictations_ts ON dictations(ts);",
    // v2 (M7): carga a frio e perfil de VRAM.
    "ALTER TABLE dictations ADD COLUMN load_ms INTEGER;
     ALTER TABLE dictations ADD COLUMN llm_load_ms INTEGER;
     ALTER TABLE dictations ADD COLUMN profile TEXT;",
];

/// Versão do esquema que este binário escreve.
pub const SCHEMA_VERSION: usize = MIGRATIONS.len();

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("histórico: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("histórico: {0}")]
    Io(#[from] std::io::Error),
    #[error("histórico com esquema v{0}, mais novo que este binário (v{SCHEMA_VERSION})")]
    Newer(usize),
    #[error(
        "histórico com esquema v{0}; inicie o daemon atualizado para migrar para v{SCHEMA_VERSION}"
    )]
    NeedsMigration(usize),
}

pub struct History {
    conn: Connection,
}

/// `$XDG_STATE_HOME/fastwisper/history.sqlite` (ou `~/.local/state/...`).
pub fn default_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".local/state"),
    };
    Some(base.join("fastwisper").join("history.sqlite"))
}

/// Limpa todas as métricas sem remover o arquivo: conexões do daemon em WAL
/// continuam no mesmo banco e os próximos ditados podem ser registrados.
/// Escritas anteriores à transação são apagadas; escritas posteriores ficam.
/// Sem banco, não cria um arquivo só para limpar. Depois, [`scrub`] tira os
/// bytes apagados (classe de app) do arquivo e do `-wal`.
pub fn clear(path: &Path) -> Result<usize, HistoryError> {
    if !path.exists() {
        return Ok(0);
    }
    let mut history = History::open(path)?;
    let tx = history
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let deleted = tx.execute("DELETE FROM dictations", [])?;
    tx.commit()?;
    scrub(&history.conn);
    Ok(deleted)
}

/// Depois de apagar a pedido do usuário (histórico, notas): com
/// `secure_delete` já ligado na conexão, o conteúdo apagado foi zerado nas
/// páginas; o checkpoint `TRUNCATE` leva as páginas zeradas para o arquivo
/// e esvazia o `-wal` (onde ficavam as versões antigas), e o `VACUUM`
/// reescreve o banco sem páginas livres. Best-effort: um leitor ocupado
/// (o daemon gravando) só adia a faxina; a limpeza em si já valeu. Limites
/// (SSD, backups, snapshots): cópias fora deste arquivo não são alcançadas.
pub(crate) fn scrub(conn: &Connection) -> bool {
    // `wal_checkpoint` devolve uma linha (busy, log, checkpointed); busy = 0
    // é checkpoint completo. Sem log: a CLI também passa por aqui.
    let checkpoint = || {
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            row.get::<_, i64>(0)
        })
        .is_ok_and(|busy| busy == 0)
    };
    let first = checkpoint();
    let vacuum = conn.execute_batch("VACUUM").is_ok();
    checkpoint() && first && vacuum
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn opt(v: Option<u64>) -> Option<i64> {
    v.map(|v| v as i64)
}

/// Roda as migrações que faltam, cada uma numa transação com a versão.
fn migrate(conn: &mut Connection) -> Result<(), HistoryError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let current = usize::try_from(current).unwrap_or(usize::MAX);
    if current > SCHEMA_VERSION {
        return Err(HistoryError::Newer(current));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

impl History {
    /// Consultas do hub: não cria banco, muda permissões, migra o esquema
    /// nem apaga linhas por retenção. A manutenção continua com o daemon.
    pub fn open_read_only(path: &Path) -> Result<History, HistoryError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_secs(2))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let version = usize::try_from(version).unwrap_or(usize::MAX);
        if version > SCHEMA_VERSION {
            return Err(HistoryError::Newer(version));
        }
        if version < SCHEMA_VERSION {
            return Err(HistoryError::NeedsMigration(version));
        }
        Ok(History { conn })
    }

    pub fn open(path: &Path) -> Result<History, HistoryError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Linha apagada (retenção ou limpeza) tem o conteúdo zerado.
        conn.pragma_update(None, "secure_delete", "ON")?;
        migrate(&mut conn)?;
        conn.execute(
            "DELETE FROM dictations WHERE ts < ?1",
            params![now_secs() - RETENTION.as_secs() as i64],
        )?;
        Ok(History { conn })
    }

    /// Registra um ditado. `app` é a classe da janela de destino (sem título).
    pub fn record(&self, summary: &LastSummary, app: Option<&str>) -> Result<(), HistoryError> {
        let m = &summary.metrics;
        let delivery = summary
            .delivery
            .map(|d| serde_json::to_value(d).unwrap_or_default());
        let llm = m
            .llm
            .and_then(|l| serde_json::to_value(l).ok()?.as_str().map(str::to_owned));
        self.conn.execute(
            "INSERT INTO dictations (ts, outcome, language, audio_ms, speech_ms, capture_start_ms, asr_ms,
                llm, llm_ms, e2e_ms, words, delivery, delivery_reason, app, load_ms, llm_load_ms, profile)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                now_secs(),
                summary.outcome.as_str(),
                summary.language.map(|l| l.code()),
                m.audio_ms as i64,
                opt(m.speech_ms),
                opt(m.capture_start_ms),
                opt(m.asr_ms),
                llm,
                opt(m.llm_ms),
                opt(m.e2e_ms),
                opt(m.words),
                delivery.as_ref().and_then(|d| d["delivery"].as_str().map(str::to_owned)),
                delivery.as_ref().and_then(|d| d["reason"].as_str().map(str::to_owned)),
                app,
                opt(m.load_ms),
                opt(m.llm_load_ms),
                m.profile.map(|p| p.as_str()),
            ],
        )?;
        Ok(())
    }

    /// Métricas mínimas para o resumo do Início, limitadas à retenção.
    /// Não expõe classe do app, texto ou título de janela.
    pub fn usage_metrics(&self, days: u32) -> Result<Vec<UsageMetric>, HistoryError> {
        let now = now_secs();
        self.usage_metrics_at(days, now)
    }

    fn usage_metrics_at(&self, days: u32, now: i64) -> Result<Vec<UsageMetric>, HistoryError> {
        let window = (u64::from(days) * 86_400).min(RETENTION.as_secs());
        let mut query = self.conn.prepare(
            "SELECT ts, outcome = 'transcribed', words, speech_ms FROM dictations
             WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts",
        )?;
        Ok(query
            .query_map(params![now - window as i64, now], |row| {
                Ok(UsageMetric {
                    ts: row.get(0)?,
                    transcribed: row.get(1)?,
                    words: row.get(2)?,
                    speech_ms: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    /// Resumo dos últimos `days` dias.
    pub fn stats(&self, days: u32) -> Result<Stats, HistoryError> {
        let since = now_secs() - i64::from(days) * 86_400;
        let mut stats = Stats::default();
        let mut by_outcome = self
            .conn
            .prepare("SELECT outcome, COUNT(*) FROM dictations WHERE ts >= ?1 GROUP BY outcome ORDER BY 2 DESC")?;
        stats.outcomes = by_outcome
            .query_map(params![since], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut delivered = self.conn.prepare(
            "SELECT COALESCE(delivery || COALESCE(':' || delivery_reason, ''), '-'), COUNT(*)
             FROM dictations WHERE ts >= ?1 AND outcome = 'transcribed' GROUP BY 1 ORDER BY 2 DESC",
        )?;
        stats.deliveries = delivered
            .query_map(params![since], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        // Classe do app (nunca título) e tipo de entrega, sem o motivo.
        let mut apps = self.conn.prepare(
            "SELECT COALESCE(app, '-'), COALESCE(delivery, '-'), COUNT(*)
             FROM dictations WHERE ts >= ?1 AND outcome = 'transcribed' GROUP BY 1, 2 ORDER BY 3 DESC, 1",
        )?;
        stats.apps = apps
            .query_map(params![since], |row| {
                Ok(AppDeliveries {
                    app: row.get(0)?,
                    delivery: row.get(1)?,
                    count: row.get(2)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut profiles = self.conn.prepare(
            "SELECT COALESCE(profile, '-'), COUNT(*) FROM dictations WHERE ts >= ?1 GROUP BY 1 ORDER BY 2 DESC",
        )?;
        stats.profiles = profiles
            .query_map(params![since], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        for (column, slot) in [
            ("asr_ms", &mut stats.asr_ms),
            ("llm_ms", &mut stats.llm_ms),
            ("e2e_ms", &mut stats.e2e_ms),
            ("load_ms", &mut stats.load_ms),
            ("llm_load_ms", &mut stats.llm_load_ms),
        ] {
            let sql = format!(
                "SELECT {column} FROM dictations WHERE ts >= ?1 AND {column} IS NOT NULL ORDER BY 1"
            );
            let values: Vec<i64> = self
                .conn
                .prepare(&sql)?
                .query_map(params![since], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            *slot = Percentiles::of(&values);
        }
        Ok(stats)
    }
}

/// Uma tentativa de ditado, exclusivamente com métricas numéricas.
#[derive(Debug, Clone, Copy)]
pub struct UsageMetric {
    pub ts: i64,
    pub transcribed: bool,
    pub words: Option<i64>,
    pub speech_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Percentiles {
    pub count: usize,
    pub p50: i64,
    pub p95: i64,
    pub max: i64,
}

impl Percentiles {
    fn of(sorted: &[i64]) -> Percentiles {
        if sorted.is_empty() {
            return Percentiles::default();
        }
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        Percentiles {
            count: sorted.len(),
            p50: at(0.5),
            p95: at(0.95),
            max: sorted[sorted.len() - 1],
        }
    }
}

/// Ditados transcritos para uma classe de app com um tipo de entrega
/// (`pasted`, `copied`, `failed` ou `-`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDeliveries {
    /// Classe da janela de destino (`-` = sem destino); nunca o título.
    pub app: String,
    pub delivery: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub outcomes: Vec<(String, i64)>,
    pub deliveries: Vec<(String, i64)>,
    /// Entregas por app, da mais frequente para a menos.
    pub apps: Vec<AppDeliveries>,
    /// Ditados por perfil de VRAM (`-` = antes do M7).
    pub profiles: Vec<(String, i64)>,
    pub asr_ms: Percentiles,
    pub llm_ms: Percentiles,
    pub e2e_ms: Percentiles,
    /// Carga a frio do ASR / do LLM (perfis sob demanda).
    pub load_ms: Percentiles,
    pub llm_load_ms: Percentiles,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::Language;
    use crate::inject::{CopyReason, Delivery};
    use crate::pipeline::{Metrics, Outcome};

    fn transcribed(asr: u64, e2e: u64, delivery: Delivery) -> LastSummary {
        let mut s = Outcome::Transcribed {
            text: "conteúdo-secreto-ditado".into(),
            language: Language::Pt,
            submit: false,
            metrics: Metrics {
                audio_ms: 3000,
                asr_ms: Some(asr),
                e2e_ms: Some(e2e),
                words: Some(5),
                ..Metrics::default()
            },
        }
        .summary();
        s.delivery = Some(delivery);
        s
    }

    #[test]
    fn records_metrics_and_never_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("h.sqlite");
        let h = History::open(&path).expect("open");
        for i in 0..10 {
            h.record(
                &transcribed(100 + i * 10, 300 + i * 10, Delivery::Pasted),
                Some("example-browser"),
            )
            .expect("record");
        }
        h.record(
            &transcribed(150, 400, Delivery::Copied(CopyReason::FocusChanged)),
            Some("ghostty"),
        )
        .expect("record");
        h.record(&Outcome::NoSpeech(Metrics::default()).summary(), None)
            .expect("record");

        let stats = h.stats(7).expect("stats");
        assert_eq!(
            stats.outcomes,
            [("transcribed".to_owned(), 11), ("no_speech".to_owned(), 1)]
        );
        assert_eq!(stats.deliveries[0], ("pasted".to_owned(), 10));
        assert_eq!(stats.deliveries[1], ("copied:focus_changed".to_owned(), 1));
        assert_eq!(stats.asr_ms.count, 11);
        assert_eq!(stats.e2e_ms.max, 400);
        let app = |app: &str, delivery: &str, count| AppDeliveries {
            app: app.into(),
            delivery: delivery.into(),
            count,
        };
        assert_eq!(
            stats.apps,
            [
                app("example-browser", "pasted", 10),
                app("ghostty", "copied", 1)
            ],
            "só transcritos, agrupados por classe e entrega"
        );
        drop(h);

        // O arquivo inteiro não contém o texto ditado.
        let mut raw = std::fs::read(&path).expect("read");
        if let Ok(wal) = std::fs::read(dir.path().join("h.sqlite-wal")) {
            raw.extend(wal);
        }
        let needle = "conteúdo-secreto-ditado".as_bytes();
        assert!(
            !raw.windows(needle.len()).any(|w| w == needle),
            "texto vazou para o histórico"
        );
    }

    #[test]
    fn clear_zeros_stats_and_keeps_the_daemon_connection_writable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("h.sqlite");
        let writer = History::open(&path).expect("daemon connection");
        writer
            .record(&transcribed(100, 300, Delivery::Pasted), Some("editor"))
            .expect("record");
        assert_eq!(clear(&path).expect("clear"), 1);
        assert_eq!(writer.stats(90).expect("stats"), Stats::default());
        assert_eq!(
            History::open(&path).expect("hub").stats(7).expect("stats"),
            Stats::default()
        );
        writer
            .record(&transcribed(150, 400, Delivery::Pasted), None)
            .expect("record after clear");
        assert_eq!(writer.stats(7).expect("stats").asr_ms.count, 1);
        assert_eq!(clear(&path).expect("second clear"), 1);
        assert_eq!(clear(&path).expect("idempotent"), 0);
    }

    /// Limpar não deixa a classe de app no arquivo nem no `-wal` (A-25),
    /// com a conexão do daemon ainda aberta (e gravando depois).
    #[test]
    fn clear_scrubs_deleted_bytes_from_the_file_and_the_wal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("h.sqlite");
        // Classe sintética, nunca de um app real.
        let sentinel = "fixture.sentinela-a25.app";
        let writer = History::open(&path).expect("daemon connection");
        for _ in 0..50 {
            writer
                .record(&transcribed(100, 300, Delivery::Pasted), Some(sentinel))
                .expect("record");
        }
        let contains = |file: &Path| {
            std::fs::read(file)
                .unwrap_or_default()
                .windows(sentinel.len())
                .any(|w| w == sentinel.as_bytes())
        };
        let wal = dir.path().join("h.sqlite-wal");
        assert!(contains(&path) || contains(&wal), "fixture gravada");
        assert_eq!(clear(&path).expect("clear"), 50);
        for file in std::fs::read_dir(dir.path()).expect("dir") {
            let file = file.expect("entry").path();
            assert!(!contains(&file), "{} ainda tem a classe", file.display());
        }
        assert_eq!(std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0), 0);
        writer
            .record(&transcribed(150, 400, Delivery::Pasted), None)
            .expect("record after clear");
        assert_eq!(writer.stats(7).expect("stats").asr_ms.count, 1);
    }

    #[test]
    fn clear_waits_for_an_in_flight_writer_transaction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("h.sqlite");
        let writer = History::open(&path).expect("daemon");
        writer
            .conn
            .execute_batch("BEGIN IMMEDIATE")
            .expect("begin writer");
        writer
            .record(&transcribed(100, 300, Delivery::Pasted), None)
            .expect("in flight record");
        let (started, ready) = std::sync::mpsc::channel();
        let (completed, result) = std::sync::mpsc::channel();
        let cleaner = std::thread::spawn(move || {
            started.send(()).expect("ready");
            completed.send(clear(&path)).expect("clear result");
        });
        ready.recv().expect("cleaner started");
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        writer.conn.execute_batch("COMMIT").expect("commit writer");
        assert_eq!(
            result
                .recv_timeout(Duration::from_secs(3))
                .expect("completed")
                .expect("clear"),
            1
        );
        cleaner.join().expect("cleaner");
        assert_eq!(writer.stats(7).expect("stats"), Stats::default());
    }

    #[test]
    fn clear_absent_history_does_not_create_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("absent.sqlite");
        assert_eq!(clear(&path).expect("clear absent"), 0);
        assert!(!path.exists());
    }

    #[test]
    fn read_only_queries_do_not_prune_chmod_or_block_a_writer() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("h.sqlite");
        let writer = History::open(&path).expect("daemon");
        writer
            .record(&transcribed(100, 300, Delivery::Pasted), None)
            .expect("record");
        writer
            .conn
            .execute("UPDATE dictations SET ts = 1", [])
            .expect("old row");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o750)).expect("mode");
        writer
            .conn
            .execute_batch("BEGIN IMMEDIATE")
            .expect("write lock");
        writer
            .record(&transcribed(150, 400, Delivery::Pasted), None)
            .expect("uncommitted");
        let reader = History::open_read_only(&path).expect("read with active writer");
        assert_eq!(reader.stats(90).expect("stats"), Stats::default());
        assert!(reader.usage_metrics(90).expect("usage").is_empty());
        assert_eq!(
            std::fs::metadata(dir.path())
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
        assert!(
            reader
                .record(&transcribed(1, 1, Delivery::Pasted), None)
                .is_err()
        );
        writer.conn.execute_batch("COMMIT").expect("commit");
        assert_eq!(reader.stats(90).expect("updated stats").asr_ms.count, 1);
        assert_eq!(reader.usage_metrics(90).expect("updated usage").len(), 1);
        assert_eq!(
            writer
                .conn
                .query_row("SELECT COUNT(*) FROM dictations", [], |row| row
                    .get::<_, i64>(0))
                .expect("retained rows"),
            2
        );
    }

    #[test]
    fn read_only_does_not_create_or_migrate_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.sqlite");
        assert!(History::open_read_only(&missing).is_err());
        assert!(!missing.exists());
        let path = dir.path().join("legacy.sqlite");
        let conn = Connection::open(&path).expect("legacy database");
        conn.execute_batch(MIGRATIONS[0]).expect("v1 schema");
        conn.pragma_update(None, "user_version", 1).expect("v1");
        assert!(matches!(
            History::open_read_only(&path),
            Err(HistoryError::NeedsMigration(1))
        ));
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("unchanged version"),
            1
        );
        conn.pragma_update(None, "user_version", (SCHEMA_VERSION + 1) as i64)
            .expect("future schema");
        assert!(matches!(
            History::open_read_only(&path),
            Err(HistoryError::Newer(_))
        ));
    }

    #[test]
    fn m5_database_is_migrated_without_losing_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("h.sqlite");
        {
            // Esquema do M5, sem `user_version`.
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(MIGRATIONS[0]).expect("v1");
            conn.execute(
                "INSERT INTO dictations (ts, outcome, asr_ms) VALUES (?1, 'transcribed', 120)",
                params![now_secs()],
            )
            .expect("linha antiga");
        }
        let h = History::open(&path).expect("migra");
        let version: i64 = h
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("versão");
        assert_eq!(version, SCHEMA_VERSION as i64);
        let mut s = transcribed(100, 300, Delivery::Pasted);
        s.metrics.load_ms = Some(95);
        s.metrics.llm_load_ms = Some(1100);
        s.metrics.profile = Some(crate::vram::Profile::SobDemanda);
        h.record(&s, None).expect("record");
        let stats = h.stats(1).expect("stats");
        assert_eq!(stats.asr_ms.count, 2, "linha do M5 preservada");
        assert_eq!((stats.load_ms.count, stats.load_ms.max), (1, 95));
        assert_eq!(stats.llm_load_ms.max, 1100);
        assert!(stats.profiles.contains(&("sob_demanda".to_owned(), 1)));
        assert!(stats.profiles.contains(&("-".to_owned(), 1)));
        drop(h);
        // Reabrir não roda nada de novo.
        History::open(&path).expect("reabre");
    }

    #[test]
    fn newer_schema_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("h.sqlite");
        {
            let conn = Connection::open(&path).expect("open");
            conn.pragma_update(None, "user_version", SCHEMA_VERSION as i64 + 1)
                .expect("versão");
        }
        assert!(matches!(History::open(&path), Err(HistoryError::Newer(_))));
    }

    #[test]
    fn usage_window_is_bounded_by_retention_and_excludes_future_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h = History::open(&dir.path().join("h.sqlite")).expect("open");
        let now = now_secs();
        for ts in [
            now - RETENTION.as_secs() as i64 - 1,
            now - 86_400,
            now,
            now + 1,
        ] {
            h.conn.execute(
                "INSERT INTO dictations (ts, outcome, words, speech_ms) VALUES (?1, 'transcribed', 20, 10000)",
                params![ts],
            ).expect("insert");
        }
        let metrics = h.usage_metrics_at(365, now).expect("metrics");
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].words, Some(20));
        assert_eq!(metrics[0].speech_ms, Some(10000));
        assert!(metrics[0].transcribed);
        assert_eq!(h.usage_metrics_at(0, now).expect("zero days").len(), 1);
    }

    #[test]
    fn percentiles() {
        let p = Percentiles::of(&[10, 20, 30, 40, 100]);
        assert_eq!((p.count, p.p50, p.max), (5, 30, 100));
        assert_eq!(Percentiles::of(&[]), Percentiles::default());
    }
}
