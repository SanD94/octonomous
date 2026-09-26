# Continuous integration notes

The protocol baseline is OpenCode `v2.0.18`. CI and local verification must use
that exact `opencode-version` when checking the pinned API document or recording
protocol fixtures.

Run the baseline checks from the repository root:

```sh
cargo build --workspace
cargo test --workspace --all-targets
scripts/fetch-openapi.sh
scripts/replay-fixture.sh >/dev/null
scripts/check-core-dependencies.sh
```

`fetch-openapi.sh` discovers the running service with `opencode service status`
unless `OPENCODE_SERVER` or a positional server URL is supplied. It reads the
service password from `$XDG_CONFIG_HOME/opencode/service.json` (falling back to
`~/.config/opencode/service.json`), normalizes both documents, and reports any
drift without modifying the committed specification.

The REST acceptance test uses `wiremock` and runs offline. The live authentication
smoke test is opt-in and expects a running local service:

```sh
cargo test -p octonomous-core --test transport \
  live_authenticated_server_info -- --ignored --exact
```

Reproduce the release-mode client memory figures in DESCRIPTION §1.1 with:

```sh
scripts/measure-client-rss.sh
```
