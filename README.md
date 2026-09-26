# octonomous

A lightweight native terminal client for [OpenCode](https://opencode.ai), written in Rust.

octonomous replaces the stock JavaScript terminal client while continuing to use the OpenCode
service for sessions, tools, and model access. It speaks the OpenCode V2 HTTP and SSE protocol
directly and keeps its protocol implementation separate from the terminal UI.

## Status

octonomous is under active development and currently targets the OpenCode `v2.0.18` API. It
supports the core interactive workflow: creating and resuming sessions, streaming responses,
interrupting work, replying to permission requests, and reconnecting with authoritative state
reconciliation.

The client currently targets macOS. It is intentionally smaller in scope than the stock OpenCode
TUI and does not yet provide features such as file and slash completion, session tabs, Markdown
rendering, or specialized tool and diff views.

## Requirements

- Rust with edition 2024 support
- OpenCode installed and configured
- macOS

## Installation

Clone the repository and install the terminal client with Cargo:

```sh
git clone git@github.com:SanD94/octonomous.git
cd octonomous
cargo install --path crates/octonomous-tui --root ~/.local
```

This installs the binary at `~/.local/bin/octonomous`. Ensure `~/.local/bin` is included in your
`PATH`.

## Usage

Run octonomous in the current directory:

```sh
octonomous
```

Or open a project directory explicitly:

```sh
octonomous /path/to/project
```

By default, octonomous resumes the most recently updated session in that directory. If no session
exists, it creates one. When no explicit server is supplied, the client discovers the local
OpenCode service and starts `opencode serve --service` if necessary.

Common options:

```text
octonomous [OPTIONS] [DIRECTORY]

  --new                    Always create a new session
  --session ID             Resume a specific session
  --agent ID               Agent for a newly created session
  --model PROVIDER/ID[@VARIANT]
                           Model for a newly created session
  --server URL             Use an explicit OpenCode server URL
  --check                  Check connectivity and exit
  -h, --help               Show help
  -V, --version            Show the client version
```

Examples:

```sh
# Start a fresh session for the current directory
octonomous --new

# Start a session with an explicit agent and model
octonomous --new --agent build --model anthropic/claude-sonnet-4

# Resume a known session
octonomous --session ses_123 /path/to/project

# Verify that the OpenCode service is reachable
octonomous --check
```

## Keybindings

| Key | Action |
|---|---|
| `i` | Open the floating composer |
| `Enter` | Send the prompt and close the composer |
| `Shift+Enter`, `Ctrl+Enter`, or `Ctrl+J` | Insert a newline in the composer |
| `Esc` | Close the composer without sending, or interrupt a running response |
| `Up` / `Down` | Navigate prompt history in the composer |
| `Page Up` / `Page Down` | Scroll the transcript |
| `Ctrl+C` | Quit |
| `1` or `o` | Allow a pending permission once |
| `a` | Always allow a pending permission |
| `r` | Reject a pending permission |

## Architecture

The workspace contains two crates:

- `octonomous-core` — reusable authentication, discovery, generated REST client, SSE event
  ingestion, session operations, and state reconciliation. It has no terminal dependencies.
- `octonomous-tui` — the Ratatui/Crossterm frontend and the `octonomous` executable.

SSE provides low-latency updates, but it is live-only and has no resume token. The core therefore
re-fetches authoritative session state after connecting or reconnecting instead of assuming the
event stream contains everything it missed.

The REST client is generated from the pinned OpenAPI document in
[`docs/openapi-v2.0.18.json`](docs/openapi-v2.0.18.json). See
[`docs/CODEGEN.md`](docs/CODEGEN.md) before updating generated code.

## Development

Run the project checks from the repository root:

```sh
cargo build --workspace
cargo test --workspace --all-targets
scripts/check-core-dependencies.sh
```

Additional documentation:

- [`docs/DESCRIPTION.md`](docs/DESCRIPTION.md) — protocol research, design, and memory measurements
- [`docs/MILESTONE.md`](docs/MILESTONE.md) — implementation milestones and future work
- [`docs/CI.md`](docs/CI.md) — complete verification and live-service checks
- [`docs/CODEGEN.md`](docs/CODEGEN.md) — OpenAPI regeneration workflow

## License

No license has been added yet.
