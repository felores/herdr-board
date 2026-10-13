use super::*;

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn evidence_is_idempotent_conflicting_and_lifecycle_neutral() {
    let d = test_daemon(Config::default());
    let card = handle_request(&d, "card.create", json!({"title":"evidence"})).unwrap();
    let card_id = card["id"].as_i64().unwrap();
    let params = json!({
        "card_id": card_id,
        "producer": "harness",
        "dedupe_key": "harness:repo:feature:run:outcome",
        "schema": "harness-board-evidence/v1",
        "payload": {"status":"verified","board_card_id":card_id}
    });
    let first = handle_request(&d, "evidence.put", params.clone()).unwrap();
    let replay = handle_request(&d, "evidence.put", params).unwrap();
    assert_eq!(first, replay);
    assert!(handle_request(
        &d,
        "evidence.put",
        json!({
            "card_id": card_id,
            "producer": "harness",
            "dedupe_key": "harness:repo:feature:run:outcome",
            "schema": "harness-board-evidence/v1",
            "payload": {"status":"changed"}
        })
    )
    .is_err());
    let detail = handle_request(&d, "card.get", json!({"id":card_id})).unwrap();
    assert_eq!(detail["card"]["status"], "idle");
    assert_eq!(detail["comments"].as_array().unwrap().len(), 0);
    assert_eq!(first["payload_hash"].as_str().unwrap().len(), 71);
    assert!(handle_request(
        &d,
        "evidence.put",
        json!({
            "card_id": card_id,
            "producer": "harness",
            "dedupe_key": "wrong-binding",
            "schema": "harness-board-evidence/v1",
            "payload": {"board_card_id":card_id + 1}
        })
    )
    .is_err());
}

#[test]
fn accepted_exact_decision_receipt_is_the_only_blocked_to_running_path() {
    let receipt_key = "11".repeat(32);
    std::env::set_var("BOARD_RECEIPT_KEY", &receipt_key);
    let d = test_daemon(Config::default());
    let (card_id, run_id) = add_run_with_pane(&d, Some("p1"));
    d.store
        .lock()
        .set_card_status(card_id, CardStatus::Blocked)
        .unwrap();
    let opened = handle_request(
        &d,
        "decision.open",
        json!({
            "board_id": 1,
            "card_id": card_id,
            "run_id": run_id,
            "question_hash": DIGEST_A,
            "target": {
                "machine_id":"local",
                "herdr_session":"default",
                "workspace_id":"w1",
                "pane_id":"p1",
                "terminal_id":"term-1",
                "agent_session_identity":"session-1"
            }
        }),
    )
    .unwrap();
    let decision_id = opened["decision_id"].as_str().unwrap();
    let fingerprint = opened["target_fingerprint"].as_str().unwrap();
    let signature = board_core::db::receipt_signature(
        &receipt_key,
        decision_id,
        "delivery-1",
        fingerprint,
        DIGEST_B,
    )
    .unwrap();

    assert!(handle_request(
        &d,
        "decision.receipt",
        json!({
            "decision_id": decision_id,
            "delivery_id":"delivery-1",
            "target_fingerprint":fingerprint,
            "answer_hash":DIGEST_B,
            "status":"accepted",
            "receipt_signature":"hmac-sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        })
    )
    .is_err());
    assert_eq!(
        d.store.lock().require_card(card_id).unwrap().status,
        CardStatus::Blocked
    );

    assert!(handle_request(
        &d,
        "decision.receipt",
        json!({
            "decision_id": decision_id,
            "delivery_id":"delivery-1",
            "target_fingerprint":fingerprint,
            "answer_hash":DIGEST_B,
            "status":"unknown",
            "receipt_signature":signature
        })
    )
    .is_err());
    assert_eq!(
        d.store.lock().require_card(card_id).unwrap().status,
        CardStatus::Blocked
    );

    let accepted = json!({
        "decision_id": decision_id,
        "delivery_id":"delivery-1",
        "target_fingerprint":fingerprint,
        "answer_hash":DIGEST_B,
        "status":"accepted",
        "receipt_signature":signature
    });
    let first = handle_request(&d, "decision.receipt", accepted.clone()).unwrap();
    let replay = handle_request(&d, "decision.receipt", accepted).unwrap();
    assert_eq!(first, replay);
    assert_eq!(first["state"], "delivered");
    assert_eq!(
        d.store.lock().require_card(card_id).unwrap().status,
        CardStatus::Running
    );
    assert!(handle_request(
        &d,
        "decision.receipt",
        json!({
            "decision_id": decision_id,
            "delivery_id":"delivery-2",
            "target_fingerprint":fingerprint,
            "answer_hash":DIGEST_A,
            "status":"accepted",
            "receipt_signature":"hmac-sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        })
    )
    .is_err());

    let same_question = json!({
        "board_id":1,"card_id":card_id,"run_id":run_id,"question_hash":DIGEST_A,
        "target":{
            "machine_id":"local","herdr_session":"default","workspace_id":"w1",
            "pane_id":"p1","terminal_id":"term-1","agent_session_identity":"session-1"
        }
    });
    // A delivered receipt does not authorize a second, identical blocking event.
    assert!(handle_request(&d, "decision.open", same_question.clone()).is_err());
    d.store
        .lock()
        .set_card_status(card_id, CardStatus::Blocked)
        .unwrap();
    let next = handle_request(&d, "decision.open", same_question.clone()).unwrap();
    assert_ne!(next["decision_id"], decision_id);
    assert_eq!(
        next,
        handle_request(&d, "decision.open", same_question.clone()).unwrap()
    );
    let mut wrong_board = same_question.clone();
    wrong_board["board_id"] = json!(999);
    assert!(handle_request(&d, "decision.open", wrong_board).is_err());
    let other = handle_request(&d, "card.create", json!({"title":"unrelated"})).unwrap();
    let other_id = other["id"].as_i64().unwrap();
    let mut wrong_card = same_question;
    wrong_card["card_id"] = json!(other_id);
    assert!(handle_request(&d, "decision.open", wrong_card).is_err());
}

#[test]
fn changed_question_supersedes_the_prior_open_occurrence() {
    let d = test_daemon(Config::default());
    let (card_id, run_id) = add_run_with_pane(&d, Some("p1"));
    d.store
        .lock()
        .set_card_status(card_id, CardStatus::Blocked)
        .unwrap();
    let target = json!({
        "machine_id":"local","herdr_session":"default","workspace_id":"w1",
        "pane_id":"p1","terminal_id":"term-1","agent_session_identity":"session-1"
    });
    let first = handle_request(
        &d,
        "decision.open",
        json!({
            "board_id":1,"card_id":card_id,"run_id":run_id,"question_hash":DIGEST_A,"target":target
        }),
    )
    .unwrap();
    let second = handle_request(
        &d,
        "decision.open",
        json!({
            "board_id":1,"card_id":card_id,"run_id":run_id,"question_hash":DIGEST_B,"target":target
        }),
    )
    .unwrap();
    assert_ne!(first["decision_id"], second["decision_id"]);
    let prior = handle_request(
        &d,
        "decision.get",
        json!({"decision_id":first["decision_id"]}),
    )
    .unwrap();
    assert_eq!(prior["state"], "superseded");
}
