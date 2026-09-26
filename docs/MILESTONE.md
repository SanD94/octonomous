# octonomous — Milestones

Sequenced so the protocol layer is proven before any terminal rendering work
begins. Every protocol milestone leaves behind a runnable probe or PoC; these
are investigation tools first, not polished interfaces. This keeps real server
behavior observable while the core evolves instead of deferring all hands-on
feedback until the UI exists. The protocol core is now frozen.

Each phase lists exit criteria that must be objectively checkable. Do not start a
phase until the previous one exits.

---

## Dependency order

```
M8 parity
 └─> M9 packaging
```

The core, headless harness, and first-cut terminal view are frozen; remaining
client work belongs in the view unless a protocol defect requires a compatible
core fix.

---

## M8 — Parity

Ordered by value, not dependency.

- [ ] `@`-file and directory completion.
- [ ] `/` slash-command palette (`GET /api/command`).
- [ ] Model picker (`/api/model`), including variant selection.
- [ ] Agent picker (`/api/agent`).
- [ ] Session picker / resume; session tabs.
- [ ] Streaming Markdown + syntax-highlighted fenced code blocks.
- [ ] Tool-call rendering with live status (pending / running / done / error).
- [ ] Inline diffs (`/api/session/{id}/diff`, `/api/vcs/status`).
- [ ] `/undo` and `/redo` via the revert endpoints.
- [ ] Session compaction (`/api/session/{id}/compact`).
- [ ] Cost and token accounting (`session.usage.updated`).
- [ ] Undo/redo of a sent prompt while streaming.
- [ ] Configurable keybindings, persisted.

**Exit:** parity checklist is explicit and each item is either done or
deliberately descoped with a written rationale.

---

## M9 — Packaging

- [ ] `cargo build --release`; report binary size and stripped RSS.
- [ ] `cargo install --path` works; single self-contained binary.
- [ ] Handle absent/stale server: detect, start via `opencode serve --service`,
      or fall back to a clear error.
- [ ] Version check against `GET /api/info`; warn on major-version skew.
- [ ] Smoke test against a freshly started server on a clean machine.

**Exit:** fresh machine, one install command, working client.

---

## Cross-cutting risks

Carried from DESCRIPTION §6. Watch these at every phase boundary:

1. **octonomous does not touch server memory.** The client win (~180 MB, §1.1)
   is independent and still holds. The server's own growth (§1.2,
   ~153 MB → 294-362 MB) is untouched by any milestone here. DESCRIPTION §9 is
   the investigation, and nothing in M8–M9 should be predicated on its outcome.
2. **Do not fork or replace the server during M8–M9.** Those phases use it as the
   behavioral reference. M10+ may implement only the bounded compatibility
   contract in DESCRIPTION §11; a full OpenCode rewrite remains rejected.
3. **Event drift.** New `type` values will appear. The catch-all variant is what
   keeps this survivable — do not remove it to "tidy up".
4. **The API is labelled experimental.** A breaking change is a matter of when,
   not whether. Keeping generated code in one directory is what makes that
   recoverable.
5. **No reference client exists for the V2 surface.** DESCRIPTION §3 is the
   reference. If reality contradicts it, the docs are wrong — fix the docs
   first, then the code.

---

## Side quests (not on the critical path)

Neither blocks M8–M9. Both are cheap and worth doing opportunistically.

### S1 — Upstream spec and docs fixes

DESCRIPTION §8.3. Four gaps, each blocking every third-party client. Small,
self-contained pull requests against the OpenCode repository:

- [ ] Add `securitySchemes` (HTTP Basic) to the OpenAPI document. §3.1
- [ ] Give `V2EventEncoded` a real object schema instead of an opaque string. §4.2
- [ ] Document `location.directory` as a body field, and state explicitly that
      the header and query-param forms do not work. §3.3
- [ ] Return 404 for unknown and unprefixed paths instead of 200 + HTML. §3

### S2 — Server growth investigation

DESCRIPTION §9. Run before proposing any server-side memory work. Roughly half
a day; the checklist is in that section.

---

## Backend phase — after middle and frontend

The frozen core is the completed **middle** phase and M8–M9 finish the
**frontend** phase. Only after M9 exits does work return to the backend. The backend
milestones implement the limited compatibility and resource contract in
DESCRIPTION §11; they do not recreate OpenCode.

