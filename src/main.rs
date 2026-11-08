//! spark-project-status — Generate a project status report from NextCloud Deck.
//!
//! Fetches board summaries from NextCloud Deck, sends them to an LLM API
//! (Spark/OpenAI-compatible), then either uploads the report to NextCloud
//! WebDAV or prints it to stdout (--dry-run).

mod config;
mod deck;
mod interactive;
mod llm;
mod webdav;

use anyhow::{Context, Result, bail};
use chrono::{Local, Offset};
use clap::Parser;
use config::{env_value, parse_env_file, Cli, Config, Schedule};
use dialoguer::Confirm;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use crate::deck::get_board_summary;
use crate::llm::{completion_api_json, send_completion};
use crate::webdav::{
    copy_file, fetch_file, nc_has_file, put_file, put_file_body, webdav_base_url,
};

#[tokio::main]
async fn main() -> Result<()> {
    // ── Show effective config and exit ───────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--show-config") {
        // Parse args excluding --show-config so Config::load works normally
        let cli = Cli::parse_from(
            args.iter()
                .filter(|a| *a != "--show-config")
                .cloned()
                .collect::<Vec<_>>()
        );
        let cfg_path = cli.config.clone().unwrap_or_else(|| PathBuf::from("./.env"));
        let env_pairs = if cfg_path.exists() {
            parse_env_file(&cfg_path).unwrap_or_default()
        } else {
            Vec::new()
        };

        println!("Effective configuration:");
        println!("────────────────────────");
        print_effective_value("NC_URL", cli.nc_url.as_ref(), env_value(&env_pairs, "NC_URL"));
        print_effective_value("NC_APP_USER", cli.nc_user.as_ref(), env_value(&env_pairs, "NC_APP_USER"));
        print_masked_value("NC_APP_PWD", cli.nc_password.as_ref(), env_value(&env_pairs, "NC_APP_PWD"));
        print_effective_value("NCDECK_BOARD_ID", cli.board_id.map(|v| v.to_string()).as_ref(), env_value(&env_pairs, "NCDECK_BOARD_ID"));
        print_effective_value("SPARK_URL", cli.spark_url.as_ref(), env_value(&env_pairs, "SPARK_URL"));
        print_masked_value("SPARK_API_KEY", cli.api_key.as_ref(), env_value(&env_pairs, "SPARK_API_KEY"));
        print_effective_value("PROJECT_NAME", cli.project_name.as_ref(), env_value(&env_pairs, "PROJECT_NAME"));
        print_effective_value("PROJECT_FILES_PARENT_PATH", cli.project_parent.as_ref(), env_value(&env_pairs, "PROJECT_FILES_PARENT_PATH"));
        print_effective_value("REPORT_FILENAME", cli.filename.as_ref(), env_value(&env_pairs, "REPORT_FILENAME"));
        // output_dir — no env var, just CLI
        let output_dir_src = if cli.output_dir.as_ref().map_or(false, |p| *p != PathBuf::from(".")) { "cli" } else { "default" };
        let output_dir_val = cli.output_dir.as_ref().map_or(String::from("."), |p| p.to_string_lossy().to_string());
        println!("  {:30} {:12} {}", "--output-dir", output_dir_src, output_dir_val);
        // Helper to show bool flags with source
        let show_bool = |label: &str, cli_val: bool, env_set: bool| {
            let source = if cli_val { "cli" } else if env_set { "env" } else { "default" };
            let val = if cli_val || env_set { "true" } else { "false" };
            println!("  {:30} {:12} {}", label, source, val);
        };
        show_bool("--dry-run", cli.dry_run, env_value(&env_pairs, "DRY_RUN").is_some());
        show_bool("--web-theme", cli.web_theme, env_value(&env_pairs, "WEB_THEME").is_some());
        show_bool("--print", cli.print, env_value(&env_pairs, "PRINT").is_some());
        show_bool("--save-only", cli.save_only, env_value(&env_pairs, "SAVE_ONLY").is_some());
        show_bool("--copy-dav", cli.copy_dav, env_value(&env_pairs, "COPY_DAV").is_some());
        let schedule_src = if cli.schedule.is_some() { "cli" } else if env_value(&env_pairs, "SPARK_SCHEDULE").is_some() { "env" } else { "default" };
        let schedule_val = cli.schedule.as_deref().unwrap_or(&"(not set)");
        println!("  {:30} {:12} {:?}", "--schedule", schedule_src, schedule_val);
        let sw_src = if cli.schedule_window.is_some() { "cli" } else if env_value(&env_pairs, "SCHEDULE_WINDOW").is_some() { "env" } else { "default" };
        let sw_val = cli.schedule_window.as_deref().unwrap_or(&"(not set)");
        println!("  {:30} {:12} {:?}", "--schedule-window", sw_src, sw_val);
        println!("  {:30} {:12} {}", "--verbose", if cli.verbose { "cli" } else { "default" }, cli.verbose.to_string());
        println!("  {:30} {:12} {:?}", "Config file", "", cfg_path);
        println!();
        return Ok(());
    }

    // ── Interactive setup ────────────────────────────────────────────
    if interactive::should_interact() {
        eprintln!("No .env file found. Starting interactive setup…");
        eprintln!();
        let proceed = Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Run the interactive setup wizard?")
            .default(true)
            .interact()?;

        if proceed {
            interactive::run_setup().await?;
            return Ok(());
        }

        eprintln!("Setup cancelled. Provide --config <file> or create a .env file manually.");
        std::process::exit(1);
    }

    let (config, cli) = Config::load().context("Failed to load configuration")?;

    if cli.verbose {
        config.log_verbose();
    }

    // ── Run once or loop ─────────────────────────────────────────────
    match &config.schedule {
        Schedule::Once => {
            run_report(&config, &cli).await?;
        }
        Schedule::Interval(interval) => {
            eprintln!("Running every {} … (Ctrl+C to stop)", humantime::format_duration(*interval));
            loop {
                let should_run = config.schedule_window.as_ref().map(|w| is_in_window(&w.start, &w.end)).unwrap_or(true);
                if should_run {
                    if let Err(e) = run_report(&config, &cli).await {
                        eprintln!("Error running report: {}", e);
                    }
                } else {
                    let next_start = next_window_start(&config.schedule_window.as_ref().unwrap().start);
                    let wait = (next_start - chrono::Local::now().with_timezone(&chrono::Utc)).to_std().unwrap_or(Duration::from_secs(60));
                    eprintln!("[verbose] Outside window. Sleeping {} until {}", humantime::format_duration(wait), next_start.format("%H:%M"));
                    tokio::time::sleep(wait).await;
                    continue;
                }
                tokio::time::sleep(*interval).await;
            }
        }
        Schedule::Daily(_) => {
            eprintln!("Running daily … (Ctrl+C to stop)");
            loop {
                let should_run = config.schedule_window.as_ref().map(|w| is_in_window(&w.start, &w.end)).unwrap_or(true);
                if should_run {
                    let _ = run_report(&config, &cli).await;
                } else {
                    let next_start = next_window_start(&config.schedule_window.as_ref().unwrap().start);
                    let wait = (next_start - chrono::Local::now().with_timezone(&chrono::Utc)).to_std().unwrap_or(Duration::from_secs(60));
                    eprintln!("[verbose] Outside window. Sleeping {} until {}", humantime::format_duration(wait), next_start.format("%H:%M"));
                    tokio::time::sleep(wait).await;
                    continue;
                }
                // Sleep until next local midnight
                let now_local = chrono::Local::now();
                let now_utc = now_local.with_timezone(&chrono::Utc);
                let tomorrow = now_local.date_naive() + chrono::Duration::days(1);
                let next_midnight_local = tomorrow.and_time(chrono::NaiveTime::MIN);
                // Convert local midnight to UTC: naive midnight + local offset
                let midnight_as_utc = next_midnight_local.and_utc();
                let offset_secs = now_local.offset().fix().local_minus_utc();
                let next_run_utc = midnight_as_utc + chrono::Duration::seconds(offset_secs as i64);
                let wait = (next_run_utc - now_utc).to_std().unwrap_or(Duration::from_secs(60));
                tokio::time::sleep(wait).await;
            }
        }
    }

    Ok(())
}

