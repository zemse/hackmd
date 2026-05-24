# HackMD Programmability: API, SDKs & CLI for Updating Notes

**TL;DR**
- Yes — HackMD has a (beta-labeled but production-stable) official Bearer-token REST API at `https://api.hackmd.io/v1` that fully supports updating existing notes via `PATCH /notes/{noteId}` (content, title, tags, description, permissions, permalink, parent folder), plus an official Node.js SDK (`@hackmd/api`) and an official CLI (`@hackmd/hackmd-cli`) with a `notes update` subcommand.
- API access is free for every signed-in HackMD account (no Prime required) — the official policy page sets the quota at **2,000 calls/month on Free and 10,000 calls/month on Prime**, with a hard rate limit of **100 calls per 5 minutes** in both tiers; per-request `X-RateLimit-User*` headers expose remaining budget.
- For self-hosted alternatives, the OSS fork **HedgeDoc 1.x** has only a minimal HTTP API (note creation via `POST /new`, exports, session-cookie auth) and is **not API-compatible** with HackMD; **HedgeDoc 2 is still pre-stable** (the docs.hedgedoc.dev site carries a "🚧⚠️🚧 HedgeDoc 2.0 is still in development" banner, and the GitHub 2.0 milestone is at 88% completion with 58 open issues as of April 2026).

## Key Findings

1. **Official public API — stable, REST/JSON, Bearer-token auth.** Base URL: `https://api.hackmd.io/v1`. Live OpenAPI 3.0 spec at `https://api.hackmd.io/v1/docs/swagger.json`; browsable Swagger UI at `https://api.hackmd.io/v1/docs`. The legacy `hackmd.io/@hackmd-api/*` markdown reference pages (User API, User Notes API, Teams API, Team Notes API) are now marked **Deprecated**; Swagger is the source of truth.
2. **Updating notes works exactly the way you want.** `PATCH /notes/{noteId}` with a JSON body — updatable fields per the live OpenAPI spec are: `title`, `content`, `description`, `tags[]`, `readPermission`, `writePermission`, `permalink`, `parentFolderId`. Returns `202 Accepted`. (Older docs that claimed only `content`, permissions, and `permalink` were mutable are outdated.)
3. **Two official client tools, both currently maintained.** `@hackmd/api` (Node.js SDK, **v2.5.0, last published ~10 months ago per npmjs.com**, TypeScript, browser-compatible, axios-based with retry+ETag support) and `@hackmd/hackmd-cli` (oclif-based CLI, **v2.4.0, last published 3 months ago per npmjs.com**, i.e. ~February 2026). Both live in the `hackmdio` GitHub org.
4. **No official Python SDK.** Community packages exist (`python-HackMD` on PyPI — last release **v1.0.3 on May 10, 2024**; `PyHackMD` flagged by Snyk Advisor as "could be considered as a discontinued project"). For Python, just hit the REST API directly with `requests` — it's trivial.
5. **Token creation is free.** Settings → API → Create API token at `https://hackmd.io/settings#api`. Tokens are shown once, stored hashed server-side, and sent as `Authorization: Bearer <token>`.

## Details

### Authentication
- Bearer token in `Authorization` header. Get one at `hackmd.io → Settings → API → Create API token`. Tokens are tied to the user account; for team operations the user must have appropriate role in the team.
- No OAuth/PKCE flow for third-party apps as of this writing — only personal access tokens.

