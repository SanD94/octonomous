# octonomous — Milestones

Sequenced so the protocol layer is proven before any terminal rendering work
begins. Phases M0–M6 involve **no** `ratatui` and are all testable headlessly.

Each phase lists exit criteria that must be objectively checkable. Do not start a
phase until the previous one exits.

---

## Dependency order

```
M0 spec pin
 └─> M1 codegen
      └─> M2 auth + discovery
           └─> M3 SSE ingestion
                └─> M4 lifecycle + reconcile
                     └─> M5 permissions / interrupt
                          └─> M6 headless harness  ← CORE FROZEN
                               └─> M7 ratatui view
                                    └─> M8 parity
                                         └─> M9 packaging
```

M6 is the gate. Until it passes, the core is expected to churn; after it, the
view is where remaining effort goes.

---

## M0 — Spec pin and workspace

**Goal:** a reproducible, versioned starting point.

- [ ] Create cargo workspace, `crates/octonomous-core`, `crates/octonomous-tui`.
- [ ] Commit `docs/openapi-v2.0.18.json` (already captured).
- [ ] Add `scripts/fetch-openapi.sh` — pulls `/openapi.json` from a running
      server, normalises, and diffs against the committed copy.
- [ ] Commit `docs/fixtures/` — the captured raw SSE stream from a real prompt
      (see §5.5 of DESCRIPTION.md).
- [ ] Add `opencode-version` to CI notes; the target is `v2.0.18`.
- [ ] Toolchain: pin Rust edition/MSRV in `Cargo.toml`. Target
      `aarch64-apple-darwin`; confirm `cargo build --release` produces a binary
      that runs on macOS 15.7.7.

**Exit:** `cargo build` succeeds on a clean checkout; committed spec matches a
freshly fetched one; fixture replays offline.

---

## M1 — Code generation

**Goal:** the 138 operations exist as Rust without being hand-written.

- [ ] Run `progenitor` against the pinned spec into
      `crates/octonomous-core/src/generated/`.
- [ ] Add `// @generated` banners and a build script or xtask to regenerate.
- [ ] Commit generated output so builds do not require `progenitor`.
- [ ] **Audit the untagged unions.** 247 schemas, 22 `anyOf`, **0
      discriminators**. Inspect what was produced; identify which enums are
      undecidable.
- [ ] Where a generated enum is undecidable, prefer `serde_json::Value` over
      fighting the generator. Do not hand-patch generated files — regenerate
      instead.

**Exit:** generated client compiles; a smoke test calls `GET /api/info`;
a written note lists every union that degraded to `Value` and why.

> **Known non-goal:** the event stream. `V2EventEncoded` is an opaque string in
> the spec; codegen cannot produce it. Deferred to M3.

---

## M2 — Auth, discovery, transport

**Goal:** an authenticated client that can find a server on its own.

- [ ] `discovery.rs` — run `opencode service status`, parse the URL from stdout.
      Fall back to `--server` flag, then `$OPENCODE_SERVER`, then
      `http://127.0.0.1:4096`.
- [ ] `auth.rs` — read `~/.config/opencode/service.json` → `.password`;
      Basic auth with username `opencode`. Honour `$XDG_CONFIG_HOME`.
- [ ] Fail with an actionable error when the file or key is missing. **Never**
      log the password.
- [ ] Transport builder: `reqwest` with Basic auth as a default header, plus
      `rustls-tls` for remote servers (not just loopback).
- [ ] `envelope.rs` — unwrap `{data}`; map `_tag`-discriminated errors
      (`UnauthorizedError`, `SessionNotFoundError`, `ConflictError`,
      `SessionBusyError`) to typed variants.

**Exit:**
- Unauthenticated request returns a typed `Unauthorized` — test asserts this.
- Authenticated `GET /api/info` returns the version.
- A unit test fails if auth headers are absent (**guards the missing
  `securitySchemes`** — see DESCRIPTION §6.2).

---

## M3 — SSE ingestion

**Goal:** a correct, resilient event stream. The highest-risk phase.

- [ ] `events.rs` — **hand-written** event enum from the captured vocabulary:
      `server.connected`, `session.execution.{started,succeeded}`,
      `session.text.{started,delta,ended}`,
      `session.step.{started,streamed,ended}`,
      `session.inbox.{enqueued,delivered}`, `session.usage.updated`,
      `session.renamed`, `session.instructions.updated`, and the
      `*.updated` family.
