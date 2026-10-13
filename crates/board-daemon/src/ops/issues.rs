use super::*;
use crate::ops::cards::stamp_card_labels;

fn issue_change(d: &Arc<Daemon>, p: CardIssueParams, refresh: bool) -> Result<Value> {
    let (mut card, changed) = {
        let _sched = d.sched.lock().unwrap();
        let db = d.store.lock();
        let before = db.require_card(p.card_id)?;
        let after = if refresh {
            db.refresh_card_issue(p.card_id, &p.issue)?
        } else {
            db.bind_card_issue(p.card_id, &p.issue)?
        };
        let changed = before.source_issue != after.source_issue;
        (after, changed)
    };
    if changed {
        d.emit_changed(BoardChangedReason::CardUpdated, Some(card.id), None);
    }
    stamp_card_labels(d, &mut card);
    Ok(json!(card))
}

pub(super) fn bind(d: &Arc<Daemon>, p: CardIssueParams) -> Result<Value> {
    issue_change(d, p, false)
}

pub(super) fn refresh(d: &Arc<Daemon>, p: CardIssueParams) -> Result<Value> {
    issue_change(d, p, true)
}

pub(super) fn unlink(d: &Arc<Daemon>, p: CardIdParams) -> Result<Value> {
    let (mut card, changed) = {
        let _sched = d.sched.lock().unwrap();
        let db = d.store.lock();
        let changed = db.require_card(p.id)?.source_issue.is_some();
        (db.unlink_card_issue(p.id)?, changed)
    };
    if changed {
        d.emit_changed(BoardChangedReason::CardUpdated, Some(card.id), None);
    }
    stamp_card_labels(d, &mut card);
    Ok(json!(card))
}

pub(super) fn history(d: &Arc<Daemon>, p: CardIdParams) -> Result<Value> {
    Ok(json!(d.store.lock().card_issue_history(p.id)?))
}
