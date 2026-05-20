# Supern — MCP SaaS for reMarkable (closed source)

Hosted entirely on DigitalOcean at **supern.space**. A multi-tenant SaaS
that lets agents — local (Claude Code, Cursor) **and remote (claude.ai,
ChatGPT, any web host)** — push to and read from a user's reMarkable
account through MCP. The service holds per-user reMarkable tokens,
caches their document tree, and exposes that tree as MCP tools and
resources.

---

## 1. Goals

- **Web-reachable MCP.** Anchor URL: `https://mcp.supern.space`. Agents
  on claude.ai, ChatGPT, Cursor (any MCP host) connect with per-user
  bearer tokens.
- **Read + write.** Push markdown as native reMarkable notebooks (via
  the `rr` crate). Read existing notebooks back as markdown for agents
  to retrieve, search, summarise.
- **Server-side cache.** Each user's document tree cached in Postgres
  with FTS5-style search; agents look up by name in <100 ms without
  paying a sync-API round-trip.
- **Multi-tenant from day one.** Strong isolation, per-tenant quotas
  and rate limits.
- **Closed source.** Separate private repo. The open-source `rr` crate
  is a regular dependency.

## 2. Non-goals (for v1)

- Self-hosted distribution. We don't ship a deployable binary; users
  sign up at supern.space.
- Real-time device-side push notifications.
- Handwriting recognition. We expose typed-text only; OCR of strokes
  is v2.
- Multiple reMarkable accounts per Supern user — one account each.

## 3. Stack

Everything on DigitalOcean. One vendor for compute, database, object
storage, DNS, TLS, and managed backups; one bill, one dashboard, one
set of API tokens.

| Layer            | Choice                                                                                | Why                                                |
|------------------|---------------------------------------------------------------------------------------|----------------------------------------------------|
| Backend language | Rust                                                                                  | Direct `rr` crate dep, zero FFI                    |
| HTTP / MCP       | `axum` + [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk)                   | Anthropic's official Rust MCP SDK                  |
| Database         | **DO Managed PostgreSQL** (Basic, AMS3)                                               | Daily backups + PITR; one node fine to start      |
| Search           | Postgres `tsvector` + GIN index                                                       | Built into PG, no extra service                    |
| Blob cache       | **DO Spaces** (S3-compatible) + Spaces CDN                                            | Content-addressed by SHA-256; same bill            |
| Frontend         | Next.js 14 (App Router), deployed on DO App Platform                                  | Marketing + dashboard share one codebase           |
| Auth (humans)    | WorkOS                                                                                | Email + OAuth, GDPR-clean, cheap free tier         |
| Auth (MCP)       | Per-user opaque bearer tokens                                                         | Hashed at rest with argon2id, revocable            |
| Billing          | Stripe (post-launch; free tier in v1)                                                 | Standard                                           |
| Token at rest    | `chacha20poly1305`, key in App Platform env / Droplet env                              | Pure-Rust, deterministic                           |
| Hosting          | **DO App Platform**, primary region `ams3` (EU users skew heaviest); add `nyc3` later | Managed PaaS; auto-builds + zero-downtime deploys  |
| DNS              | **DO Domains** (free DNS hosting; nameservers `ns1/ns2/ns3.digitalocean.com`)         | Domain itself is bought elsewhere (Porkbun, etc.)  |
| TLS              | Let's Encrypt via App Platform (auto-renews)                                          | No manual certificate management                   |
| Edge / WAF       | App Platform's edge proxy + per-route rate-limit middleware                           | Good enough for v1; add Cloudflare in front if needed |
| Observability    | DO Monitoring (metrics + alerts) + Sentry (errors)                                    | Stays inside one vendor where possible             |
| CI               | GitHub Actions (private repo) → `doctl apps create-deployment`                        | Same flow as `rr`; deploy from CI                  |

## 4. Public surface

| Domain                    | Purpose                                            |
|---------------------------|----------------------------------------------------|
| `supern.space`            | Marketing site, signup, pricing                    |
| `app.supern.space`        | Logged-in dashboard (MCP key, pairing, settings)   |
| `mcp.supern.space`        | **The MCP endpoint** agents connect to             |
| `api.supern.space`        | Internal REST for dashboard ↔ backend (optional)   |
| `docs.supern.space`       | Developer docs, agent integration guides           |

