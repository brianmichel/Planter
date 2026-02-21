# Planter Bootstrap Plan Tracker

## Metadata
- Plan ID: `PB-BOOTSTRAP-20260221`
- Created: `2026-02-21`
- Status scale: `todo | in_progress | blocked | done`
- Task ID format: `PB-M<milestone>-T<nn>` (globally unique in this plan)

## Milestone 0 — Repo initialization
| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PB-M0-T01` | Create repo and cargo workspace root | `done` | Workspace created at `/Users/brianmichel/Development/planter` |
| `PB-M0-T02` | Add workspace members (`planter-core`, `planter-ipc`, `planter-platform`, `planter-platform-macos`, `planterd`, `planter`) | `done` | Members wired in root `Cargo.toml` |
| `PB-M0-T03` | Add root `.gitignore` | `done` | Includes Rust and editor artifacts |
| `PB-M0-T04` | Add root `README.md` with daemon/CLI quickstart | `done` | Includes `planterd` and `planter version` usage |
| `PB-M0-T05` | Scaffold all crates | `done` | All six crates present under `crates/` |
| `PB-M0-T06` | Add formatting baseline (`rustfmt.toml`) | `done` | Added at repo root |
| `PB-M0-T07` | Pin toolchain (`rust-toolchain.toml`) | `done` | Pinned to `1.93.0` |
| `PB-M0-T08` | Add mise tooling/tasks (`mise.toml`) | `done` | Tasks include `setup/build/test/smoke` |

## Milestone 1 — Protocol types (`planter-core`)
| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PB-M1-T01` | Define ID newtypes (`CellId`, `JobId`, `ReqId`) | `done` | Implemented in `ids.rs` |
| `PB-M1-T02` | Define `ErrorCode` and `PlanterError` | `done` | Implemented in `errors.rs` |
| `PB-M1-T03` | Add time helper `now_ms()` | `done` | Implemented in `time.rs` |
| `PB-M1-T04` | Define request/response envelopes | `done` | `RequestEnvelope<T>`, `ResponseEnvelope<T>` in `protocol.rs` |
| `PB-M1-T05` | Define minimal request enum (`Version`, `Health`) | `done` | Implemented in `protocol.rs` |
| `PB-M1-T06` | Define minimal response enum (`Version`, `Health`, `Error`) | `done` | Implemented in `protocol.rs` |
| `PB-M1-T07` | Define `PROTOCOL_VERSION = 1` | `done` | Implemented in `protocol.rs` |
| `PB-M1-T08` | Add CBOR roundtrip tests | `done` | `tests/protocol_roundtrip.rs` |

## Milestone 2 — IPC framing and codec (`planter-ipc`)
| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PB-M2-T01` | Implement length-delimited frame writer | `done` | 4-byte BE length + payload |
| `PB-M2-T02` | Implement length-delimited frame reader | `done` | Enforces max frame size |
| `PB-M2-T03` | Enforce max frame size (8 MiB) | `done` | `MAX_FRAME_SIZE` guard |
| `PB-M2-T04` | Implement CBOR encode/decode helpers | `done` | `codec.rs` |
| `PB-M2-T05` | Define IPC error model (`IpcError`) | `done` | Includes io/encode/decode/timeout/mismatch |
| `PB-M2-T06` | Implement minimal client connect/call | `done` | `PlanterClient` with 5s timeout |
| `PB-M2-T07` | Implement Unix socket server + dispatcher trait | `done` | `serve_unix` + `RequestHandler` |
| `PB-M2-T08` | Handle malformed request behavior | `done` | Returns structured error when req_id recoverable, else closes |
| `PB-M2-T09` | Add framing tests | `done` | Roundtrip, oversized, truncated |
| `PB-M2-T10` | Add client/server integration test | `done` | `Version` + `Health` verified |

## Milestone 3 — Daemon (`planterd`)
| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PB-M3-T01` | Parse daemon args (`--socket`) | `done` | Default `/tmp/planterd.sock` |
| `PB-M3-T02` | Handle pre-existing socket path safely | `done` | Remove if socket, fail otherwise |
| `PB-M3-T03` | Start IPC server from daemon main | `done` | Uses `serve_unix` |
| `PB-M3-T04` | Log startup with protocol/version | `done` | `tracing` startup line |
| `PB-M3-T05` | Implement request handlers (`Version`, `Health`) | `done` | `handlers.rs` |
| `PB-M3-T06` | Wire dispatcher to IPC trait | `done` | `dispatch.rs` |

## Milestone 4 — CLI (`planter`)
| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PB-M4-T01` | Add CLI parser + global `--socket` | `done` | Clap CLI implemented |
| `PB-M4-T02` | Add `version` subcommand RPC | `done` | Prints `planterd <daemon> (protocol <protocol>)` |
| `PB-M4-T03` | Add `health` subcommand RPC | `done` | Prints `ok` |
| `PB-M4-T04` | Add RPC/daemon error handling and non-zero exit | `done` | Handles transport + daemon errors + unexpected variants |

## Milestone 5 — First real functionality (stubbed)
| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PB-M5-T01` | Extend protocol with `CellCreate` and `JobRun` | `done` | Added request + response variants |
| `PB-M5-T02` | Add `CellSpec`, `CommandSpec`, `CellInfo`, `JobInfo`, `ExitStatus` | `done` | Added in `planter-core` protocol types |
| `PB-M5-T03` | Add file-backed state at `~/.planter/state/` | `done` | `planterd` stores JSON metadata under `cells/`, `jobs/`, `logs/` |
| `PB-M5-T04` | Implement process spawn + stdout/stderr log capture | `done` | `JobRun` spawns process and captures logs to files |
| `PB-M5-T05` | Implement `planter logs <job_id> -f` v1 (file tail) | `done` | CLI supports `logs` and `logs -f` direct file tail |

## Milestone 6 — macOS backend skeleton (out of bootstrap scope)
| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PB-M6-T01` | Define `PlatformOps` trait boundary | `todo` | Deferred |
| `PB-M6-T02` | Hook `planterd` to platform abstraction | `todo` | Deferred |
| `PB-M6-T03` | Implement `MacosOps` basic placeholder | `todo` | Deferred |
| `PB-M6-T04` | Add TODO stubs (`compile_sandbox_profile`, `spawn_sandboxed`, `lease_user`) | `todo` | Deferred |

## Milestone 7 — Dev ergonomics (out of bootstrap scope)
| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PB-M7-T01` | Add Makefile/justfile shortcuts | `todo` | Deferred |
| `PB-M7-T02` | Add standalone `scripts/smoke.sh` | `todo` | Deferred |
| `PB-M7-T03` | Add CI bootstrap workflow | `todo` | Deferred |

## Bootstrap acceptance checks
| Check ID | Validation | Status |
|---|---|---|
| `PB-AC-01` | `cargo build` succeeds | `done` |
| `PB-AC-02` | `planterd` starts on `/tmp/planterd.sock` | `done` |
| `PB-AC-03` | `planter version` prints daemon + protocol | `done` |
| `PB-AC-04` | `planter health` prints `ok` | `done` |
