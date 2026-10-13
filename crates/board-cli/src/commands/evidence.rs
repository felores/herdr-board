use anyhow::{Context, Result};
use board_core::client::BoardClient;
use board_core::model::DecisionTarget;
use board_core::protocol::{
    DecisionGetParams, DecisionOpenParams, DecisionReceiptParams, EvidenceGetParams,
    EvidencePutParams,
};

use crate::args::{DecisionCmd, EvidenceCmd};
use crate::context::Ctx;
use crate::render::emit_line;

pub(crate) fn cmd_evidence(sub: EvidenceCmd, ctx: &mut Ctx) -> Result<()> {
    let json = ctx.json();
    let value = match sub {
        EvidenceCmd::Put {
            card_id,
            producer,
            dedupe_key,
            schema,
            payload_json,
        } => {
            let payload = serde_json::from_str(&payload_json).context("--payload-json")?;
            let params = EvidencePutParams {
                card_id,
                producer,
                dedupe_key,
                schema,
                payload,
            };
            ctx.client()?
                .call("evidence.put", serde_json::to_value(params)?)?
        }
        EvidenceCmd::Get {
            card_id,
            producer,
            dedupe_key,
        } => {
            let params = EvidenceGetParams {
                card_id,
                producer,
                dedupe_key,
            };
            ctx.client()?
                .call("evidence.get", serde_json::to_value(params)?)?
        }
    };
    emit_line(&value, json, "Board evidence confirmed")
}

pub(crate) fn cmd_decision(sub: DecisionCmd, ctx: &mut Ctx) -> Result<()> {
    let json = ctx.json();
    let (method, params, message) = match sub {
        DecisionCmd::Open {
            board_id,
            card_id,
            run_id,
            question_hash,
            target_json,
        } => (
            "decision.open",
            serde_json::to_value(DecisionOpenParams {
                board_id,
                card_id,
                run_id,
                question_hash,
                target: serde_json::from_str::<DecisionTarget>(&target_json)
                    .context("--target-json")?,
            })?,
            "Board decision opened",
        ),
        DecisionCmd::Get { decision_id } => (
            "decision.get",
            serde_json::to_value(DecisionGetParams { decision_id })?,
            "Board decision loaded",
        ),
        DecisionCmd::Receipt {
            decision_id,
            delivery_id,
            target_fingerprint,
            answer_hash,
            status,
            receipt_signature,
        } => (
            "decision.receipt",
            serde_json::to_value(DecisionReceiptParams {
                decision_id,
                delivery_id,
                target_fingerprint,
                answer_hash,
                status,
                receipt_signature,
            })?,
            "Board decision receipt adopted",
        ),
    };
    let value = ctx.client()?.call(method, params)?;
    emit_line(&value, json, message)
}