## 5. MCP surface (v1)

### Tools

| Tool                    | Args                                | Returns                                |
|-------------------------|-------------------------------------|----------------------------------------|
| `push_markdown`         | `markdown`, `title?`, `device?`     | `{ doc_id, page_count, url }`          |
| `list_documents`        | `folder?`, `refresh?`               | `[{ uuid, name, type, modified }, …]`  |
| `search_documents`      | `query`, `limit?`                   | top-N hits with snippet                |
| `get_document_markdown` | `uuid_or_name`                      | markdown string + metadata             |
| `create_folder`         | `name`, `parent?`                   | `{ uuid }`                             |
| `delete_document`       | `uuid_or_name`                      | `{ deleted: true }`                    |
| `refresh_cache`         | (none)                              | `{ docs_indexed }`                     |
| `whoami`                | (none)                              | account email + quota state            |

### Resources

| URI pattern                                | Content                       |
|--------------------------------------------|-------------------------------|
| `rm://documents`                           | cached document list (JSON)   |
| `rm://documents/{uuid}`                    | document metadata             |
| `rm://documents/{uuid}/markdown`           | decoded markdown text          |
| `rm://documents/{uuid}/pages/{n}/markdown` | markdown of single page       |

## 6. Database (Postgres)

```sql
-- humans
CREATE TABLE users (
  id              UUID PRIMARY KEY,
  email           CITEXT UNIQUE NOT NULL,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  stripe_id       TEXT,
  plan            TEXT NOT NULL DEFAULT 'free'
);

-- linked reMarkable accounts; 1:1 with users in v1
CREATE TABLE rm_accounts (
  user_id              UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  rm_email             TEXT NOT NULL,
  rm_user_token_enc    BYTEA NOT NULL,    -- chacha20poly1305
  rm_device_token_enc  BYTEA NOT NULL,
  tectonic             TEXT,
  last_refreshed_at    TIMESTAMPTZ
);

-- per-user MCP credentials
CREATE TABLE mcp_keys (
  id              UUID PRIMARY KEY,
  user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  hash            BYTEA NOT NULL,        -- argon2id of the secret
  prefix          TEXT NOT NULL,         -- shown in dashboard ("sk_lp_…")
  label           TEXT,                  -- "claude.ai", "cursor laptop"
  last_used_at    TIMESTAMPTZ,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  revoked_at      TIMESTAMPTZ
);

-- the document cache: one row per (user, doc)
CREATE TABLE documents (
  user_id           UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  uuid              TEXT NOT NULL,
  visible_name      TEXT NOT NULL,
  doc_type          TEXT NOT NULL,        -- 'notebook'|'pdf'|'folder'
  parent_uuid       TEXT,
  last_modified     TIMESTAMPTZ,
  content_hash      TEXT,                 -- sync v3 blob hash
  raw_content_json  JSONB,                -- the doc's .content JSON
  search_tsv        TSVECTOR
                    GENERATED ALWAYS AS (to_tsvector('simple', visible_name)) STORED,
  PRIMARY KEY (user_id, uuid)
);
CREATE INDEX docs_user_search ON documents USING gin (search_tsv);
CREATE INDEX docs_user_modified ON documents (user_id, last_modified DESC);

-- per-tenant quota + rate-limit counters
CREATE TABLE quotas (
  user_id       UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  bytes_used    BIGINT NOT NULL DEFAULT 0,
  docs_count    INT NOT NULL DEFAULT 0,
  resets_at     TIMESTAMPTZ
);

-- audit trail (we touch user tokens; this is non-negotiable)
CREATE TABLE audit (
  id            BIGSERIAL PRIMARY KEY,
  user_id       UUID NOT NULL,
  at            TIMESTAMPTZ NOT NULL DEFAULT now(),
  action        TEXT NOT NULL,            -- 'push'|'list'|'pair'|'revoke_key'…
  metadata      JSONB
);
```

