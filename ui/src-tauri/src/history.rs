//! Histórico: o mesmo resumo do `fastwisper history` (contagens, p50/p95,
//! entregas por app), só métricas. O banco nem tem coluna para texto.
use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use fw_core::history::{self, History, Percentiles, Stats, UsageMetric};
use serde::Serialize;

/// Janelas oferecidas na tela; a retenção do histórico é de 90 dias.
const DAYS: [u32; 3] = [7, 30, 90];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Count {
    name: String,
    count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Latency {
    count: usize,
    p50: i64,
    p95: i64,
    max: i64,
}

/// Entregas de uma classe de app (nunca o título da janela).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AppRow {
    app: String,
    pasted: i64,
    copied: i64,
    failed: i64,
    total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryView {
    days: u32,
    /// Ainda não há banco: nenhum ditado registrado.
    empty: bool,
    total: i64,
    consecutive_weeks: usize,
    words: u64,
    ppm: u64,
    outcomes: Vec<Count>,
    deliveries: Vec<Count>,
    profiles: Vec<Count>,
    apps: Vec<AppRow>,
    asr_ms: Latency,
    llm_ms: Latency,
    e2e_ms: Latency,
    load_ms: Latency,
    llm_load_ms: Latency,
}

#[derive(Debug, Default, PartialEq)]
struct Usage {
    consecutive_weeks: usize,
    words: u64,
    ppm: u64,
}

/// Semanas de calendário (segunda a domingo) no fuso do usuário. Só ditados
/// transcritos contam para a sequência: `too_short`, `no_speech`, falhas e
/// cancelados não. A semana atual ainda vazia não quebra a sequência da
/// semana anterior. Converter cada timestamp antes de agrupar também respeita
/// mudanças de horário.
fn usage<T: TimeZone>(metrics: &[UsageMetric], now: DateTime<T>) -> Usage {
    let week = |date: chrono::NaiveDate| {
        (date.num_days_from_ce() - date.weekday().num_days_from_monday() as i32).div_euclid(7)
    };
    let current = week(now.date_naive());
    let mut weeks = BTreeSet::new();
    let mut result = Usage::default();
    let mut speech_words = 0u64;
    let mut speech_ms = 0u64;
    for metric in metrics {
        if metric.transcribed
            && let Some(date) = DateTime::<Utc>::from_timestamp(metric.ts, 0)
        {
            weeks.insert(week(date.with_timezone(&now.timezone()).date_naive()));
        }
        let words = metric.words.and_then(|n| u64::try_from(n).ok());
        result.words = result.words.saturating_add(words.unwrap_or_default());
        if metric.transcribed
            && let (Some(words), Some(ms)) = (words, metric.speech_ms)
            && ms > 0
        {
            speech_words = speech_words.saturating_add(words);
            speech_ms = speech_ms.saturating_add(ms as u64);
        }
    }
    let mut cursor = if weeks.contains(&current) {
        current
    } else {
        current - 1
    };
    while weeks.contains(&cursor) {
        result.consecutive_weeks += 1;
        cursor -= 1;
    }
    if speech_ms > 0 {
        result.ppm = (speech_words as f64 / speech_ms as f64 * 60_000.0).round() as u64;
    }
    result
}

fn counts(rows: &[(String, i64)]) -> Vec<Count> {
    rows.iter()
        .map(|(name, count)| Count {
            name: name.clone(),
            count: *count,
        })
        .collect()
}

fn latency(p: Percentiles) -> Latency {
    Latency {
        count: p.count,
        p50: p.p50,
        p95: p.p95,
        max: p.max,
    }
}

/// Uma linha por app, com colado/copiado/falhou lado a lado.
fn app_rows(stats: &Stats) -> Vec<AppRow> {
    let mut rows: Vec<AppRow> = Vec::new();
    for entry in &stats.apps {
        let index = match rows.iter().position(|row| row.app == entry.app) {
            Some(index) => index,
            None => {
                rows.push(AppRow {
                    app: entry.app.clone(),
                    ..AppRow::default()
                });
                rows.len() - 1
            }
        };
        let row = &mut rows[index];
        match entry.delivery.as_str() {
            // "Enviar agora" também colou.
            "pasted" | "submitted" => row.pasted += entry.count,
            "copied" => row.copied += entry.count,
            _ => row.failed += entry.count,
        }
        row.total += entry.count;
    }
    rows.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.app.cmp(&b.app)));
    rows
}

