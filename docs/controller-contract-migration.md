# Deterministic controller implementation sequence

The approved controller architecture is pinned by SHA-256
`9dc15b15988440ad8b2307f2d20af76431012db3c8c64a3e2764c0abbda4c722`.
The external artifact remains the design authority. This page records only the
repository implementation sequence and compatibility gates.

## Completed phases

The first implementation slice adds
`crates/board-core/tests/fixtures/controller_contract_characterization.json`
and `crates/board-core/tests/controller_contract_characterization.rs`. Together
they pin the nine observed failures, their owning crates, acceptance scenario
IDs, and the boundaries that later work must preserve:

- tab labels are presentation and exact IDs remain identity;
- pane names stay card based;
- legacy `card-<id>` and pre-v11 `kanban` behavior is not adopted by label;
- descendants resolve to one canonical Git root;
- a Human Gate checkpoint binds its request, artifact digest, effect, and
  authority before any decision can apply.

The characterization slice changed no production behavior or SQLite shape.

Schema v16 is the first production slice. It adds durable storage and
constraints for controller events, card contracts, review receipts, Human Gate
checkpoints and decisions, mutable-target leases, effect outbox rows, workspace
bindings, runtime ownership, executor-session lineage, Board-aware steering,
watchdog lease/health, and diagnostics. Existing operational rows are not
rewritten and runtime code does not populate the new tables in this phase. It
also adds todo projection items and reconciliation receipts so future resumes
rebuild commitments from Board events rather than transcript memory, preserve
unbound commitments explicitly, and report decision coverage and drift.

The steering schema incorporates the approved live and direct-tab contracts.
Human input is persisted before delivery with exact card, run, and executor
session identity; classified as `guidance`, `answer`, `scope_change`, or
`protected_decision`; deduplicated; and tracked through queued, delivered,
acknowledged, or rejected. Scope changes wait for future contract reduction,
protected decisions reference Human Gate, and executor replacement records the
prior session, replacement session, and cause.

The orchestrator watch bridge is explicitly two-ended: a worker Pi extension
captures `pi.on("input")` text, source, and `steer`/`followUp` behavior before
processing; boardd persists and publishes it; an orchestrator Pi extension
consumes by durable cursor and presents a deduplicated custom message in the
main chat. Herdr output is only a health sensor. The orchestrator responds in
the main chat and does not inject review objections into the worker tab.

Persistence adopts the reviewed Notebook patterns without its runtime: SQLite
events remain the append-only authority; card/project snapshots and executor
session checkpoints carry generation, parent generation, hash, validated
manifest, and complete/writing state; conflicts preserve baseline, current, and
candidate; critical bindings can be pinned; and startup restores the latest
complete generation with fallback. Board does not serialize Deno/V8 heaps or
arbitrary objects, and acknowledged external effects are not rolled back.

The pure reducer slice adds `board-core::engine::controller` without activating
the daemon or writing schema-v16 rows. Its public, synchronous decisions cover:

- idempotent event replay ordered by causation, per-source sequence, and ingest
  sequence, with changed-digest conflicts failing closed;
- review classification into approve-only, approve-with-follow-up,
  revision-required, or reject, while unmarked approved feedback requires an
  exact Human Gate classification;
- executable eligibility and stable candidate ordering by priority, deadline,
  readiness age, and card id;
- aggregate reduction that distinguishes pre-prompt infrastructure
  interruption from started-work failure and binds gate decisions to exact
  request and artifact digest;
- role-derived tab cardinality that never treats a label as ownership; and
- conservative recovery plans for restart/outbox uncertainty, dead review
  shells, stale queue entries, duplicate exact tab IDs, close acknowledgement,
  topology drift, and unknown lock liveness.

These values are effect intents only. They perform no I/O and do not make
boardd consume controller events yet.

## Ordered migration plan

1. **Complete:** add the pure `board-core::engine` reducer, receipt classifier,
   deterministic candidate ordering, tab-cardinality policy, and recovery
   planner.
2. Add daemon ingestion and controller operations. One transaction accepts an
   event, reduces the aggregate, acquires locks, and writes effects. External
   I/O runs only from the committed outbox and records an acknowledgement.
3. Add exact-ID Herdr effects for rename, child cleanup, and tab close. Startup
   reconciliation runs before queue dispatch. Unknown liveness never releases
   a lock or closes an unproven tab.
4. Add canonical `(session, repo_root) -> workspace_id` admission. A descendant
   remains `task_path`; launch cwd is the canonical Git root. Conflicts block
   dispatch instead of selecting by label, focus, or recency.
5. Add the structured Human Gate protocol and TUI actions Approve, Request
   changes, and Block. A decision writes a durable event and the controller
   automatically advances only when the resulting aggregate is eligible.
6. Add Board-aware Steer actions in card detail and the associated tab bridge.
   Persist before delivering to the exact executor session; queue when no
   executor is active; keep Verification fresh/read-only; route scope changes
   through card-contract reduction and protected decisions through Human Gate.
7. Reconcile the visible todo projection after every material event and before
   user-facing responses. Link each commitment to card/phase/artifact/owner and
   next predicate, keep unclassified items `unbound`, and reconstruct from
   Board on start/resume with explicit coverage and drift diagnostics.
8. Add the Pi worker/orchestrator watch extensions and boardd event relay. Only
   boardd may pause effects for lock conflict, protected/destructive decisions,
   or unauthorized scope changes; cursor replay and idempotency survive resume
   and restart without injecting messages into the worker.
9. Materialize generation/hash snapshots and session-id checkpoints from the
   journal, preserve three-way conflicts, honor pins, and restore the latest
   complete generation before dispatch on startup.
10. Add provider-free E2E scenarios for restart/outbox replay, stale queue,
   duplicate tabs, dead review shell, digest conflict, label migration,
   cardinality, cleanup failure, and workspace-root admission.

## Legacy fallback

Legacy cards continue under their existing launch contract until an authorized
contract update supplies project and outcome display codes. Reconciliation may
rename only the tab reconstructed from durable pane identity. It never scans
labels to claim a tab. Pre-v11 shared `kanban` placement requires a separate,
explicit migration and is untouched by lazy card migration.

## Gate

After the pure reducer passes the Docker sandbox gates and isolated live Herdr
E2E suite, daemon ingestion is the next legal implementation slice. Controller
activation still requires migration rehearsal on a copy, rollback, local
plugin replacement, and a controlled daemon restart. The user Herdr session is
never a test target.
