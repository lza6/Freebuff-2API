# Freebuff2API

> English version. 中文文档：[README.md](README.md)

Freebuff2API is a local proxy server that reverse-engineers the [Codebuff Freebuff](https://www.codebuff.com) free tier into **OpenAI-compatible** and **Claude-compatible** API endpoints. Run one Go binary and use Freebuff's free models from any OpenAI/Claude client, SDK, or CLI tool.

> The full reverse-engineering write-up (protocol, pitfalls, architecture) is in **[REVERSE_ENGINEERING.md](REVERSE_ENGINEERING.md)**.

## Features

- **Dual-protocol output** — `POST /v1/chat/completions` (OpenAI, streaming + non-streaming) and `POST /v1/messages` (Claude), works with LobeChat, NextChat, Claude Code, Codex, Cursor, and any OpenAI SDK.
- **Automatic run hierarchy** — manages the session → root run (`base2-free`) → subagent run tree required by upstream; subagent runs are created lazily with the root as their sole ancestor.
- **Session keep-alive** — refreshes the freebuff session before expiry, returns `Retry-After` while queued, cools down tokens on 401.
- **Multi-token rotation** — cycle through multiple auth tokens with periodic rotation and per-token concurrency leases.
- **Curated model registry** — a hardcoded list of combinations verified end-to-end against the live upstream, supplemented (never shrunk) by a periodic fetch of the upstream `free-agents.ts`.
- **HTTP proxy support** — route all outbound traffic through a configurable upstream proxy.

## Getting Auth Tokens

Freebuff2API requires one or more Freebuff **auth tokens**.

### Method 1 — Web (Recommended)

Visit **[https://freebuff.llm.pm](https://freebuff.llm.pm)**, log in with your Freebuff account, and copy the displayed auth token.

### Method 2 — Freebuff CLI

```bash
npm i -g freebuff
freebuff   # first launch guides you through login
```

After logging in, the token is saved locally:

| OS | Credentials Path |
|---|---|
| Windows | `C:\Users\<username>\.config\manicode\credentials.json` |
| Linux / macOS | `~/.config/manicode/credentials.json` |

Copy the `authToken` value from that file into **AUTH_TOKENS**.

> **Tip:** Configure tokens from multiple accounts for higher throughput.

## Configuration

Configuration is via a JSON file and/or environment variables (keys are identical). By default the app reads `config.json` from the working directory; use `-config` to point elsewhere. See `config.example.json`.

```json
{
  "LISTEN_ADDR": ":8080",
  "UPSTREAM_BASE_URL": "https://codebuff.com",
  "AUTH_TOKENS": ["token"],
  "ROTATION_INTERVAL": "6h",
  "REQUEST_TIMEOUT": "15m",
  "API_KEYS": [],
  "HTTP_PROXY": ""
}
```

| Key / Env Var | Description |
|---|---|
| `LISTEN_ADDR` | Proxy listen address (default `:8080`) |
| `UPSTREAM_BASE_URL` | Freebuff backend URL (default `https://codebuff.com`) |
| `AUTH_TOKENS` | Freebuff auth tokens (JSON array or comma-separated env var) |
| `ROTATION_INTERVAL` | Run rotation interval (default `6h`) |
| `REQUEST_TIMEOUT` | Upstream request timeout (default `15m`) |
| `API_KEYS` | Client API keys for proxy auth (empty = open access) |
| `HTTP_PROXY` | HTTP proxy for outbound requests |

Environment variables override JSON values when both are set.

## Available Models & Upstream Limits

The usable model list is **not** everything Freebuff advertises — the upstream free tier currently rejects most models at request time. The table below reflects what is **actually accepted**, verified by end-to-end tests against the live upstream (2026-07).

| Model | Status |
|---|---|
| `google/gemini-2.5-flash-lite` | ✅ Working (chat, streaming, Claude protocol) |
| `google/gemini-3.1-flash-lite-preview` | ✅ Working |
| `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | ❌ Rejected (`free_mode_invalid_agent_model`) |
| `minimax/minimax-m3`, `z-ai/glm-v5.2`, `moonshotai/kimi-k2-thinking` | ❌ Rejected |
| `xiaomi/mimo-v2.5-flash/pro`, `hy3/hy3*`, `poolside/laguna-s-2-1*` | ❌ Rejected |

**Why the gap:** Codebuff tightened enforcement on the free tier. Their open-source `free-agents.ts` still lists many models, but the backend now only honors specific agent+model combinations; non-Gemini models return:

```json
{"error":"free_mode_invalid_agent_model","message":"Free mode is only available for specific agent and model combinations."}
```

The model registry therefore ships a curated hardcoded list (in `models.go`) as the authoritative baseline. It still fetches upstream `free-agents.ts` periodically as a supplement, so upstream source refactors (e.g. the switch to `FREEBUFF_*_MODEL_ID` constant references that a regex cannot resolve) never shrink the available list.

### Run Hierarchy (why it matters)

Freebuff enforces a session-rooted run tree:

1. A **session** must exist (`POST /api/v1/freebuff/session`).
2. A **root run** (`base2-free`) must be started under it.
3. Any **subagent run** (e.g. `file-picker`, `code-reviewer-*`) must declare the root run in `ancestorRunIds` — and only the root.

Violating this returns `free_mode_invalid_agent_hierarchy`. Freebuff2API handles it automatically: one root run per token is kept alive and rotated on schedule; subagent runs are created lazily on first use with the root as their sole ancestor.

## Usage

```bash
# OpenAI
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"google/gemini-2.5-flash-lite","messages":[{"role":"user","content":"你好"}]}'

# Claude
curl http://localhost:8080/v1/messages \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"google/gemini-2.5-flash-lite","max_tokens":1024,"messages":[{"role":"user","content":"你好"}]}'

# Model list
curl http://localhost:8080/v1/models
```

Point any OpenAI SDK at `http://localhost:8080/v1`, or set `ANTHROPIC_BASE_URL=http://localhost:8080` for Claude Code.

## Deployment

### Build from Source

**Requirements:** Go 1.23+

```bash
git clone https://github.com/lza6/Freebuff-2API.git
cd Freebuff-2API
go build -o freebuff2api .
./freebuff2api -config config.json
```

### Docker

```bash
docker build -t freebuff2api .
docker run -d -p 8080:8080 -e AUTH_TOKENS="token1,token2" freebuff2api
```

> A GitHub Actions workflow (`.github/workflows/docker.yml`) builds multi-arch images on push. Update the `IMAGE_NAME` env var there to your own GHCR namespace before relying on published images.

## Cloudflare Worker (experimental, currently blocked)

The `cfworker/` directory contains a Cloudflare Worker port. It is **currently non-functional against the live upstream**: Codebuff rejects Worker-originated requests with `free_mode_cli_required`, a TLS-fingerprint-level check (Client Hello / JA3) that cannot be bypassed by changing HTTP headers. The local Go binary's native TLS stack passes. Use the Go server; `cfworker/` is kept for research only.

## Links

- [linux.do](https://linux.do)

## Disclaimer

This project has no official affiliation with OpenAI, Codebuff, or Freebuff. All related trademarks and copyrights belong to their respective owners.

All contents within this repository are provided solely for communication, experimentation, and learning, and do not constitute production-ready services or professional advice. This project is provided on an "As-Is" basis, and users must use it at their own risk. The author assumes no liability for any direct or indirect damages resulting from the use, modification, or distribution of this project, nor provides any warranties of any kind, express or implied.

## License

MIT
