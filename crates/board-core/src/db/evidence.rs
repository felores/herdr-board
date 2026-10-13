use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use uuid::Uuid;

use super::Db;
use crate::model::{DecisionOccurrence, DecisionTarget, Evidence};
use crate::protocol::CardStatus;
use crate::{Error, Result};

fn sha256_bytes(value: &[u8]) -> [u8; 32] {
    const H: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut bytes = value.to_vec();
    let bit_len = (bytes.len() as u64) * 8;
    bytes.push(0x80);
    while bytes.len() % 64 != 56 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&bit_len.to_be_bytes());
    let mut state = H;
    for chunk in bytes.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut output = [0u8; 32];
    for (chunk, word) in output.chunks_exact_mut(4).zip(state) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    output
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256(value: &str) -> String {
    format!("sha256:{}", hex(&sha256_bytes(value.as_bytes())))
}

pub fn receipt_signature(
    key_hex: &str,
    decision_id: &str,
    delivery_id: &str,
    target_fingerprint: &str,
    answer_hash: &str,
) -> Option<String> {
    if key_hex.len() != 64 || !key_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut key = [0u8; 64];
    for (index, pair) in key_hex.as_bytes().chunks_exact(2).enumerate() {
        key[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    let mut inner = key.map(|byte| byte ^ 0x36).to_vec();
    inner.extend_from_slice(
        format!("v1|{decision_id}|{delivery_id}|{target_fingerprint}|{answer_hash}|accepted")
            .as_bytes(),
    );
    let inner_hash = sha256_bytes(&inner);
    let mut outer = key.map(|byte| byte ^ 0x5c).to_vec();
    outer.extend_from_slice(&inner_hash);
    Some(format!("hmac-sha256:{}", hex(&sha256_bytes(&outer))))
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(items) => Value::Object(
            items
                .iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn target_fingerprint(target: &DecisionTarget) -> String {
    sha256(&format!(
        "v1|{}|{}|{}|{}|{}|{}",
        target.machine_id,
        target.herdr_session,
        target.workspace_id,
        target.pane_id,
        target.terminal_id,
        target.agent_session_identity
    ))
}

fn row_to_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<Evidence> {
    let payload_json: String = row.get("payload_json")?;
    let payload = serde_json::from_str(&payload_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(Evidence {
        id: row.get("id")?,
        card_id: row.get("card_id")?,
        producer: row.get("producer")?,
        dedupe_key: row.get("dedupe_key")?,
        schema: row.get("schema_name")?,
        payload,
        payload_hash: row.get("payload_hash")?,
        created_at: row.get("created_at")?,
    })
}

fn row_to_decision(row: &rusqlite::Row<'_>) -> rusqlite::Result<DecisionOccurrence> {
    Ok(DecisionOccurrence {
        decision_id: row.get("id")?,
        board_id: row.get("board_id")?,
        card_id: row.get("card_id")?,
        run_id: row.get("run_id")?,
        question_hash: row.get("question_hash")?,
        target: DecisionTarget {
            machine_id: row.get("machine_id")?,
            herdr_session: row.get("herdr_session")?,
            workspace_id: row.get("workspace_id")?,
            pane_id: row.get("pane_id")?,
            terminal_id: row.get("terminal_id")?,
            agent_session_identity: row.get("agent_session_identity")?,
        },
        target_fingerprint: row.get("target_fingerprint")?,
        state: row.get("state")?,
        delivery_id: row.get("delivery_id")?,
        answer_hash: row.get("answer_hash")?,
        created_at: row.get("created_at")?,
        delivered_at: row.get("delivered_at")?,
    })
}

const EVIDENCE_SELECT: &str = "SELECT id,card_id,producer,dedupe_key,schema_name,payload_json,payload_hash,created_at FROM evidence";
const DECISION_SELECT: &str = "SELECT id,board_id,card_id,run_id,question_hash,machine_id,herdr_session,workspace_id,pane_id,terminal_id,agent_session_identity,target_fingerprint,state,delivery_id,answer_hash,created_at,delivered_at FROM decisions";

impl Db {
    pub fn put_evidence(
        &self,
        card_id: i64,
        producer: &str,
        dedupe_key: &str,
        schema: &str,
        payload: &Value,
    ) -> Result<Evidence> {
        self.require_card(card_id)?;
        if !payload.is_object() {
            return Err(Error::BadRequest(
                "evidence payload must be an object".into(),
            ));
        }
        let payload_json = serde_json::to_string(&canonical(payload))?;
        if payload_json.len() > 64 * 1024 {
            return Err(Error::BadRequest("evidence payload exceeds 64 KiB".into()));
        }
        let payload_hash = sha256(&payload_json);
        if let Some(existing) = self.get_evidence(card_id, producer, dedupe_key)? {
            if existing.schema == schema && existing.payload_hash == payload_hash {
                return Ok(existing);
            }
            return Err(Error::InvalidState(
                "evidence dedupe key already has a different payload".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO evidence(card_id,producer,dedupe_key,schema_name,payload_json,payload_hash) VALUES(?1,?2,?3,?4,?5,?6)",
            params![card_id, producer, dedupe_key, schema, payload_json, payload_hash],
        )?;
        self.get_evidence(card_id, producer, dedupe_key)?
            .ok_or_else(|| Error::InvalidState("inserted evidence could not be read".into()))
    }

    pub fn get_evidence(
        &self,
        card_id: i64,
        producer: &str,
        dedupe_key: &str,
    ) -> Result<Option<Evidence>> {
        Ok(self
            .conn
            .query_row(
                &format!("{EVIDENCE_SELECT} WHERE card_id=?1 AND producer=?2 AND dedupe_key=?3"),
                params![card_id, producer, dedupe_key],
                row_to_evidence,
            )
            .optional()?)
    }

    pub fn open_decision(
        &self,
        board_id: i64,
        card_id: i64,
        run_id: i64,
        question_hash: &str,
        target: &DecisionTarget,
    ) -> Result<DecisionOccurrence> {
        let tx = self.conn.unchecked_transaction()?;
        let fingerprint = target_fingerprint(target);
        let replay = tx
            .query_row(
                &format!("{DECISION_SELECT} WHERE run_id=?1 AND question_hash=?2 AND target_fingerprint=?3 AND state IN ('open','delivered') ORDER BY created_at DESC LIMIT 1"),
                params![run_id, question_hash, fingerprint],
                row_to_decision,
            )
            .optional()?;
        if let Some(replay) = replay {
            tx.commit()?;
            return Ok(replay);
        }
        let card = tx
            .query_row(
                "SELECT board_id,status FROM cards WHERE id=?1",
                params![card_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("card {card_id}")))?;
        if card.0 != board_id || card.1 != CardStatus::Blocked.as_str() {
            return Err(Error::InvalidState(
                "decision requires the exact blocked card and board".into(),
            ));
        }
        let run_target = tx.query_row(
            "SELECT card_id,herdr_workspace_id,herdr_pane_id,session FROM runs WHERE id=?1 AND ended_at IS NULL",
            params![run_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?)),
        ).optional()?.ok_or_else(|| Error::NotFound(format!("open run {run_id}")))?;
        if run_target.0 != card_id
            || run_target.1.as_deref() != Some(target.workspace_id.as_str())
            || run_target.2.as_deref() != Some(target.pane_id.as_str())
            || run_target.3.as_deref().unwrap_or("default") != target.herdr_session
        {
            return Err(Error::InvalidState(
                "decision target does not match the exact open run".into(),
            ));
        }
        let existing = tx
            .query_row(
                &format!("{DECISION_SELECT} WHERE run_id=?1 AND state='open'"),
                params![run_id],
                row_to_decision,
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing.question_hash == question_hash && existing.target_fingerprint == fingerprint
            {
                tx.commit()?;
                return Ok(existing);
            }
            tx.execute(
                "UPDATE decisions SET state='superseded' WHERE id=?1",
                params![existing.decision_id],
            )?;
        }
        let decision_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO decisions(id,board_id,card_id,run_id,question_hash,machine_id,herdr_session,workspace_id,pane_id,terminal_id,agent_session_identity,target_fingerprint,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'open')",
            params![decision_id, board_id, card_id, run_id, question_hash, target.machine_id, target.herdr_session, target.workspace_id, target.pane_id, target.terminal_id, target.agent_session_identity, fingerprint],
        )?;
        let decision = tx.query_row(
            &format!("{DECISION_SELECT} WHERE id=?1"),
            params![decision_id],
            row_to_decision,
        )?;
        tx.commit()?;
        Ok(decision)
    }

    pub fn get_decision(&self, decision_id: &str) -> Result<Option<DecisionOccurrence>> {
        Ok(self
            .conn
            .query_row(
                &format!("{DECISION_SELECT} WHERE id=?1"),
                params![decision_id],
                row_to_decision,
            )
            .optional()?)
    }

    pub fn has_open_decision(&self, run_id: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM decisions WHERE run_id=?1 AND state='open')",
            params![run_id],
            |row| row.get(0),
        )?)
    }

    pub fn adopt_decision_receipt(
        &self,
        decision_id: &str,
        delivery_id: &str,
        target_fingerprint: &str,
        answer_hash: &str,
    ) -> Result<DecisionOccurrence> {
        let tx = self.conn.unchecked_transaction()?;
        let decision = tx
            .query_row(
                &format!("{DECISION_SELECT} WHERE id=?1"),
                params![decision_id],
                row_to_decision,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("decision {decision_id}")))?;
        if decision.state == "delivered" {
            if decision.delivery_id.as_deref() == Some(delivery_id)
                && decision.answer_hash.as_deref() == Some(answer_hash)
                && decision.target_fingerprint == target_fingerprint
            {
                tx.commit()?;
                return Ok(decision);
            }
            return Err(Error::InvalidState(
                "decision already has a different delivery receipt".into(),
            ));
        }
        if decision.state != "open" || decision.target_fingerprint != target_fingerprint {
            return Err(Error::InvalidState(
                "receipt does not match an open decision target".into(),
            ));
        }
        let current = tx.query_row(
            "SELECT c.status,r.card_id,r.ended_at,r.herdr_workspace_id,r.herdr_pane_id,r.session FROM cards c JOIN runs r ON r.id=?1 WHERE c.id=?2",
            params![decision.run_id, decision.card_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, Option<String>>(4)?, row.get::<_, Option<String>>(5)?)),
        )?;
        if current.0 != CardStatus::Blocked.as_str()
            || current.1 != decision.card_id
            || current.2.is_some()
            || current.3.as_deref() != Some(decision.target.workspace_id.as_str())
            || current.4.as_deref() != Some(decision.target.pane_id.as_str())
            || current.5.as_deref().unwrap_or("default") != decision.target.herdr_session
        {
            return Err(Error::InvalidState(
                "decision no longer matches the exact blocked run".into(),
            ));
        }
        tx.execute(
            "UPDATE decisions SET state='delivered',delivery_id=?2,answer_hash=?3,delivered_at=datetime('now') WHERE id=?1",
            params![decision_id, delivery_id, answer_hash],
        )?;
        tx.execute(
            "UPDATE cards SET status='running',awaiting_reason=NULL,updated_at=datetime('now') WHERE id=?1",
            params![decision.card_id],
        )?;
        let delivered = tx.query_row(
            &format!("{DECISION_SELECT} WHERE id=?1"),
            params![decision_id],
            row_to_decision,
        )?;
        tx.commit()?;
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::sha256;

    #[test]
    fn sha256_matches_the_standard_empty_vector() {
        assert_eq!(
            sha256(""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn receipt_hmac_matches_node_crypto_vector() {
        assert_eq!(
            super::receipt_signature(
                &"11".repeat(32),
                "decision-1",
                "delivery-1",
                &format!("sha256:{}", "a".repeat(64)),
                &format!("sha256:{}", "b".repeat(64)),
            )
            .as_deref(),
            Some("hmac-sha256:279c1a2ea20c9236a05ef28c44430fef31c96ad0b8ae993441c41c7d8a3effa7")
        );
    }
}
