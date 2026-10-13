//! Public contract tests for the pure deterministic controller engine.

use board_core::engine::{
    classify_review, decide_tab_policy, order_controller_events, plan_recovery, reduce_controller,
    select_candidate, AggregateRole, Candidate, ControllerAggregate, ControllerEvent,
    ControllerEventEnvelope, ControllerReduction, EffectIntent, EventOrderError, GateDecision,
    Observation, ReceiptOutcome, RecoveryCase, RecoveryPlan, ReviewClass, ReviewNote,
    ReviewNoteMarker, TabPolicy, WorkState,
};

fn event(
    id: &str,
    key: &str,
    digest: &str,
    source: &str,
    source_sequence: Option<u64>,
    ingest_sequence: u64,
    causation_id: Option<&str>,
) -> ControllerEventEnvelope {
    ControllerEventEnvelope {
        event_id: id.into(),
        idempotency_key: key.into(),
        payload_digest: digest.into(),
        source: source.into(),
        source_sequence,
        ingest_sequence,
        causation_id: causation_id.map(str::to_owned),
        event: ControllerEvent::EvidenceObserved,
    }
}
#[test]
fn event_order_honors_causation_then_source_sequence_then_ingest_sequence() {
    let events = vec![
        event("child", "k4", "d4", "git", Some(1), 1, Some("root")),
        event("later", "k3", "d3", "git", Some(3), 3, None),
        event("root", "k2", "d2", "board", None, 4, None),
        event("earlier", "k1", "d1", "git", Some(2), 2, None),
    ];
    let ordered = order_controller_events(&events).unwrap();
    let ids: Vec<_> = ordered
        .iter()
        .map(|event| event.event_id.as_str())
        .collect();
    assert_eq!(ids, ["earlier", "later", "root", "child"]);
}

#[test]
fn causation_precedes_an_inverted_source_sequence_without_deadlock() {
    let events = vec![
        event("child", "child", "d1", "board", Some(1), 1, Some("root")),
        event("root", "root", "d2", "board", Some(2), 2, None),
    ];
    let ordered = order_controller_events(&events).unwrap();
    let ids: Vec<_> = ordered
        .iter()
        .map(|event| event.event_id.as_str())
        .collect();
    assert_eq!(ids, ["root", "child"]);
}

#[test]
fn duplicate_event_is_noop_but_changed_digest_blocks() {
    let same = vec![
        event("one", "same", "digest", "board", None, 1, None),
        event("two", "same", "digest", "board", None, 2, None),
    ];
    assert_eq!(order_controller_events(&same).unwrap().len(), 1);

    let exact_replay = vec![
        event("one", "same", "digest", "board", None, 1, None),
        event("one", "same", "digest", "board", None, 1, None),
    ];
    assert_eq!(order_controller_events(&exact_replay).unwrap().len(), 1);

    let conflict = vec![
        event("one", "same", "digest-a", "board", None, 1, None),
        event("two", "same", "digest-b", "board", None, 2, None),
    ];
    assert_eq!(
        order_controller_events(&conflict),
        Err(EventOrderError::DigestConflict {
            idempotency_key: "same".into(),
            first_digest: "digest-a".into(),
            conflicting_digest: "digest-b".into(),
        })
    );
}

#[test]
fn review_classifier_preserves_approved_artifact_and_routes_uncertain_notes_to_gate() {
    assert_eq!(
        classify_review(ReceiptOutcome::Approved, true, &[]),
        ReviewClass::ApproveOnly
    );
    assert_eq!(
        classify_review(
            ReceiptOutcome::Approved,
            true,
            &[ReviewNote {
                text: "Evaluate a direct-click access path".into(),
                marker: Some(ReviewNoteMarker::FollowUp),
            }],
        ),
        ReviewClass::ApproveWithFollowUp {
            note_indexes: vec![0]
        }
    );
    assert_eq!(
        classify_review(
            ReceiptOutcome::Approved,
            true,
            &[ReviewNote {
                text: "This changes current acceptance".into(),
                marker: Some(ReviewNoteMarker::RevisionRequired),
            }],
        ),
        ReviewClass::RevisionRequired {
            note_indexes: vec![0]
        }
    );
    assert_eq!(
        classify_review(
            ReceiptOutcome::Approved,
            true,
            &[ReviewNote {
                text: "Please make this clearer".into(),
                marker: None,
            }],
        ),
        ReviewClass::NeedsHumanClassification {
            note_indexes: vec![0]
        }
    );
    assert_eq!(
        classify_review(ReceiptOutcome::Approved, false, &[]),
        ReviewClass::DigestConflict
    );
    assert_eq!(
        classify_review(ReceiptOutcome::Rejected, true, &[]),
        ReviewClass::Reject
    );
    assert_eq!(
        classify_review(
            ReceiptOutcome::ChangesRequested,
            true,
            &[ReviewNote {
                text: "Free-form explanation".into(),
                marker: None,
            }],
        ),
        ReviewClass::RevisionRequired {
            note_indexes: vec![0]
        }
    );
}

