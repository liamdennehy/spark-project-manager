//! Configuration loading: env file parsing + CLI argument overrides.

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// spark-project-status — Generate a project status report from NextCloud Deck.
#[derive(Parser, Debug)]
#[command(name = "spark-project-status")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Path to the env configuration file (default: ./.env)
    #[arg(short, long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// NextCloud URL (overrides env file)
    #[arg(long = "nc-url", value_name = "URL")]
    pub nc_url: Option<String>,

    /// NextCloud app-user token (overrides env file)
    #[arg(long = "nc-user", value_name = "TOKEN")]
    pub nc_user: Option<String>,

    /// NextCloud app-password/token (overrides env file)
    #[arg(long = "nc-password", value_name = "TOKEN")]
    pub nc_password: Option<String>,

    /// Deck board ID to query (overrides env file)
    #[arg(long = "board-id", value_name = "ID")]
    pub board_id: Option<u64>,

    /// Spark / OpenAI-compatible API base URL (overrides env file)
    #[arg(long = "spark-url", value_name = "URL")]
    pub spark_url: Option<String>,

    /// API key for the LLM server (sent as Authorization Bearer header)
    #[arg(long = "api-key", value_name = "KEY")]
    pub api_key: Option<String>,

    /// Project name used in report filename (overrides env file)
    #[arg(long = "project-name", value_name = "NAME")]
    pub project_name: Option<String>,

    /// Project files parent path in NextCloud (overrides env file)
    #[arg(long = "project-parent", value_name = "PATH")]
    pub project_parent: Option<String>,

    /// Do not upload the report to NextCloud; print to stdout instead
    #[arg(long)]
    pub dry_run: bool,

    /// Output directory for local file saving (default: current directory)
    #[arg(long = "output-dir", value_name = "DIR")]
    pub output_dir: Option<PathBuf>,

    /// Run continuously: `once`, `interval::<duration>` (e.g. interval::1h),
    /// or `daily::<HH:MM>` (default midnight)
    #[arg(long = "schedule", value_name = "MODE")]
    pub schedule: Option<String>,

    /// Active scheduling window: `HH:MM-HH:MM` — scheduler only runs inside this window
    #[arg(long = "schedule-window", value_name = "WINDOW")]
    pub schedule_window: Option<String>,

    /// Do not prepend the web-theme to the report
    #[arg(long = "no-web-theme")]
    pub no_web_theme: bool,

    /// Print the report to stdout (default: only in --dry-run, or when --output-dir is set)
    #[arg(long = "print")]
    pub print: bool,

    /// Save the report locally only — do not upload to WebDAV
    #[arg(long = "save-only")]
    pub save_only: bool,

    /// Report filename (default: <timestamp> Project Status.md; set to a static name like project-status.md)
    #[arg(long = "filename")]
    pub filename: Option<String>,

    /// Also copy the report to a timestamped name after upload (requires --filename)
    #[arg(long = "copy-dav")]
    pub copy_dav: bool,

    /// Verbose output — print intermediate JSON to stderr
    #[arg(short, long)]
    pub verbose: bool,
}

/// Fields to ignore when serializing deck cards to JSON.
/// These are noise/boilerplate fields that inflate prompt size but
/// add no meaningful context for the LLM's status analysis.
///
/// Fields kept (useful for LLM): id, title, description, duedate,
/// startdate, archived, overdue, assignedUsers[].participant.displayname,
/// assignedUsers[].participant.uid
const DEFAULT_IGNORED_FIELDS: &[&str] = &[
    ".color",              // visual-only, no semantic value
    ".stackId",            // known from context (which column we're in)
    ".type",               // always "plain" — noise
    ".lastModified",       // timestamp irrelevant for status
    ".lastEditor",         // who edited last — irrelevant
    ".attachmentCount",    // count without URLs — useless
    ".order",              // sort order — irrelevant
    ".commentsCount",      // count without content — useless
    ".ETag",               // internal sync token
    ".referenceData",      // internal reference (usually null)
    ".assignedUsers[].id",            // numeric ID — useless without context
    ".assignedUsers[].cardId",        // redundant (card is already identified)
    ".assignedUsers[].type",          // numeric enum — irrelevant
    ".assignedUsers[].participant.type", // numeric enum — irrelevant
    ".assignedUsers[].participant.primaryKey", // username handle — displayname is better
    ".createdAt",            // creation timestamp — noise
    ".deletedAt",            // deletion marker — 0 means alive
    ".commentsUnread",       // notification count — noise
    ".dependentCards",       // dependency tracking — rarely populated
    ".owner",                // board owner — known from context
    ".archived",             // keeping this? no — it's boolean noise
];