/// Generate a single report (used by both one-shot and scheduled modes).
async fn run_report(config: &Config, cli: &Cli) -> Result<()> {
    let now = Local::now().format("%Y-%m-%d %H:%M").to_string();

    // ── Runtime directory ──────────────────────────────────────────
    let runtime_dir = match env::var("RUNTIME_DIRECTORY") {
        Ok(dir) => {
            let path = PathBuf::from(dir);
            if cli.verbose {
                eprintln!("[verbose] Using systemd RuntimeDirectory: {:?}", path);
            }
            path
        }
        Err(_) => {
            let dir = std::env::temp_dir().join(format!(
                "spark-project-status-{}",
                chrono::Utc::now().timestamp()
            ));
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create temp dir: {:?}", dir))?;
            if cli.verbose {
                eprintln!("[verbose] Created temp RuntimeDirectory: {:?}", dir);
            }
            dir
        }
    };

    // ── Build WebDAV base URL ──────────────────────────────────────
    let project_folder = format!(
        "{}/{}",
        config.project_files_parent_path, config.project_name
    );
    let files_base = webdav_base_url(&config.nc_url, &config.nc_app_user, &project_folder);

    if cli.verbose {
        eprintln!("[verbose] WebDAV base URL: {}", files_base);
    }

    // ── Fetch web-theme (if enabled) ───────────────────────────────
    let web_theme_path = runtime_dir.join("web-theme.txt");
    if config.web_theme {
        if nc_has_file(&files_base, "web-theme.txt", &config.nc_app_user, &config.nc_app_pwd).await? {
            fetch_file(
                &files_base,
                "web-theme.txt",
                &config.nc_app_user,
                &config.nc_app_pwd,
                &web_theme_path,
            )
            .await?;
        } else {
            let default_theme =
                fs::read_to_string("web-theme-default.txt").unwrap_or_else(|_| String::new());
            fs::write(&web_theme_path, &default_theme)?;
            if !cli.dry_run {
                put_file(
                    &files_base,
                    "web-theme.txt",
                    &web_theme_path,
                    &config.nc_app_user,
                    &config.nc_app_pwd,
                )
                .await?;
            }
        }
    }

    // ── Fetch Spark Instructions ───────────────────────────────────
    let spark_instructions_path = runtime_dir.join("Spark-Instructions.md");
    if nc_has_file(
        &files_base,
        "Spark-Instructions.md",
        &config.nc_app_user,
        &config.nc_app_pwd,
    )
    .await?
    {
        fetch_file(
            &files_base,
            "Spark-Instructions.md",
            &config.nc_app_user,
            &config.nc_app_pwd,
            &spark_instructions_path,
        )
        .await?;
    } else {
        bail!(
            "No Spark Instructions Found.\n\
             Make sure to create the instructions file 'Spark-Instructions.md' \
             in the Project folder"
        );
    }

    let system_prompt = fs::read_to_string(&spark_instructions_path)
        .with_context(|| {
            format!(
                "Failed to read Spark instructions from {:?}",
                spark_instructions_path
            )
        })?;

    if cli.verbose {
        eprintln!("[verbose] Spark instructions loaded ({} chars)", system_prompt.len());
    }

    // ── Get board summary from Deck API ────────────────────────────
    let board_summary = get_board_summary(
        &config.nc_url,
        &config.nc_app_user,
        &config.nc_app_pwd,
        config.ncdeck_board_id,
        &config.cards_ignored_fields,
    )
    .await
    .context("Failed to get board summary from Deck API")?;

    if cli.dry_run {
        let card_count: usize = board_summary.0.values().map(|v| v.len()).sum();
        eprintln!("[dry-run] Deck board #{} fetched ({} cards)", config.ncdeck_board_id, card_count);
    }

    if cli.verbose {
        let json = board_summary.to_json()?;
        eprintln!("[verbose] Board summary ({} bytes):\n{}", json.len(), json);
    }

    // ── Build LLM payload and send ─────────────────────────────────
    let json_payload = completion_api_json(&system_prompt, &now, &board_summary.to_json()?);

    if cli.verbose {
        eprintln!("[verbose] LLM payload ({} bytes)", json_payload.len());
    }

    let response_json = send_completion(
        &config.spark_url,
        config.spark_api_key.as_deref(),
        &json_payload,
    )
    .await
    .context("Failed to send completion request")?;

    if cli.dry_run {
        eprintln!("[dry-run] LLM inference complete");
    }

    if cli.verbose {
        let resp_path = runtime_dir.join("response.json");
        fs::write(&resp_path, &response_json)?;
        eprintln!("[verbose] Response saved to {:?}", resp_path);
    }

    // ── Extract LLM response content ───────────────────────────────
    let report_content: String = serde_json::from_str::<serde_json::Value>(&response_json)
        .with_context(|| "Failed to parse LLM response JSON")?
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(String::from)
        .context("No message content in LLM response")?;

    // Prepend web theme to report (if not disabled)
    let full_report = if config.web_theme {
        let web_theme =
            fs::read_to_string(&web_theme_path).with_context(|| "Failed to read web-theme.txt")?;
        format!("{}\n{}", web_theme, report_content)
    } else {
        report_content
    };

    // Re-parse CLI for flag access (Config::load() consumes parse internally)
    let cli = Cli::parse();

    // ── Output ─────────────────────────────────────────────────────
    let report_name = config.filename.clone().unwrap_or_else(|| format!("{} Project Status.md", now));
    let encoded_name = urlencoding::encode(&report_name);
    let dest_url = format!("{}/{}", files_base, encoded_name);

    // Determine whether to print to stdout.
    // --dry-run and --print always print.
    // --save-only prints only when there is no other destination
    // (--output-dir, --filename, or --copy-dav would satisfy the user).
    let has_other_dest = !config.output_dir.as_os_str().is_empty()
        || config.filename.is_some()
        || config.copy_dav;
    let should_print = config.print
        || cli.dry_run
        || (config.save_only && !has_other_dest);

    if cli.dry_run {
        // ── Dry-run mode: check WebDAV state without uploading ──
        // Check parent folder exists
        if nc_has_file(&files_base, "", &config.nc_app_user, &config.nc_app_pwd).await? {
            eprintln!("[dry-run] WebDAV folder exists");
        } else {
            eprintln!("[dry-run] WebDAV folder does not exist — upload would fail");
        }

        // Check if target file already exists (would be overwritten)
        if nc_has_file(&files_base, &report_name, &config.nc_app_user, &config.nc_app_pwd).await? {
            eprintln!("[dry-run] Destination file exists — would be overwritten: {}", dest_url);
        } else {
            eprintln!("[dry-run] Destination file does not exist yet: {}", dest_url);
        }

        // Write local copy if --output-dir specified
        if !config.output_dir.as_os_str().is_empty() {
            let out_path = config.output_dir.join(&report_name);
            fs::create_dir_all(&config.output_dir)
                .with_context(|| format!("Failed to create output dir: {:?}", config.output_dir))?;
            fs::write(&out_path, &full_report)
                .with_context(|| format!("Failed to write report to {:?}", out_path))?;
            eprintln!("[dry-run] Local copy saved to: {:?}", out_path);
        }
    } else if config.save_only {
        // ── Save-only mode: local file only, no WebDAV upload ──
        fs::create_dir_all(&config.output_dir)
            .with_context(|| format!("Failed to create output dir: {:?}", config.output_dir))?;
        let out_path = config.output_dir.join(&report_name);
        fs::write(&out_path, &full_report)
            .with_context(|| format!("Failed to write report to {:?}", out_path))?;
        eprintln!("Report saved locally to: {:?}", out_path);
    } else {
        // ── Production mode: upload to WebDAV ──
        put_file_body(
            &files_base,
            &encoded_name,
            &full_report,
            &config.nc_app_user,
            &config.nc_app_pwd,
        )
        .await
        .with_context(|| format!("Failed to upload report {}", report_name))?;

        eprintln!("Report uploaded to: {}", dest_url);

        // ── Copy to timestamped name if requested ──
        if config.copy_dav {
            let timestamped_name = format!("{} Project Status.md", now);
            let timestamped_encoded = urlencoding::encode(&timestamped_name);
            copy_file(
                &files_base,
                &encoded_name,
                &timestamped_name,
                &config.nc_app_user,
                &config.nc_app_pwd,
            )
            .await
            .with_context(|| format!("Failed to copy {} → {}", report_name, timestamped_name))?;
            let timestamped_dest = format!("{}/{}", files_base, timestamped_encoded);
            eprintln!("Report also copied to: {}", timestamped_dest);
        }
    }
    if should_print {
        println!("{}", full_report);
    }

    // ── Print stats ────────────────────────────────────────────────
    print_stats(&response_json);

    Ok(())
}