- [ ] **Catch-all `Unknown(JsonValue)` variant.** New server event types must
      never break or crash the client.
- [ ] Parse the full envelope: `id`, `created`, `type`, `location`, `data`,
      `durable`.
- [ ] Use `reqwest-eventsource` for framing. Assert `: heartbeat` comment lines
      are consumed and not surfaced as events.
- [ ] Broadcast events over `tokio::sync::broadcast` so multiple consumers can
      subscribe without contending.
- [ ] Reconnect with **exponential backoff + jitter**, via `backon` or
      equivalent. Track `durable.seq` to detect gaps.
- [ ] Emit a synthetic `Reconnected` signal to consumers so state can be
      re-verified.

**Exit:**
- Replaying the committed fixture yields the expected typed event sequence.
- Killing and restarting the server produces automatic recovery with no
      duplicate or missing terminal events, and a logged gap.
- A synthetic event with an unrecognised `type` is delivered as `Unknown`
      rather than erroring.

---

## M4 — Session lifecycle and reconciliation

**Goal:** correct state, not merely received events.

- [ ] Create session with `location.directory` **in the body**.
- [ ] **Assert** the response's `data.location.directory` matches what was
      requested; error loudly otherwise. Guards the silent-wrong-cwd trap
      (DESCRIPTION §5.3).
- [ ] List sessions with cursor pagination.
- [ ] `GET /session/{id}/message` for authoritative history.
- [ ] Send prompts: `POST /session/{id}/prompt`. Note it returns an inbox item,
      not a reply — all output arrives via SSE.
- [ ] **Reconcile on `server.connected`** (which is also the reconnect signal):
      re-poll authoritative message state and diff against SSE-derived state.
- [ ] Maintain core-side session state as a reducer over events, with the
      authoritative poll able to overwrite it.
- [ ] Support steer vs. queue delivery (inbox `Delivery`:
      `steer` | `queue`) if exposed by the API.

**Exit:**
- A prompt produces a transcript identical to the stock TUI for the same input.
- Force-disconnect mid-response, reconnect, and confirm the transcript is
      correct and gap-free.
- A test passes `location.directory` and fails if the server returns a
      different one.

---

## M5 — Interactive correctness

**Goal:** the client can be trusted to act, not just display.

- [ ] Permission requests: subscribe, present, and reply via
      `POST /session/{id}/permission/{reqID}/reply` with
      `once | always | reject`.
- [ ] **Track pending permissions in core state**, re-fetched on reconnect —
      not merely mirrored from stream events.
- [ ] Idempotency: replying twice must not corrupt state; surface already-settled
      errors clearly.
- [ ] Forms: list, reply, cancel (`/api/session/{id}/form`).
- [ ] `POST /session/{id}/interrupt` to cancel in-flight work.
- [ ] `GET /api/fs/find?query=` backing `@`-file completion.
- [ ] Detect `SessionBusyError` and surface it rather than retrying blindly.

**Exit:**
- A tool requiring approval is approved once, rejected once, and the transcript
  is correct in both cases.
- A pending permission survives a mid-prompt reconnect and can still be
  answered.

---

## M6 — Headless harness — CORE FREEZE

**Goal:** prove the core end-to-end with no UI. This is the gate for M7.

- [ ] `octonomous-core/examples/repl.rs` — a plain stdin/stdout REPL: create
      session, prompt, stream events as plain lines, answer permissions by
      number.
- [ ] Integration tests: `wiremock` fixtures for the REST surface; a live-server
      smoke test behind an ignored/feature-gated test.
- [ ] Record the committed SSE fixture as a regression test; fail on drift.
- [ ] Measure and publish client RSS during a streaming session. Confirm the
      ~5–15 MB target from DESCRIPTION §1.1 — and report the real number even if
      it misses. That figure is currently a projection, not a measurement.
- [ ] Measure client-side RSS in three states: idle at the prompt, mid-stream
      with a long response, and with a full transcript loaded. The mid-stream
      figure is the one that matters and the one most likely to surprise.
- [ ] Confirm `octonomous-core` has **no** `ratatui`/`crossterm` dependency
      (`cargo tree` check, ideally a CI assertion).
- [ ] Freeze the core's public API. Breaking changes after this point are
      costly.

