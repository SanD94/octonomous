# octonomous — Description

A native macOS terminal client for OpenCode, written in Rust.

- **Status:** implementation
- **Protocol:** OpenCode **V2** only — the `/api/*` surface of the pinned
  `v2.0.18` spec. No other version of the OpenCode API is in scope, and nothing
  in this document is written against one.

---

## 1. Motivation

The stock OpenCode TUI is a Bun/Solid/OpenTUI application. Running it means a
JavaScript runtime in the terminal alongside the OpenCode server, which already
occupies substantial memory.

The intent of this project is a thin, native client that speaks OpenCode's HTTP +
SSE protocol directly, with the view layer added only once the protocol layer is
proven.

See `MILESTONE.md` for the phased plan. Memory measurements below were taken on
the author's machine; where a figure is a projection rather than a measurement,
it says so.

### 1.1 Measured memory profile

An `opencode` invocation spawns **two** processes, and conflating them leads to
the wrong conclusion about where the memory goes. Measured on the author's
machine, 2026-09-26:

| Process | TTY | RSS | Identity |
|---|---|---|---|
| `opencode` (72168) | `ttys011` | **190 MB** | The **interactive TUI** — parent is `zsh` on that tty |
| `opencode serve --service` (72169) | none | **294 MB** (peaked at 362 MB) | Shared background **server**, child of the TUI |
| fresh `opencode serve`, idle | none | **152.7 MB** | Server **floor**, no sessions loaded |

Identity was established via `ps -o tty` plus parentage, not by guessing: a TUI
has a controlling terminal and a `zsh` parent; the service shows `??` for TTY
and is spawned by the client.

So the real split is roughly:

```
client (TUI)   190 MB   <- octonomous replaces this
server floor   153 MB   <- unavoidable; a floor, not a total
server growth  ~300-360 MB after ~20 min of use
```

**The client is the larger and far cheaper half of the win.** The release-mode
headless client measures **9.0 MB mid-stream**, implying a client-side saving on
the order of **180 MB** before the terminal view is added, with no server changes
whatsoever.

> An earlier draft of this document claimed the opposite — that the server
> dominated and the client was secondary — because it measured only one of the
> two processes. That was wrong, and the ratio matters for prioritisation.

Release-mode headless client RSS was measured on the author's machine on
2026-09-26 with `scripts/measure-client-rss.sh`. The figures below are medians of
five consecutive runs; the probe keeps the authenticated HTTP transports
resident, accumulates a 1 MiB response for the streaming state, then retains
that response plus 100 × 100 KiB transcript messages.

| Client state | Median RSS | Observed range |
|---|---:|---:|
| Idle at the prompt | **3.8 MB** | 3.7–4.0 MB |
| Mid-stream, 1 MiB response | **9.0 MB** | 9.0–9.2 MB |
| Full 10.8 MiB transcript loaded | **31.5 MB** | 30.7–35.8 MB |

The important mid-stream figure meets the original 5–15 MB target. The full
transcript result also makes the cost of retaining history explicit. These are
core/headless measurements; the future terminal view's incremental RSS remains
to be measured separately.

### 1.2 The server-side problem, stated precisely

The server's *floor* is unremarkable. Its **growth** is the actual issue:

| State | RSS |
|---|---|
| Fresh, idle | 152.7 MB |
| After ~20 min of normal use | 294 MB, observed peaking at 362 MB |

Roughly doubled without a restart, alongside a **192 MB** `opencode.db`
(`opencode.db-wal` a further 2 MB at time of writing). Likely contributors are
SQLite page cache and in-memory session/message state, but **this is not
verified** — see §9. Whether the growth is bounded or unbounded over days is
unknown, and it is the most important open question about the server.

This is not addressable from a client, and it is the honest limit of what
octonomous achieves on its own.

---

## 2. Goals

1. A reusable, UI-agnostic Rust **protocol core** (`octonomous-core`) covering
   auth, discovery, session lifecycle, SSE event ingestion, permissions, and
   forms.
2. A terminal view (`ratatui`) built strictly on top of that core, with no
   protocol knowledge leaking into the view.
3. Protocol correctness under adverse conditions: reconnects, dropped frames,
   out-of-order delivery, and authoritative reconciliation.

---

## 3. Verified V2 protocol reference

