# planter

Planter is a local process orchestration prototype with a daemon + CLI split.
`planterd` serves local RPC over a Unix socket using CBOR payloads in framed messages.
`planter` sends requests to the daemon and renders command-friendly output.
Current scope includes lifecycle and log RPCs: `Version`, `Health`, `CellCreate`, `JobRun`,
`JobStatus`, `JobKill`, `CellRemove`, `LogsRead`, and PTY session RPCs
(`PtyOpen`, `PtyInput`, `PtyRead`, `PtyResize`, `PtyClose`).
Protocol version is currently fixed to `1`.

Tooling is managed with `mise.toml` (Rust 1.93.0 + standard tasks):
`mise run setup`, `mise run build`, `mise run lint`, `mise run test`, and `mise run smoke`.

Run daemon directly:
`cargo run -p planterd -- --socket /tmp/planterd.sock`

Run daemon with explicit sandbox mode:
`cargo run -p planterd -- --socket /tmp/planterd.sock --sandbox-mode enforced`

Run CLI version check directly:
`cargo run -p planter -- --socket /tmp/planterd.sock version`

Create a cell:
`cargo run -p planter -- --socket /tmp/planterd.sock create --name demo`

Run a job in that cell:
`cargo run -p planter -- --socket /tmp/planterd.sock run <cell_id> -- /bin/sh -c 'echo hello'`

Read logs:
`cargo run -p planter -- --socket /tmp/planterd.sock logs <job_id> -f`

Get job status:
`cargo run -p planter -- --socket /tmp/planterd.sock job status <job_id>`

Kill a job:
`cargo run -p planter -- --socket /tmp/planterd.sock job kill <job_id> --force`

Remove a cell:
`cargo run -p planter -- --socket /tmp/planterd.sock cell rm <cell_id> --force`

Open an interactive PTY session:
`cargo run -p planter -- --socket /tmp/planterd.sock session open --shell /bin/zsh`

Attach to a PTY session:
`cargo run -p planter -- --socket /tmp/planterd.sock session attach <session_id>`

PTY sessions default to an isolated per-session build directory
(`<state>/sessions/pty-<id>/build-cell`) and
an anonymous-style shell environment (`HOME`, `USER`, `LOGNAME`, `ZDOTDIR`).
For default zsh/bash sessions, planter installs a session-local rc file that blocks
`cd` outside the per-session build-cell and recenters cwd on each prompt.
The daemon also disables shell profile loading by default for `bash`/`zsh`.
PTY shells now launch via `sandbox-exec` when sandbox mode is `enforced`
(or best-effort with fallback in `permissive` mode).
The OS-level uid is unchanged in this bootstrap implementation.

State directory defaults to `~/.planter/state` and can be overridden with `PLANTER_STATE_DIR`.