/// Parsed env file key-value pairs (simple `KEY=VALUE` lines).
/// Supports:
///   - Unquoted values: `KEY=value`
///   - Single-line quoted values: `KEY="value"` or `KEY='value'`
///   - Inline comments after unquoted values are NOT supported
///     (to avoid ambiguity with `=` in values)
pub fn parse_env_file(path: &Path) -> Result<Vec<(String, String)>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read env file: {}", path.display()))?;
    let mut pairs = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let raw = line[eq_pos + 1..].trim();
            let value = if raw.len() >= 2
                && raw.starts_with('"')
                && raw.ends_with('"')
            {
                raw[1..raw.len() - 1].to_string()
            } else if raw.len() >= 2
                && raw.starts_with('\'')
                && raw.ends_with('\'')
            {
                raw[1..raw.len() - 1].to_string()
            } else {
                raw.to_string()
            };
            pairs.push((key, value));
        }
    }
    Ok(pairs)
}

pub fn env_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Full configuration used by the application.
#[derive(Debug)]
pub struct Config {
    pub nc_url: String,
    pub nc_app_user: String,
    pub nc_app_pwd: String,
    pub ncdeck_board_id: u64,
    pub spark_url: String,
    pub spark_api_key: Option<String>,
    pub project_name: String,
    pub project_files_parent_path: String,
    pub cards_ignored_fields: String,
    /// Where to save local files (e.g. for --dry-run output).
    pub output_dir: PathBuf,
    /// Schedule mode: "once", "interval::<duration>", or "daily::<HH:MM>"
    pub schedule: Schedule,
    /// Whether to skip prepending the web-theme to reports.
    pub no_web_theme: bool,
    /// Active scheduling window — scheduler only runs inside this window.
    pub schedule_window: Option<ScheduleWindow>,
    /// Whether to print the report to stdout.
    pub print: bool,
    /// Save the report locally only — do not upload to WebDAV.
    pub save_only: bool,
    /// Report filename override. If None, use the default timestamped name.
    pub filename: Option<String>,
    /// Whether to also copy to a timestamped name via WebDAV COPY.
    pub copy_dav: bool,
}

/// A time window (start-end) within which scheduled runs are allowed.
pub struct ScheduleWindow {
    pub start: chrono::NaiveTime,
    pub end: chrono::NaiveTime,
}

impl std::fmt::Debug for ScheduleWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.start, self.end)
    }
}

/// Schedule mode for continuous report generation.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// Run once and exit (default).
    Once,
    /// Run repeatedly at a duration interval.
    Interval(std::time::Duration),
    /// Run once per day at a specific time (parsed at startup; always resets to midnight).
    #[allow(dead_code)]
    Daily(humantime::Duration),
}

impl Config {
    /// Load configuration: read env file, then apply CLI overrides.
    /// Also returns the parsed CLI for flag access in main().
    pub fn load() -> Result<(Self, Cli)> {
        let cli = Cli::parse();

        // Determine env file path
        let env_path = cli
            .config
            .as_ref()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("./.env"));

        let env_pairs = if env_path.exists() {
            parse_env_file(&env_path)?
        } else {
            Vec::new()
        };

        // Helper: resolve a required value — CLI arg > env file > panic.
        // Empty strings are treated as missing.
        let resolve = |cli_val: Option<String>, env_key: &str| -> Result<String> {
            if let Some(v) = cli_val {
                if v.is_empty() {
                    anyhow::bail!("Missing required config: provide --{} on CLI or set {} in env file",
                        env_key.replace('_', "-"), env_key);
                }
                Ok(v)
            } else if let Some(v) = env_value(&env_pairs, env_key) {
                if v.is_empty() {
                    anyhow::bail!("Missing required config: provide --{} on CLI or set {} in env file",
                        env_key.replace('_', "-"), env_key);
                }
                Ok(v.to_string())
            } else {
                anyhow::bail!("Missing required config: provide --{} on CLI or set {} in env file",
                    env_key.replace('_', "-"), env_key);
            }
        };

        let nc_url = resolve(cli.nc_url.clone(), "NC_URL")?;
        let nc_app_user = resolve(cli.nc_user.clone(), "NC_APP_USER")?;
        let nc_app_pwd = resolve(cli.nc_password.clone(), "NC_APP_PWD")?;
        let spark_url = resolve(cli.spark_url.clone(), "SPARK_URL")?;

        // API key is optional — only set if provided
        let spark_api_key = if let Some(v) = cli.api_key.clone() {
            Some(v)
        } else if let Some(v) = env_value(&env_pairs, "SPARK_API_KEY") {
            Some(v.to_string())
        } else {
            None
        };

