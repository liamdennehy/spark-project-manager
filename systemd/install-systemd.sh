#!/usr/bin/env bash
# install-systemd.sh — Install systemd units for spark-project-status
#
# Usage:
#   sudo ./install-systemd.sh [binary_path]
#
# Installs:
#   - Systemd service & timer templates
#   - Sysusers file (unprivileged user)
#   - tmpfiles.d rule (directory cleanup)
#   - Copy of sample.env to /etc/spark-project-status/sample.env

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SYSTEMD_DIR="${SCRIPT_DIR}/systemd"
INSTALL_ROOT="${INSTALL_ROOT:-/}"
BINARY="${1:-/usr/local/bin/spark-project-status}"

# ── Pre-flight checks ─────────────────────────────────────────────
if [[ ! -x "$BINARY" ]]; then
    echo "Error: binary not found or not executable: $BINARY" >&2
    exit 1
fi

if [[ ! -d "$SYSTEMD_DIR" ]]; then
    echo "Error: systemd/ directory not found" >&2
    exit 1
fi

echo "Installing spark-project-status systemd units..."

# ── 1. Install binary ──────────────────────────────────────────────
echo "  → Binary: $BINARY → ${INSTALL_ROOT}/usr/local/bin/spark-project-status"
install -Dm0755 "$BINARY" "${INSTALL_ROOT}/usr/local/bin/spark-project-status"

# ── 2. Install systemd templates ───────────────────────────────────
echo "  → Service template: spark-project-status@.service"
install -Dm0644 \
    "${SYSTEMD_DIR}/spark-project-status@.service" \
    "${INSTALL_ROOT}/etc/systemd/system/spark-project-status@.service"

echo "  → Timer template: spark-project-status@.timer"
install -Dm0644 \
    "${SYSTEMD_DIR}/spark-project-status@.timer" \
    "${INSTALL_ROOT}/etc/systemd/system/spark-project-status@.timer"

# ── 3. Install sysusers (unprivileged user) ────────────────────────
echo "  → Sysusers: spark-project-status.sysusers"
install -Dm0644 \
    "${SYSTEMD_DIR}/spark-project-status.sysusers" \
    "${INSTALL_ROOT}/etc/sysusers.d/spark-project-status.sysusers"

# Run systemd-sysusers to create the user
echo "  → Creating spark-project-status user..."
systemd-sysusers 2>/dev/null || {
    echo "  (Note: systemd-sysusers failed — user may already exist)" >&2
}

# ── 4. Install tmpfiles.d (directory cleanup) ──────────────────────
echo "  → tmpfiles.d: spark-project-status.tmpfiles.d"
install -Dm0644 \
    "${SYSTEMD_DIR}/spark-project-status.tmpfiles.d" \
    "${INSTALL_ROOT}/etc/tmpfiles.d/spark-project-status.conf"

# Apply tmpfiles immediately
echo "  → Applying tmpfiles.d..."
systemd-tmpfiles --create 2>/dev/null || {
    echo "  (Note: systemd-tmpfiles --create failed — may need root)" >&2
}

# ── 5. Install sample .env ─────────────────────────────────────────
echo "  → Sample .env → ${INSTALL_ROOT}/etc/spark-project-status/sample.env"
install -Dm0600 \
    "${SYSTEMD_DIR}/sample.env" \
    "${INSTALL_ROOT}/etc/spark-project-status/sample.env"

# ── 6. Reload systemd ──────────────────────────────────────────────
echo "  → Reloading systemd daemon..."
systemctl daemon-reload

# ── 7. Enable timer ────────────────────────────────────────────────
echo ""
echo "Installation complete!"
echo ""
echo "Next steps:"
echo "  1. Create a config dir for your project:"
echo "     mkdir -p /etc/spark-project-status/<project-name>"
echo "     cp /etc/spark-project-status/sample.env \\"
echo "        /etc/spark-project-status/<project-name>/.env"
echo "     # Edit /etc/spark-project-status/<project-name>/.env with your settings"
echo ""
echo "  2. Enable and start the timer (runs daily at 06:00):"
echo "     systemctl enable --now spark-project-status@<project-name>.timer"
echo ""
echo "  3. Run once immediately (without timer):"
echo "     systemctl start spark-project-status@<project-name>.service"
echo ""
echo "  4. View logs:"
echo "     journalctl -u spark-project-status@<project-name>.service -f"
echo ""
echo "  5. Find output files:"
echo "     ls -la /run/spark-project-status/<project-name>/"
echo ""