fn view(days: u32, stats: Option<Stats>) -> HistoryView {
    let empty = stats.is_none();
    let stats = stats.unwrap_or_default();
    HistoryView {
        days,
        empty,
        total: stats.outcomes.iter().map(|(_, n)| n).sum(),
        consecutive_weeks: 0,
        words: 0,
        ppm: 0,
        outcomes: counts(&stats.outcomes),
        deliveries: counts(&stats.deliveries),
        profiles: counts(&stats.profiles),
        apps: app_rows(&stats),
        asr_ms: latency(stats.asr_ms),
        llm_ms: latency(stats.llm_ms),
        e2e_ms: latency(stats.e2e_ms),
        load_ms: latency(stats.load_ms),
        llm_load_ms: latency(stats.llm_load_ms),
    }
}

/// Como a CLI: sem banco, "sem histórico ainda" (e nada é criado).
fn load(path: &Path, days: u32) -> Result<HistoryView, String> {
    if !path.exists() {
        return Ok(view(days, None));
    }
    let history = History::open_read_only(path).map_err(|error| error.to_string())?;
    let stats = history.stats(days).map_err(|error| error.to_string())?;
    let metrics = history
        .usage_metrics(days)
        .map_err(|error| error.to_string())?;
    let usage = usage(&metrics, Local::now());
    let mut view = view(days, Some(stats));
    view.consecutive_weeks = usage.consecutive_weeks;
    view.words = usage.words;
    view.ppm = usage.ppm;
    Ok(view)
}

#[tauri::command]
pub async fn history_stats(days: u32) -> Result<HistoryView, String> {
    if !DAYS.contains(&days) {
        return Err("Janela inválida: use 7, 30 ou 90 dias".to_owned());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let path = history::default_path().ok_or("Sem HOME: histórico indisponível")?;
        load(&path, days)
    })
    .await
    .map_err(|_| "Falha ao ler o histórico".to_owned())?
}

#[cfg(test)]
mod tests {
    use fw_core::asr::Language;
    use fw_core::inject::{CopyReason, Delivery};
    use fw_core::pipeline::{Failure, Metrics, Outcome};

    use super::*;

    const MARKER: &str = "marcador-sintetico-historico";

    fn transcribed(asr: u64, delivery: Delivery) -> fw_core::pipeline::LastSummary {
        let mut summary = Outcome::Transcribed {
            text: MARKER.into(),
            language: Language::Pt,
            submit: false,
            metrics: Metrics {
                audio_ms: 2000,
                words: Some(7),
                speech_ms: Some(3500),
                asr_ms: Some(asr),
                e2e_ms: Some(asr + 100),
                ..Metrics::default()
            },
        }
        .summary();
        summary.delivery = Some(delivery);
        summary
    }

    #[test]
    fn consecutive_calendar_weeks_stop_at_gaps() {
        let now = Utc
            .with_ymd_and_hms(2026, 10, 13, 12, 0, 0)
            .single()
            .expect("date");
        let metric = |days| UsageMetric {
            ts: (now - chrono::Duration::days(days)).timestamp(),
            transcribed: true,
            words: Some(10),
            speech_ms: Some(5000),
        };
        assert_eq!(
            usage(&[metric(0), metric(7), metric(7), metric(21)], now).consecutive_weeks,
            2
        );
        assert_eq!(usage(&[metric(7), metric(14)], now).consecutive_weeks, 2);
        assert_eq!(usage(&[metric(14)], now).consecutive_weeks, 0);
        assert_eq!(usage(&[], now), Usage::default());
    }

    #[test]
    fn only_transcribed_dictations_count_for_the_week_streak() {
        let now = Utc
            .with_ymd_and_hms(2026, 10, 13, 12, 0, 0)
            .single()
            .expect("date");
        let metric = |days, transcribed| UsageMetric {
            ts: (now - chrono::Duration::days(days)).timestamp(),
            transcribed,
            words: None,
            speech_ms: None,
        };
        // Semana atual só com tentativas (too_short, no_speech, falha,
        // cancelado): não conta, mas não quebra a sequência anterior.
        let mixed = [
            metric(0, false),
            metric(0, false),
            metric(7, true),
            metric(7, false),
            metric(14, false),
            metric(21, true),
        ];
        assert_eq!(usage(&mixed, now).consecutive_weeks, 1);
        assert_eq!(
            usage(&[metric(0, false), metric(7, false)], now).consecutive_weeks,
            0
        );
        assert_eq!(
            usage(&[metric(0, true), metric(7, false), metric(7, true)], now).consecutive_weeks,
            2
        );
    }

