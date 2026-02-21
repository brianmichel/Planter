# planter

Planter is a local process orchestration prototype with a daemon + CLI split.
`planterd` serves local RPC over a Unix socket using CBOR payloads in framed messages.
`planter` sends requests to the daemon and renders command-friendly output.
The bootstrap scope includes `Version`, `Health`, `CellCreate`, and `JobRun`.
Protocol version is currently fixed to `1`.

Tooling is managed with `mise.toml` (Rust 1.93.0 + standard tasks):
`mise run setup`, `mise run build`, `mise run test`, and `mise run smoke`.

Run daemon directly:
`cargo run -p planterd -- --socket /tmp/planterd.sock`

Run CLI version check directly:
`cargo run -p planter -- --socket /tmp/planterd.sock version`

Create a cell:
`cargo run -p planter -- --socket /tmp/planterd.sock create --name demo`

Run a job in that cell:
`cargo run -p planter -- --socket /tmp/planterd.sock run <cell_id> -- /bin/sh -c 'echo hello'`

Read logs:
`cargo run -p planter -- --socket /tmp/planterd.sock logs <job_id>`

State directory defaults to `~/.planter/state` and can be overridden with `PLANTER_STATE_DIR`.