### Endpoint inventory (live OpenAPI spec)
| Method | Path | Purpose |
|---|---|---|
| GET | `/me` | Current user (id, email, name, userPath, teams[], upgraded) |
| GET | `/history?limit=N` | Recently read notes |
| GET | `/notes` | List user's notes |
| POST | `/notes` | Create note (body: `title`, `content`, `description`, `tags[]`, `readPermission`, `writePermission`, `commentPermission`, `suggestEditPermission`, `noteFeatures`, `permalink`, `parentFolderId`, `origin`) |
| GET | `/notes/{noteId}` | Fetch note (supports `If-None-Match` ETag → 304) |
| **PATCH** | **`/notes/{noteId}`** | **Update note** (fields above minus the `*Permission` extras and `noteFeatures`/`origin`) |
| DELETE | `/notes/{noteId}` | Delete note (204) |
| POST | `/notes/{noteId}/images` | Upload image (multipart `image` field) |
| GET | `/teams` | List teams the user belongs to |
| GET | `/teams/{teampath}/notes` | List team notes |
| POST | `/teams/{teampath}/notes` | Create team note |
| GET | `/teams/{teampath}/notes/{noteId}` | Get team note |
| PATCH | `/teams/{teampath}/notes/{noteId}` | Update team note |
| DELETE | `/teams/{teampath}/notes/{noteId}` | Delete team note |
| GET/POST/PATCH/DELETE | `/folders`, `/folders/{folderId}`, `/teams/{teampath}/folders…` | Folder management (added in v2.5 SDK) |

Permission enums: `readPermission` / `writePermission` ∈ `owner | signed_in | guest`. `commentPermission` ∈ `disabled | forbidden | owners | signed_in_users | everyone` (only on create per current spec).

### What can be updated via PATCH
- ✅ `content` (full Markdown body)
- ✅ `title`
- ✅ `tags[]`
- ✅ `description`
- ✅ `readPermission`, `writePermission`
- ✅ `permalink` (the user-friendly URL slug)
- ✅ `parentFolderId`
- ❌ `commentPermission`, `suggestEditPermission`, `noteFeatures`, `origin` — set-once at create time, not in PATCH schema

### Official Node.js SDK (`@hackmd/api`)
- Package: `npm i @hackmd/api` — v2.5.0, TypeScript, ESM+CJS, runs in Node and the browser.
- Repo: `https://github.com/hackmdio/api-client` (Node SDK lives at `/nodejs/src/index.ts` on `master`).
- v2.x is a full rewrite; not backward-compatible with v1.x.
- Constructor: `new API(token, endpoint = 'https://api.hackmd.io/v1', { wrapResponseErrors, timeout: 30000, retryConfig: { maxRetries: 3, baseDelay: 100 } })`
- Note-related methods:
  - `getMe()`, `getHistory()`, `getNoteList()`, `getNote(noteId, { etag })`
  - `createNote(payload)`
  - **`updateNote(noteId, payload)`** — PATCH with arbitrary `UpdateNoteOptions`
  - `updateNoteContent(noteId, content)` — convenience wrapper that PATCHes only `{ content }`
  - `deleteNote(noteId)`
- Team variants: `getTeams()`, `getTeamNotes(teamPath)`, `createTeamNote(teamPath, payload)`, `updateTeamNote(teamPath, noteId, payload)`, `updateTeamNoteContent(teamPath, noteId, content)`, `deleteTeamNote(teamPath, noteId)`.
- Auto-retry with exponential backoff on 5xx/429/network errors for idempotent verbs (GET/HEAD/OPTIONS/PUT/DELETE). Stops retrying once `x-ratelimit-userremaining: 0`.
- Custom error classes: `MissingRequiredArgument`, `InternalServerError`, `TooManyRequestsError` (exposes `x-ratelimit-userlimit/userremaining/userreset`), `HttpResponseError`.

### Official CLI (`@hackmd/hackmd-cli`)
- Repo: `https://github.com/hackmdio/hackmd-cli` (oclif-based). Current: **v2.4.0**, last published 3 months ago per npmjs.com (≈Feb 2026). Snyk classifies maintenance as "Sustainable" with a recent release cadence.
- Install: `npm i -g @hackmd/hackmd-cli`. Login: `hackmd-cli login` (paste token) → stored in `~/.hackmd/config.json`. Or set `HMD_API_ACCESS_TOKEN` (and optionally `HMD_API_ENDPOINT_URL` for HackMD EE).
- Relevant commands:
  - `hackmd-cli whoami`
  - `hackmd-cli notes` / `hackmd-cli notes create [--title --content --readPermission … -e]` (`-e` opens `$EDITOR`)
  - **`hackmd-cli notes update --noteId=<id> --content='# new content'`**
  - `hackmd-cli notes delete --noteId=<id>`
  - `hackmd-cli teams`, `hackmd-cli team-notes [create|update|delete]`
  - `hackmd-cli export --noteId=<id>`, `hackmd-cli history`
