use super::enqueue;
use board_core::db::{Db, FinalizeRun, RuntimeLaunch, RuntimeTopology, BOARD_ID};
use board_core::protocol::{CardCreateParams, CardStatus, ColumnCreateParams, RunOutcome};
use rusqlite::{types::Value, Connection};

const CONTROLLER_TABLES: [&str; 20] = [
    "controller_events",
    "card_contracts",
    "review_receipts",
    "human_gate_checkpoints",
    "human_gate_decisions",
    "mutable_target_leases",
    "effect_outbox",
    "workspace_bindings",
    "runtime_ownership",
    "card_executor_sessions",
    "steering_events",
    "controller_runtime",
    "todo_projection_items",
    "todo_reconciliations",
    "orchestrator_watch_cursors",
    "orchestrator_notifications",
    "controller_snapshots",
    "executor_session_checkpoints",
    "projection_conflicts",
    "controller_pins",
];

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )
    .unwrap()
}

fn raw_rows(conn: &Connection, table: &str) -> Vec<Vec<Value>> {
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get(column))
                .collect::<rusqlite::Result<Vec<Value>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn controller_shape(conn: &Connection) -> Vec<(String, String)> {
    let mut statement = conn
        .prepare(
            "SELECT name, sql FROM sqlite_master
             WHERE (type='table' OR type='index')
               AND (name LIKE 'controller_%'
                    OR name LIKE 'card_contract%'
                    OR name LIKE 'review_receipt%'
                    OR name LIKE 'human_gate_%'
                    OR name LIKE 'mutable_target_%'
                    OR name LIKE 'effect_outbox%'
                    OR name LIKE 'workspace_binding%'
                    OR name LIKE 'runtime_ownership%'
                    OR name LIKE 'diagnostic%')
             ORDER BY type, name",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn setup_runtime_close(db: &Db) -> (i64, String) {
    let card = db
        .create_card(&CardCreateParams {
            title: "runtime close fixture".into(),
            ..Default::default()
        })
        .unwrap();
    db.activate_runtime_contract(card.id, "tab lifecycle repair")
        .unwrap();
    let run = db
        .enqueue_run_uow(&enqueue(card.id, card.column_id))
        .unwrap();
    db.promote_run_with_anchor_uow(run.id, Some("w1"), Some("w1:p1"), None, None, None)
        .unwrap();
    db.record_runtime_launch(&RuntimeLaunch {
        card_id: card.id,
        run_id: run.id,
        herdr_session: "default",
        workspace_id: "w1",
        tab_id: "w1:t1",
        pane_id: "w1:p1",
        launch_cwd: Some("/repo"),
        desired_tab_label: "card-1 tab lifecycle repair",
        stage: "executing",
    })
    .unwrap();
    db.finalize_run_uow(&FinalizeRun {
        run_id: run.id,
        outcome: RunOutcome::Ok,
        summary: None,
        comments: &[],
        target_column_id: None,
        final_status: CardStatus::Done,
        final_awaiting_reason: None,
        next: None,
    })
    .unwrap();
    let effect = db.pending_runtime_closes().unwrap().pop().unwrap();
    (card.id, effect.effect_id)
}

#[test]
fn runtime_contract_materializes_ownership_and_idempotent_close_outbox() {
    let db = Db::open_in_memory().unwrap();
    let card = db
        .create_card(&CardCreateParams {
            title: "Legacy title is not the label authority".into(),
            ..Default::default()
        })
        .unwrap();
    db.activate_runtime_contract(card.id, "tab lifecycle repair")
        .unwrap();
    assert_eq!(
        db.runtime_label_description(card.id).unwrap().as_deref(),
        Some("tab lifecycle repair")
    );
    let run = db
        .enqueue_run_uow(&enqueue(card.id, card.column_id))
        .unwrap();
    db.promote_run_with_anchor_uow(run.id, Some("w1"), Some("w1:p1"), None, None, None)
        .unwrap();
    db.record_runtime_launch(&RuntimeLaunch {
        card_id: card.id,
        run_id: run.id,
        herdr_session: "default",
        workspace_id: "w1",
        tab_id: "w1:t1",
        pane_id: "w1:p1",
        launch_cwd: Some("/repo"),
        desired_tab_label: "card-1 tab lifecycle repair",
        stage: "executing",
    })
    .unwrap();
    db.finalize_run_uow(&FinalizeRun {
        run_id: run.id,
        outcome: RunOutcome::Ok,
        summary: None,
        comments: &[],
        target_column_id: None,
        final_status: CardStatus::Done,
        final_awaiting_reason: None,
        next: None,
    })
    .unwrap();
    let effects = db.pending_runtime_closes().unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].tab_id, "w1:t1");
    db.acknowledge_runtime_close(&effects[0].effect_id).unwrap();
    assert!(db.pending_runtime_closes().unwrap().is_empty());
}

