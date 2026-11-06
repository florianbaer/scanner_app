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

## Architecture

See [AGENTS.md](AGENTS.md).