- **Breaking-change note:** v2 dropped CodiMD support — only `hackmd.io` and HackMD EE ≥ 1.38.1 are supported. For self-hosted CodiMD use the legacy 1.x branch.

### Update-a-note examples

**curl:**
```bash
curl -X PATCH "https://api.hackmd.io/v1/notes/${NOTE_ID}" \
  -H "Authorization: Bearer ${HMD_API_ACCESS_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
        "title": "Release notes 2026-05-24",
        "content": "# Updated\n\nNew body...",
        "tags": ["release","auto"],
        "readPermission": "signed_in",
        "writePermission": "owner"
      }'
# → 202 Accepted
```

**CLI (one-liner from a file):**
```bash
hackmd-cli notes update --noteId=$NOTE_ID --content="$(cat README.md)"
```

**Node SDK:**
```ts
import HackMDAPI from '@hackmd/api'
const client = new HackMDAPI(process.env.HMD_API_ACCESS_TOKEN!)
await client.updateNote(noteId, {
  title: 'Release notes 2026-05-24',
  content: fs.readFileSync('README.md', 'utf8'),
  tags: ['release', 'auto'],
  readPermission: 'signed_in',
  writePermission: 'owner',
})
```

### Rate limits & pricing
- Per the official API policy page (`hackmd.io/@hackmd-api/api-policy`, last updated Feb 11, 2022, verbatim): *"There is a quota of 2000 calls per month and the rate limit is 100 calls every 5 minutes,"* and *"Upgrade to Prime plan to get 10,000 calls per month."* The per-window rate limit (100 / 5 min) is unchanged on Prime.
- Response headers on every call: `X-RateLimit-UserLimit`, `X-RateLimit-UserRemaining`, `X-RateLimit-UserReset` (Unix seconds).
- No surcharge for the API itself — you just need a HackMD account. Prime is $5/user/month and unlocks other product features (unlimited invitees, GitHub push/pull, custom templates, etc.) alongside the higher quota.

### Community / third-party SDKs
- **Python**: `python-HackMD` on PyPI — wraps the same endpoints (`get_notes`, `get_note`, `create_note`, `update_note`, `delete_note`); last release v1.0.3 on May 10, 2024. `PyHackMD` (GitHub: GoatWang/PyHackMD, eugene87222/python-HackMD) flagged as inactive by Snyk. For new code, prefer raw `requests`.
- **.NET / C#**: `HackMD.API` on NuGet (`isdaviddong/HackMD.API` on GitHub) — community, not official.
- **Markdown→HTML conversion**: `hackmd-to-html-cli` (ksw2000/hackmd-to-html-cli) — render-only utility, not an API client.

### CodiMD / HedgeDoc — self-hosted alternative
- **HedgeDoc 1.x** (the maintained OSS fork after CodiMD): API is minimal and based on session cookies, not Bearer tokens. The documented public surface in `docs.hedgedoc.org/dev/api/` is essentially `POST /new` (create a note from Markdown body) and a few export/info endpoints. There is **no equivalent of PATCH /notes/{noteId}** — updates to existing notes happen over the realtime socket.io session, not a stable REST PATCH. **It is NOT API-compatible with HackMD.**
- **HedgeDoc 2** is a ground-up rewrite that introduces a proper Bearer-token public API, but is still pre-stable: the GitHub HedgeDoc 2.0 milestone is at 88% completion with 58 open / 456 closed issues (last updated April 9, 2026), and `docs.hedgedoc.dev` still carries a "🚧⚠️🚧 HedgeDoc 2.0 is still in development" banner. Do not rely on it for production CLI workflows yet.
- **`hedgedoc/cli`** (GitHub: hedgedoc/cli) — tiny CLI wrapper that supports `import`, `export --pdf|--md|--html|--slides`, and `publish`. It does **not** support updating an existing note.
- **Practical implication**: if you want CLI-driven push-to-existing-document workflows today, hackmd.io (or a self-hosted HackMD EE) is the only path. HedgeDoc is a good self-hosted editor but its automation story is much weaker.