#[test]
fn changes_requested_preserves_every_note_as_revision_source() {
    let notes = vec![
        ReviewNote {
            text: "Exact free-form feedback".into(),
            marker: None,
        },
        ReviewNote {
            text: "Explicit revision detail".into(),
            marker: Some(ReviewNoteMarker::RevisionRequired),
        },
        ReviewNote {
            text: "Context that must travel with the revision".into(),
            marker: Some(ReviewNoteMarker::Informational),
        },
    ];
    assert_eq!(
        classify_review(ReceiptOutcome::ChangesRequested, true, &notes),
        ReviewClass::RevisionRequired {
            note_indexes: vec![0, 1, 2]
        }
    );
}

#[test]
fn candidate_selection_is_eligible_and_stably_ordered() {
    let mut candidates = vec![
        Candidate::eligible(9, 1, None, 10),
        Candidate::eligible(7, 0, Some(50), 20),
        Candidate::eligible(6, 0, Some(50), 10),
        Candidate::eligible(5, 0, Some(40), 30),
    ];
    candidates.push(Candidate {
        card_id: 1,
        priority_rank: -1,
        deadline: Some(1),
        ready_at: 1,
        dependencies_satisfied: true,
        blockers_resolved: true,
        gate_resolved: true,
        no_open_run: true,
        workspace_converged: true,
        locks_available: false,
        adapter_available: true,
    });
    assert_eq!(select_candidate(&candidates).map(|c| c.card_id), Some(5));
}

#[test]
fn tab_cardinality_is_derived_from_role_not_labels() {
    assert_eq!(
        decide_tab_policy(AggregateRole::Ready, false),
        TabPolicy::Forbidden
    );
    assert_eq!(
        decide_tab_policy(AggregateRole::Executing, false),
        TabPolicy::Required
    );
    assert_eq!(
        decide_tab_policy(AggregateRole::Verifying, false),
        TabPolicy::Required
    );
    assert_eq!(
        decide_tab_policy(AggregateRole::HumanGate, true),
        TabPolicy::Inspectable
    );
    assert_eq!(
        decide_tab_policy(AggregateRole::HumanGate, false),
        TabPolicy::Forbidden
    );
    assert_eq!(
        decide_tab_policy(AggregateRole::Blocked, true),
        TabPolicy::Forbidden
    );
    assert_eq!(
        decide_tab_policy(AggregateRole::Done, true),
        TabPolicy::Forbidden
    );
}

#[test]
fn reducer_distinguishes_infrastructure_interruption_from_work_failure() {
    let queued = ControllerAggregate {
        role: AggregateRole::Queued,
        work_state: WorkState::NotStarted,
        open_run_id: Some(41),
        retry_budget: 1,
        ..ControllerAggregate::default()
    };
    assert_eq!(
        reduce_controller(&queued, &ControllerEvent::SpawnFailedPreAck { run_id: 41 }),
        ControllerReduction {
            aggregate: ControllerAggregate {
                role: AggregateRole::Ready,
                work_state: WorkState::Interrupted,
                open_run_id: None,
                retry_budget: 0,
                ..ControllerAggregate::default()
            },
            effects: vec![EffectIntent::RetrySpawn { run_id: 41 }],
            diagnostics: vec![],
        }
    );

    let running = ControllerAggregate {
        role: AggregateRole::Executing,
        work_state: WorkState::Started,
        open_run_id: Some(42),
        prompt_acknowledged: true,
        ..ControllerAggregate::default()
    };
    let reduced = reduce_controller(&running, &ControllerEvent::PaneExitedPostAck { run_id: 42 });
    assert_eq!(reduced.aggregate.work_state, WorkState::Failed);
    assert_eq!(reduced.aggregate.role, AggregateRole::Blocked);
}