```
frozen middle/core
   └─> M8–M9 frontend
          └─> M10 contract + baseline
                 └─> M11 sessions + events
                        └─> M12 model loop
                               └─> M13 tools + permissions
                                      └─> M14 switchover + measurement
```

### M10 — Backend contract and measured baseline

**Goal:** freeze what the small backend must do before implementing it.

- [ ] Record stock-server wire fixtures for every route in DESCRIPTION §11.1,
      including success, validation failure, missing resource, and cancellation.
- [ ] Treat the V2 surface as the whole contract: no route, event name, or error
      form from any other version of the OpenCode API gets implemented or
      translated. Anything outside §11.1 is a structured `404`.
- [ ] Define a capability response so clients can distinguish unsupported
      features without probing routes or relying on version strings.
- [ ] Measure stock-server RSS at idle, while streaming, with a long transcript,
      and after cancellation using a reproducible harness.
- [ ] Fix the first backend to one model provider and document that choice; do
      not build provider abstraction before a second provider is required.
- [ ] Add `crates/octonomous-server` without changing the frozen public API of
      `octonomous-core`.

**Exit:** fixtures, capability schema, memory harness, and stock baseline are
committed; the new server binary starts but performs no agent work.

### M11 — Sessions, persistence, and events

**Goal:** establish the authoritative state and streaming boundaries.

- [ ] Implement `GET /api/info` and capability advertisement.
- [ ] Implement session creation/listing and paginated message reads with the
      exact `location.directory` behavior required by the core.
- [ ] Persist sessions and messages in SQLite without loading full transcripts
      into process memory.
- [ ] Implement `/api/event` with bounded subscriber channels, heartbeats, and
      stable event envelopes.
- [ ] Return structured `404` or capability errors for unsupported routes; never
      serve an HTML fallback.
- [ ] Replay M10 fixtures against both servers and document intentional
      differences.

**Exit:** octonomous can create and resume a session against either backend, and
RSS remains bounded as persisted transcript size grows.

### M12 — Minimal model and agent loop

**Goal:** stream a useful assistant response through the compatibility surface.

- [ ] Call one model provider directly over HTTP and stream output incrementally.
- [ ] Implement one bounded prompt/tool loop with explicit context and iteration
      limits.
- [ ] Persist output incrementally and emit the corresponding SSE lifecycle and
      text events.
- [ ] Implement prompt enqueueing and interruption; cancellation must stop
      provider work and settle durable session state.
- [ ] Bound queues, model context, response buffers, and error payloads.

**Exit:** the headless harness completes, streams, reconnects to, and cancels a
text-only prompt against `octonomous-server`.

### M13 — Essential tools and permissions

**Goal:** support the smallest safe coding-agent workflow.

- [ ] Implement read, search, edit/write, and shell tools only.
- [ ] Scope all filesystem operations to the session's explicit working
      directory and reject escapes.
- [ ] Require permission decisions for mutating or shell operations and persist
      unresolved requests.
- [ ] Implement permission listing and `once | always | reject` replies.
- [ ] Stream large tool output without retaining it in full and enforce output
      and execution limits.
- [ ] Re-run the client reconnect, duplicate-reply, rejection, and interrupt cases
      against both backends.

**Exit:** a permission-gated edit and shell command work end to end, remain
recoverable after reconnect, and cannot access paths outside the session scope.

### M14 — Backend switchover and resource acceptance

**Goal:** decide from evidence whether the custom backend should become the
default.

- [ ] Run the same functional harness and workload against stock OpenCode and
      `octonomous-server`.
- [ ] Measure idle, streaming, long-transcript, tool-output, and
      post-cancellation RSS for both.
- [ ] Confirm event queues, transcripts, and tool output remain bounded under a
      sustained session.
- [ ] Keep stock OpenCode selectable as a compatibility backend.
- [ ] Publish unsupported capabilities and observed behavioral differences.
- [ ] Make the custom backend the default only if it passes the functional
      contract and demonstrates a material measured memory reduction.

**Exit:** a reproducible report supports the default-backend decision; there is
no projected or assumed memory claim in place of measurements.

## Backend non-goals

Multi-agent orchestration would be considered but after seeing the usage.
