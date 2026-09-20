# Official Source Registry

Status: ACCEPTED
Review state: ACTIVE
Last checked: 2026-09-20

This is a discovery map, not a substitute for an integration evidence note.
URLs and APIs can change. A coding agent must fetch the current index/page again
at implementation time and record the exact version used.

`VERIFIED INDEX` means the URL returned a usable official AI-readable index on
the last-checked date. `NO INDEX AT TESTED PATH` means official docs exist but
the tested product `/llms.txt` URL returned 404; try current site discovery
again before concluding none exists.

## Protocols and Agent Runtimes

| System | AI-readable docs | Normative/source entry | Status and implementation note |
| --- | --- | --- | --- |
| Model Context Protocol | https://modelcontextprotocol.io/llms.txt | https://modelcontextprotocol.io/specification/latest and https://github.com/modelcontextprotocol/specification | VERIFIED INDEX; select an exact dated spec, never `latest`, in code |
| Official MCP Rust SDK | Main repository README is current feature index | https://github.com/modelcontextprotocol/rust-sdk | Official SDK; verify pinned release features/conformance before use |
| OpenClaw | https://docs.openclaw.ai/llms.txt | https://github.com/openclaw/openclaw | VERIFIED INDEX; append `.md` or request Markdown for focused pages |
| Goose | https://goose-docs.ai/llms.txt | https://github.com/aaif-goose/goose | VERIFIED INDEX; repository moved from `block/goose`; verify license from repository LICENSE, not index summary |
| OpenAI Codex | https://developers.openai.com/codex/llms.txt | https://github.com/openai/codex | VERIFIED parent index; use app-server/protocol docs for adapter work |
| OpenAI Agents SDK (Python) | https://openai.github.io/openai-agents-python/llms.txt | https://github.com/openai/openai-agents-python | VERIFIED INDEX; external runtime only, not core dependency |
| LangGraph | https://docs.langchain.com/oss/python/langgraph/llms.txt | https://github.com/langchain-ai/langgraph | VERIFIED section index; external Python runtime |
| Temporal | https://docs.temporal.io/llms.txt and https://docs.temporal.io/develop/rust/llms.txt | https://github.com/temporalio | VERIFIED INDEX; optional future workflow backend |

## Model and Local Inference Providers

| System | AI-readable docs | API/source entry | Status and implementation note |
| --- | --- | --- | --- |
| OpenAI API | https://developers.openai.com/api/llms.txt | https://developers.openai.com/api/ | VERIFIED via parent https://developers.openai.com/llms.txt; Responses, Realtime, tools, auth, limits, changelog must be re-read |
| Anthropic/Claude | https://platform.claude.com/docs/llms.txt | https://platform.claude.com/docs/en/api/overview.md | VERIFIED INDEX; no official Rust SDK found in reviewed SDK list, so do not assume one |
| Gemini API | https://ai.google.dev/gemini-api/docs/llms.txt | https://ai.google.dev/api | VERIFIED INDEX; current docs distinguish recommended Interactions API from legacy generateContent |
| Ollama | https://docs.ollama.com/llms.txt | https://docs.ollama.com/openapi.yaml | VERIFIED INDEX and OpenAPI; verify local versus cloud auth separately |

## Voice and Desktop

| System | AI-readable docs | Schema/source entry | Status and implementation note |
| --- | --- | --- | --- |
| ElevenLabs | https://elevenlabs.io/docs/llms.txt | https://elevenlabs.io/docs/openapi.json and https://elevenlabs.io/docs/asyncapi.json | VERIFIED INDEX plus OpenAPI/AsyncAPI; individual pages support `.md` |
| Tauri v2 | https://tauri.app/llms.txt | https://github.com/tauri-apps/tauri and https://v2.tauri.app/ | VERIFIED INDEX; use guides/reference split and native CI |

## Connectors

| System | AI-readable docs | Schema/source entry | Status and implementation note |
| --- | --- | --- | --- |
| GitHub | https://docs.github.com/llms.txt | https://github.com/github/rest-api-description and https://github.com/github/github-mcp-server | VERIFIED INDEX; docs provide Article/Page List/Search APIs |
| Gmail/Google Workspace | Tested https://developers.google.com/workspace/gmail/api/llms.txt | https://developers.google.com/workspace/gmail/api/guides and REST reference | NO INDEX AT TESTED PATH; use official guides/reference/release notes and generated discovery schema |
| Microsoft Graph | Tested https://learn.microsoft.com/en-us/graph/llms.txt | https://learn.microsoft.com/en-us/graph/overview and https://github.com/microsoftgraph/microsoft-graph-docs-contrib | NO INDEX AT TESTED PATH; use versioned `v1.0` docs, metadata/OpenAPI where available, auth/change notification docs |
| Home Assistant | Tested https://developers.home-assistant.io/llms.txt | https://developers.home-assistant.io/docs/ and https://github.com/home-assistant/core | NO INDEX AT TESTED PATH; integration and quality-scale guidance is authoritative |

## Persistence and Rust Infrastructure

These sources did not receive a full integration review during documentation
bootstrap. Refresh them before selecting versions.

| System | Official entry | Required review |
| --- | --- | --- |
| Rust | https://doc.rust-lang.org/ and https://blog.rust-lang.org/ | Supported toolchain/targets and release notes |
| Tokio | https://tokio.rs/ and https://docs.rs/tokio | Runtime, cancellation, process/signal behavior |
| Axum/Tower | https://docs.rs/axum and https://docs.rs/tower | Server limits, graceful shutdown, middleware |
| SQLx | https://github.com/launchbadge/sqlx and https://docs.rs/sqlx | SQLite/Postgres features, migrations, offline metadata |
| SQLite | https://sqlite.org/docs.html | WAL, locking, backup, integrity, FTS5, platform behavior |
| PostgreSQL | https://www.postgresql.org/docs/ | Supported server versions, locks, RLS, backup, JSON/search |
| pgvector | https://github.com/pgvector/pgvector | Extension/version/index/recall and migration behavior |
| OpenTelemetry Rust | https://github.com/open-telemetry/opentelemetry-rust | SDK maturity, exporters, shutdown and sensitive data |

## Standards

Use normative RFCs/specifications for OAuth 2.1-era behavior, PKCE, protected
resource metadata, webhook signature primitives, JSON Schema 2020-12, JSON
Canonicalization Scheme (RFC 8785), UUID formats, RFC 3339 time, W3C Trace
Context, TLS, and content/media types. Provider guides can narrow or extend a
standard but do not replace its security requirements.

## Refresh Procedure

1. Fetch the current `llms.txt` or official docs index.
2. Follow only pages relevant to the proposed feature.
3. Check exact pinned SDK/API/protocol version and release notes.
4. Update the integration evidence note and this registry only if the discovery
   entry itself changed.
5. Record a failed/moved/redirected URL; do not silently replace provenance.