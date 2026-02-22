# Plan: Split Control Plane (`planterd`) from Sandboxed Executor (`planter-execd`)

## Summary
Move all job and PTY execution out of `planterd` into a per-cell sandboxed worker process (`planter-execd`), with `planterd` remaining privileged control plane and single public API endpoint.

Chosen constraints to implement:
- Per-cell worker model.
- Immediate hard cutover (no legacy in-daemon execution fallback).
- Seatbelt-only worker in phase 1 (same uid as daemon).
- Jobs and PTY migrate together.
- Dedicated internal daemon-worker RPC.
- Breaking cleanup allowed on public API.
- On-demand worker spawn + idle GC.
- Daemon owns durable metadata/log ownership.
- Worker launch/handshake failure fails request hard.
- Worker IPC via inherited socketpair + one-time auth token.

## Execution Tracker
- Status scale: `todo | in_progress | blocked | done`
- Task ID format: `PX-M<milestone>-T<nn>`

| Task ID | Task | Status | Notes |
|---|---|---|---|
| `PX-M0-T01` | Persist migration plan to `plans/` | `done` | Plan saved as `plans/planterd-planter-execd-migration.md` |
| `PX-M1-T01` | Add new workspace members for worker crates | `done` | Added `planter-execd-proto` and `planter-execd` in workspace |
| `PX-M1-T02` | Scaffold internal daemon-worker protocol crate | `done` | Added request/response envelopes and protocol tests |
| `PX-M1-T03` | Scaffold `planter-execd` control loop with Hello+Ping | `done` | Auth-token handshake implemented; unsupported operations return explicit errors |
| `PX-M1-T04` | Add daemon-side worker client primitives | `done` | Added `/crates/planterd/src/worker.rs` with call/hello/ping helpers |
| `PX-M1-T05` | Apply protocol baseline changes (`PROTOCOL_VERSION=2`, `ErrorCode::Unavailable`) | `done` | Updated `planter-core` |
| `PX-M2-T01` | Implement `WorkerManager` process spawn + socketpair handshake | `done` | Added `crates/planterd/src/worker_manager.rs` with socketpair spawn + hello timeout path |
| `PX-M2-T02` | Route `StateStore` job + PTY execution through worker path | `done` | `StateStore` now calls `WorkerManager` for `RunJob`, `JobSignal`, and `Pty*`; added per-cell call serialization to prevent session races during attach |
| `PX-M2-T03` | Remove legacy direct in-daemon job/PTy execution path | `done` | Removed execution fallback branches and deleted daemon PTY module (`crates/planterd/src/pty.rs`) |
| `PX-M2-T04` | Remove `stdout_path`/`stderr_path` from public `JobInfo` and adapt callers | `done` | Added `StoredJobInfo` for on-disk metadata and kept public `JobInfo` path-free |

## Public API / Interface Changes
1. Bump public protocol version from `1` to `2` in `crates/planter-core/src/protocol.rs`.
2. Break `JobInfo` API shape to remove internal filesystem leakage:
- Remove `stdout_path` and `stderr_path` from externally returned `JobInfo`.
- Keep log access exclusively through `LogsRead`.
3. Add `ErrorCode::Unavailable` in `crates/planter-core/src/errors.rs` for worker launch/handshake/runtime unavailable conditions.
4. Keep CLI command verbs mostly stable in `crates/planter/src/main.rs`, but adapt output/parsing to protocol v2 response bodies.
5. Keep `LogsRead` and PTY request families public, but route through worker-backed execution path.

## New Internal Interfaces
1. Add new crate `crates/planter-execd`:
- Runs one worker process per cell.
- Handles job lifecycle, PTY lifecycle, usage probe, and process-tree signaling within sandbox.
2. Add new crate `crates/planter-execd-proto`:
- Internal RPC enums only for daemon-worker, not exposed to CLI.
- Suggested requests: `Hello`, `Ping`, `RunJob`, `JobStatus`, `JobSignal`, `PtyOpen`, `PtyInput`, `PtyRead`, `PtyResize`, `PtyClose`, `UsageProbe`, `Shutdown`.
- Suggested responses: `HelloAck`, `Pong`, `JobStarted`, `JobState`, `PtyOpened`, `PtyChunk`, `PtyAck`, `UsageSample`, `ExecError`.
3. Keep transport in existing `crates/planter-ipc` framing/CBOR, but create a separate worker client wrapper in `planterd`.

## Runtime Boundary and Privilege Split
1. `planterd` responsibilities:
- Owns public socket and API dispatch.
- Owns durable state (job metadata, cell metadata, logs files).
- Compiles/applies seatbelt profile when spawning worker.
- Supervises worker lifecycle (spawn/reconnect/idle-stop).
2. `planter-execd` responsibilities:
- Owns actual child process and PTY creation.
- Never calls nested `sandbox-exec`; it already runs sandboxed.
- Maintains in-memory runtime maps for jobs and PTY sessions.
- Streams PTY output and reports job status/usage through internal RPC.
3. Hard rule:
- If worker cannot be launched/handshaken in enforced mode, return `Unavailable` and do not run workload.