All facts below were obtained by probing the live local server on 2026-09-26.
The full spec is saved at `docs/openapi-v2.0.18.json` (251 KB).

### 3.1 Transport and auth

| Concern | Finding |
|---|---|
| Auth | HTTP **Basic**. Username is literally `opencode`; password from `~/.config/opencode/service.json` → `.password` (43 chars). |
| Auth discovery | `~/.config/opencode/service.json` contains **only** the password. The endpoint URL is **not** stored there. |
| Endpoint discovery | `opencode service status` prints the URL on stdout (e.g. `http://127.0.0.1:49374`). Must be parsed or shelled out. |
| Unauthenticated | All routes return `{"_tag":"UnauthorizedError","message":"Authentication required"}` with 401. |
| TLS | Loopback plaintext HTTP. A remote server would need TLS; keep the transport swappable. |

### 3.2 Response envelope

Nearly every route wraps its payload:

```json
{ "data": { "id": "ses_...", "location": { "directory": "/path" } } }
```

List routes add a cursor: `{"data": [...], "cursor": {"previous": null, "next": "..."}}`.
Errors are Effect-style tagged unions discriminated by `_tag`, e.g.
`{"_tag":"UnauthorizedError","message":"..."}`, `SessionNotFoundErrorEncoded`,
`ConflictErrorEncoded`, `SessionBusyErrorEncoded`.

### 3.3 Working directory — critical gotcha

The session's working directory **must** be set in the JSON request body as
`location.directory`. Two plausible alternatives exist in the binary and
**neither works**; both fail silently by creating the session in the server's
own cwd.

```
POST /api/session  {"location":{"directory":"/project"}}      -> directory: "/project"      CORRECT
POST /api/session  header  x-opencode-directory: /project     -> directory: "/Users/me"     silently wrong
POST /api/session  query   ?directory=/project               -> directory: "/Users/me"     silently wrong
```

`x-opencode-directory` does appear in the binary and is used elsewhere in the
stack, which makes this trap likely. The core must set location in the body on
**every** location-scoped call and should assert the returned
`data.location.directory` matches the request.

### 3.4 Event stream

`GET /api/event` → `text/event-stream`. Two line kinds arrive; the second must be
skipped or it corrupts the parse:

```
data: {"id":"evt_...","created":1790403606159,"type":"session.step.started","location":{"directory":"/project"},"data":{...},"durable":{"aggregateID":"ses_...","seq":5,"version":1}}

: heartbeat
```

Envelope fields: `id`, `created`, `type`, `location` (present on many, absent on
some), `data` (payload, varies by type), `durable` (present on some — carries
`aggregateID`, `seq`, `version`, which look useful for gap detection).

First frame on connect is always `{"type":"server.connected","data":{}}`.
Subscriptions are **live-only**: no replay, no resume token. See §5.3.

#### Heartbeats are a real parser hazard — measured

Census of the committed fixture (`fixtures/prompt-basic-v2.0.18.sse`, one real
prompt): 144 lines = **69** `data:` frames + **3** `: heartbeat` comments + 72
blank separators.

Splitting on blank lines yields 72 chunks, of which **3 contain no `data:` line
at all**:

| chunk shape | count |
|---|---|
| exactly one `data:` line | 69 |
| one comment line, **zero** `data:` lines | 3 |

A naive `split("\n\n")` + "parse the `data:` field" parser therefore hits 3
empty-event or null-deref cases per prompt. This is why §4.1 specifies
`reqwest-eventsource` rather than hand-rolled framing, and why the fixture is
committed — it reproduces the hazard deterministically, offline.

### 3.5 Event vocabulary (captured live)

A real prompt produced these `type` values. This is a partial capture from one
code path, not the complete set.

```
server.connected              session.execution.started     session.execution.succeeded
session.text.started          session.text.delta            session.text.ended
session.step.started          session.step.streamed         session.step.ended
session.inbox.enqueued        session.inbox.delivered       session.usage.updated
session.renamed               session.instructions.updated
agent.updated                 command.updated               model.updated
plugin.updated                provider.updated              skill.updated
reference.updated             integration.updated           project.updated
websearch.updated
```

The three families are independent and each must be tracked separately:
`session.execution.*` brackets a whole run, `session.step.*` brackets one step
within it, and `session.text.*` carries assistant output with an `ordinal` and
a `delta`. Text is never delivered as a whole-message replacement, so the event
enum keeps the three apart and the view reassembles them in order.