#[test]
fn runtime_auto_hop_persists_executor_role_and_keeps_the_canonical_tab() {
    let db = Db::open_in_memory().unwrap();
    let source = db
        .create_column(&ColumnCreateParams {
            name: "Execute one".into(),
            fresh_session: Some(false),
            ..Default::default()
        })
        .unwrap();
    let target = db
        .create_column(&ColumnCreateParams {
            name: "Execute two".into(),
            fresh_session: Some(false),
            ..Default::default()
        })
        .unwrap();
    let card = db
        .create_card(&CardCreateParams {
            title: "executor auto hop".into(),
            column_id: Some(source.id),
            ..Default::default()
        })
        .unwrap();
    db.activate_runtime_contract(card.id, "tab lifecycle repair")
        .unwrap();
    let run = db.enqueue_run_uow(&enqueue(card.id, source.id)).unwrap();
    db.promote_run_uow(run.id, Some("w1"), Some("w1:p1"), None)
        .unwrap();
    db.record_runtime_launch(&RuntimeLaunch {
        card_id: card.id,
        run_id: run.id,
        herdr_session: "default",
        workspace_id: "w1",
        tab_id: "w1:t1",
        pane_id: "w1:p1",
        launch_cwd: Some("/repo"),
        desired_tab_label: "card-1 tab lifecycle repair",
        stage: "executing",
    })
    .unwrap();

    let effects = db
        .finalize_run_uow(&FinalizeRun {
            run_id: run.id,
            outcome: RunOutcome::Ok,
            summary: None,
            comments: &[],
            target_column_id: Some(target.id),
            final_status: CardStatus::Queued,
            final_awaiting_reason: None,
            next: Some(enqueue(card.id, target.id)),
        })
        .unwrap();
    let next = effects.next_run.unwrap();

    assert_eq!(
        db.runtime_role_for_run(next.id).unwrap().as_deref(),
        Some("executor")
    );
    assert!(
        db.pending_runtime_closes().unwrap().is_empty(),
        "executor auto-hop must retain the exact canonical tab"
    );
}