## Recommendations

1. **For your CLI push-update workflow, use the official stack**: a personal API token + either `hackmd-cli notes update` for shell scripts or `@hackmd/api`'s `updateNote()` for richer Node integrations. Both hit the same `PATCH /notes/{noteId}`. Start with `hackmd-cli` for a 5-minute POC and graduate to the SDK if you need batching, retries beyond defaults, or folder/permission logic.
2. **Wire the token in via `HMD_API_ACCESS_TOKEN`**, not `~/.hackmd/config.json`, so it's CI-safe. Add `HMD_API_ENDPOINT_URL` only if pointing at HackMD EE.
3. **Set permissions explicitly on update**. The PATCH endpoint will rewrite whatever fields you send; if you only send `content`, the others are untouched. Always include `readPermission`/`writePermission` if your workflow ever rotates them.
4. **Plan for rate-limiting**: 100 calls per 5 minutes is generous for human-driven push, but if you're updating many notes in a loop, throttle to <20 req/min and respect `X-RateLimit-User*` headers — the SDK already does this in its retry interceptor. The 2,000-call/month Free quota is plenty for one developer pushing a few notes per day (≈65/day budget); switch to Prime if you exceed it.
5. **If you ever expect >2,000 calls/month or want product SLAs**, budget for Prime ($5/user/month, 10,000 calls/month per the policy page). For organization-wide deployments with SSO and on-prem requirements, evaluate HackMD EE rather than HedgeDoc — the EE instance keeps the same API surface, so your CLI code is portable.
6. **Don't pick HedgeDoc/CodiMD if "CLI pushes update existing notes" is core.** It's not what they're built for today.

**Triggers to revisit this recommendation:** (a) HedgeDoc 2 reaches a stable release with documented PATCH endpoints (currently 88% per the 2.0 milestone — worth re-checking quarterly); (b) HackMD changes the policy page to gate token creation behind Prime; (c) you start needing OAuth-style third-party app authorization, which HackMD doesn't offer today.

## Caveats

- The "Beta" label has been on the API for a while; in practice it has been stable and additive (folders, image upload, ETag, etc. added without breaking existing endpoints). HackMD does not publish a formal deprecation policy, so monitor the API changelog (`hackmd.io/@docs/changelog`) before depending on edge fields.
- The HackMD API Policy page is dated **February 11, 2022** — the quoted quotas (2,000 Free / 10,000 Prime, 100 per 5 min) have not been refreshed publicly since then, so treat them as a floor rather than a contract. If you have a Prime account, verify by inspecting `X-RateLimit-UserLimit` on a live response.
- Community Python wrappers (`PyHackMD`, `python-HackMD`) are functional but appear inactive (last `python-HackMD` release: May 10, 2024); rely on the raw REST API for forward-compatibility.
- The OpenAPI spec uses lowercase `{teampath}` as the path parameter name; the SDK exposes it as `teamPath`. Pass the team's path slug exactly as it appears in HackMD's team URL.
- `PATCH /notes/{noteId}` returns `202 Accepted` because the update is processed asynchronously through HackMD's realtime/CRDT layer. If you immediately `GET` the note you may briefly see stale content — use the ETag flow (`If-None-Match` + 304) if you need a strong confirm.
- HackMD does not currently expose OAuth 2.0 / third-party app authorization; only personal access tokens. If you need to act on behalf of other users, each user must mint their own token.