Additional types referenced in official docs but not observed in this capture:
`permission.asked`, and others under the `file.*` / `pty.*` families.

Sample payloads:

```jsonc
// session.text.delta
{"sessionID":"ses_...","assistantMessageID":"msg_...","ordinal":0,"delta":"ok"}

// session.step.started
{"sessionID":"ses_...","agent":"build","model":{"id":"...","providerID":"..."},
 "assistantMessageID":"msg_...","started":1790403604538}
```

### 3.6 Core endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/info` | Server version, pid, urls. Health/auth check. |
| `POST` | `/api/session` | Create session. Body `location.directory`. |
| `GET` | `/api/session` | List sessions (paginated). |
| `GET` | `/api/session/{id}/message` | Authoritative message history. `limit`, `order`, `cursor`, `type`. |
| `POST` | `/api/session/{id}/prompt` | Send prompt. Returns inbox item, **not** the reply. |
| `POST` | `/api/session/{id}/interrupt` | Cancel in-flight work. |
| `GET` | `/api/event` | SSE subscription. |
| `GET` | `/api/session/{id}/permission` | List pending permission requests. |
| `POST` | `/api/session/{id}/permission/{reqID}/reply` | Body: `{"decision": "once"\|"always"\|"reject", "message"?: string}`. |
| `GET` | `/api/session/{id}/form`, `POST .../form/{formID}/reply`, `DELETE .../form/{formID}` | Structured form requests. |
| `GET` | `/api/fs/find?query=` | `@`-file autocomplete. |
| `GET` | `/api/model`, `/api/agent`, `/api/command` | Pickers. |
| `GET` | `/api/session/{id}/diff`, `/api/vcs/status` | Diff / VCS state. |
| `POST` | `/api/session/{id}/compact`, `/fork`, `/revert/*` | Session management. |

`POST /prompt` returning immediately is significant: **all** output arrives on the
SSE stream, so the event loop is the only place assistant text appears.

---

## 4. Dependency strategy — generate, don't hand-roll

The instruction is to avoid building things that already exist. Applied honestly,
that rules out hand-writing 138 endpoints: the client is generated from the
pinned V2 spec, not typed out by hand.

### 4.1 Use

| Concern | Choice | Rationale |
|---|---|---|
| Protocol types + REST client | **`progenitor`** (OpenAPI 3.1 codegen for Rust) | Spec is 3.1.0 with all 138 `operationId`s present and only 22 `anyOf` unions. Well suited. |
| HTTP | **`reqwest`** 0.13 (`rustls`, `stream`) | Async, streaming, no OpenSSL; the transport the generated client sits on. |
| SSE framing | **`kameleoon-reqwest-eventsource`** 0.6 (imported as `reqwest-eventsource`) | Maintained reqwest 0.13-compatible fork; handles frame boundaries and heartbeats, which §3.4 shows are not optional details. |
| Async runtime | **`tokio`** | `reqwest`/`progenitor` assume it. |
| Serialization | **`serde`** / **`serde_json`** | Required by codegen output. |
| Errors | **`thiserror`** 2 | Generated code expects it. |
| TUI (later) | **`ratatui`** + **`crossterm`** | Deferred; not a protocol dependency. |

Codegen is re-run from the spec rather than hand-maintained, so tracking a future
OpenCode release is a spec refresh plus a diff review.

### 4.2 Must be hand-written (codegen cannot help)

1. **The event enum.** The spec declares the SSE payload as
   `V2EventEncoded = { "type": "string", "contentMediaType": "application/json" }`
   — an opaque string. Real payloads are rich JSON objects. Generated code would
   yield `String` and lose everything. The event enum is authored by hand from
   §3.5 plus further live capture, with a catch-all `Unknown` variant so unknown
   types never break the client.
2. **Auth injection.** The spec publishes **no** `securitySchemes`
   (`components.securitySchemes` is `{}`). Generated code will contain no
   authentication at all. Basic auth is attached in transport construction.
3. **Endpoint discovery.** The password file has no URL; `opencode service status`
   must be shelled out and parsed.
4. **Location assertion.** The silent-wrong-cwd failure in §3.3 needs an explicit
   post-condition check, which no generated client will perform.
5. **SSE → state reconciliation.** The `durable.seq` gap signal and
   poll-on-reconnect logic are policy, not schema.

