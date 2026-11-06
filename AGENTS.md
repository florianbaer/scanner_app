# scanner_app — agent guide

Rust app for scanning over the network from an Epson FastFoto FF-680W (and any other
SANE-supported scanner): a CLI and a web UI for discovering scanners, starting scans and
downloading the PDFs.

## Always think about good software architecture

Every change should keep the codebase clean, layered, and easy to extend. Before writing
code, decide where it belongs and what it depends on. If a change doesn't fit the
structure, fix the structure first. Don't bolt the change on.

### Principles

- **Layered, dependencies point inward.** Front-ends (CLI, web) depend on the core library.
  The core never depends on a front-end. Infrastructure (SANE FFI, mDNS, file system)
  sits behind small interfaces that the core owns.
- **Thin front-ends.** `main.rs` and the web handlers only parse input, call the core, and
  present results. There is no scanning, image, or PDF logic in them. Anything the CLI
  can do, the web UI must be able to reuse without copying code.
- **Domain types over primitives.** Use `ScanSettings`, `ColorMode`, `Source`, `Page`, and
  similar types, not loose strings and ints. Translate to backend-specific strings
  (e.g. `"ADF Duplex"`) at the edge, in the SANE adapter only.
- **Traits at boundaries.** Use a trait wherever there could plausibly be a second
  implementation or a test double: the scanner backend, device discovery, and output
  sinks (PDF, JPEG, PNG). Don't add traits for things that will only ever have one
  implementation.
- **Isolate `unsafe`.** All FFI lives in the `sane` module. It has a small `ffi` part
  (raw declarations) and a safe wrapper. No `unsafe` anywhere else.
- **Single responsibility, small modules.** One concept per file. Split a module when it
  grows past one clear job.
- **Errors.** Library code returns typed errors or `anyhow::Result` with context, and it
  never `panic!`s, `unwrap()`s, or `process::exit`s. Only the front-end decides how to
  report errors. Expected conditions, like an empty feeder, are values, not crashes.
- **Explicit resource lifetimes.** SANE handles are RAII types (`Drop` cancels and
  closes). Long-running or concurrent use, such as the web server, must go through one
  owner of the device (e.g. a scan job queue/actor), because the scanner can only do one
  job at a time.
- **Testable core.** Pure logic (pixel conversion, PDF writing, settings validation,
  file naming) must be testable without hardware. Add unit tests for it.
- **No hidden config.** Device names, ports, and output paths come from CLI flags,
  environment variables, or discovery. Never hard-code them, and never require editing
  system config files.

### Components

A Cargo workspace; crate boundaries enforce the dependency direction.

```
crates/
  scanner-core   domain model + Backend/Scanner traits, raster decoding, output
                 (PDF/JPEG/PNG + file library), discovery merging, and service/:
                 the scan service = JobStore + single Worker thread (job queue).
                 No SANE, no HTTP. Unit-tested with a fake backend.
  scanner-sane   SANE FFI (the only `unsafe` code), SANE adapter, Epson mDNS discovery.
                 Implements core's `Backend`. Only crate that links libsane.
  scanner-destinations
                 Where finished files go besides the local folder: Paperless-ngx, S3.
                 Implements core's `Destination`; TOML config with `env:VAR` secrets.
  scanner-cli    `scanner` binary. Thin front-end.
  scanner-web    `scanner-web` binary: axum JSON API + embedded static UI. Thin front-end.
```

Dependency rule: `cli`/`web` → `sane`, `destinations` → `core`. Core depends on
nothing above it. Front-ends compose backend + destinations and hand them to core.

Destinations: files are always written locally first. Only completed batches are
delivered; one failing destination never blocks the others. "Delete after upload"
removes a local file only if *every* selected destination accepted it
(`destination::deliver_files`, shared by worker and CLI). Secrets stay server-side:
the API exposes only destination ids, labels and kinds.

Retries: failed uploads (any destination) go to the outbox
(`service/outbox.rs`, persisted as `.pending-uploads.json` in the scans folder, so
retries survive restarts). A timer asks the worker to retry due entries (default
hourly); delete-after-upload waits until no upload for that file is pending.

Scan service: one worker thread owns the `Backend`; scans, discovery and capability
queries are commands on an mpsc channel, so device access is serialised. Job state
lives in the shared `JobStore`, which front-ends read without waiting on the worker.
Blocking service calls must run on a blocking thread in async code (`spawn_blocking`).

### Workflow

- Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` before calling work done.
- Prefer few, well-known dependencies. Explain why in the commit message when adding one.
- When unsure where something goes, write down the trade-off briefly in the PR or reply
  instead of guessing quietly.

## Environment notes

- Needs `sane-backends` (Homebrew: `brew install sane-backends`). It is linked via pkg-config.
- The FF-680W uses SANE's `epsonds` backend over the network (TCP 1865). Its device name
  is `epsonds:net:<ip>`. It advertises itself over Bonjour as `_scanner._tcp`. SANE's own
  `net autodiscovery` did not find it on this network, so discovery needs an mDNS fallback.