#[test]
fn runtime_auto_hop_persists_verification_role_and_plans_fresh_placement() {
    let db = Db::open_in_memory().unwrap();
    let source = db
        .create_column(&ColumnCreateParams {
            name: "Execute".into(),
            fresh_session: Some(false),
            ..Default::default()
        })
        .unwrap();
    let target = db
        .create_column(&ColumnCreateParams {
            name: "Verification".into(),
            fresh_session: Some(true),
            ..Default::default()
        })
        .unwrap();
    let card = db
        .create_card(&CardCreateParams {
            title: "verification auto hop".into(),
            column_id: Some(source.id),
            ..Default::default()
        })
        .unwrap();
    db.activate_runtime_contract(card.id, "tab lifecycle repair")
        .unwrap();
    let run = db.enqueue_run_uow(&enqueue(card.id, source.id)).unwrap();
    db.promote_run_uow(run.id, Some("w1"), Some("w1:p1"), None)
        .unwrap();
    db.record_runtime_launch(&RuntimeLaunch {
        card_id: card.id,
        run_id: run.id,
        herdr_session: "default",
        workspace_id: "w1",
        tab_id: "w1:t1",
        pane_id: "w1:p1",
        launch_cwd: Some("/repo"),
        desired_tab_label: "card-1 tab lifecycle repair",
        stage: "executing",
    })
    .unwrap();

    let effects = db
        .finalize_run_uow(&FinalizeRun {
            run_id: run.id,
            outcome: RunOutcome::Ok,
            summary: None,
            comments: &[],
            target_column_id: Some(target.id),
            final_status: CardStatus::Queued,
            final_awaiting_reason: None,
            next: Some(enqueue(card.id, target.id)),
        })
        .unwrap();
    let next = effects.next_run.unwrap();

    assert_eq!(
        db.runtime_role_for_run(next.id).unwrap().as_deref(),
        Some("verification")
    );
    assert_eq!(db.pending_runtime_closes().unwrap().len(), 1);
}