### 4.3 Codegen findings

- **Zero discriminators across 247 schemas.** The 22 named `anyOf` schemas were
  audited during code generation. Twenty are distinguishable by JSON shape or a
  required constant field and retain typed representations. `Form.Value` and
  `Model.ReasoningField` overlap and therefore degrade to `serde_json::Value`;
  see `CODEGEN.md`. Progenitor's generated flattened representation of
  `Session.Message.Info` also drops every subtype during deserialization, so
  authoritative message pages remain JSON at that boundary rather than losing
  transcript data.
- **`additionalProperties: false` everywhere** plus forward-compatible server
  additions means strict types may fail to deserialize on a newer server. Plan
  for `#[serde(default)]` on generated structs, and prefer lenient types for
  event payloads.
- **OpenAPI 3.1 normalization.** Progenitor 0.15 targets OpenAPI 3.0. The
  regeneration utility losslessly converts the nullable and exclusive-bound
  forms used by the pinned spec before generation. `V2EventEncoded` remains an
  opaque string, so the event layer is still hand-written.

---

## 5. Architecture

### 5.1 Crate layout

```
octonomous/
├── Cargo.toml                 # workspace
├── crates/
│   ├── octonomous-core/         # protocol layer. No ratatui, no crossterm.
│   │   ├── src/
│   │   │   ├── generated/     # progenitor output. Do not hand-edit.
│   │   │   ├── auth.rs        # password discovery, Basic auth
│   │   │   ├── discovery.rs   # `opencode service status` parsing
│   │   │   ├── transport.rs   # authenticated wrapper over generated client
│   │   │   ├── events.rs      # HAND-WRITTEN event enum + SSE reader
│   │   │   ├── envelope.rs    # {data} unwrap, _tag error mapping
│   │   │   └── reconcile.rs   # SSE vs authoritative poll
│   │   └── tests/             # wiremock fixtures + live-server smoke test
│   └── octonomous-tui/          # ratatui view. Consumes octonomous-core only.
└── docs/
```

`octonomous-core` must not depend on `ratatui` or `crossterm`. That
constraint is what makes the core reusable and is the whole point of
splitting it.

### 5.2 Layering rule

```
   view (ratatui)  ──depends──>  octonomous-core  ──depends──>  reqwest / SSE / openapi
        │                              │
        └────── emits UI intents ──────┘
```

The view never performs I/O. The core never knows about terminals. A test for
the core must be runnable with no TTY.

### 5.3 State ownership and reconciliation

SSE is the fast path and is **not authoritative**: live-only, no replay, no
resume token, frames can be dropped. The `durable` block's `seq` is a usable
gap signal.

The core therefore maintains state as: **SSE for latency, `GET /message` for
truth.** On `server.connected` — which is also what a reconnect produces — the
core re-polls authoritative message state and reconciles, rather than trusting
the stream to have caught up. A permission prompt must be survivable across a
reconnect, so pending permissions are tracked in core state and re-fetched, not
merely mirrored from stream events.

This reconcile-on-connect behaviour is the single most important correctness
property of the core. A live-only stream with no resume token cannot be trusted
to have caught up, so re-polling on connect is the only correct option rather
than an optimisation.

### 5.4 Frozen core API

The public `octonomous-core` API is frozen as of 2026-09-26. Consumers may rely
on the exported auth/discovery, transport, session, event, interaction, and
reconciliation modules. Compatible additions and protocol-defect fixes are
allowed; removals, renames, signature changes, and semantic changes require an
explicit versioned migration. `generated` remains public because core methods
return pinned V2 protocol types, but it changes only when the pinned protocol
specification is deliberately updated.

---

## 6. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| Server growth (~153 MB → ~300-360 MB) is unbounded and unavoidable from a client | High | Measure before optimising anything; see §9. octonomous still captures the ~180 MB client win independently. |
| Terminal view increment is not yet measured | Low | §1.1 publishes the headless baseline; re-measure the release binary after the view lands. |
| Generated types fail to deserialize on newer servers | Medium | Lenient event types; `serde(default)`; version-pin the spec in-repo. |
| Event enum drifts as V2 evolves | Medium | Catch-all `Unknown` variant; capture fixture tests from live streams. |
| No `securitySchemes` — auth silently absent from codegen | Medium | Covered in §4.2; add a test that fails without auth. |
| Silent wrong-cwd from location handling | Medium | Assert `data.location.directory`; regression test. §3.3. |
| V2 API marked experimental; routes may change | Medium | Spec refresh is cheap; keep generated code isolated in one directory. |
| No reference client exists for the V2 surface, so there is nothing to diff behaviour against | Medium | This document's §3 capture and the committed fixture are the reference. |
| Progenitor cannot directly consume the pinned 3.1 document | Low | The isolated regeneration utility normalizes its 3.1 constructs to equivalent 3.0 forms; `CODEGEN.md` records each adaptation. |

