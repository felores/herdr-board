//! Phase-1 characterization for the approved deterministic controller.
//!
//! These tests freeze the evidence and compatibility boundaries before schema
//! v16 changes any production behavior. They intentionally do not implement
//! the reducer, controller loop, Human Gate UI, or Herdr cleanup effects.

use std::collections::BTreeSet;
use std::path::Path;

use board_core::capability::{run_pane_name, run_pane_name_unique};
use board_core::scope::resolve_scope_path;
use serde::Deserialize;

const FIXTURE: &str = include_str!("fixtures/controller_contract_characterization.json");

#[derive(Debug, Deserialize)]
struct Characterization {
    schema: String,
    architecture: Architecture,
    phase: String,
    failures: Vec<Failure>,
    invariants: Invariants,
    baseline_tests: Vec<String>,
    next_schema: i64,
}

#[derive(Debug, Deserialize)]
struct Architecture {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct Failure {
    id: String,
    owner: String,
    baseline: String,
    target: String,
    acceptance: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Invariants {
    identity: Vec<String>,
    legacy: Vec<String>,
    workspace: Vec<String>,
    human_gate: Vec<String>,
}

fn fixture() -> Characterization {
    serde_json::from_str(FIXTURE).expect("controller characterization fixture must stay valid")
}

#[test]
fn approved_architecture_and_nine_failure_cases_are_pinned() {
    let value = fixture();
    assert_eq!(value.schema, "board-controller-characterization/v1");
    assert_eq!(value.phase, "characterization-tests");
    assert_eq!(value.next_schema, 16);
    assert_eq!(value.failures.len(), 9);
    assert_eq!(
        value.architecture.sha256,
        "9dc15b15988440ad8b2307f2d20af76431012db3c8c64a3e2764c0abbda4c722"
    );
    assert!(value
        .architecture
        .path
        .ends_with("/2026-09-16-155940_contrato-orquestador-board.md"));

    let ids: BTreeSet<_> = value.failures.iter().map(|case| case.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        value.failures.len(),
        "failure ids must be unique"
    );
    assert_eq!(
        ids,
        BTreeSet::from([
            "approved-feedback-stalled",
            "async-state-display-drift",
            "controller-returned-control",
            "dead-review-shell",
            "description-became-authority",
            "free-text-card-drift",
            "material-work-bypassed-board",
            "readiness-was-manual",
            "spawn-failure-counted-as-work",
        ])
    );
}

#[test]
fn every_failure_has_one_owner_and_executable_acceptance_evidence() {
    let value = fixture();
    let allowed_owners = BTreeSet::from(["board-core", "board-daemon"]);
    for case in value.failures {
        assert!(allowed_owners.contains(case.owner.as_str()), "{}", case.id);
        assert!(!case.baseline.trim().is_empty(), "{} baseline", case.id);
        assert!(!case.target.trim().is_empty(), "{} target", case.id);
        assert!(!case.acceptance.is_empty(), "{} acceptance", case.id);
        assert!(
            case.acceptance.iter().all(|id| id
                .strip_prefix('A')
                .is_some_and(|n| n.parse::<u8>().is_ok())),
            "{} acceptance ids",
            case.id
        );
    }
}

#[test]
fn identity_legacy_workspace_and_human_gate_boundaries_are_explicit() {
    let value = fixture();
    assert_eq!(value.invariants.identity.len(), 3);
    assert_eq!(value.invariants.legacy.len(), 2);
    assert_eq!(value.invariants.workspace.len(), 3);
    assert_eq!(value.invariants.human_gate.len(), 3);
    assert!(value
        .invariants
        .identity
        .iter()
        .any(|rule| rule == "tab labels never prove ownership"));
    assert!(value
        .invariants
        .human_gate
        .iter()
        .any(|rule| rule.contains("request_id")));
}

#[test]
fn baseline_test_map_covers_core_daemon_and_tui_boundaries() {
    let value = fixture();
    for owner in ["board-core::", "board-daemon::", "board-tui::"] {
        assert!(
            value
                .baseline_tests
                .iter()
                .any(|test| test.starts_with(owner)),
            "missing baseline test for {owner}"
        );
    }
}

#[test]
fn pane_names_remain_card_identity_not_human_tab_labels() {
    assert_eq!(run_pane_name(7, "Human Gate"), "card-7-human-gate");
    assert_eq!(
        run_pane_name_unique(7, "Verification", 43),
        "card-7-verification-r43"
    );
    assert!(!run_pane_name(7, "Executing").contains('·'));
}

#[test]
fn descendant_task_path_resolves_to_one_canonical_git_root() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .canonicalize()
        .expect("canonical workspace root");
    let descendant = repo.join("crates/board-core/tests");
    assert_eq!(resolve_scope_path(&descendant).unwrap(), repo);
}