**Exit:** the REPL drives a full session — prompt, stream, approve, interrupt,
reconnect — in a non-TTY environment; memory figure published; dependency
constraint asserted in CI.

---

## M7 — ratatui view, v1

**Goal:** first usable terminal UI, built only on the frozen core.

- [ ] `octonomous-tui` crate; terminal setup/teardown via `crossterm`, restored
      cleanly on panic and on signal.
- [ ] Layout: scrollable transcript, composer, status bar.
- [ ] Event loop consuming the `octonomous-core` broadcast; rendering on a tick
      so a fast token stream does not starve input.
- [ ] Composer: multiline editing, history, `Enter` to send, `Shift+Enter` /
      `Ctrl+J` for newline, `Esc` to cancel.
- [ ] Render assistant text from `session.text.delta`; a spinner on
      `session.execution.started`, cleared on `succeeded`.
- [ ] Modal permission prompt; blocks interaction until answered.
- [ ] Resize handling; reflow on width change.
- [ ] No protocol logic in this crate. If you need one, add it to core instead.

**Exit:** a real session is driven entirely from the TUI; a permission prompt is
answered in-app; no protocol imports exist in `octonomous-tui`.

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

Carried from DESCRIPTION §8. Watch these at every phase boundary:

1. **octonomous does not touch server memory.** The client win (~180 MB, §1.1)
   is independent and still holds. The server's own growth (§1.2,
   ~153 MB → 294-362 MB) is untouched by any milestone here. DESCRIPTION §11 is
   the investigation, and nothing in M0-M9 should be predicated on its outcome.
2. **Do not fork or replace the server during M0–M9.** Those phases use it as the
   behavioral reference. M10+ may implement only the bounded compatibility
   contract in DESCRIPTION §13; a full OpenCode rewrite remains rejected.
3. **Event drift.** New `type` values will appear. The catch-all variant is what
   keeps this survivable — do not remove it to "tidy up".
4. **The API is labelled experimental.** A breaking change is a matter of when,
   not whether. Keeping generated code in one directory is what makes that
   recoverable.
5. **No working V2 reference implementation exists.** DESCRIPTION §5 is the
   reference. If reality contradicts it, the docs are wrong — fix the docs
   first, then the code.

---

## Side quests (not on the critical path)

Neither blocks M0-M9. Both are cheap and worth doing opportunistically.

### S1 — Upstream spec and docs fixes

DESCRIPTION §10.3. Four gaps, each blocking every third-party client. Small,
self-contained pull requests against the OpenCode repository:

- [ ] Add `securitySchemes` (HTTP Basic) to the OpenAPI document. §5.1
- [ ] Give `V2EventEncoded` a real object schema instead of an opaque string. §6.2
- [ ] Document `location.directory` as a body field, and state explicitly that
      the header and query-param forms do not work. §5.3
- [ ] Return 404 or 410 for retired V1 routes instead of 200 + HTML. §4.1

### S2 — Server growth investigation

DESCRIPTION §11. Run before proposing any server-side memory work. Roughly half
a day; the checklist is in that section.

---

## Backend phase — after middle and frontend

M0–M6 remain the **middle** phase and M7–M9 remain the **frontend** phase. Only
after M9 exits does work return to the backend. The backend milestones implement
the limited compatibility and resource contract in DESCRIPTION §13; they do not
recreate OpenCode.

```
M0–M6 middle/core
   └─> M7–M9 frontend
          └─> M10 contract + baseline
                 └─> M11 sessions + events
                        └─> M12 model loop
                               └─> M13 tools + permissions
                                      └─> M14 switchover + measurement
```

### M10 — Backend contract and measured baseline

**Goal:** freeze what the small backend must do before implementing it.

- [ ] Record stock-server wire fixtures for every route in DESCRIPTION §13.1,
      including success, validation failure, missing resource, and cancellation.
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

**Exit:** the headless M6 harness completes, streams, reconnects to, and cancels
a text-only prompt against `octonomous-server`.

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
- [ ] Re-run the M5 reconnect, duplicate-reply, rejection, and interrupt cases
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

Even after M14, full OpenCode parity remains out of scope: no web UI, sharing,
plugins, MCP, LSP, worktrees, snapshots, formatter management, multi-agent
orchestration, provider catalogue, or general OpenCode configuration emulation
without a later demonstrated requirement and milestone.