---

## 7. References

- Client (TS, for protocol semantics): <https://opencode.ai/v2/docs/build/client>
- HTTP API: <https://opencode.ai/v2/docs/api>
- TUI plugin API (extension path, not used here): <https://opencode.ai/v2/docs/build/plugins/cli>
- Progenitor: <https://docs.rs/progenitor>

---

## 8. Server-side options

The client win in §1.1 requires no server changes. The server *growth* in §1.2 does.
There are three responses, and they are not comparable in cost. The fourth (§8.4)
is not a server change at all, and is the reason none of this blocks the client
work.

### 8.1 Fork the server — recommended against

A full reimplementation in Rust or Go would mean rebuilding agents, provider and
model routing, tool execution, the permission model, MCP, LSP, formatters,
snapshots, compaction, worktrees, VCS, and the web UI. Cost is multi-person-year,
and the fork then diverges from upstream permanently, requiring a manual re-merge
of every subsequent release. It also makes memory *worse* before it makes it
better, because the rewrite starts from zero.

This option is recorded so the decision is deliberate, not because it is
attractive.

### 8.2 Config-level reduction — cheap, effectiveness unverified

No fork required. Candidate levers, none yet measured:

- Prune or archive old sessions; `opencode.db` is 192 MB and is the prime suspect
  for SQLite page-cache growth.
- Disable unused MCP servers and LSP servers, each of which is resident state.
- Reduce loaded plugins.
- `opencode service restart` as a mitigation of last resort.

**Do not treat these as a plan until §9 has been run.** The growth in §1.2 has
not been attributed to a cause, so any lever chosen now is a guess.

### 8.3 Upstream contributions — small, real, and directly on-topic

Every spec and documentation gap found in §3 and §4.3 blocks *any* third-party or
native client, not only octonomous. These are the highest-leverage
server-side changes available, because they cost little and compound across
every consumer:

| Gap | Evidence | Impact on third-party clients |
|---|---|---|
| `components.securitySchemes` is `{}` | §3.1 | No generated client can produce auth. Silent 401s. |
| `V2EventEncoded` is an opaque `string` | §4.2 | Codegen yields `String`, discarding the entire event payload. |
| `location.directory` undocumented | §3.3 | Header and query-param forms silently target the wrong cwd. |
| Unprefixed and unknown paths return 200 + HTML | §3 | Actively misleads client authors; every probe must assert on content, not status. |

Reporting or patching these is a legitimate and contained way to "change the
server side". None requires a fork.

### 8.4 The decoupling that makes this low-regret

Because octonomous's core targets the documented `/api/*` surface rather than
server internals, the client is **backend-agnostic**. Any server implementing
that surface works: OpenCode today, a different implementation later, or a mock
in tests. Two consequences:

- The generated client, authenticated transport, event ingestion, and frozen
  protocol core are reusable regardless of what is decided about the server,
  so they are not speculative.
- The server-side question in §8.1–§8.3 can be deferred without blocking
  client progress.

---

## 9. Open question: what drives the server's growth

The highest-value unanswered question about this project. Until it is measured,
§8.2 is guesswork and any claim about server memory is speculation.

- [ ] Baseline: fresh `opencode serve`, RSS at idle. *(152.7 MB, already known.)*
- [ ] RSS against session count loaded.
- [ ] RSS against messages per session.
- [ ] RSS during and after long tool-output runs.
- [ ] Watch `opencode.db` size and `opencode.db-wal` alongside RSS.
- [ ] Check whether the growth is reclaimable — does RSS fall after memory
      pressure, or does it stay resident (a true leak rather than a cache)?
- [ ] Confirm whether growth correlates with the MCP/LSP/plugin set.
- [ ] Test over hours, not minutes. Twenty minutes of observation (§1.2) cannot
      distinguish a bounded cache from a leak.