**Spaces layout** for cached `.rm` and `.png` blobs (DO Spaces exposes
an S3-compatible API; bucket lives in the same `ams3` region as the DB,
with Spaces CDN in front so blob reads can edge-cache globally):

```
https://supern-blobs.ams3.digitaloceanspaces.com/<sha256-hex>
                                                ↑ content-addressed, immutable
```

## 7. Onboarding flow

1. User visits `supern.space`, clicks **Sign up**, completes
   email + OAuth via WorkOS/Clerk.
2. Dashboard shows **"Pair your reMarkable"**: user clicks, a popup
   walks the standard reMarkable device-pair (one-time code from
   <my.remarkable.com/device/desktop/connect>), tokens encrypted and
   stored in `rm_accounts`.
3. Dashboard shows **MCP endpoint** + **Generate API key** button.
4. User copies one of three onboarding snippets:
   - **claude.ai Custom Connector** (paste URL + key)
   - **Cursor `~/.cursor/mcp.json` block**
   - **Claude Code `claude mcp add` command**
5. From inside any of those, the agent now sees Supernote's tools.

First-push completes in <60 s end-to-end from signup.

## 8. Auth & token security

- MCP requests carry `Authorization: Bearer sk_<env>_<random>`. Server
  splits on `_`, looks up by prefix, verifies the rest with argon2id.
- ReMarkable tokens encrypted with chacha20poly1305; KEK lives in
  Fly secrets, never in the database, never in logs.
- Token rotation: server auto-refreshes the user token via the device
  token when JWT expires (the same flow `rr` already implements).
- `revoke_key` immediately invalidates the bearer and writes an audit
  row.

## 9. Rate limits & quotas (free tier first)

| Limit                  | Free  | Pro (paid v1.1)    |
|------------------------|-------|--------------------|
| Pushes per day         | 50    | 1,000              |
| Cached documents       | 500   | unlimited          |
| Blob cache size        | 50 MB | 5 GB               |
| MCP requests / minute  | 30    | 300                |
| Search queries / min   | 60    | 600                |

Enforced via a Redis-or-Postgres token bucket per `user_id`.

## 10. Privacy & compliance

- **GDPR** is the binding constraint (reMarkable accounts skew EU; the
  `tectonic` JWT claim names "eu" frequently). Host primary region in
  `ams` (Fly), data residency message on the privacy page.
- **Data we hold**: email + encrypted reMarkable tokens + the cache
  (which is *just* document metadata + on-demand content blobs the
  user already has in their reMarkable cloud). No new content
  creation outside what the user asks the agent to push.
- **Data retention**: cache entries TTL 90 days unless touched; on
  account deletion, everything wiped within 24 h.