        let project_name = resolve(cli.project_name.clone(), "PROJECT_NAME")?;
        let project_parent = resolve(cli.project_parent.clone(), "PROJECT_FILES_PARENT_PATH")?;

        let ncdeck_board_id = if let Some(v) = cli.board_id {
            v
        } else if let Some(v) = env_value(&env_pairs, "NCDECK_BOARD_ID") {
            v.parse().context("NCDECK_BOARD_ID must be a number")?
        } else {
            anyhow::bail!("Missing required config: --board-id or NCDECK_BOARD_ID in env file");
        };

        // Build ignored fields jq filter
        let ignored_fields: HashSet<&str> = DEFAULT_IGNORED_FIELDS.iter().copied().collect();
        let cards_ignored_fields = format!("del({})", ignored_fields.iter().copied().collect::<Vec<_>>().join(", "));

        let output_dir = cli
            .output_dir
            .as_ref()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));

        let schedule = parse_schedule(&cli.schedule.clone().or_else(|| env_value(&env_pairs, "SPARK_SCHEDULE").map(String::from)))?;
        let schedule_window = parse_schedule_window(cli.schedule_window.clone().or_else(|| env_value(&env_pairs, "SCHEDULE_WINDOW").map(String::from)))?;
        let no_web_theme = cli.no_web_theme || env_value(&env_pairs, "NO_WEB_THEME").is_some();
        let print = cli.print;
        let save_only = cli.save_only || env_value(&env_pairs, "SAVE_ONLY").is_some();
        let filename = match cli.filename.as_ref() {
            Some(v) => Some(v.clone()),
            None => env_value(&env_pairs, "REPORT_FILENAME").map(String::from),
        };
        let copy_dav = cli.copy_dav || env_value(&env_pairs, "COPY_DAV").is_some();

        Ok((
            Config {
                nc_url,
                nc_app_user,
                nc_app_pwd,
                ncdeck_board_id,
                spark_url,
                spark_api_key,
                project_name,
                project_files_parent_path: project_parent,
                cards_ignored_fields,
                output_dir,
                schedule,
                no_web_theme,
                schedule_window,
                print,
                save_only,
                filename,
                copy_dav,
            },
            cli,
        ))
    }

    /// Print config summary to stderr (useful for --verbose).
    pub fn log_verbose(&self) {
        eprintln!("[verbose] NC_URL: {}", self.nc_url);
        eprintln!("[verbose] NC_USER: {}", self.nc_app_user);
        eprintln!("[verbose] BOARD_ID: {}", self.ncdeck_board_id);
        eprintln!("[verbose] SPARK_URL: {}", self.spark_url);
        if let Some(ref key) = self.spark_api_key {
            let masked = format!("{}...{}", &key[..4.min(key.len())], &key[key.len().saturating_sub(4)..]);
            eprintln!("[verbose] SPARK_API_KEY: {}", masked);
        }
        eprintln!("[verbose] PROJECT_NAME: {}", self.project_name);
        eprintln!("[verbose] OUTPUT_DIR: {:?}", self.output_dir);
        eprintln!("[verbose] SCHEDULE: {:?}", self.schedule);
        if let Some(ref w) = self.schedule_window {
            eprintln!("[verbose] SCHEDULE_WINDOW: {:?}", w);
        } else {
            eprintln!("[verbose] SCHEDULE_WINDOW: (no window — always active)");
        }
        eprintln!("[verbose] NO_WEB_THEME: {}", self.no_web_theme);
        if let Some(ref f) = self.filename {
            eprintln!("[verbose] REPORT_FILENAME: {}", f);
        } else {
            eprintln!("[verbose] REPORT_FILENAME: (default — timestamped)");
        }
        eprintln!("[verbose] COPY_DAV: {}", self.copy_dav);
    }
}