Outcome determines whether §8.2 has anything to offer. If the growth turns out to
be a bounded cache, the correct response is to do nothing and accept a higher
steady-state RSS. If it is unbounded, that is an upstream bug report with real
evidence behind it — which is a far better outcome than a fork.

## 10. Artifacts in this directory

| File | Contents |
|---|---|
| `CI.md` | Reproducible baseline checks and the pinned OpenCode version. |
| `DESCRIPTION.md` | This document. |
| `MILESTONE.md` | Remaining phased plan, M7–M14, with exit criteria. |
| `openapi-v2.0.18.json` | Full V2 spec fetched from the live server (248 KB, 138 operations, 247 schemas). Source for generated client code. |
| `fixtures/prompt-basic-v2.0.18.sse` | Raw captured SSE stream from one real prompt (69 frames, 3 heartbeats). Event-ingestion regression fixture. |
| `fixtures/prompt-basic-v2.0.18.session-id.txt` | The session ID used for that capture, for reproducing it live. |

---

## 11. Project contract: middle, frontend, then backend

The implementation order is deliberate:

1. **Middle (complete):** the UI-independent protocol and state core is proven
   and frozen against the stock OpenCode server.
2. **Frontend (M7–M9):** build and package the native terminal client on that
   core while the stock server remains the behavioral reference.
3. **Backend (M10+):** return to the server problem only after the client works,
   then implement a small Rust backend for the subset octonomous actually uses.

This section refines the blanket rejection in §8.1. A **full OpenCode rewrite
remains rejected**. The new backend is instead a deliberately incomplete,
single-user implementation of a small compatibility contract. No backend work may
move ahead of the middle or frontend phases merely to chase projected memory
savings.

### 11.1 Compatibility contract

The contract is defined **solely** by the OpenCode **V2** HTTP + SSE surface, as
published in the pinned `docs/openapi-v2.0.18.json`. There is no second API
version in scope: no route, event name, payload shape, or error form from any
other version of the OpenCode API is implemented, emulated, or adapted to. If a
capability is not in the table below, the answer is `404`, not a translation
layer.

The initial `octonomous-server` must implement these V2 routes:

| Method | Path | Contract |
|---|---|---|
| `GET` | `/api/info` | Version, process, and capability information. |
| `GET` | `/api/event` | Live SSE stream with heartbeats and typed event envelopes. |
| `POST` | `/api/session` | Create a session scoped by `location.directory`. |
| `GET` | `/api/session` | List sessions with bounded pagination. |
| `GET` | `/api/session/{id}/message` | Read authoritative, paginated transcript state. |
| `POST` | `/api/session/{id}/prompt` | Enqueue a prompt; output arrives through SSE. |
| `POST` | `/api/session/{id}/interrupt` | Cancel in-flight model or tool work. |
| `GET` | `/api/session/{id}/permission` | List unresolved permission requests. |
| `POST` | `/api/session/{id}/permission/{requestID}/reply` | Resolve a request with `once`, `always`, or `reject`. |

Any V2 route outside this table must return an explicit structured `404` or
capability error, never the web UI or a false-positive `200`. The client must
degrade by advertised capability rather than infer support from the server name
or version.

### 11.2 Initial backend scope

The backend owns only:

- durable sessions and messages;
- direct model-provider HTTP streaming;
- one bounded agent/tool loop;
- read, search, edit/write, and shell tools;
- permission gating, interruption, and event delivery;
- one explicit working directory per session.

The first implementation does **not** include the OpenCode web UI, sharing,
plugins, MCP, LSP, worktrees, snapshots, formatters, multiple agents, provider
catalogues, or general OpenCode configuration compatibility. Features move into
scope only when a demonstrated client requirement justifies their resident
memory and maintenance cost.

### 11.3 Resource contract

Low memory is an acceptance criterion, not an architectural assumption. The
server must avoid retaining complete transcripts or unbounded tool output in
memory, use bounded event channels, page durable state from SQLite, bound model
context explicitly, and stream large output. Idle, streaming, long-transcript,
and post-cancellation RSS must be measured against the stock server baseline.
No target is claimed until those measurements exist.

The stock server remains supported throughout backend development. This gives
octonomous a behavioral oracle, keeps frontend work unblocked, and allows each
route to be compared before the custom backend becomes the default.
