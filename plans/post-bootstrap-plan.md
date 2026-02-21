# Planter Post-Bootstrap Plan Tracker

## Metadata
- Plan ID: `PB-POSTBOOTSTRAP-20260221`
- Created: `2026-02-21`
- Status scale: `todo | in_progress | blocked | done`
- Task ID format: `PBX-M<milestone>-T<nn>` (globally unique in this plan)
- Check ID format: `PBX-AC-<nn>`

## Milestone 8 — Sandbox Enforcement v1
| Task ID | Task | Status | Depends On | Notes |
|---|---|---|---|---|
| `PBX-M8-T01` | Add daemon config `sandbox_mode` with values `disabled|permissive|enforced` | `done` | - | Added `--sandbox-mode` in daemon with default `permissive` |
| `PBX-M8-T02` | Implement `spawn_sandboxed` to invoke macOS sandbox runtime with compiled profile | `done` | `PBX-M8-T01` | Uses `/usr/bin/sandbox-exec -f <profile>` in macOS backend |
| `PBX-M8-T03` | Restrict profile permissions to state root, cell dir, temp, and explicit runtime needs | `done` | `PBX-M8-T02` | Profile fragments now limit write paths and narrow runtime reads |
| `PBX-M8-T04` | In `enforced` mode, fail job start on sandbox launch/profile errors; in `permissive`, log + fallback | `done` | `PBX-M8-T02` | Implemented strict fail vs warning fallback behavior |
| `PBX-M8-T05` | Add integration tests for allowed/denied filesystem behavior | `done` | `PBX-M8-T03` | Added macOS backend tests for enforced-mode denied writes and profile rendering checks |

## Milestone 9 — Job Lifecycle RPCs
| Task ID | Task | Status | Depends On | Notes |
|---|---|---|---|---|
| `PBX-M9-T01` | Add lifecycle request/response protocol variants in `planter-core` | `done` | - | Added `JobStatus`, `JobKill`, `CellRemove` request/response variants |
| `PBX-M9-T02` | Implement daemon handlers and dispatcher wiring for lifecycle RPCs | `done` | `PBX-M9-T01` | Handlers now serve status/kill/remove via state store |
| `PBX-M9-T03` | Wire `kill_job_tree` into `JobKill` and persist termination reason | `done` | `PBX-M9-T02` | Force/non-force kill wired with persisted termination reason |
| `PBX-M9-T04` | Add CLI subcommands for lifecycle operations | `done` | `PBX-M9-T02` | Added `planter job status`, `planter job kill`, `planter cell rm` |
| `PBX-M9-T05` | Add integration tests for lifecycle flows and edge cases | `done` | `PBX-M9-T04` | Added handler-driven lifecycle tests including running-cell removal failure |

## Milestone 10 — Logs over IPC
| Task ID | Task | Status | Depends On | Notes |
|---|---|---|---|---|
| `PBX-M10-T01` | Add `LogsRead`/`LogsChunk` protocol with offset+follow semantics | `done` | - | Added protocol variants with stream/offset/max_bytes/follow/wait_ms |
| `PBX-M10-T02` | Implement daemon log reader service with bounded chunk size and wait timeout | `done` | `PBX-M10-T01` | Implemented state-backed log chunk reads with follow timeout polling |
| `PBX-M10-T03` | Update CLI `logs`/`logs -f` to use IPC only | `done` | `PBX-M10-T02` | CLI logs now call `LogsRead` RPC instead of direct file reads |
| `PBX-M10-T04` | Add tests for offset resume, follow polling, stderr selection | `done` | `PBX-M10-T03` | Added log-read coverage in handler lifecycle test and protocol roundtrip tests |

## Milestone 11 — Durable State (SQLite)
| Task ID | Task | Status | Depends On | Notes |
|---|---|---|---|---|
| `PBX-M11-T01` | Introduce SQLite schema + migration runner for cells/jobs/events | `todo` | - | DB under state root |
| `PBX-M11-T02` | Replace JSON metadata reads/writes with SQLite repository layer | `todo` | `PBX-M11-T01` | Keep log files filesystem-backed |
| `PBX-M11-T03` | Add startup reconciliation for jobs across daemon restarts | `todo` | `PBX-M11-T02` | Mark stale/running jobs correctly |
| `PBX-M11-T04` | Add migration/backfill from existing JSON metadata | `todo` | `PBX-M11-T02` | One-way bootstrap migration |
| `PBX-M11-T05` | Add persistence/restart integration tests | `todo` | `PBX-M11-T04` | Restart retains and reconciles state |

## Milestone 12 — Socket Authz and Hardening
| Task ID | Task | Status | Depends On | Notes |
|---|---|---|---|---|
| `PBX-M12-T01` | Enforce secure socket file mode/ownership on startup | `todo` | - | Validate before accept loop |
| `PBX-M12-T02` | Extract peer credentials and authorize by uid/group policy | `todo` | `PBX-M12-T01` | Reject unauthorized clients |
| `PBX-M12-T03` | Add daemon config for allowed group and strict mode toggle | `todo` | `PBX-M12-T02` | Safe local-dev defaults |
| `PBX-M12-T04` | Add authz denial/success tests | `todo` | `PBX-M12-T03` | Explicit error responses |

## Milestone 13 — Limits and Monitoring
| Task ID | Task | Status | Depends On | Notes |
|---|---|---|---|---|
| `PBX-M13-T01` | Add `ResourceLimits` to command model and persistence | `todo` | `PBX-M11-T02` | timeout/rss/log quotas |
| `PBX-M13-T02` | Add monitor loop using `probe_usage` + elapsed-time checks | `todo` | `PBX-M13-T01` | periodic sampler task |
| `PBX-M13-T03` | Enforce limits by terminating jobs and recording reason | `todo` | `PBX-M13-T02` | timeout/rss/log-quota reasons |
| `PBX-M13-T04` | Surface limit results in CLI status output | `todo` | `PBX-M13-T03` | actionable termination metadata |
| `PBX-M13-T05` | Add tests for each limit type and races | `todo` | `PBX-M13-T04` | deterministic threshold tests |

## Milestone 14 — Release-grade DX and CI
| Task ID | Task | Status | Depends On | Notes |
|---|---|---|---|---|
| `PBX-M14-T01` | Expand `mise` tasks for integration suites and strict checks | `todo` | - | Keep mise as single task surface |
| `PBX-M14-T02` | Add CI matrix for lint/test/smoke/integration on macOS | `todo` | `PBX-M14-T01` | Merge gating |
| `PBX-M14-T03` | Add operator docs for sandbox modes, authz, limits, troubleshooting | `todo` | `PBX-M8-T04`,`PBX-M12-T03`,`PBX-M13-T03` | Include recovery flows |
| `PBX-M14-T04` | Add release checklist + versioning process | `todo` | `PBX-M14-T02` | tag/changelog/artifact checks |

## Post-Bootstrap Acceptance Checks
| Check ID | Validation | Status |
|---|---|---|
| `PBX-AC-01` | Enforced sandbox mode blocks disallowed filesystem writes | `done` |
| `PBX-AC-02` | CLI can status/kill/remove resources entirely via RPC | `done` |
| `PBX-AC-03` | `planter logs -f` uses IPC and supports resume from offset | `done` |
| `PBX-AC-04` | Restarting daemon preserves and reconciles state from SQLite | `todo` |
| `PBX-AC-05` | Unauthorized socket clients are rejected with explicit errors | `todo` |
| `PBX-AC-06` | Limits terminate jobs with correct reason and observable status | `todo` |
| `PBX-AC-07` | CI passes lint/test/smoke/integration on main branch | `todo` |
