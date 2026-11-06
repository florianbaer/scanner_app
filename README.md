# scanner_app

Scan wirelessly from an Epson FastFoto FF-680W (or any SANE-supported scanner), from the
command line or a small web UI.

## Requirements

- Rust (stable)
- SANE: `brew install sane-backends`

## Web UI

```sh
cargo run --release -p scanner-web            # http://127.0.0.1:8080
cargo run --release -p scanner-web -- --host 0.0.0.0 --port 8080 --out ~/Scans
```

Pick a scanner, choose settings (color, resolution, sides, crop, straighten, file type),
start a scan, then open or download the PDF. There is no authentication, so only bind to
`0.0.0.0` on a network you trust.

## CLI

```sh
cargo run --release -p scanner-cli -- list
cargo run --release -p scanner-cli -- scan --duplex -r 600 -o ~/Scans
cargo run --release -p scanner-cli -- options     # raw scanner options
```

Scans go to `./scans` by default.

## Destinations (Paperless-ngx, S3)

Scans are always saved locally first; they can also be sent to Paperless-ngx or any
S3-compatible storage. Copy `scanner.example.toml` to `scanner.toml` (git-ignored) and
edit it, or pass `--config <file>`. Secrets can be given as `env:VARIABLE`.

In the web UI, tick destinations under **Send to**, optionally with **Delete local copy
after successful upload** (only deletes once every selected destination accepted the
file). If an upload fails (destination down, wrong permissions, …), the web service keeps
the file and retries every hour (`--retry-minutes`), also across restarts; the UI
lists waiting uploads with a **Retry now** button. On the CLI (one-shot, no retries):

```sh
scanner destinations
scanner scan --to paperless --to archive --delete-after-upload
```

## Deploy to a Raspberry Pi / Debian host

```sh
deploy/build-linux-arm64.sh            # builds in a bookworm arm64 container (podman/docker)
deploy/install.sh user@host            # installs/updates the systemd service (port 8888)
```

On the host: binaries in `/opt/scanner`, scans in `/var/lib/scanner/scans`, config in
`/etc/scanner/scanner.toml`, secrets in `/etc/scanner/scanner.env` (root-only, e.g.
`PAPERLESS_TOKEN=…`). Re-running the installer keeps config and secrets.
Logs: `journalctl -u scanner-web -f`.

## Architecture

See [AGENTS.md](AGENTS.md).