#[test]
fn failed_runtime_close_survives_restart_and_is_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("close-retry.db");
    let db = Db::open(&path).unwrap();
    let (card_id, effect_id) = setup_runtime_close(&db);
    db.fail_runtime_close(&effect_id, "injected close failure")
        .unwrap();
    drop(db);

    let reopened = Db::open(&path).unwrap();
    let pending = reopened.pending_runtime_closes().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].effect_id, effect_id);
    reopened.acknowledge_runtime_close(&effect_id).unwrap();
    drop(reopened);

    let conn = Connection::open(path).unwrap();
    let effect: (String, i64, Option<String>) = conn
        .query_row(
            "SELECT status,attempt_count,last_error FROM effect_outbox WHERE effect_id=?1",
            [&effect_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(effect, ("acknowledged".into(), 2, None));
    let ownership: (Option<String>, String, String) = conn
        .query_row(
            "SELECT tab_id,tab_policy,cleanup_state FROM runtime_ownership WHERE card_id=?1",
            [card_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(ownership, (None, "forbidden".into(), "none".into()));
}

#[test]
fn runtime_close_ack_rolls_back_if_ownership_cleanup_is_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("close-ack-atomic.db");
    let db = Db::open(&path).unwrap();
    let (card_id, effect_id) = setup_runtime_close(&db);
    drop(db);

    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER abort_runtime_cleanup BEFORE UPDATE ON runtime_ownership
         WHEN NEW.tab_id IS NULL
         BEGIN SELECT RAISE(ABORT,'injected ownership cleanup interruption'); END;",
    )
    .unwrap();
    drop(conn);

    let db = Db::open(&path).unwrap();
    let error = db.acknowledge_runtime_close(&effect_id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected ownership cleanup interruption"),
        "{error}"
    );
    drop(db);

    let conn = Connection::open(&path).unwrap();
    let effect_status: String = conn
        .query_row(
            "SELECT status FROM effect_outbox WHERE effect_id=?1",
            [&effect_id],
            |row| row.get(0),
        )
        .unwrap();
    let ownership: (Option<String>, String) = conn
        .query_row(
            "SELECT tab_id,cleanup_state FROM runtime_ownership WHERE card_id=?1",
            [card_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(effect_status, "pending");
    assert_eq!(ownership, (Some("w1:t1".into()), "pending".into()));
}

#[test]
fn runtime_rename_outbox_survives_failure_and_acknowledges_exact_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rename-retry.db");
    let db = Db::open(&path).unwrap();
    let card = db
        .create_card(&CardCreateParams {
            title: "rename fixture".into(),
            ..Default::default()
        })
        .unwrap();
    db.activate_runtime_contract(card.id, "tab lifecycle repair")
        .unwrap();
    let run = db
        .enqueue_run_uow(&enqueue(card.id, card.column_id))
        .unwrap();
    db.record_runtime_launch(&RuntimeLaunch {
        card_id: card.id,
        run_id: run.id,
        herdr_session: "default",
        workspace_id: "w1",
        tab_id: "w1:t1",
        pane_id: "w1:p1",
        launch_cwd: Some("/repo"),
        desired_tab_label: "card-1 tab lifecycle repair",
        stage: "executing",
    })
    .unwrap();
    let effect = db.pending_runtime_renames().unwrap().pop().unwrap();
    assert_eq!(effect.tab_id, "w1:t1");
    assert_eq!(effect.label, "card-1 tab lifecycle repair");
    db.fail_runtime_effect(&effect.effect_id, "injected rename failure")
        .unwrap();
    drop(db);

    let reopened = Db::open(&path).unwrap();
    let effect = reopened.pending_runtime_renames().unwrap().pop().unwrap();
    reopened.acknowledge_runtime_rename(&effect).unwrap();
    drop(reopened);

    let conn = Connection::open(path).unwrap();
    let status: (String, i64) = conn
        .query_row(
            "SELECT status,attempt_count FROM effect_outbox WHERE effect_id=?1",
            [&effect.effect_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, ("acknowledged".into(), 2));
    let observed: String = conn
        .query_row(
            "SELECT observed_tab_label FROM runtime_ownership WHERE card_id=?1",
            [card.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(observed, "card-1 tab lifecycle repair");
}

#[test]
fn stale_tab_cleanup_outbox_protects_canonical_and_active_ids() {
    let db = Db::open_in_memory().unwrap();
    let card = db
        .create_card(&CardCreateParams {
            title: "duplicate cleanup fixture".into(),
            ..Default::default()
        })
        .unwrap();
    let topology = RuntimeTopology {
        card_id: card.id,
        herdr_session: "default".into(),
        workspace_id: "w1".into(),
        canonical_tab_id: "w1:t-canonical".into(),
        active_run_id: Some(7),
        active_run_pane_id: Some("w1:p-active".into()),
        durable_pane_ids: vec!["w1:p-active".into(), "w1:p-stale".into()],
    };

    assert!(db
        .plan_stale_runtime_tab_closes(
            &topology,
            Some("w1:t-canonical"),
            &["w1:t-canonical".into()]
        )
        .is_err());
    assert!(db
        .plan_stale_runtime_tab_closes(&topology, Some("w1:t-active-other"), &["w1:t-stale".into()])
        .is_err());
    db.plan_stale_runtime_tab_closes(&topology, Some("w1:t-canonical"), &["w1:t-stale".into()])
        .unwrap();
    let closes = db.pending_runtime_closes().unwrap();
    assert_eq!(closes.len(), 1);
    assert_eq!(closes[0].tab_id, "w1:t-stale");
}

#[test]
fn fresh_schema_v16_contains_the_controller_state_plane() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh-v16.db");
    drop(Db::open(&path).unwrap());
    let conn = Connection::open(path).unwrap();

    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        16
    );
    for table in CONTROLLER_TABLES.into_iter().chain(["diagnostics"]) {
        assert!(table_exists(&conn, table), "missing v16 table {table}");
    }

    let checkpoint_columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('human_gate_checkpoints') ORDER BY cid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for required in [
        "request_id",
        "question",
        "options_json",
        "artifact_path",
        "artifact_digest",
        "effect_json",
        "authority_json",
    ] {
        assert!(
            checkpoint_columns.iter().any(|column| column == required),
            "checkpoint must bind {required}"
        );
    }
}

#[test]
fn v15_to_v16_is_additive_and_matches_the_fresh_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upgrade-v15.db");
    let (card_id, run_id, before) = {
        let db = Db::open(&path).unwrap();
        let card = db
            .create_card(&CardCreateParams {
                title: "preserved across v16".into(),
                ..Default::default()
            })
            .unwrap();
        db.add_comment(card.id, "user", "keep this history")
            .unwrap();
        let run = db
            .enqueue_run_uow(&enqueue(card.id, card.column_id))
            .unwrap();
        drop(db);

        let conn = Connection::open(&path).unwrap();
        let before = (
            raw_rows(&conn, "projects"),
            raw_rows(&conn, "boards"),
            raw_rows(&conn, "columns"),
            raw_rows(&conn, "cards"),
            raw_rows(&conn, "comments"),
            raw_rows(&conn, "runs"),
        );
        conn.execute_batch(
            "DROP TABLE runtime_ownership;
             DROP TABLE workspace_bindings;
             DROP TABLE orchestrator_notifications;
             DROP TABLE orchestrator_watch_cursors;
             DROP TABLE steering_events;
             DROP TABLE card_executor_sessions;
             DROP TABLE controller_runtime;
             DROP TABLE controller_pins;
             DROP TABLE projection_conflicts;
             DROP TABLE executor_session_checkpoints;
             DROP TABLE controller_snapshots;
             DROP TABLE todo_reconciliations;
             DROP TABLE todo_projection_items;
             DROP TABLE effect_outbox;
             DROP TABLE mutable_target_leases;
             DROP TABLE human_gate_decisions;
             DROP TABLE human_gate_checkpoints;
             DROP TABLE review_receipts;
             DROP TABLE card_contracts;
             DROP TABLE diagnostics;
             DROP TABLE controller_events;
             PRAGMA user_version = 15;",
        )
        .unwrap();
        (card.id, run.id, before)
    };

    let db = Db::open(&path).expect("v15 -> v16 migration");
    assert_eq!(db.user_version().unwrap(), 16);
    assert_eq!(
        db.get_card(card_id).unwrap().unwrap().title,
        "preserved across v16"
    );
    assert_eq!(db.get_run(run_id).unwrap().id, run_id);
    drop(db);

    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        (
            raw_rows(&conn, "projects"),
            raw_rows(&conn, "boards"),
            raw_rows(&conn, "columns"),
            raw_rows(&conn, "cards"),
            raw_rows(&conn, "comments"),
            raw_rows(&conn, "runs"),
        ),
        before,
        "v16 must not rewrite existing operational rows"
    );

    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh_path = fresh_dir.path().join("fresh.db");
    drop(Db::open(&fresh_path).unwrap());
    let fresh = Connection::open(fresh_path).unwrap();
    assert_eq!(controller_shape(&conn), controller_shape(&fresh));
}

#[test]
fn v16_migration_failure_rolls_back_the_whole_state_plane() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("malformed-v15.db");
    drop(Db::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE runtime_ownership;
         DROP TABLE workspace_bindings;
         DROP TABLE orchestrator_notifications;
         DROP TABLE orchestrator_watch_cursors;
         DROP TABLE steering_events;
         DROP TABLE card_executor_sessions;
         DROP TABLE controller_runtime;
         DROP TABLE controller_pins;
         DROP TABLE projection_conflicts;
         DROP TABLE executor_session_checkpoints;
         DROP TABLE controller_snapshots;
         DROP TABLE todo_reconciliations;
         DROP TABLE todo_projection_items;
         DROP TABLE effect_outbox;
         DROP TABLE mutable_target_leases;
         DROP TABLE human_gate_decisions;
         DROP TABLE human_gate_checkpoints;
         DROP TABLE review_receipts;
         DROP TABLE card_contracts;
         DROP TABLE diagnostics;
         DROP TABLE controller_events;
         CREATE INDEX workspace_bindings ON cards(id);
         PRAGMA user_version = 15;",
    )
    .unwrap();
    drop(conn);

    for attempt in 0..2 {
        let error = match Db::open(&path) {
            Ok(_) => panic!("attempt {attempt}: malformed v15 unexpectedly migrated"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("workspace_bindings"), "{error}");
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            15
        );
        assert!(
            !table_exists(&conn, "controller_events"),
            "v16 DDL before the conflict must roll back"
        );
    }
}

