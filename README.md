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
2. Validates credentials via Deck API and WebDAV (no CSRF issues)
3. Lists available Deck boards; offers to create one if none exist
4. **Board configurator** — if the selected board is missing required stacks
   (To-Do, Doing, Done, Cancelled), offers to create them automatically
5. Enter project name and parent folder path
6. Chooses prompt instructions:
   - If `Spark-Instructions.md` already exists in the remote project folder,
     the wizard offers to keep it or select from built-in templates
   - If a template is selected and remote instructions exist, a confirmation
     is shown to prevent accidental overwrites
7. Uploads selected instructions to the remote folder and generates `.env`

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
  --config ./.env
```

### CLI flags

| Flag | Description |
|------|-------------|
| `-c, --config <FILE>` | Path to config file (default: `./.env`) |
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
| `--schedule <MODE>` | Run continuously: `once` (default), `interval::<duration>` (e.g. `interval::1h`), or `daily::<HH:MM>` (default midnight) |
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
spark-project-status --dry-run
```

Outputs the full report (theme + LLM content) to stdout and prints stats. Checks WebDAV state without uploading:

```
[dry-run] Deck board #13 fetched (24 cards)
[dry-run] LLM inference complete
[dry-run] WebDAV folder exists
[dry-run] Destination file does not exist yet: https://...
```

```
Model: default, Prompt Duration: 0.8s, Prompt TPS: 12.5, Generation Duration: 2.3s, Generation TPS: 8.1
Usage: prompt=1500, completion=420, total=1920
```

## Scheduled / continuous reporting

The `--schedule` flag runs report generation on a repeating cycle. When omitted, runs once and exits.

```bash
# Run every hour
spark-project-status --schedule "interval::1h"

# Run every 30 minutes
spark-project-status --schedule "interval::30m"

# Run once per day at midnight (default when schedule is given without time)
spark-project-status --schedule "daily"

# Run once per day at 6:00 AM
spark-project-status --schedule "daily::06:00"

# Run once and exit (default — omit --schedule entirely)
spark-project-status
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

## Systemd deployment

The `systemd/` directory contains instantiated service and timer templates for multi-instance deployment (one per project).

### Quick start

```bash
# Build
cargo build --release

# Install units, create system user, set up directories
sudo ./systemd/install-systemd.sh ./target/release/spark-project-status

# Create a project configuration
sudo mkdir -p /etc/spark-project-status/my-project
cp systemd/sample.env /etc/spark-project-status/my-project/.env
# Edit /etc/spark-project-status/my-project/.env with your settings

# Enable and start
sudo systemctl enable --now spark-project-status@my-project.timer
```

### Manual installation

Copy the unit files from `systemd/`:

| File | Destination |
|------|-------------|
| `spark-project-status@.service` | `/etc/systemd/system/` |
| `spark-project-status@.timer` | `/etc/systemd/system/` |
| `spark-project-status.sysusers` | `/etc/sysusers.d/` |
| `spark-project-status.tmpfiles.d` → `spark-project-status.conf` | `/etc/tmpfiles.d/` |

Then: `sudo systemctl daemon-reload`

### Per-project management

```bash
# Start once (without timer)
sudo systemctl start spark-project-status@my-project.service

# View logs
journalctl -u spark-project-status@my-project.service -f

# Find reports
ls -la /run/spark-project-status/my-project/

# Disable/remove
sudo systemctl disable --now spark-project-status@my-project.timer
sudo rm -rf /etc/spark-project-status/my-project
```

See `systemd/README.md` for full documentation.

## Ignored fields

The tool strips the following fields from Deck cards before sending to the LLM. These reduce prompt size without losing meaningful context:

| Field | Reason |
|-------|--------|
| `.color` | Visual-only |
| `.stackId` | Known from context (which column we're in) |
| `.type` | Always `"plain"` — noise |
| `.lastModified`, `.createdAt`, `.deletedAt` | Timestamps irrelevant for status |
| `.lastEditor` | Irrelevant |
| `.attachmentCount` | Count without URLs — useless |
| `.order` | Sort order — irrelevant |
| `.commentsCount`, `.commentsUnread` | Count/notification without content |
| `.ETag` | Internal sync token |
| `.referenceData` | Internal reference (usually null) |
| `.dependentCards` | Dependency tracking — rarely populated |
| `.owner` | Board owner — known from context |
| `.assignedUsers[].id`, `.cardId`, `.type` | Numeric IDs without context |
| `.assignedUsers[].participant.type`, `.participant.primaryKey` | Internal identifiers |
| `.archived` | Boolean archive flag — noise |

## Project structure

```
src/
├── main.rs       — CLI dispatch, orchestration, stats output
├── config.rs     — Env file parsing, CLI argument parsing (clap)
├── deck.rs       — NextCloud Deck API client, stack validation, field stripping
├── llm.rs        — LLM completion payload builder and HTTP client
└── webdav.rs     — WebDAV operations (HEAD check, fetch, upload)
```
