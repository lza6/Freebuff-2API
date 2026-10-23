# Freebuff2API

> English version. 中文文档：[README.md](README.md)

> ## ⚠️ Status (verified 2026-10-23)
>
> **Chat forwarding is currently non-functional.** Upstream deprecated the web transport
> (`409 chat_moved`) and put the bearer transport behind a client-version gate plus a
> CLI-only policy (`403 free_mode_cli_required`, with an explicit warning that calling the
> API directly may get your account banned).
>
> Full reverse-engineering record: [`docs/UPSTREAM_PROTOCOL_2026-10.md`](docs/UPSTREAM_PROTOCOL_2026-10.md).
> **Still working**: dashboard, credential management, account health, usage/telemetry,
> doctor, import/export, skills & memory layers. Details in [README_zh.md](README_zh.md).

Freebuff2API reverse-engineers the [Freebuff](https://freebuff.com) free tier into **OpenAI-compatible** and **Anthropic-compatible** local API endpoints. Implemented in **Rust (axum)** — single binary, zero runtime deps — usable from Claude Code, Codex, Cursor, LobeChat, or any OpenAI SDK.

## Features

- Dual protocol: `POST /v1/chat/completions` (OpenAI) + `POST /v1/messages` (Claude)
- Multi-account rotation: health-scored pool with cooldown/circuit-breaking
- Dual-bucket concurrency semaphore (reverse-engineered from desktop, implemented in v0.8): free `{slot:1, concurrency:3}`, subscriber `{slot:3, concurrency:8}` — gateway-global. Each request takes 1 slot + 1 concurrent permit → real concurrency cap = slot capacity (free 1, subscriber 3), 2s timeout → 429
- Session keepalive: 45s heartbeat + ad-based quota refresh
- Reasoning-effort downgrade (from upstream efforts field)
- Balance/quota query: `GET /api/account/balance` (freebucks, per-model daily remaining)
- One-click credential import: curl / HAR / Cookie
- Web protocol adapter: Cookie-auth SSE chat, multimodal upload, tool-call mapping
- SQLite usage stats + built-in dashboard (`/ui`)
- Electron desktop shell + Docker + GitHub Actions CI/CD

## Quick Start

### Desktop (recommended)
Download the latest `Freebuff2API Setup x64.exe` from Releases (currently v0.9.x) → install → launch → gateway auto-starts → dashboard opens. Use tray "Login new account" to auto-capture cookies.

### Source
```bash
cargo build --release
./target/release/freebuff2api --config config.json
```

### Docker
```bash
docker build -t freebuff2api -f docker/Dockerfile .
docker run -d -p 47821:47821 -v /data:/data freebuff2api
```

## API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v1/chat/completions` | POST | OpenAI chat |
| `/v1/messages` | POST | Claude chat |
| `/v1/models` | GET | Model list |
| `/api/tokens/import` | POST | Import curl/HAR/Cookie |
| `/api/account/balance` | GET | Credits / per-model quota |
| `/api/account/detail` | POST | Account detail card |
| `/api/usage/*` | GET | Usage stats |
| `/ui` | GET | Dashboard |
| `/healthz` | GET | Health check |

Full guide: [docs/API_GUIDE.md](docs/API_GUIDE.md)

## License
MIT
