#!/usr/bin/env bash
# Installs or updates scanner-web on a Debian/Raspberry Pi host over SSH.
# Usage: deploy/install.sh user@host
# Needs passwordless sudo on the host. Re-run to update; config and secrets are kept.
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET="${1:?usage: deploy/install.sh user@host}"
BIN=target/linux-arm64/release

[ -x "$BIN/scanner-web" ] || { echo "build first: deploy/build-linux-arm64.sh" >&2; exit 1; }

STAGE=$(ssh "$TARGET" mktemp -d)
scp -q "$BIN/scanner-web" "$BIN/scanner" deploy/scanner-web.service scanner.example.toml "$TARGET:$STAGE/"

ssh "$TARGET" STAGE="$STAGE" 'bash -s' <<'REMOTE'
set -euo pipefail
sudo apt-get install -y -qq libsane1 >/dev/null
# Own user/group: Debian's SANE packages already use a group called "scanner".
id scanner-web >/dev/null 2>&1 || sudo useradd --system --user-group --home /var/lib/scanner --shell /usr/sbin/nologin scanner-web
sudo install -d -o scanner-web -g scanner-web -m 750 /var/lib/scanner /var/lib/scanner/scans
sudo install -d -o root -g scanner-web -m 750 /etc/scanner
sudo install -d -m 755 /opt/scanner
sudo install -m 755 "$STAGE/scanner-web" "$STAGE/scanner" /opt/scanner/
sudo install -m 644 "$STAGE/scanner.example.toml" /etc/scanner/scanner.example.toml
# Keep existing config/secrets; create empty ones on first install.
[ -f /etc/scanner/scanner.toml ] || printf '# See scanner.example.toml. No destinations configured yet.\n' \
  | sudo install -o root -g scanner-web -m 640 /dev/stdin /etc/scanner/scanner.toml
[ -f /etc/scanner/scanner.env ] || sudo install -o root -g root -m 600 /dev/null /etc/scanner/scanner.env
sudo install -m 644 "$STAGE/scanner-web.service" /etc/systemd/system/scanner-web.service
sudo systemctl daemon-reload
sudo systemctl enable --quiet scanner-web
sudo systemctl restart scanner-web
rm -rf "$STAGE"
sleep 2
systemctl is-active scanner-web
REMOTE