/// Parse a schedule string into a `Schedule` enum.
///
/// Accepted formats:
///   - `once` — run once and exit (default)
///   - `interval::<duration>` — e.g. `interval::1h`, `interval::30m`, `interval::15min`
///   - `daily` or `daily::<HH:MM>` — once per day, default midnight (default: `daily::00:00`)
fn parse_schedule(input: &Option<String>) -> Result<Schedule> {
    let raw = input
        .as_ref()
        .map(|s| s.as_str())
        .unwrap_or("once");

    if let Some(rest) = raw.strip_prefix("interval::") {
        let duration = humantime::parse_duration(rest)
            .with_context(|| format!("Invalid interval duration: '{}'. Use formats like 1h, 30m, 15min", rest))?;
        return Ok(Schedule::Interval(duration));
    }

    if let Some(rest) = raw.strip_prefix("daily") {
        if rest.is_empty() || rest == ":" {
            return Ok(Schedule::Daily(humantime::Duration::from(std::time::Duration::from_secs(3600 * 24))));
        }
        if let Some(time_str) = rest.strip_prefix(":") {
            // Parse HH:MM and convert to duration since midnight
            let parts: Vec<&str> = time_str.split(':').collect();
            if parts.len() != 2 {
                bail!("Invalid daily time format: '{}'. Expected HH:MM", time_str);
            }
            let hours: u32 = parts[0].parse().with_context(|| format!("Invalid hours in daily time: '{}'", parts[0]))?;
            let minutes: u32 = parts[1].parse().with_context(|| format!("Invalid minutes in daily time: '{}'", parts[1]))?;
            if hours > 23 || minutes > 59 {
                bail!("Invalid time: hours must be 0-23, minutes 0-59");
            }
            let duration = std::time::Duration::from_secs((hours * 3600 + minutes * 60) as u64);
            return Ok(Schedule::Daily(humantime::Duration::from(duration)));
        }
    }

    if raw == "once" || raw.is_empty() {
        return Ok(Schedule::Once);
    }

    bail!(
        "Invalid schedule: '{}'. Use 'once', 'interval::<duration>' (e.g. 1h), or 'daily::<HH:MM>' (default midnight)",
        raw
    )
}

/// Parse a schedule window string like "08:00-19:00".
pub fn parse_schedule_window(raw: Option<String>) -> Result<Option<ScheduleWindow>> {
    let raw = match raw {
        Some(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };

    let parts: Vec<&str> = raw.split('-').collect();
    if parts.len() != 2 {
        bail!(
            "Invalid schedule window: '{}'. Expected format HH:MM-HH:MM",
            raw
        );
    }

    let parse_hm = |s: &str| -> Result<chrono::NaiveTime> {
        let segs: Vec<&str> = s.trim().split(':').collect();
        if segs.len() != 2 {
            bail!("Invalid time in window: '{}'. Expected HH:MM", s);
        }
        let hours: u32 = segs[0].parse().map_err(|_| anyhow::anyhow!("Invalid hour in window: '{}'", segs[0]))?;
        let minutes: u32 = segs[1].parse().map_err(|_| anyhow::anyhow!("Invalid minute in window: '{}'", segs[1]))?;
        if hours > 23 || minutes > 59 {
            bail!("Invalid time: hours must be 0-23, minutes 0-59");
        }
        chrono::NaiveTime::from_hms_opt(hours, minutes, 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid time: {}:{}", hours, minutes))
    };

    let start = parse_hm(parts[0])?;
    let end = parse_hm(parts[1])?;

    if end <= start {
        bail!(
            "Invalid schedule window: end ({}) must be after start ({})",
            end, start
        );
    }

    Ok(Some(ScheduleWindow { start, end }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_env_simple() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "KEY1=value1").unwrap();
        writeln!(tmp, "KEY2=\"value with spaces\"").unwrap();
        writeln!(tmp, "KEY3='single quoted'").unwrap();
        writeln!(tmp, "# comment").unwrap();
        writeln!(tmp, "").unwrap();
        writeln!(tmp, "KEY4=unquoted").unwrap();

        let pairs = parse_env_file(tmp.path()).unwrap();
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();

        assert_eq!(map["KEY1"], "value1");
        assert_eq!(map["KEY2"], "value with spaces");
        assert_eq!(map["KEY3"], "single quoted");
        assert_eq!(map["KEY4"], "unquoted");
        assert!(!map.contains_key("# comment"));
    }

    #[test]
    fn test_parse_env_multiline_not_supported() {
        // The parser does NOT support multi-line quoted values.
        // Each KEY=VALUE must be on a single line.
        // This is intentional — the Rust code uses hardcoded ignored fields.
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "NC_URL=https://nextcloud.example.com").unwrap();
        writeln!(tmp, "NC_APP_USER=liam").unwrap();
        // Opening quote on its own "value" — not terminated on same line
        writeln!(tmp, r#"CARDS_FIELDS=""#).unwrap();
        writeln!(tmp, r#".color,"#).unwrap();

        let pairs = parse_env_file(tmp.path()).unwrap();
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();

        assert_eq!(map["NC_URL"], "https://nextcloud.example.com");
        assert_eq!(map["NC_APP_USER"], "liam");
        // The CARDS_FIELDS line is treated as `CARDS_FIELDS="` —
        // a single double-quote character (quoted value of length 1).
        // Our parser handles this gracefully: len < 2 is single-char quote,
        // so it falls through to the unquoted branch and stores `"` literally.
        assert_eq!(map["CARDS_FIELDS"], "\"");
    }
}