- **Transparency**: dashboard shows a live audit feed ("agent X ran
  search_documents at 14:02").
- **TOS + Privacy Policy** required before beta opens.

## 11. Repo + deployment

### Repo layout (private)

```
supern/                       # private repo, proprietary license
├── Cargo.toml                # workspace
├── crates/
│   ├── server/               # axum + rmcp + REST
│   ├── mcp_tools/            # the 8 v1 tools
│   ├── cache/                # postgres + R2 layer
│   ├── readback/             # v6 .rm → markdown
│   └── tokens/               # encrypted token store + rotation
├── web/                      # Next.js 14 (marketing + dashboard)
│   ├── app/
│   ├── components/
│   └── …
├── deploy/
│   ├── app.api.yaml          # DO App Platform spec for the Rust API
│   ├── app.web.yaml          # DO App Platform spec for the Next.js app
│   ├── Dockerfile.api
│   ├── Dockerfile.web
│   ├── migrations/           # sqlx migrate
│   └── runbook.md
└── docs/                     # internal architecture docs
```

### Topology on DigitalOcean (one vendor, four resources)

- **`supern-api`** (Rust binary, App Platform service) — handles
  `mcp.supern.space` and `api.supern.space`. Starts on a `basic-xs`
  instance ($5/mo), scales horizontally as load grows.
- **`supern-web`** (Next.js, App Platform service) — handles
  `supern.space` and `app.supern.space`. `basic-xxs` is enough for
  marketing + dashboard at v1 scale.
- **`supern-db`** (DO Managed PostgreSQL) — primary in `ams3`,
  smallest tier ($15/mo) covers v1. Automated daily backups + 7-day
  PITR included.
- **`supern-blobs`** (DO Spaces bucket `supern-blobs` in `ams3`) — $5/mo
  flat for 250 GB and 1 TB egress, way more than v1 needs. CDN
  endpoint on by default.
- **DNS** managed via DO Domains (free) on `supern.space`. Point the
  registrar's nameservers at `ns1/ns2/ns3.digitalocean.com`. TLS is
  automatic via Let's Encrypt on App Platform.
- **App-level env** holds the chacha20poly1305 KEK, the WorkOS
  secret, Stripe webhooks (later), and the reMarkable cloud root
  override (for staging). DO encrypts secrets at rest.

## 12. Phases / milestones

| Phase | Scope                                                                                           | Est.  |
|-------|-------------------------------------------------------------------------------------------------|-------|
| 0     | Private repo `supern` bootstrap, license/TOS templates, DO resources provisioned (apps/db/spaces/domain) | 1 d   |
| 1     | Postgres schema + migrations + sqlx layer; encrypted token store                                | 2 d   |
| 2     | reMarkable pairing UX (dashboard step 1), MCP key generation (step 2), `whoami` tool            | 2 d   |
| 3     | `push_markdown` + `list_documents` + `delete_document` + `create_folder` tools end-to-end       | 2 d   |
| 4     | Cache + FTS + `search_documents` + `refresh_cache`                                              | 2 d   |
| 5     | Read-back: v6 → markdown (`get_document_markdown`), R2 blob cache, page-level resources         | 3 d   |
| 6     | Marketing site + signup + WorkOS/Clerk wired                                                    | 3 d   |
| 7     | Rate-limit + audit + abuse-control middleware                                                   | 1 d   |
| 8     | Beta launch: App Platform deploy, DO Domains live, monitoring + Sentry, runbook, status page    | 2 d   |
| 9     | Smoke-test from claude.ai / ChatGPT / Cursor against staging account; close 20+ rough edges     | 3 d   |
| 10    | Public beta opens at supern.space                                                               | —     |

**Total: ~3 weeks** to public beta. Stripe + paid tier is a v1.1.

## 13. Open decisions (need your input before bootstrap)

| # | Decision                                              | Default if you don't pick                |
|---|-------------------------------------------------------|------------------------------------------|
| 1 | Auth provider                                          | **WorkOS** (cheapest, GDPR-clean)         |
| 2 | Free tier limits (table in §9)                         | as listed                                |
| 3 | Pricing for paid tier (v1.1)                           | $10/mo flat, unlimited                   |
| 4 | License on private repo                                | full proprietary                         |
| 5 | Beta gating (waitlist or open)                         | waitlist of first 100, then open         |
| 6 | Audit log retention                                    | 90 days                                  |
| 7 | Region rollout                                         | EU first, US replica after first 100 paid users |

## 14. Definition of done for v1 beta

A user can:

1. Sign up at supern.space with email + OAuth.
2. Pair their reMarkable in the dashboard via the standard device code.
3. Generate an MCP key from the dashboard.
4. Paste a one-liner into claude.ai's Custom Connector configuration.
5. From inside any agent: ask "list my notebooks" → cached results in
   <200 ms; "search for X" → FTS hit; "read me the X notebook" →
   markdown; "save these notes as a notebook called Y" → new notebook
   appears on the device, agent confirms with uuid.

Plus:
- Audit feed live in the dashboard.
- One-click revoke for any MCP key.
- TOS + Privacy Policy linked from every page.
- Sentry catches all unhandled errors; metrics surface in Grafana.

---

## What I need from you to start coding

1. **Auth provider.** WorkOS / Clerk / Auth.js — pick one.
2. **Beta gating model.** Waitlist or open from day one?
3. **License + TOS posture.** Full proprietary + my standard templates,
   or something else?

Once those three are decided I'll spin up the `supern` private repo
and run through phase 0.