#[test]
fn v16_constraints_pin_idempotency_exact_identity_and_one_workspace_per_root() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("constraints.db");
    let db = Db::open(&path).unwrap();
    let card = db
        .create_card(&CardCreateParams {
            title: "controller fixture".into(),
            ..Default::default()
        })
        .unwrap();
    let run = db
        .enqueue_run_uow(&enqueue(card.id, card.column_id))
        .unwrap();
    drop(db);
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    conn.execute(
        "INSERT INTO controller_events
         (event_id,idempotency_key,source,source_id,card_id,observed_at,payload_digest,payload_json)
         VALUES ('evt-1','source:key:1','board','run-1',?1,'2026-09-17T00:00:00Z','sha256:one','{}')",
        [card.id],
    )
    .unwrap();
    assert!(conn
        .execute(
            "INSERT INTO controller_events
             (event_id,idempotency_key,source,source_id,card_id,observed_at,payload_digest,payload_json)
             VALUES ('evt-2','source:key:1','board','run-2',?1,'2026-09-17T00:00:01Z','sha256:two','{}')",
            [card.id],
        )
        .is_err());

    conn.execute(
        "INSERT INTO human_gate_checkpoints
         (request_id,card_id,revision,gate_kind,question,options_json,artifact_path,
          artifact_digest,effect_json,authority_json,created_event_id)
         VALUES ('gate-1',?1,1,'artifact_approval','Approve exact artifact?',
                 '[\"approve\",\"request_changes\",\"block\"]','docs/artifact.md',
                 'sha256:artifact','{}','{\"actor\":\"human\"}','evt-1')",
        [card.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO human_gate_decisions
         (decision_id,request_id,action,actor,checkpoint_revision,artifact_digest,event_id)
         VALUES ('decision-1','gate-1','approve','user',1,'sha256:artifact','evt-1')",
        [],
    )
    .unwrap();
    assert!(conn
        .execute(
            "INSERT INTO human_gate_decisions
             (decision_id,request_id,action,actor,checkpoint_revision,artifact_digest,event_id)
             VALUES ('decision-2','gate-1','skip','user',1,'sha256:artifact',NULL)",
            [],
        )
        .is_err());

    conn.execute(
        "INSERT INTO workspace_bindings
         (project_id,herdr_session,repo_root,workspace_id,expected_launch_cwd,
          binding_revision,binding_digest,source_event_id)
         VALUES (?1,'default','/repo','w1','/repo',1,'sha256:binding','evt-1')",
        [BOARD_ID],
    )
    .unwrap();
    for sql in [
        "INSERT INTO workspace_bindings
         (project_id,herdr_session,repo_root,workspace_id,expected_launch_cwd,binding_revision,binding_digest)
         VALUES (1,'default','/repo','w2','/repo',2,'sha256:other')",
        "INSERT INTO workspace_bindings
         (project_id,herdr_session,repo_root,workspace_id,expected_launch_cwd,binding_revision,binding_digest)
         VALUES (1,'default','/other','w1','/other',2,'sha256:other')",
    ] {
        assert!(conn.execute(sql, []).is_err(), "binding conflict accepted");
    }

    conn.execute(
        "INSERT INTO runtime_ownership
         (card_id,herdr_session,workspace_id,tab_id,canonical_pane_id,owned_child_pane_ids_json,
          launch_cwd,desired_tab_label,stage,active_run_id,topology_state,tab_policy,revision)
         VALUES (?1,'default','w1','w1:t1','w1:p1','[]','/repo',
                 'C09 · HB · Controller · EXE R1','executing',?2,'converged','required',1)",
        [card.id, run.id],
    )
    .unwrap();
    assert!(conn
        .execute(
            "INSERT INTO runtime_ownership
             (card_id,herdr_session,workspace_id,tab_id,owned_child_pane_ids_json,
              topology_state,tab_policy,revision)
             VALUES (?1,'default','w1','w1:t1','[]','converged','required',1)",
            [card.id + 999],
        )
        .is_err());

    conn.execute(
        "INSERT INTO mutable_target_leases
         (target_uri,card_id,run_id,lease_epoch,acquired_at,expires_at,heartbeat_at)
         VALUES ('file:///repo/schema.sql',?1,?2,1,'now','later','now')",
        [card.id, run.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO effect_outbox
         (effect_id,card_id,causation_event_id,effect_kind,idempotency_key,payload_json)
         VALUES ('effect-1',?1,'evt-1','tab.rename','tab-label:w1:t1:1:executing','{}')",
        [card.id],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO card_executor_sessions
         (card_id,executor_session_id,created_run_id,status,source_event_id)
         VALUES (?1,'executor-session-1',?2,'current','evt-1')",
        [card.id, run.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steering_events
         (steer_id,idempotency_key,card_id,run_id,executor_session_id,author,steered_at,
          text,objective,kind,input_surface,input_source,streaming_behavior,status,source_event_id)
         VALUES ('steer-1','steer:card:run:1',?1,?2,'executor-session-1','user',
                 '2026-09-17T01:13:15Z','Keep the same objective','controller objective',
                 'guidance','tab','interactive','steer','queued','evt-1')",
        [card.id, run.id],
    )
    .unwrap();
    assert!(conn
        .execute(
            "INSERT INTO steering_events
             (steer_id,idempotency_key,card_id,run_id,executor_session_id,author,steered_at,
              text,objective,kind,input_surface,input_source,streaming_behavior,status,source_event_id)
             VALUES ('steer-2','steer:card:run:1',?1,?2,'executor-session-1','user',
                     '2026-09-17T01:13:16Z','duplicate','controller objective',
                     'guidance','board','interactive','follow_up','queued','evt-1')",
            [card.id, run.id],
        )
        .is_err());
    conn.execute(
        "INSERT INTO orchestrator_watch_cursors
         (subscriber_id,last_ingest_sequence,last_event_id)
         VALUES ('main-orchestrator',1,'evt-1')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO orchestrator_notifications
         (notification_id,idempotency_key,steer_id,subscriber_id,message_json,status)
         VALUES ('notification-1','watch:main:steer-1','steer-1','main-orchestrator','{}','queued')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO controller_runtime
         (id,lease_holder,lease_epoch,health,last_reconciled_at,next_action_json,blocker_json)
         VALUES (1,'boardd:test',1,'healthy','2026-09-17T01:13:20Z','{}',NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO todo_projection_items
         (todo_id,commitment_digest,card_id,phase_ref,owner,next_predicate_json,
          authority_event_id,source_kind,source_ref,text,status,projection_revision,
          first_observed_at,last_reconciled_at)
         VALUES ('todo-1','sha256:todo',?1,'schema-v16','board-core','{}','evt-1',
                 'steer','steer-1','Persist durable steering','pending',1,'now','now')",
        [card.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO todo_reconciliations
         (card_id,trigger_event_id,projection_revision,decisions_total,
          decisions_represented,new_items_count,unowned_count,discrepancies_json,reconciled_at)
         VALUES (?1,'evt-1',1,1,1,1,0,'[]','now')",
        [card.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO controller_snapshots
         (snapshot_id,project_id,card_id,projection_kind,generation,parent_generation,
          state_hash,manifest_json,state_json,through_ingest_sequence,status,completed_at)
         VALUES ('snapshot-1',1,?1,'card',1,NULL,'sha256:state','{}','{}',1,'complete','now')",
        [card.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO executor_session_checkpoints
         (checkpoint_id,card_id,project_id,executor_session_id,generation,parent_generation,
          state_hash,manifest_json,through_event_id,status,completed_at)
         VALUES ('checkpoint-1',?1,1,'executor-session-1',1,NULL,
                 'sha256:checkpoint','{}','evt-1','complete','now')",
        [card.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO controller_pins
         (pin_id,target_kind,target_id,actor,reason,source_event_id)
         VALUES ('pin-1','snapshot','snapshot-1','boardd:test','startup restore','evt-1')",
        [],
    )
    .unwrap();
}
