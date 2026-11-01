# spark-project-status — Systemd Deployment

Systemd units for deploying `spark-project-status` as a **multi-instance** service on Linux. Each project gets its own instance with isolated configuration and output directories.

## Files

| File | Purpose | Install Path |
|------|---------|-------------|
| `spark-project-status@.service` | Service template (oneshot) | `/etc/systemd/system/` |
| `spark-project-status@.timer` | Timer template (daily at 06:00) | `/etc/systemd/system/` |
| `spark-project-status.sysusers` | Unprivileged system user | `/etc/sysusers.d/` |
| `spark-project-status.tmpfiles.d` | Runtime directory cleanup | `/etc/tmpfiles.d/spark-project-status.conf` |
| `sample.env` | Example configuration | `/etc/spark-project-status/sample.env` |
| `install-systemd.sh` | One-command installer | — |

## Architecture

```
spark-project-status@<project>.timer
    └─> spark-project-status@<project>.service (oneshot)
            ├─ Reads config from ConfigurationDirectory
            ├─ Runs spark-project-status binary
            ├─ Writes report to RuntimeDirectory
            └─ Exits (timer triggers next run)
```

Each instance is completely isolated — different config, different output directory, different journal namespace.

## Installation

```bash
# Build the binary first
cargo build --release

# Run the installer (requires root for system-wide install)
sudo ./install-systemd.sh ./target/release/spark-project-status
```

Or install manually:

```bash
# Copy unit files
sudo cp spark-project-status@.service /etc/systemd/system/
sudo cp spark-project-status@.timer /etc/systemd/system/

# Create system user
sudo cp spark-project-status.sysusers /etc/sysusers.d/
sudo systemd-sysusers

# Apply tmpfiles rules
sudo cp spark-project-status.tmpfiles.d /etc/tmpfiles.d/spark-project-status.conf
sudo systemd-tmpfiles --create

# Reload systemd
sudo systemctl daemon-reload
```

## Per-Project Setup

### 1. Create configuration directory

```bash
sudo mkdir -p /etc/spark-project-status/my-project
sudo cp /etc/spark-project-status/sample.env /etc/spark-project-status/my-project/.env
sudo chmod 600 /etc/spark-project-status/my-project/.env
```

### 2. Edit the configuration

```bash
sudoedit /etc/spark-project-status/my-project/.env
```

Edit the values in `.env` — at minimum set:
- `NC_URL`, `NC_APP_USER`, `NC_APP_PWD` — NextCloud connection
- `NCDECK_BOARD_ID` — The Deck board to report on
- `SPARK_URL`, `SPARK_API_KEY` — LLM endpoint
- `PROJECT_NAME`, `PROJECT_FILES_PARENT_PATH` — Project metadata

### 3. Enable & start

```bash
# Enable the timer (runs daily at 06:00)
sudo systemctl enable --now spark-project-status@my-project.timer

# Or run once immediately (no timer needed)
sudo systemctl start spark-project-status@my-project.service
```

## Management

### View logs

```bash
journalctl -u spark-project-status@my-project.service -f
```

### Run manually

```bash
# One-shot (without timer)
sudo systemctl start spark-project-status@my-project.service

# Check last run status
sudo systemctl status spark-project-status@my-project.service
```

### Find output reports

Reports are written to the RuntimeDirectory:

```bash
ls -la /run/spark-project-status/my-project/
```

### Change schedule

Override the timer schedule:

```bash
# Run every 4 hours instead of daily
sudo systemctl edit spark-project-status@my-project.timer
```

Add:
```ini
[Timer]
OnCalendar=*-*-* 00/4:00:00
Persistent=true
```

Then reload:
```bash
sudo systemctl daemon-reload
sudo systemctl restart spark-project-status@my-project.timer
```

### Stop a project

```bash
sudo systemctl disable --now spark-project-status@my-project.timer
sudo systemctl stop spark-project-status@my-project.service
sudo rm -rf /etc/spark-project-status/my-project
```

## Security

The service runs as the unprivileged `spark-project-status` system user with:

- **Strict read-only system** — can only write to `/run/spark-project-status/<instance>`
- **No new privileges** — cannot escalate
- **Protected kernel** — kernel tunables, modules, logs, and cgroups are hidden
- **Namespace isolation** — private tmp, restricted namespaces
- **Encrypted secrets** — `.env` file has mode `0600`

## Customization

### Custom binary location

If the binary is at a non-standard path, edit the service file:

```bash
sudo systemctl edit spark-project-status@my-project.service
```

Add:
```ini
[Service]
ExecStart=
ExecStart=/custom/path/spark-project-status \
    --config /etc/spark-project-status/%i/.env \
    --print \
    --output-dir /run/spark-project-status/%i
```

(The empty `ExecStart=` clears the template value before redefining it.)

### Custom configuration directory

By default, the service looks for `.env` in `/etc/spark-project-status/<instance>/` and
`/usr/local/share/spark-project-status/<instance>/`. To use a different location,
override the `--config` path in the service template.

## Design Decisions

- **Timer + oneshot service** — systemd timers are the native way to schedule periodic
  tasks (replacing cron). The service is `Type=oneshot` so systemd tracks success/failure
  of each individual run.
- **`ConfigurationDirectory=`** — systemd-native path for configuration files (creates
  `/etc/spark-project-status/<instance>/` automatically, with correct ownership).
- **`RuntimeDirectory=`** — systemd manages the output directory lifecycle (created on
  service start, cleaned on boot).
- **Instantiated units** (`@.service`) — a single template handles all projects without
  duplicating unit files.
