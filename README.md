# ProxyMan ⚡

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)
[![Edition](https://img.shields.io/badge/Edition-2021-purple.svg)](Cargo.toml)

**ProxyMan** is a high-performance, resilient multi-proxy and DNS service supervisor and manager built in Rust. It offers process lifecycle supervision, leftover process adoption, automated health checking with exponential backoff, an interactive real-time Web console, and an ergonomic CLI (`proxyman`).

---

## ✨ Features

- 🚀 **Multi-Driver Proxy & DNS Orchestration**:
  - **OverTLS** (`overtls`): High-performance TLS-obfuscated proxy tunnel client.
  - **OverTLS Chain** (`overtls-chain`): Multi-hop chained OverTLS proxy routing.
  - **Clean DNS** (`clean-dns`): Anti-pollution DNS forwarding and filtering daemon (supports YAML auto-generation or existing external config files).
  - **GOST** (`gost`): Multi-protocol proxy forwarder (HTTP, SOCKS5, Shadowsocks, Relay chains).
  - **Custom** (`custom`): Supervise any arbitrary command-line daemon or network process.

- 🛡️ **Intelligent Process Supervision & Leftover Adoption**:
  - **Zero-Disruption Process Adoption**: When starting up or restarting, ProxyMan scans for existing processes already running and listening on target ports (TCP/UDP). If a matching process is detected, it attaches to the existing PID seamlessly without interrupting active network connections.
  - **Port Conflict Protection**: Refuses to start if a foreign/unrelated process occupies target ports, displaying clear and actionable diagnostics.
  - **Configurable Restart Policies**: Supports `always`, `on_failure`, and `never`.
  - **Auto-Restart Rate Limiting**: Configure maximum retry limits (`max_restart_retries`, where `0` means infinite loop restart) and exponential/custom backoff delays (`restart_backoff_secs`).

- 🌐 **Modern Web Dashboard**:
  - Built-in reactive single-page dashboard served directly by the Axum server (no external runtime or bundler required).
  - Real-time updates over WebSockets (`/api/events/ws`).
  - Live log streaming with tail and autoscroll (`/api/services/{id}/logs/ws`).
  - Interactive latency probe testing with visual metrics.
  - Modals for adding, configuring, restarting, and deleting services.

- 💻 **Ergonomic CLI (`proxyman`)**:
  - Unicode formatted terminal tables with colorized health status, memory usage, uptime, and latency.
  - Commands for starting, stopping, restarting individual services or `all` at once.
  - Live log tailing in terminal (`proxyman logs -f <id>`).
  - Offline fallback: Read configured services directly from local storage even when the daemon is stopped.

- 📡 **Developer-Friendly REST & WebSocket API**:
  - Full HTTP REST API for seamless automation and integration into shell scripts, CI/CD pipelines, or external dashboards.

---

## 🏗️ Architecture

```
proxy-man/
├── Cargo.toml               # Cargo workspace configuration
├── src-core/                # Shared core engine library (proxy-man-core)
│   ├── config/              # Service models, JSON configuration store, and runtime persistence
│   ├── drivers/             # Service drivers (overtls, clean-dns, gost, custom)
│   ├── supervisor/          # Process supervisor, watchdog, health probe, and PID adoption engine
│   ├── server/              # Axum HTTP REST server, WebSocket handlers & static UI server
│   └── tests/               # Core integration and regression test suite
├── src-cli/                 # Command-line interface binary (proxyman)
│   └── src/main.rs          # CLI argument parsing, colored formatting, and IPC client
└── ui/                      # Web console frontend (HTML5 / Vanilla CSS / Modern JS)
    ├── index.html           # Dashboard interface
    ├── style.css            # Dark glassmorphism theme and typography
    └── app.js               # Reactive WebSocket client, card rendering, and modal controllers
```

---

## 🚀 Quick Start

### 1. Prerequisites

- [Rust toolchain](https://rustup.rs/) (Rust 1.80+ recommended, 2021 edition)
- Binary dependencies for the services you plan to manage (e.g. `overtls`, `clean-dns`, `gost`) installed in your `$PATH` or specified via `--bin-path`.

### 2. Build from Source

Clone the repository and compile the workspace:

```bash
git clone https://github.com/tigerzhang/proxy-man.git
cd proxy-man

# Build debug binaries
cargo build

# Or compile release binaries
cargo build --release
```

The compiled CLI binary will be located at:
```bash
./target/release/proxyman
```

Optionally install it into your Cargo bin path (`~/.cargo/bin`):

```bash
cargo install --path src-cli
```

---

## 📖 Usage Guide

### Starting the Daemon & Web Dashboard

To run the supervisor daemon in the foreground:

```bash
proxyman daemon
```

By default, the daemon binds to `http://127.0.0.1:8920` and serves the static Web UI.

To launch the daemon and automatically open the Web dashboard in your default browser:

```bash
proxyman web
```

To specify a custom port or address:

```bash
proxyman daemon --bind 0.0.0.0:9000 --ui-dir ./ui
```

---

### Managing Services via CLI

#### 1. Check Service Status
Display a colorized table of all registered services, their runtime state, PID, memory, uptime, and latency:

```bash
proxyman status
# or shorthand:
proxyman ps
```

To get machine-readable output:
```bash
proxyman status --json
```

#### 2. Adding a New Service

##### OverTLS
```bash
proxyman add \
  --id overtls-us \
  --name "US East OverTLS" \
  --service-type overtls \
  --listen-host 127.0.0.1 \
  --listen-port 1080 \
  --server-host "vpn.example.com" \
  --server-port 443 \
  --password "YourSecretPassword" \
  --tunnel-path "/secret-tunnel-path/"
```

##### Clean DNS
```bash
# Using auto-generated configuration
proxyman add \
  --id dns-main \
  --name "Anti-Pollution DNS" \
  --service-type clean-dns \
  --listen-host 127.0.0.1 \
  --listen-port 5353

# Or pointing to an existing clean-dns YAML config file
proxyman add \
  --id dns-custom \
  --name "Custom Clean DNS" \
  --service-type clean-dns \
  --listen-host 127.0.0.1 \
  --listen-port 5353 \
  --config-path /path/to/clean-dns.yaml
```

##### GOST (GO Simple Tunnel)
```bash
proxyman add \
  --id gost-forwarder \
  --name "GOST HTTP Forwarder" \
  --service-type gost \
  --listen-host 127.0.0.1 \
  --listen-port 8080 \
  --gost-listen "http://:8080" \
  --gost-forward "socks5://127.0.0.1:1080"
```

##### Custom Command
```bash
proxyman add \
  --id custom-worker \
  --name "Custom Proxy Script" \
  --service-type custom \
  --listen-host 127.0.0.1 \
  --listen-port 9999 \
  --custom-cmd "/usr/local/bin/my-proxy" \
  --custom-args "--config" "/etc/my-proxy.conf"
```

#### 3. Service Lifecycle Controls

```bash
# Start a service
proxyman start overtls-us

# Start all configured services
proxyman start all

# Stop a service
proxyman stop overtls-us

# Stop all services
proxyman stop all

# Restart a service
proxyman restart overtls-us

# Restart all services
proxyman restart all
```

#### 4. Viewing Logs

```bash
# View last 50 lines of logs
proxyman logs overtls-us -n 50

# Follow live log stream in real time (via WebSocket)
proxyman logs overtls-us -f
```

#### 5. Probing Latency & Health

Run an immediate connectivity and latency test:

```bash
proxyman test overtls-us
```

#### 6. Removing a Service

```bash
proxyman remove overtls-us
# or shorthand:
proxyman rm overtls-us
```

---

## ⚙️ Configuration & Storage

ProxyMan maintains its configurations and runtime state in the user's standard application config directory:

- **Linux / macOS**: `~/.config/proxy-man/` (or `$XDG_CONFIG_HOME/proxy-man/`)
- **Windows**: `%APPDATA%\proxy-man\`

Directory layout:

| Path | Description |
|---|---|
| `services.json` | Persisted configuration of all registered services. |
| `runtime.json` | Runtime state across supervisor restarts (PIDs, restart counts, timestamps). |
| `configs/` | Generated configuration files for sub-drivers (e.g., OverTLS client JSONs). |
| `logs/` | Service stdout/stderr log files (`<service-id>.log`). |

---

## 🔌 HTTP REST & WebSocket API

The daemon provides a full JSON API on `http://127.0.0.1:8920`:

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/health` | Daemon health check and version info. |
| `GET` | `/api/services` | List all configured services and their live runtime states. |
| `POST` | `/api/services` | Register and persist a new service instance. |
| `GET` | `/api/services/{id}` | Get configuration and runtime state for a specific service. |
| `PUT` | `/api/services/{id}` | Update an existing service configuration. |
| `DELETE`| `/api/services/{id}` | Stop and remove a service. |
| `POST` | `/api/services/{id}/start` | Start service (or adopt running leftover process). |
| `POST` | `/api/services/{id}/stop` | Gracefully terminate a service. |
| `POST` | `/api/services/{id}/restart` | Restart a service. |
| `POST` | `/api/services/{id}/probe` | Run an immediate latency & connectivity test. |
| `GET` | `/api/services/{id}/logs?limit=N` | Fetch recent log lines. |
| `WS` | `/api/services/{id}/logs/ws` | WebSocket stream for live log tailing. |
| `WS` | `/api/events/ws` | WebSocket stream for global supervisor events. |

---

## 🧪 Testing

ProxyMan includes extensive automated unit and integration tests covering drivers, supervisor mechanics, adoption logic, and configuration stores:

```bash
# Run all tests
cargo test

# Run tests with output printed
cargo test -- --nocapture
```

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
