# spark-project-status

Generate project status reports from [NextCloud Deck](https://apps.nextcloud.com/apps/deck) boards, enhanced by an LLM.

## Overview

This tool:

1. **Fetches** card data from a NextCloud Deck board (To-Do, Doing, Done, Cancelled stacks).
2. **Sends** the board summary to an LLM API (Spark / OpenAI-compatible) for analysis.
3. **Uploads** the generated report back to NextCloud WebDAV as a Markdown file.

Runs as a standalone CLI or as a `systemd` service with automatic temp-dir management.

## Building

```bash
cargo build --release
```

The binary is placed at `target/release/spark-project-status`.

## Configuration

### Configuration files

Copy `sample.env` to `.env` and fill in your values:

```bash
cp sample.env .env
# Edit .env with your actual credentials
```

The `.env` file is git-ignored. See `sample.env` for documented parameter descriptions.

#### Interactive setup

If no `.env` file exists, the tool launches an interactive wizard on start:

1. Enter your NextCloud URL, username, and app-password
2. Validates credentials and lists available Deck boards
3. **Board creation** — if no boards exist, offers to create one via the Deck API
4. **Board configurator** — if the selected board is missing required stacks
   (To-Do, Doing, Done, Cancelled), offers to create them automatically
5. Selects the board to use
6. Chooses the prompt variant (e.g. Agile, Travel, Meal Planning)
7. Generates `.env` and copies the selected `Spark-Instructions.md`

To skip the wizard when no `.env` exists, pass `--help` or any CLI flag (e.g. `--nc-url`):

```bash
spark-project-status --help
spark-project-status --nc-url https://nextcloud.example.com
```

Configuration can also be supplied via environment variables or CLI flags.
Priority (highest to lowest): **CLI flags > env vars > .env file**.

```bash
NC_URL="https://nextcloud.example.com"
NC_APP_USER="your-app-user"
NC_APP_PWD="your-app-password"
NCDECK_BOARD_ID=13
SPARK_URL="http://127.0.0.1:8214"
PROJECT_NAME="My Project"
PROJECT_FILES_PARENT_PATH="Personal"
```

**Required files** must exist in the NextCloud project folder (`<parent>/<project_name>`):

- `Spark-Instructions.md` — LLM system prompt. Select one from `prompts/*.md`:
  ```bash
  cp prompts/agile-software-dev.md Spark-Instructions.md
  ```
  Available prompts:
  - `agile-software-dev.md` — Agile & Software Development
  - `home-improvement-structured.md` — Home Improvement (with deadlines)
  - `home-improvement-unstructured.md` — Home Improvement (general todo)
  - `meal-planning.md` — Meal Planning & Recipes
  - `personal-productivity.md` — Personal Productivity & Goals
  - `excursions-travel.md` — Travel & Excursions
- `web-theme.txt` (optional) — YAML frontmatter theme; falls back to `web-theme-default.txt` if missing

### CLI overrides

Every env file value can be overridden on the command line:

```bash
spark-project-status \
  --nc-url "https://nextcloud.example.com" \
  --nc-user "liam" \
  --nc-password "token" \
  --board-id 13 \
  --spark-url "http://localhost:8214" \
  --project-name "My Project" \
  --project-parent "Personal" \
  --config ./env
```

### CLI flags

| Flag | Description |
|------|-------------|
| `-c, --config <FILE>` | Path to config file (default: `./env`) |
| `--nc-url <URL>` | NextCloud URL |
| `--nc-user <TOKEN>` | NextCloud app-user |
| `--nc-password <TOKEN>` | NextCloud app-password |
| `--board-id <ID>` | Deck board ID |
| `--spark-url <URL>` | LLM API base URL |
| `--project-name <NAME>` | Project name |
| `--project-parent <PATH>` | Files parent path in NextCloud |
| `--dry-run` | Check WebDAV state without uploading; prints report to stdout |
| `--output-dir <DIR>` | Save local copy of report |
| `--no-web-theme` | Skip prepending the web-theme to the report |
| `--schedule <MODE>` | Run continuously: `once`, `interval::<duration>` (e.g. `1h`), or `daily::<HH:MM>` (default midnight) |
| `--schedule-window <WIN>` | Only run inside a time window, e.g. `08:00-19:00` (idle overnight) |
| `--print` | Always print report to stdout (even in production mode) |
| `--save-only` | Save report locally only — skip WebDAV upload |
| `--filename <NAME>` | Use a static report filename instead of timestamped |
| `--copy-dav` | Also copy the report to a timestamped name via WebDAV (requires `--filename`) |
| `-v, --verbose` | Print intermediate JSON to stderr |
| `-h, --help` | Show help |
| `-V, --version` | Show version |

### Dry-run

```bash
spark-project-status --dry-run --spark-url "http://127.0.0.1:8214"
```

Outputs the full report (theme + LLM content) to stdout and prints stats. Also checks the WebDAV folder and target file to confirm the upload destination is valid:

```
[dry-run] WebDAV folder exists
[dry-run] Destination file does not exist yet: https://...
```

```
Model: default, Prompt Duration: 0.8s, Prompt TPS: 12.5, Generation Duration: 2.3s, Generation TPS: 8.1
Usage: prompt=1500, completion=420, total=1920
```

## Scheduled / continuous reporting

The `--schedule` flag runs report generation on a repeating cycle:

```bash
# Run every hour
spark-project-status --schedule "interval::1h"

# Run every 30 minutes
spark-project-status --schedule "interval::30m"

# Run once per day at midnight (default)
spark-project-status --schedule "daily"

# Run once per day at 6:00 AM
spark-project-status --schedule "daily::06:00"

# Run once and exit (default)
spark-project-status --schedule "once"
```

### Active scheduling window

Combine `--schedule` with `--schedule-window` to only run reports during business hours:

```bash
# Run every hour, but only between 8 AM and 7 PM
spark-project-status --schedule "interval::1h" --schedule-window "08:00-19:00"

# Run daily at midnight, but only on weekdays (combine with cron or systemd)
# The window itself can't do day-of-week filtering, so use --schedule-window "00:00-00:01"
```

Outside the window, the scheduler sleeps until the next start time and prints a message.
Overnight windows also work (e.g. `22:00-06:00`).

Press `Ctrl+C` to stop scheduled runs.

## Systemd service

When running under `systemd`, the `RuntimeDirectory=` directive provides a managed temp directory. The tool detects `RUNTIME_DIRECTORY` and uses it directly.

### Example service

```ini
[Unit]
Description=Spark Project Status Report
After=network-online.target

[Service]
Type=oneshot
RuntimeDirectory=spark-project-status
ExecStart=/usr/local/bin/spark-project-status --spark-url "http://127.0.0.1:8214"
Environment=RUNTIME_DIRECTORY=/run/spark-project-status

[Install]
WantedBy=multi-user.target
```

### Example timer

```ini
[Unit]
Description=Generate weekly project status report

[Timer]
OnCalendar=weekly
Persistent=true

[Install]
WantedBy=timers.target
```

## Ignored fields

The tool strips the following fields from Deck cards before sending to the LLM. These reduce prompt size without losing meaningful context:

| Field | Reason |
|-------|--------|
| `.color` | Visual-only |
| `.stackId` | Known from context |
| `.type` | Always `"plain"` |
| `.lastModified`, `.createdAt` | Timestamps irrelevant for status |
| `.lastEditor` | Irrelevant |
| `.attachmentCount` | Count without URLs |
| `.order`, `.commentsCount`, `.commentsUnread` | Noise |
| `.ETag` | Internal sync token |
| `.referenceData`, `.dependentCards` | Rarely populated |
| `.owner` | Known from context |
| `.assignedUsers[].id`, `.cardId`, `.type` | Numeric IDs without context |
| `.assignedUsers[].participant.type`, `.primaryKey` | Internal identifiers |

## Project structure

```
src/
├── main.rs       — CLI dispatch, orchestration, stats output
├── config.rs     — Env file parsing, CLI argument parsing (clap)
├── deck.rs       — NextCloud Deck API client, stack validation, field stripping
├── llm.rs        — LLM completion payload builder and HTTP client
└── webdav.rs     — WebDAV operations (HEAD check, fetch, upload)
```