#[test]
fn gate_decision_applies_only_to_the_exact_request_and_digest() {
    let state = ControllerAggregate {
        role: AggregateRole::HumanGate,
        pending_gate: Some(board_core::engine::PendingGate {
            request_id: "request-1".into(),
            artifact_digest: "sha256:one".into(),
        }),
        ..ControllerAggregate::default()
    };
    let stale = reduce_controller(
        &state,
        &ControllerEvent::GateDecided {
            request_id: "request-1".into(),
            artifact_digest: "sha256:old".into(),
            decision: GateDecision::Approve,
        },
    );
    assert_eq!(stale.aggregate, state);
    assert_eq!(stale.diagnostics, vec!["gate_decision_mismatch"]);

    let accepted = reduce_controller(
        &state,
        &ControllerEvent::GateDecided {
            request_id: "request-1".into(),
            artifact_digest: "sha256:one".into(),
            decision: GateDecision::Approve,
        },
    );
    assert_eq!(accepted.aggregate.role, AggregateRole::Ready);
    assert_eq!(accepted.aggregate.pending_gate, None);
    assert_eq!(accepted.effects, vec![EffectIntent::EvaluateCandidates]);
}

#[test]
fn recovery_plans_are_conservative_and_exact_id_based() {
    assert_eq!(
        plan_recovery(RecoveryCase::ReviewShellGoneWithoutReceipt),
        RecoveryPlan::ReturnToHumanGate
    );
    assert_eq!(
        plan_recovery(RecoveryCase::OutboxEffectAfterRestart {
            observation: Observation::Unknown,
        }),
        RecoveryPlan::KeepPending
    );
    assert_eq!(
        plan_recovery(RecoveryCase::OutboxEffectAfterRestart {
            observation: Observation::Present,
        }),
        RecoveryPlan::AcknowledgeObservedEffect
    );
    assert_eq!(
        plan_recovery(RecoveryCase::DuplicateOwnedTabs {
            canonical_tab_id: "w1:t1".into(),
            stale_tab_ids: vec!["w1:t2".into()],
            active_run_tab_id: Some("w1:t1".into()),
        }),
        RecoveryPlan::CloseExactTabs {
            tab_ids: vec!["w1:t2".into()]
        }
    );
    assert_eq!(
        plan_recovery(RecoveryCase::ActiveTopologyDrift),
        RecoveryPlan::BlockNewEffects
    );
    assert_eq!(
        plan_recovery(RecoveryCase::ClosePending {
            observation: Observation::Gone,
        }),
        RecoveryPlan::AcknowledgeObservedEffect
    );
    assert_eq!(
        plan_recovery(RecoveryCase::ClosePending {
            observation: Observation::Present,
        }),
        RecoveryPlan::ReissueSameEffect
    );
}

#[test]
fn duplicate_tab_cleanup_blocks_any_protected_or_ambiguous_close_id() {
    for stale_tab_ids in [
        vec!["w1:t1".into(), "w1:t2".into()],
        vec!["w1:t2".into(), "w1:t2".into()],
        vec!["".into()],
    ] {
        assert_eq!(
            plan_recovery(RecoveryCase::DuplicateOwnedTabs {
                canonical_tab_id: "w1:t1".into(),
                stale_tab_ids,
                active_run_tab_id: Some("w1:t1".into()),
            }),
            RecoveryPlan::BlockConflict
        );
    }

    assert_eq!(
        plan_recovery(RecoveryCase::DuplicateOwnedTabs {
            canonical_tab_id: "w1:t1".into(),
            stale_tab_ids: vec!["w1:t3".into()],
            active_run_tab_id: Some("w1:t3".into()),
        }),
        RecoveryPlan::BlockConflict
    );
}