    #[test]
    fn ppm_uses_summed_transcribed_speech_and_handles_missing_metrics() {
        let now = Utc::now();
        let metric = |transcribed, words, speech_ms| UsageMetric {
            ts: now.timestamp(),
            transcribed,
            words,
            speech_ms,
        };
        let result = usage(
            &[
                metric(true, Some(30), Some(10000)),
                metric(true, Some(30), Some(20000)),
                metric(false, Some(100), Some(1000)),
                metric(true, Some(10), Some(0)),
                metric(true, Some(10), None),
                metric(true, None, Some(10000)),
                metric(true, Some(-1), Some(-1)),
            ],
            now,
        );
        assert_eq!(result.words, 180);
        assert_eq!(result.ppm, 120, "razão das somas, não média dos PPM");
        assert_eq!(usage(&[metric(false, Some(100), Some(1000))], now).ppm, 0);
        assert_eq!(usage(&[metric(true, Some(10), Some(0))], now).ppm, 0);
    }

    #[test]
    fn weeks_follow_local_dates_at_monday_boundary_and_new_year() {
        let now = Utc
            .with_ymd_and_hms(2026, 1, 5, 0, 30, 0)
            .single()
            .expect("date");
        let metrics = [
            UsageMetric {
                ts: now.timestamp(),
                transcribed: true,
                words: None,
                speech_ms: None,
            },
            UsageMetric {
                ts: (now - chrono::Duration::days(7)).timestamp(),
                transcribed: true,
                words: None,
                speech_ms: None,
            },
        ];
        let west = chrono::FixedOffset::west_opt(3 * 3600).expect("offset");
        let east = chrono::FixedOffset::east_opt(2 * 3600).expect("offset");
        assert_eq!(
            usage(&metrics, now.with_timezone(&west)).consecutive_weeks,
            2
        );
        assert_eq!(
            usage(
                &metrics[..1],
                (now + chrono::Duration::days(15)).with_timezone(&east)
            )
            .consecutive_weeks,
            0
        );
        // Domingo UTC já é segunda no fuso leste: uma semana vazia real
        // interrompe a sequência, mas no oeste ainda é a semana anterior.
        let sunday = Utc
            .with_ymd_and_hms(2026, 1, 18, 23, 30, 0)
            .single()
            .expect("date");
        let boundary_metrics: Vec<_> = metrics
            .iter()
            .map(|metric| UsageMetric {
                ts: metric.ts + 12 * 3600,
                ..*metric
            })
            .collect();
        assert_eq!(
            usage(&boundary_metrics, sunday.with_timezone(&east)).consecutive_weeks,
            0
        );
        assert_eq!(
            usage(&boundary_metrics, sunday.with_timezone(&west)).consecutive_weeks,
            2
        );
    }

    #[test]
    fn missing_database_is_empty_and_not_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fastwisper/history.sqlite");
        let view = load(&path, 7).expect("vazio");
        assert!(view.empty);
        assert_eq!(view.total, 0);
        assert_eq!((view.consecutive_weeks, view.words, view.ppm), (0, 0, 0));
        assert!(!path.exists());
    }

    #[test]
    fn counts_latencies_and_deliveries_per_app_without_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.sqlite");
        let h = History::open(&path).expect("open");
        for asr in [100, 200, 300] {
            h.record(&transcribed(asr, Delivery::Pasted), Some("example-browser"))
                .expect("record");
        }
        h.record(
            &transcribed(150, Delivery::Copied(CopyReason::FocusChanged)),
            Some("example-browser"),
        )
        .expect("record");
        h.record(&transcribed(120, Delivery::Pasted), Some("ghostty"))
            .expect("record");
        h.record(
            &Outcome::Failed(Failure::MicSilent, Metrics::default()).summary(),
            None,
        )
        .expect("record");
        drop(h);

        let view = load(&path, 7).expect("stats");
        assert!(!view.empty);
        assert_eq!(view.total, 6);
        assert_eq!((view.consecutive_weeks, view.words, view.ppm), (1, 35, 120));
        assert_eq!(view.outcomes[0].name, "transcribed");
        assert_eq!(view.outcomes[0].count, 5);
        assert_eq!(
            (view.asr_ms.count, view.asr_ms.p50, view.asr_ms.max),
            (5, 150, 300)
        );
        assert_eq!(
            view.apps,
            [
                AppRow {
                    app: "example-browser".into(),
                    pasted: 3,
                    copied: 1,
                    failed: 0,
                    total: 4,
                },
                AppRow {
                    app: "ghostty".into(),
                    pasted: 1,
                    total: 1,
                    ..AppRow::default()
                },
            ]
        );
        let json = serde_json::to_string(&view).expect("json");
        assert!(!json.contains(MARKER));
    }
}
