//! mimir-imserver: stdio JSON-RPC engine server for the macOS frontend.
//!
//! Reads newline-delimited JSON-RPC 2.0 requests from stdin and writes one
//! response per line to stdout. Logs go to stderr (`RUST_LOG` controls the
//! filter; defaults to `info`). The learning cache is saved on EOF, so the
//! frontend should close the child's stdin (or send `save_learning`) before
//! terminating it.
//!
//! `--prefetch-models` downloads every conversion model defined in the
//! config's `[models]` table into the HuggingFace cache and exits (used by
//! `make install` to avoid a multi-minute download on first launch).
//!
//! `--profile-models` measures every `[models]` entry — top-1 accuracy and
//! time per conversion — and prints one JSON line per model (used by the
//! settings window; a separate process so the IME never stalls on it).

use std::io::{BufRead, Write};

use anyhow::Context;
use mimir_im::config::Settings;
use mimir_im::server::ImServer;

/// Warm the HF cache for every `[models]` entry; local-path entries are
/// only checked for existence. Stops at the first failure: the next one
/// would only repeat the cause after another network timeout.
fn prefetch_models() -> anyhow::Result<()> {
    let settings = Settings::load()?;
    for key in settings.models.keys() {
        let (gguf, tokenizer) = settings
            .model_source(key)?
            .resolve()
            .with_context(|| format!("model '{key}'"))?;
        tracing::info!(
            "Model '{}' ready: {} (tokenizer: {})",
            key,
            gguf.display(),
            tokenizer.display()
        );
    }
    Ok(())
}

/// A conversion to measure: left context, reading, the expected surface.
type Sample = (String, String, String);

/// Everyday sentences, used when the learning history is too small.
const BUILTIN_SAMPLES: &[(&str, &str, &str)] = &[
    ("", "きょうはいいてんきです", "今日はいい天気です"),
    ("", "かいぎのしりょうをじゅんびする", "会議の資料を準備する"),
    (
        "",
        "らいしゅうのよていをかくにんしてください",
        "来週の予定を確認してください",
    ),
    ("", "でんしゃがおくれています", "電車が遅れています"),
    ("", "このほんはとてもおもしろい", "この本はとても面白い"),
    ("", "しゅくだいをわすれました", "宿題を忘れました"),
    ("", "えきまでむかえにいきます", "駅まで迎えに行きます"),
    ("", "かぜをひいてねつがある", "風邪をひいて熱がある"),
    ("", "あたらしいぱそこんをかった", "新しいパソコンを買った"),
    ("", "しけんのけっかがでた", "試験の結果が出た"),
    ("", "りょこうのけいかくをたてる", "旅行の計画を立てる"),
    ("", "へやのそうじをする", "部屋の掃除をする"),
    ("", "めーるをへんしんしました", "メールを返信しました"),
    ("", "ばぐをしゅうせいする", "バグを修正する"),
    ("", "せっていをへんこうする", "設定を変更する"),
    ("入力を", "かくていする", "確定する"),
    ("ファイルを", "ほぞんする", "保存する"),
    ("", "にほんごをにゅうりょくする", "日本語を入力する"),
    ("", "としょかんでべんきょうする", "図書館で勉強する"),
    ("", "あさごはんをたべた", "朝ご飯を食べた"),
];

/// Up to this many recent learned conversions make the sample set.
const MAX_LEARNED_SAMPLES: usize = 50;
/// Fewer learned conversions than this: use the built-in sentences.
const MIN_LEARNED_SAMPLES: usize = 10;

/// The user's own recent conversions (reading → what they chose, with the
/// context it was chosen in) when there are enough, else the built-ins.
fn profile_samples() -> (Vec<Sample>, &'static str) {
    let learned: Vec<Sample> = Settings::learning_file()
        .and_then(|path| mimir_engine::LearningCache::load(&path, Default::default()).ok())
        .map(|cache| {
            let mut entries: Vec<_> = cache
                .entries()
                .into_iter()
                .filter(|(reading, e)| *reading != e.surface)
                .map(|(reading, e)| {
                    let ctx = e.contexts.first().cloned().unwrap_or_default();
                    (e.last_access, (ctx, reading.to_string(), e.surface.clone()))
                })
                .collect();
            entries.sort_by_key(|e| std::cmp::Reverse(e.0));
            entries
                .into_iter()
                .take(MAX_LEARNED_SAMPLES)
                .map(|(_, s)| s)
                .collect()
        })
        .unwrap_or_default();
    if learned.len() >= MIN_LEARNED_SAMPLES {
        return (learned, "learning");
    }
    let builtin = BUILTIN_SAMPLES
        .iter()
        .map(|(c, r, e)| (c.to_string(), r.to_string(), e.to_string()))
        .collect();
    (builtin, "builtin")
}

fn profile_model(
    settings: &Settings,
    key: &str,
    samples: &[Sample],
) -> anyhow::Result<serde_json::Value> {
    let start = std::time::Instant::now();
    let source = settings.model_source(key)?;
    let mut converter = mimir_engine::KanaKanjiConverter::from_source(&source, key)?;
    if settings.conversion.n_threads > 0 {
        converter.set_n_threads(settings.conversion.n_threads);
    }
    let load_ms = start.elapsed().as_millis();
    // Warm up: the first inference pays one-off setup.
    let _ = converter.convert("てすと", "", 1);
    let mut correct = 0;
    let start = std::time::Instant::now();
    for (ctx, reading, expected) in samples {
        let got = converter.convert(reading, ctx, 1)?;
        if got.first().map(|g| mimir_engine::kana::normalize_nfkc(g))
            == Some(mimir_engine::kana::normalize_nfkc(expected))
        {
            correct += 1;
        }
    }
    let avg_ms = start.elapsed().as_secs_f64() * 1000.0 / samples.len().max(1) as f64;
    Ok(serde_json::json!({
        "model": key,
        "n": samples.len(),
        "accuracy": correct as f64 / samples.len().max(1) as f64,
        "avg_ms": avg_ms,
        "load_ms": load_ms,
    }))
}

/// Measure every `[models]` entry on the same samples, one JSON line each.
fn profile_models() {
    let settings = Settings::load_or_default();
    let (samples, source) = profile_samples();
    for key in settings.models.keys() {
        let mut line = profile_model(&settings, key, &samples)
            .unwrap_or_else(|e| serde_json::json!({ "model": key, "error": format!("{e:#}") }));
        line["samples"] = source.into();
        println!("{line}");
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    if std::env::args().any(|arg| arg == "--profile-models") {
        profile_models();
        return;
    }

    if std::env::args().any(|arg| arg == "--prefetch-models") {
        if let Err(e) = prefetch_models() {
            tracing::error!("model prefetch failed: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    let mut server = ImServer::new();
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();

    tracing::info!("mimir-imserver started (pid={})", std::process::id());

    for line in stdin.lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                tracing::error!("stdin read error: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_line(&line)
            && writeln!(stdout, "{response}")
                .and_then(|_| stdout.flush())
                .is_err()
        {
            // stdout closed: frontend is gone
            break;
        }
    }

    tracing::info!("stdin closed, saving learning cache and exiting");
    server.save_learning();
}