/// Print usage/timing stats from the LLM response JSON.
fn print_stats(response_json: &str) {
    let value: serde_json::Value = match serde_json::from_str(response_json) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to parse response for stats: {}", e);
            return;
        }
    };

    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    static EMPTY_TIMINGS: LazyLock<serde_json::Value> = LazyLock::new(|| serde_json::json!({}));
    let timings = value.get("timings").unwrap_or(&EMPTY_TIMINGS);
    let prompt_ms = timings.get("prompt_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let predicted_ms = timings.get("predicted_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let prompt_per_second = timings
        .get("prompt_per_second")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let predicted_per_second = timings
        .get("predicted_per_second")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let prompt_duration = round1(prompt_ms / 1000.0);
    let gen_duration = round1(predicted_ms / 1000.0);
    let prompt_tps = round1(prompt_per_second);
    let gen_tps = round1(predicted_per_second);

    println!(
        "Model: {}, Prompt Duration: {}s, Prompt TPS: {}, Generation Duration: {}s, Generation TPS: {}",
        model, prompt_duration, prompt_tps, gen_duration, gen_tps
    );

    // Print usage if available
    if let Some(usage) = value.get("usage") {
        println!(
            "Usage: prompt={}, completion={}, total={}",
            usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        );
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Check if the current local time is inside the given window.
fn is_in_window(start: &chrono::NaiveTime, end: &chrono::NaiveTime) -> bool {
    let now = Local::now().time();
    if end > start {
        // Normal window: e.g. 08:00-19:00
        now >= *start && now < *end
    } else {
        // Overnight window: e.g. 22:00-06:00
        now >= *start || now < *end
    }
}

/// Find the next local time the window starts, returned as UTC.
fn next_window_start(start: &chrono::NaiveTime) -> chrono::DateTime<chrono::Utc> {
    let now_local = chrono::Local::now();
    let now_utc = now_local.with_timezone(&chrono::Utc);
    let offset_seconds = now_local.offset().fix().local_minus_utc();
    let today_start_naive = now_local.date_naive().and_time(*start);
    let today_start_utc = today_start_naive.and_utc() + chrono::Duration::seconds(offset_seconds as i64);
    if today_start_utc > now_utc {
        today_start_utc
    } else {
        let tomorrow = now_local.date_naive() + chrono::Duration::days(1);
        let tomorrow_start_naive = tomorrow.and_time(*start);
        tomorrow_start_naive.and_utc() + chrono::Duration::seconds(offset_seconds as i64)
    }
}

/// Print an effective config value with source: CLI arg > env file, shown as `(not set)` if missing.
fn print_effective_value(key: &str, cli_val: Option<&String>, env_val: Option<&str>) {
    let (display, source) = match (cli_val, env_val) {
        (Some(v), _) if !v.is_empty() => (v.as_str(), "cli"),
        (_, Some(v)) if !v.is_empty() => (v, "env"),
        _ => ("(not set)", "default"),
    };
    eprintln!("  {:30} {:12} {}", key, source, display);
}

/// Print a fully obscured password value with source.
fn print_masked_value(key: &str, cli_val: Option<&String>, env_val: Option<&str>) {
    let source = match (cli_val, env_val) {
        (Some(v), _) if !v.is_empty() => "cli",
        (_, Some(v)) if !v.is_empty() => "env",
        _ => "default",
    };
    if source == "default" {
        eprintln!("  {:30} {:12} {}", key, source, "(not set)");
    } else {
        eprintln!("  {:30} {:12} {}", key, source, "********");
    }
}