## Data Flow (Decision Complete)
1. `JobRun`:
- `planterd` validates cell/job request, allocates `job_id`, creates durable record as `running`.
- `planterd` ensures worker for cell via WorkerManager.
- `planterd` sends `RunJob` to worker with command/env/cwd and daemon-selected log paths.
- Worker spawns job under its sandbox, writing to provided log files.
- `planterd` tracks completion via `JobStatus` polling or worker completion events and persists terminal state.
2. `PtyOpen`:
- `planterd` ensures worker for cell (or default isolated session cell scope if no explicit cell binding is exposed yet).
- `planterd` sends `PtyOpen`; worker returns session handle.
- `PtyRead/Input/Resize/Close` are pure pass-through to worker.
3. `JobKill`/`CellRemove`:
- `planterd` forwards signals/teardown to worker.
- Worker kills tracked child/process group and returns status.
- `planterd` persists termination reason.
4. Worker crash:
- In-flight requests fail with `Unavailable`.
- Running jobs/sessions in that worker are marked terminal with `termination_reason=unknown` unless reattach succeeds.
- Next request triggers fresh worker spawn.
5. Idle GC:
- Worker stopped after configurable idle timeout only when no running jobs and no open PTY sessions.

## Implementation Steps
1. Protocol and model refactor:
- Update public protocol v2 and API models in `planter-core`.
- Introduce daemon-internal durable structs distinct from public response structs in `planterd` state layer.
2. Introduce worker protocol + binary:
- Add `planter-execd-proto`.
- Add `planter-execd` main with internal dispatcher and runtime registries.
3. Add `planterd` WorkerManager:
- New module in `crates/planterd/src/worker_manager.rs`.
- Spawn worker with socketpair + auth token.
- Perform startup `Hello` handshake and protocol check.
- Cache per-cell worker connection and activity timestamps.
4. Replace direct execution paths in `StateStore`:
- Remove direct calls to `PlatformOps::spawn_job` and `PtyManager` as active path.
- Route `run_job`, `kill_job`, `open_pty`, `pty_*` through WorkerManager.
5. Sandbox launch path changes:
- `planterd` applies seatbelt to worker process, not per-job/per-pty.
- Keep worker profile scoped to that cell + needed runtime paths.
6. Cleanup and hard cutover:
- Delete or fully retire legacy in-daemon execution path for jobs/PTY.
- Keep `planter-platform*` only if still needed for non-execution OS helpers; otherwise remove in follow-up cleanup PR.
7. CLI adaptation:
- Update CLI handling for protocol v2 response models and any changed fields.

## Testing and Acceptance Scenarios
1. Unit tests:
- Worker handshake/auth token validation.
- WorkerManager spawn/reuse/idle-evict behavior.
- Mapping from internal job records to public `JobInfo` v2.
- `Unavailable` error mapping.
2. Integration tests (`planterd` + `planter-execd`):
- Job run/status/logs end-to-end through worker.
- PTY open/attach/read/write/resize/close end-to-end through worker.
- Hard failure on worker launch failure with no legacy fallback.
- Worker crash mid-job and mid-pty behavior.
- Cell removal terminates worker and running processes.
3. Security-focused tests:
- Worker cannot access paths outside allowed sandbox profile.
- Public API no longer exposes raw log file paths.
- Daemon rejects worker handshake with bad token/protocol mismatch.
4. Regression tests:
- Existing CLI flows (`create/run/logs/job status/job kill/cell rm/session attach`) continue functioning semantically with protocol v2 adjustments.
5. Done criteria:
- No request path in `planterd` directly spawns jobs or PTYs.
- All execution operations require healthy worker path.
- Protocol v2 tests pass; protocol v1 intentionally unsupported after cutover.

## Rollout and Operational Controls
1. Add daemon config flags:
- `--worker-idle-timeout-ms` default `300000`.
- `--worker-handshake-timeout-ms` default `2000`.
- `--worker-ping-interval-ms` default `10000`.
2. Add structured logs in `planterd` and `planter-execd`:
- Worker spawn start/success/failure, handshake result, idle shutdown, crash detection, RPC latency buckets.
3. Add health behavior:
- `planterd health` remains `ok` for control-plane health; include degraded detail if worker subsystem is unavailable.

## Assumptions and Defaults
1. macOS remains primary target for this phase; seatbelt runtime `/usr/bin/sandbox-exec` is required.
2. Same-uid execution is acceptable for phase 1; uid/gid drop deferred.
3. JSON-backed durable state remains for now; SQLite migration (planned milestone) happens after worker split stabilizes.
4. Breaking public protocol is accepted now; clients must upgrade with daemon.
5. No compatibility layer for protocol v1 will be maintained in this migration.
