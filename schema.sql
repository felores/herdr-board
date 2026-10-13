-- herdr-board SQLite schema (WAL mode; boardd is the only writer).
-- This file is the CURRENT (schema v16) shape: a fresh DB is created directly
-- from it and stamped `PRAGMA user_version = 16`. Existing databases are upgraded
-- by migrations in board-core/src/db/migrations.rs (kept in sync with this file).
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- A Project is a named collection of boards, identified by a canonical
-- filesystem path (a Git root or a plain existing directory). The special
-- Global project (id=1, scope_path NULL) preserves the pre-v14 flat boards.
-- archived_at is NULL while the project is active; archiving is reversible
-- and never deletes boards, cards, history, or the reserved scope path.
CREATE TABLE projects (
  id          INTEGER PRIMARY KEY,
  scope_path  TEXT,
  archived_at TEXT,
  created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Canonical path identity is unique for scoped projects while allowing exactly
-- one special Global row (NULL).
CREATE UNIQUE INDEX idx_projects_scope_path ON projects(scope_path) WHERE scope_path IS NOT NULL;

-- Boards belong to exactly one project. Names are unique case-insensitively
-- within the same project (COLLATE NOCASE on the UNIQUE index); the same name
-- on a different project is legal. Every project's first board is named 'main'.
-- archived_at is NULL while the board is active; archiving is reversible and
-- never deletes columns, cards, comments, runs, or the reserved name.
CREATE TABLE boards (
  id          INTEGER PRIMARY KEY,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name        TEXT NOT NULL COLLATE NOCASE,
  archived_at TEXT,
  created_at  TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE (project_id, name)
);

-- Persistent context: the selected project (singleton row; absent = no
-- selection yet, e.g. right after migration).
CREATE TABLE selection (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- The selected board per project (a project always has a selected board once
-- it has been used; otherwise its first board 'main' is the context board).
CREATE TABLE board_selection (
  project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  board_id   INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Recency (most recent first, capped at 3 per project / 3 projects).
CREATE TABLE project_recents (
  project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  rank       INTEGER NOT NULL,
  used_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE board_recents (
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  board_id   INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
  rank       INTEGER NOT NULL,
  used_at    TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (project_id, board_id)
);

-- A fresh board gets exactly one seeded column: 'Todo' (trigger=manual).
-- Everything else (names, count, order, config) is user-created.
CREATE TABLE columns (
  id                   INTEGER PRIMARY KEY,
  board_id             INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
  name                 TEXT NOT NULL,
  position             INTEGER NOT NULL,
  system_prompt        TEXT,                -- prepended via --append-system-prompt
  trigger              TEXT NOT NULL DEFAULT 'manual'
                         CHECK (trigger IN ('manual','auto')),
  on_success_column_id INTEGER REFERENCES columns(id) ON DELETE SET NULL,
  on_fail_column_id    INTEGER REFERENCES columns(id) ON DELETE SET NULL,
  fresh_session        INTEGER NOT NULL DEFAULT 0,  -- 1 = never --resume in this column
  harness_override     TEXT,
  model_override       TEXT,
  effort_override      TEXT,
  permission_override  TEXT,
  timeout_minutes      INTEGER,
  UNIQUE (board_id, name)
);

CREATE TABLE cards (
  id              INTEGER PRIMARY KEY,
  board_id        INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
  column_id       INTEGER NOT NULL REFERENCES columns(id),
  position        INTEGER NOT NULL,
  title           TEXT NOT NULL,
  description     TEXT NOT NULL DEFAULT '',   -- the base prompt
  harness         TEXT NOT NULL DEFAULT 'pi',
  model           TEXT,
  effort          TEXT CHECK (effort IN (NULL,'off','minimal','low','medium','high','xhigh','max')),
  permission_mode TEXT,                        -- e.g. acceptEdits, plan; bypass = explicit opt-in
  session         TEXT,                        -- herdr session name; NULL = daemon's default session
  space_kind      TEXT NOT NULL DEFAULT 'workspace'
                    CHECK (space_kind IN ('workspace','new_workspace')),
  space_ref       TEXT,                        -- workspace id (workspace) | new-workspace label (new_workspace)
  space_cwd       TEXT,                        -- working dir when space_kind='new_workspace'
  status          TEXT NOT NULL DEFAULT 'idle'
                    CHECK (status IN ('idle','queued','running','blocked','failed','awaiting','done')),
  awaiting_reason TEXT,                        -- 'agent_done'|'idle_expired'; set in awaiting, NULL otherwise
  session_id      TEXT,                        -- harness conversation id for --resume
  created_at      TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
  archived_at     TEXT,                        -- NULL = active; timestamp = archived
  CHECK (
    (status = 'awaiting' AND awaiting_reason IS NOT NULL
      AND awaiting_reason IN ('agent_done','idle_expired'))
    OR
    (status <> 'awaiting' AND awaiting_reason IS NULL)
  )
);

CREATE TABLE comments (
  id         INTEGER PRIMARY KEY,
  card_id    INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  author     TEXT NOT NULL,                    -- 'user' | 'agent:<run_id>' | 'system'
  body       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  deleted_at TEXT                              -- NULL = current; timestamp = soft-deleted
);

-- Immutable snapshots of every user-visible comment state.  The current row
-- remains in `comments` so its identity and ownership are stable; history is
-- retained after a soft delete for audit and authorization decisions.
CREATE TABLE comment_history (
  id         INTEGER PRIMARY KEY,
  comment_id INTEGER NOT NULL REFERENCES comments(id) ON DELETE CASCADE,
  card_id    INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  author     TEXT NOT NULL,
  body       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  deleted_at TEXT
);

CREATE INDEX idx_comment_history_comment ON comment_history(comment_id, id);

-- System and daemon-generated comments use the same insertion path as user
-- comments (including run finalization), so every durable comment gets its
-- initial audit snapshot without relying on one caller remembering to do so.
CREATE TRIGGER comments_audit_insert
AFTER INSERT ON comments
BEGIN
  INSERT INTO comment_history(comment_id, card_id, author, body, created_at, deleted_at)
  VALUES(NEW.id, NEW.card_id, NEW.author, NEW.body, NEW.created_at, NEW.deleted_at);
END;

CREATE TABLE runs (
  id                 INTEGER PRIMARY KEY,
  card_id            INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  column_id          INTEGER NOT NULL REFERENCES columns(id),
  harness            TEXT NOT NULL,
  argv_json          TEXT NOT NULL,
  prompt_snapshot    TEXT NOT NULL,
  system_prompt_snapshot TEXT,                    -- NULL marks a legacy pre-v7 launch
  launch_spec_json TEXT,                          -- NULL marks a pre-v11 launch
  herdr_workspace_id TEXT,
  herdr_pane_id      TEXT,
  herdr_anchor_pane_id TEXT,                 -- exact board-owned card-tab shell anchor
  session_id         TEXT,                     -- harness conversation id (--resume)
  session            TEXT,                     -- herdr session name; NULL = default session
  started_at         TEXT,
  timeout_deadline_at_ms INTEGER,             -- durable wall-clock deadline; NULL = unlimited
  timeout_paused_at_ms INTEGER,               -- awaiting began; NULL while timeout is running
  ended_at           TEXT,
  outcome            TEXT CHECK (outcome IN (NULL,'ok','fail','cancelled','lost')),
  result_summary     TEXT,
  log_path           TEXT
);

CREATE INDEX idx_cards_column   ON cards(column_id, position);
CREATE INDEX idx_comments_card  ON comments(card_id, created_at);
CREATE INDEX idx_runs_card      ON runs(card_id, started_at);
-- A card has at most one queued/running/awaiting lifecycle at a time.
CREATE UNIQUE INDEX idx_runs_one_open_per_card ON runs(card_id) WHERE ended_at IS NULL;
CREATE INDEX idx_runs_queued_fifo ON runs(id) WHERE started_at IS NULL AND ended_at IS NULL;
CREATE INDEX idx_runs_active_open ON runs(id) WHERE started_at IS NOT NULL AND ended_at IS NULL;

-- Schema v16 adds the durable state plane for the deterministic controller.
-- This slice is storage-only: later reducer/daemon slices are the sole writers
-- of these rows. Existing card/run lifecycle columns remain authoritative until
-- those slices activate the controller.
CREATE TABLE IF NOT EXISTS controller_events (
  ingest_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id         TEXT NOT NULL UNIQUE,
  idempotency_key  TEXT NOT NULL UNIQUE,
  source           TEXT NOT NULL CHECK (source IN
                     ('direct_user','board','flow','plannotator','harness','git',
                      'aos','herdr','portfolio','watchdog')),
  source_id        TEXT NOT NULL,
  card_id          INTEGER REFERENCES cards(id) ON DELETE CASCADE,
  project_ref      TEXT,
  observed_at      TEXT NOT NULL,
  causation_id     TEXT,
  source_sequence  INTEGER,
  payload_digest   TEXT NOT NULL,
  payload_json     TEXT NOT NULL,
  accepted_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_controller_events_card_sequence
  ON controller_events(card_id, ingest_sequence);
CREATE INDEX IF NOT EXISTS idx_controller_events_causation
  ON controller_events(causation_id);

CREATE TABLE IF NOT EXISTS card_contracts (
  card_id                     INTEGER PRIMARY KEY REFERENCES cards(id) ON DELETE CASCADE,
  contract_version            TEXT NOT NULL DEFAULT 'board-card/v1',
  revision                    INTEGER NOT NULL CHECK (revision >= 1),
  contract_json               TEXT NOT NULL,
  project_code                TEXT,
  outcome_code                TEXT,
  task_path                   TEXT,
  repo_root                   TEXT,
  human_context               TEXT NOT NULL DEFAULT '',
  projection_revision         INTEGER NOT NULL DEFAULT 0 CHECK (projection_revision >= 0),
  projection_source_event_id  TEXT,
  rendered_description_digest TEXT,
  updated_at                  TEXT NOT NULL DEFAULT (datetime('now')),
  CHECK (project_code IS NULL OR (length(project_code) BETWEEN 1 AND 20)),
  CHECK (outcome_code IS NULL OR
         (length(outcome_code) BETWEEN 1 AND 20
          AND outcome_code NOT GLOB '*[^A-Za-z0-9_-]*'
          AND outcome_code GLOB '[A-Za-z0-9]*'))
);

CREATE TABLE IF NOT EXISTS review_receipts (
  receipt_id       TEXT PRIMARY KEY,
  card_id          INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  artifact_path    TEXT NOT NULL,
  artifact_digest  TEXT NOT NULL,
  receipt_digest   TEXT NOT NULL UNIQUE,
  outcome          TEXT NOT NULL CHECK (outcome IN ('approved','changes_requested','rejected')),
  classification   TEXT CHECK (classification IN
                     ('approve_only','approve_with_follow_up','revision_required','reject')),
  notes_json       TEXT NOT NULL DEFAULT '[]',
  source_event_id  TEXT,
  observed_at      TEXT NOT NULL,
  created_at       TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE (card_id, receipt_id)
);
CREATE INDEX IF NOT EXISTS idx_review_receipts_card
  ON review_receipts(card_id, created_at);

CREATE TABLE IF NOT EXISTS human_gate_checkpoints (
  request_id        TEXT PRIMARY KEY,
  card_id           INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  revision          INTEGER NOT NULL CHECK (revision >= 1),
  gate_kind         TEXT NOT NULL CHECK (gate_kind IN
                      ('product_decision','artifact_approval','permission','spend',
                       'publish','destructive','credentials','feedback_classification')),
  question          TEXT NOT NULL,
  options_json      TEXT NOT NULL,
  artifact_path     TEXT NOT NULL,
  artifact_digest   TEXT NOT NULL,
  effect_json       TEXT NOT NULL,
  authority_json    TEXT NOT NULL,
  status            TEXT NOT NULL DEFAULT 'pending' CHECK (status IN
                      ('pending','approved','changes_requested','blocked','superseded')),
  created_event_id  TEXT NOT NULL,
  resolved_event_id TEXT,
  created_at        TEXT NOT NULL DEFAULT (datetime('now')),
  resolved_at       TEXT,
  UNIQUE (card_id, request_id, revision)
);
CREATE INDEX IF NOT EXISTS idx_human_gate_pending
  ON human_gate_checkpoints(card_id, request_id) WHERE status='pending';

CREATE TABLE IF NOT EXISTS human_gate_decisions (
  id                  INTEGER PRIMARY KEY,
  decision_id         TEXT NOT NULL UNIQUE,
  request_id          TEXT NOT NULL REFERENCES human_gate_checkpoints(request_id) ON DELETE CASCADE,
  action              TEXT NOT NULL CHECK (action IN ('approve','request_changes','block')),
  actor               TEXT NOT NULL,
  checkpoint_revision INTEGER NOT NULL CHECK (checkpoint_revision >= 1),
  artifact_digest     TEXT NOT NULL,
  rationale           TEXT,
  event_id            TEXT UNIQUE,
  created_at          TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_human_gate_decisions_request
  ON human_gate_decisions(request_id, id);

CREATE TABLE IF NOT EXISTS mutable_target_leases (
  target_uri   TEXT PRIMARY KEY,
  card_id      INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  run_id       INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  lease_epoch  INTEGER NOT NULL CHECK (lease_epoch >= 1),
  acquired_at  TEXT NOT NULL,
  expires_at   TEXT NOT NULL,
  heartbeat_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mutable_target_leases_run
  ON mutable_target_leases(run_id);

CREATE TABLE IF NOT EXISTS effect_outbox (
  effect_id            TEXT PRIMARY KEY,
  card_id              INTEGER REFERENCES cards(id) ON DELETE CASCADE,
  causation_event_id   TEXT NOT NULL,
  effect_kind          TEXT NOT NULL,
  idempotency_key      TEXT NOT NULL UNIQUE,
  payload_json         TEXT NOT NULL,
  status               TEXT NOT NULL DEFAULT 'pending' CHECK (status IN
                         ('pending','executing','acknowledged','failed')),
  attempt_count        INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
  available_at         TEXT NOT NULL DEFAULT (datetime('now')),
  last_attempt_at      TEXT,
  acknowledged_at      TEXT,
  acknowledgement_json TEXT,
  last_error           TEXT,
  created_at           TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_effect_outbox_ready
  ON effect_outbox(status, available_at, effect_id)
  WHERE status IN ('pending','failed');

CREATE TABLE IF NOT EXISTS workspace_bindings (
  id                  INTEGER PRIMARY KEY,
  project_id          INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  herdr_session       TEXT NOT NULL,
  repo_root           TEXT NOT NULL,
  workspace_id        TEXT NOT NULL,
  expected_launch_cwd TEXT NOT NULL,
  binding_revision    INTEGER NOT NULL CHECK (binding_revision >= 1),
  binding_digest      TEXT NOT NULL,
  source_event_id     TEXT,
  active              INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0,1)),
  created_at          TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
  CHECK (repo_root = expected_launch_cwd)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_bindings_active_root
  ON workspace_bindings(herdr_session, repo_root) WHERE active=1;
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_bindings_active_workspace
  ON workspace_bindings(herdr_session, workspace_id) WHERE active=1;
CREATE INDEX IF NOT EXISTS idx_workspace_bindings_project
  ON workspace_bindings(project_id, active);

CREATE TABLE IF NOT EXISTS runtime_ownership (
  card_id                    INTEGER PRIMARY KEY REFERENCES cards(id) ON DELETE CASCADE,
  herdr_session              TEXT NOT NULL,
  workspace_id               TEXT NOT NULL,
  tab_id                     TEXT,
  canonical_pane_id          TEXT,
  owned_child_pane_ids_json  TEXT NOT NULL DEFAULT '[]',
  launch_cwd                 TEXT,
  desired_tab_label          TEXT,
  observed_tab_label         TEXT,
  stage                      TEXT CHECK (stage IN
                               ('intake','ready','queued','executing','verifying',
                                'human_gate','blocked','done')),
  active_run_id              INTEGER REFERENCES runs(id) ON DELETE SET NULL,
  topology_state             TEXT NOT NULL CHECK (topology_state IN
                               ('converged','migration_pending','drift_blocked','legacy')),
  tab_policy                 TEXT NOT NULL CHECK (tab_policy IN
                               ('required','inspectable','forbidden','close_pending')),
  supersedes_tab_id          TEXT,
  pin_state                  TEXT NOT NULL DEFAULT 'none' CHECK (pin_state IN
                               ('none','user_pinned','long_running')),
  pin_actor                  TEXT,
  pin_reason                 TEXT,
  pin_expires_at             TEXT,
  last_close_effect_id       TEXT REFERENCES effect_outbox(effect_id) ON DELETE SET NULL,
  last_close_ack_at          TEXT,
  cleanup_state              TEXT NOT NULL DEFAULT 'none' CHECK (cleanup_state IN
                               ('none','pending','pinned','failed')),
  revision                   INTEGER NOT NULL CHECK (revision >= 1),
  updated_at                 TEXT NOT NULL DEFAULT (datetime('now')),
  CHECK (pin_state = 'none' OR (pin_actor IS NOT NULL AND pin_reason IS NOT NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_ownership_live_tab
  ON runtime_ownership(herdr_session, tab_id) WHERE tab_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_runtime_ownership_active_run
  ON runtime_ownership(active_run_id) WHERE active_run_id IS NOT NULL;

-- The executor conversation belongs to the card, not to a verification run.
-- A replacement is explicit and keeps the prior session identity and cause.
CREATE TABLE IF NOT EXISTS card_executor_sessions (
  id                    INTEGER PRIMARY KEY,
  card_id               INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  executor_session_id   TEXT NOT NULL,
  created_run_id        INTEGER REFERENCES runs(id) ON DELETE SET NULL,
  status                TEXT NOT NULL CHECK (status IN ('current','superseded','unavailable')),
  supersedes_session_id TEXT,
  replacement_cause     TEXT,
  source_event_id       TEXT,
  created_at            TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at            TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE (card_id, executor_session_id),
  CHECK (supersedes_session_id IS NULL OR supersedes_session_id <> executor_session_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_card_executor_sessions_current
  ON card_executor_sessions(card_id) WHERE status='current';

-- Interactive steering refines the fixed objective without minting a run,
-- tab, or conversation. Delivery and acknowledgement remain separate durable
-- states so retries can use the idempotency key without duplicating guidance.
CREATE TABLE IF NOT EXISTS steering_events (
  steer_id             TEXT PRIMARY KEY,
  idempotency_key      TEXT NOT NULL UNIQUE,
  card_id              INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  run_id               INTEGER REFERENCES runs(id) ON DELETE SET NULL,
  executor_session_id  TEXT NOT NULL,
  author               TEXT NOT NULL,
  steered_at           TEXT NOT NULL,
  text                 TEXT NOT NULL,
  objective            TEXT NOT NULL,
  kind                 TEXT NOT NULL
                         CHECK (kind IN ('guidance','answer','scope_change','protected_decision')),
  input_surface        TEXT NOT NULL CHECK (input_surface IN ('board','tab')),
  input_source         TEXT NOT NULL,
  streaming_behavior   TEXT CHECK (streaming_behavior IN ('steer','follow_up')),
  status               TEXT NOT NULL DEFAULT 'queued'
                         CHECK (status IN ('queued','delivered','acknowledged','rejected')),
  contract_changed     INTEGER NOT NULL DEFAULT 0 CHECK (contract_changed IN (0,1)),
  protected_request_id TEXT REFERENCES human_gate_checkpoints(request_id) ON DELETE SET NULL,
  delivery_effect_id   TEXT REFERENCES effect_outbox(effect_id) ON DELETE SET NULL,
  delivered_at         TEXT,
  acknowledged_at      TEXT,
  acknowledgement_text TEXT,
  rejection_reason     TEXT,
  source_event_id      TEXT NOT NULL,
  created_at           TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at           TEXT NOT NULL DEFAULT (datetime('now')),
  CHECK (kind <> 'protected_decision' OR protected_request_id IS NOT NULL),
  CHECK (kind <> 'scope_change' OR contract_changed=0 OR status IN ('delivered','acknowledged'))
);
CREATE INDEX IF NOT EXISTS idx_steering_events_card
  ON steering_events(card_id, created_at);
CREATE INDEX IF NOT EXISTS idx_steering_events_pending
  ON steering_events(card_id, steer_id) WHERE status='queued';

-- The orchestrator-side Pi extension consumes the durable stream by cursor.
-- Notifications are a separate channel from worker delivery: objections and
-- review prompts appear in the main chat and are never injected into the tab.
CREATE TABLE IF NOT EXISTS orchestrator_watch_cursors (
  subscriber_id        TEXT PRIMARY KEY,
  last_ingest_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_ingest_sequence >= 0),
  last_event_id         TEXT,
  updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS orchestrator_notifications (
  notification_id TEXT PRIMARY KEY,
  idempotency_key TEXT NOT NULL UNIQUE,
  steer_id        TEXT NOT NULL REFERENCES steering_events(steer_id) ON DELETE CASCADE,
  subscriber_id   TEXT NOT NULL REFERENCES orchestrator_watch_cursors(subscriber_id) ON DELETE CASCADE,
  message_json    TEXT NOT NULL,
  status          TEXT NOT NULL DEFAULT 'queued' CHECK (status IN
                    ('queued','presented','turn_requested','acknowledged','rejected')),
  presented_at    TEXT,
  acknowledged_at TEXT,
  created_at      TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_orchestrator_notifications_pending
  ON orchestrator_notifications(subscriber_id, notification_id)
  WHERE status IN ('queued','turn_requested');

-- Singleton watchdog lease and operator-facing health projection. It is
-- intentionally separate from mutable-target leases, which belong to runs.
CREATE TABLE IF NOT EXISTS controller_runtime (
  id                  INTEGER PRIMARY KEY CHECK (id=1),
  lease_holder        TEXT,
  lease_epoch         INTEGER NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
  lease_expires_at    TEXT,
  health              TEXT NOT NULL DEFAULT 'starting'
                        CHECK (health IN ('starting','healthy','degraded','stopped')),
  last_reconciled_at  TEXT,
  next_action_json    TEXT,
  blocker_json        TEXT,
  updated_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Todos are a rebuildable Board projection, never session-local memory. Every
-- item has durable source authority; unresolved items without a card remain
-- explicitly unbound until the controller can classify them.
CREATE TABLE IF NOT EXISTS todo_projection_items (
  todo_id              TEXT PRIMARY KEY,
  commitment_digest    TEXT NOT NULL UNIQUE,
  card_id               INTEGER REFERENCES cards(id) ON DELETE CASCADE,
  phase_ref             TEXT,
  artifact_path         TEXT,
  owner                 TEXT,
  next_predicate_json   TEXT NOT NULL,
  authority_event_id    TEXT NOT NULL,
  source_kind           TEXT NOT NULL CHECK (source_kind IN
                          ('card','comment','flow','human_decision','steer','blocker','verification')),
  source_ref            TEXT NOT NULL,
  text                  TEXT NOT NULL,
  status                TEXT NOT NULL CHECK (status IN
                          ('unbound','pending','in_progress','completed','blocked','superseded')),
  projection_revision   INTEGER NOT NULL CHECK (projection_revision >= 1),
  first_observed_at     TEXT NOT NULL,
  last_reconciled_at    TEXT NOT NULL,
  resolved_at           TEXT,
  CHECK ((card_id IS NULL AND status='unbound') OR card_id IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_todo_projection_card_status
  ON todo_projection_items(card_id, status, todo_id);
CREATE INDEX IF NOT EXISTS idx_todo_projection_unbound
  ON todo_projection_items(todo_id) WHERE status='unbound';

CREATE TABLE IF NOT EXISTS todo_reconciliations (
  id                    INTEGER PRIMARY KEY,
  card_id               INTEGER REFERENCES cards(id) ON DELETE CASCADE,
  trigger_event_id      TEXT NOT NULL,
  projection_revision   INTEGER NOT NULL CHECK (projection_revision >= 1),
  decisions_total       INTEGER NOT NULL CHECK (decisions_total >= 0),
  decisions_represented INTEGER NOT NULL CHECK (decisions_represented >= 0),
  new_items_count       INTEGER NOT NULL DEFAULT 0 CHECK (new_items_count >= 0),
  unowned_count         INTEGER NOT NULL DEFAULT 0 CHECK (unowned_count >= 0),
  discrepancies_json    TEXT NOT NULL DEFAULT '[]',
  reconciled_at         TEXT NOT NULL,
  CHECK (decisions_represented <= decisions_total)
);
CREATE INDEX IF NOT EXISTS idx_todo_reconciliations_card
  ON todo_reconciliations(card_id, id);

-- Recoverable materializations are derived from the event journal. Generation
-- and parent_generation support three-way comparison; hashes/manifests are
-- validated before a completed snapshot can replace the prior projection.
CREATE TABLE IF NOT EXISTS controller_snapshots (
  snapshot_id          TEXT PRIMARY KEY,
  project_id           INTEGER REFERENCES projects(id) ON DELETE CASCADE,
  card_id              INTEGER REFERENCES cards(id) ON DELETE CASCADE,
  projection_kind      TEXT NOT NULL,
  generation           INTEGER NOT NULL CHECK (generation >= 1),
  parent_generation    INTEGER CHECK (parent_generation >= 1),
  state_hash           TEXT NOT NULL,
  manifest_json        TEXT NOT NULL,
  state_json           TEXT NOT NULL,
  through_ingest_sequence INTEGER NOT NULL CHECK (through_ingest_sequence >= 0),
  status               TEXT NOT NULL CHECK (status IN ('writing','complete','superseded','conflict')),
  created_at           TEXT NOT NULL DEFAULT (datetime('now')),
  completed_at         TEXT,
  CHECK (project_id IS NOT NULL OR card_id IS NOT NULL)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_controller_snapshots_generation
  ON controller_snapshots(projection_kind, project_id, card_id, generation);
CREATE INDEX IF NOT EXISTS idx_controller_snapshots_restore
  ON controller_snapshots(projection_kind, card_id, generation DESC)
  WHERE status='complete';

CREATE TABLE IF NOT EXISTS executor_session_checkpoints (
  checkpoint_id        TEXT PRIMARY KEY,
  card_id              INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  project_id           INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  executor_session_id  TEXT NOT NULL,
  generation           INTEGER NOT NULL CHECK (generation >= 1),
  parent_generation    INTEGER CHECK (parent_generation >= 1),
  state_hash           TEXT NOT NULL,
  manifest_json        TEXT NOT NULL,
  through_event_id     TEXT NOT NULL,
  status               TEXT NOT NULL CHECK (status IN ('writing','complete','superseded','conflict')),
  created_at           TEXT NOT NULL DEFAULT (datetime('now')),
  completed_at         TEXT,
  UNIQUE (card_id, executor_session_id, generation)
);
CREATE INDEX IF NOT EXISTS idx_executor_checkpoints_restore
  ON executor_session_checkpoints(card_id, executor_session_id, generation DESC)
  WHERE status='complete';

CREATE TABLE IF NOT EXISTS projection_conflicts (
  conflict_id          TEXT PRIMARY KEY,
  projection_kind      TEXT NOT NULL,
  project_id           INTEGER REFERENCES projects(id) ON DELETE CASCADE,
  card_id              INTEGER REFERENCES cards(id) ON DELETE CASCADE,
  baseline_hash        TEXT NOT NULL,
  current_hash         TEXT NOT NULL,
  candidate_hash       TEXT NOT NULL,
  baseline_generation  INTEGER,
  current_generation   INTEGER,
  candidate_generation INTEGER,
  details_json         TEXT NOT NULL,
  status               TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','resolved','superseded')),
  source_event_id      TEXT NOT NULL,
  created_at           TEXT NOT NULL DEFAULT (datetime('now')),
  resolved_at          TEXT
);
CREATE INDEX IF NOT EXISTS idx_projection_conflicts_open
  ON projection_conflicts(card_id, conflict_id) WHERE status='open';

-- Explicit pins protect critical bindings/checkpoints. They never imply that
-- an external effect can be rolled back; outbox acknowledgement remains fact.
CREATE TABLE IF NOT EXISTS controller_pins (
  pin_id          TEXT PRIMARY KEY,
  target_kind     TEXT NOT NULL CHECK (target_kind IN
                    ('workspace_binding','runtime_ownership','executor_session','snapshot','cursor')),
  target_id       TEXT NOT NULL,
  actor           TEXT NOT NULL,
  reason          TEXT NOT NULL,
  state           TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active','released','expired')),
  expires_at      TEXT,
  source_event_id TEXT NOT NULL,
  created_at      TEXT NOT NULL DEFAULT (datetime('now')),
  released_at     TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_controller_pins_active_target
  ON controller_pins(target_kind, target_id) WHERE state='active';

CREATE TABLE IF NOT EXISTS diagnostics (
  id               INTEGER PRIMARY KEY,
  card_id          INTEGER REFERENCES cards(id) ON DELETE CASCADE,
  code             TEXT NOT NULL,
  severity         TEXT NOT NULL CHECK (severity IN ('info','warning','error','blocking')),
  status           TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','resolved')),
  details_json     TEXT NOT NULL,
  source_event_id  TEXT,
  created_at       TEXT NOT NULL DEFAULT (datetime('now')),
  resolved_at      TEXT
);
CREATE INDEX IF NOT EXISTS idx_diagnostics_open_card
  ON diagnostics(card_id, severity, id) WHERE status='open';
