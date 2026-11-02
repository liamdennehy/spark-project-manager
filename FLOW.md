# spark-project-status — Processing Flow

## Overview

The tool processes a NextCloud Deck board into an LLM-enhanced report through three
distinct phases: **Ingestion**, **Inference**, and **Distribution**. Each phase has a
clear purpose, independent failure characteristics, and distinct behaviour in
`--dry-run` mode.

All configuration directives are available through three channels, in this precedence
order:

1. **Command-line flags** (highest)
2. **Environment variables**
3. **Config file** (lowest)

See the end of this document for the full directive reference table.

---

## Phase 1 — Ingestion

**Purpose:** Retrieve project data and arrange cards into a JSON structure by state,
removing fields not needed by the project manager. Also fetch project instructions.

**Operations:**

- Fetch Deck board metadata via `GET /apps/deck/api/v1/boards/{id}`
- Fetch stack detail (`TODO`, `DOING`, `DONE`, `CANCELLED`) for each column
- Strip ignored fields from card JSON (`.color`, `.order`, `.ETag`, etc.)
- Fetch `Spark-Instructions.md` from the remote project folder via WebDAV
- Validate that the project folder exists on WebDAV
- Validate that the project folder is writable (or at least accessible)

**Impact:** Low — Deck API calls are simple and fast; JSON processing is minimal.

**Failure mode:** Fast failure — invalid credentials, missing board, or missing
project folder are detected immediately with clear error messages.

**Dry-run behaviour:** All ingestion steps run normally. If the project folder does
not exist or is inaccessible, a warning is emitted but processing continues (since
Ingestion itself cannot fail).

---

## Phase 2 — Inference

**Purpose:** Have the LLM analyse the cards against the project goals and produce a
full status report.

**Operations:**

- Build the JSON board summary from the ingested card data
- Build the LLM prompt by combining system instructions (Spark-Instructions.md) with
  the board summary and project metadata (name, parent path, timestamp)
- Send the completion request to the configured LLM API endpoint

**Impact:** May incur cost — commercial LLM providers charge per token; local models
consume inference time that may be limited or shared.

**Dry-run behaviour:**

- A **probe inference** is sent with a maximum of one card from each state
- The LLM request uses `max_tokens` (e.g. 10) to limit output
- This confirms the LLM connection, credentials, and prompt structure are valid
- The full inference is **not** sent — the user sees a short confirmation message
- If the probe fails, the user knows the issue is connection/credential/prompt-related
  before committing to a full (costly) inference

---

## Phase 3 — Distribution

**Purpose:** Place the report file(s) in locations where users or downstream processes
can consume them.

**Potential targets** (can be combined):

| Target | Directive | Behaviour |
|--------|-----------|-----------|
| Console (stdout) | (implicit in `--dry-run`) | Emit the full report with web-theme to stdout |
| Local file | `--output-dir <DIR>` or `RUNTIME_DIRECTORY` | Save report to a directory (timestamped filename) |
| DAV upload | `--filename <NAME>` | Upload report to WebDAV at the specified path |
| DAV copy | `--copy-dav` | Copy the uploaded file to a timestamped name via WebDAV `COPY` verb |

**WebDAV filename templating:**

Supports simple path templating in `--dav-filename` / `REPORT_FILENAME`:

- `{timestamp}` — replaced with current date/time (e.g. `2025-11-01 14-30`)
- `{date}` — replaced with current date (e.g. `2025-11-01`)
- `{name}` — replaced with project name

Example: `Archive/{date} Project Status.md` → `Archive/2025-11-01 Project Status.md`

**Dry-run behaviour:**

- **Console:** Always emits the full report (theme + LLM content) to stdout
- **Local file:** Checks the output directory exists and is writable. Emits a
  warning if the directory is not visible or not writable. Does not write any file.
- **DAV:** Checks that each parent folder in the proposed filename path exists.
  Emits a warning for any missing folder. Does not upload or copy any file.

**Destructiveness:** Phase 3 is the only potentially destructive phase (overwrites
existing files on DAV). `--dry-run` prevents all writes while still validating paths.

---

## Directive Reference

Each directive is available in up to three channels. All respect the precedence:
**CLI flag > env var > config file**.

| Directive | Purpose | Config File | Env Var | CLI Flag |
|-----------|---------|-------------|---------|----------|
| Config file path | Location of the env file | — | — | `--config <FILE>` / `-c` |
| NextCloud URL | Instance base URL | `NC_URL` | `NC_URL` | `--nc-url <URL>` |
| NextCloud user | App-user token | `NC_APP_USER` | `NC_APP_USER` | `--nc-user <TOKEN>` |
| NextCloud password | App-password / token | `NC_APP_PWD` | `NC_APP_PWD` | `--nc-password <TOKEN>` |
| Deck board ID | Board to query | `NCDECK_BOARD_ID` | `NCDECK_BOARD_ID` | `--board-id <ID>` |
| LLM API URL | Spark / OpenAI-compatible endpoint | `SPARK_URL` | `SPARK_URL` | `--spark-url <URL>` |
| LLM API key | Bearer token for LLM server | `SPARK_API_KEY` | `SPARK_API_KEY` | `--api-key <KEY>` |
| Project name | Display name for reports | `PROJECT_NAME` | `PROJECT_NAME` | `--project-name <NAME>` |
| Project parent | Files parent path in NextCloud | `PROJECT_FILES_PARENT_PATH` | `PROJECT_FILES_PARENT_PATH` | `--project-parent <PATH>` |
| Schedule mode | `once`, `interval::<dur>`, `daily::<HH:MM>` | `SPARK_SCHEDULE` | `SPARK_SCHEDULE` | `--schedule <MODE>` |
| Schedule window | Active window `HH:MM-HH:MM` | `SCHEDULE_WINDOW` | `SCHEDULE_WINDOW` | `--schedule-window <WIN>` |
| Prepend web-theme | Include YAML front-matter theme in report | `WEB_THEME` | `WEB_THEME` | `--web-theme` |
| Print to stdout | Always output report to stdout | `SAVE_ONLY` (inverse) | `SAVE_ONLY` | `--print` |
| Save only | Local file only, no DAV upload | `SAVE_ONLY` | `SAVE_ONLY` | `--save-only` |
| Report filename | Static filename for DAV upload | `REPORT_FILENAME` | `REPORT_FILENAME` | `--filename <NAME>` |
| DAV copy | Copy to timestamped name after upload | `COPY_DAV` | `COPY_DAV` | `--copy-dav` |
| Verbose output | Print intermediate JSON to stderr | — | — | `-v` / `--verbose` |
| Dry-run | Validate without uploading or inferring | — | — | `--dry-run` |
| Output directory | Local file output directory | — | `RUNTIME_DIRECTORY` | `--output-dir <DIR>` |

### Notes

- `NC_APP_PWD` and `SPARK_API_KEY` are masked in all output
- `--dry-run` validates all phases up to and including a probe inference, but
  performs no writes to DAV or the local filesystem
- `--print` forces stdout output regardless of other distribution settings
- `--save-only` and `--copy-dav` are independent flags that can be combined with
  each other and with `--print`
- `SPARK_SCHEDULE` env var is namespaced with `SPARK_` to avoid conflicts with other
  tools that may also use `SCHEDULE` in the same environment
