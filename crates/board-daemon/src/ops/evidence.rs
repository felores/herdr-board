use super::*;

fn bounded(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 {
        return Err(Error::BadRequest(format!("{field} must be 1..=256 bytes")));
    }
    Ok(())
}

fn digest(value: &str, field: &str) -> Result<()> {
    if !value.starts_with("sha256:") || value.len() != 71 {
        return Err(Error::BadRequest(format!(
            "{field} must be a sha256:<64-hex> digest"
        )));
    }
    if !value[7..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::BadRequest(format!("{field} is not hexadecimal")));
    }
    Ok(())
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

pub(super) fn evidence_put(d: &Arc<Daemon>, p: EvidencePutParams) -> Result<Value> {
    bounded(&p.producer, "producer")?;
    bounded(&p.dedupe_key, "dedupe_key")?;
    bounded(&p.schema, "schema")?;
    if p.schema == "harness-board-evidence/v1"
        && (p.producer != "harness"
            || p.payload.get("board_card_id").and_then(Value::as_i64) != Some(p.card_id))
    {
        return Err(Error::BadRequest(
            "Harness evidence must bind the same card_id and producer".into(),
        ));
    }
    let evidence = d.store.lock().put_evidence(
        p.card_id,
        p.producer.trim(),
        p.dedupe_key.trim(),
        p.schema.trim(),
        &p.payload,
    )?;
    Ok(json!(evidence))
}

pub(super) fn evidence_get(d: &Arc<Daemon>, p: EvidenceGetParams) -> Result<Value> {
    bounded(&p.producer, "producer")?;
    bounded(&p.dedupe_key, "dedupe_key")?;
    let evidence = d
        .store
        .lock()
        .get_evidence(p.card_id, p.producer.trim(), p.dedupe_key.trim())?
        .ok_or_else(|| Error::NotFound("evidence not found".into()))?;
    Ok(json!(evidence))
}

pub(super) fn decision_open(d: &Arc<Daemon>, p: DecisionOpenParams) -> Result<Value> {
    digest(&p.question_hash, "question_hash")?;
    for (field, value) in [
        ("machine_id", p.target.machine_id.as_str()),
        ("herdr_session", p.target.herdr_session.as_str()),
        ("workspace_id", p.target.workspace_id.as_str()),
        ("pane_id", p.target.pane_id.as_str()),
        ("terminal_id", p.target.terminal_id.as_str()),
        (
            "agent_session_identity",
            p.target.agent_session_identity.as_str(),
        ),
    ] {
        bounded(value, field)?;
    }
    let decision = d.store.lock().open_decision(
        p.board_id,
        p.card_id,
        p.run_id,
        &p.question_hash,
        &p.target,
    )?;
    Ok(json!(decision))
}

pub(super) fn decision_get(d: &Arc<Daemon>, p: DecisionGetParams) -> Result<Value> {
    bounded(&p.decision_id, "decision_id")?;
    let decision = d
        .store
        .lock()
        .get_decision(&p.decision_id)?
        .ok_or_else(|| Error::NotFound(format!("decision {}", p.decision_id)))?;
    Ok(json!(decision))
}

pub(super) fn decision_receipt(d: &Arc<Daemon>, p: DecisionReceiptParams) -> Result<Value> {
    bounded(&p.decision_id, "decision_id")?;
    bounded(&p.delivery_id, "delivery_id")?;
    digest(&p.target_fingerprint, "target_fingerprint")?;
    digest(&p.answer_hash, "answer_hash")?;
    if p.status != "accepted" {
        return Err(Error::InvalidState(
            "only an accepted Shepherdr receipt may resolve a decision".into(),
        ));
    }
    let key = std::env::var("BOARD_RECEIPT_KEY")
        .map_err(|_| Error::InvalidState("Board receipt verification is not configured".into()))?;
    let expected = board_core::db::receipt_signature(
        &key,
        &p.decision_id,
        &p.delivery_id,
        &p.target_fingerprint,
        &p.answer_hash,
    )
    .ok_or_else(|| Error::InvalidState("Shepherdr receipt key is invalid".into()))?;
    if !constant_time_eq(&expected, &p.receipt_signature) {
        return Err(Error::InvalidState(
            "Shepherdr receipt signature is invalid".into(),
        ));
    }
    let was_open = d
        .store
        .lock()
        .get_decision(&p.decision_id)?
        .is_some_and(|decision| decision.state == "open");
    let decision = d.store.lock().adopt_decision_receipt(
        &p.decision_id,
        &p.delivery_id,
        &p.target_fingerprint,
        &p.answer_hash,
    )?;
    if was_open {
        d.emit_changed(
            BoardChangedReason::CardUpdated,
            Some(decision.card_id),
            None,
        );
    }
    Ok(json!(decision))
}
