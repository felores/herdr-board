use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Present,
    Gone,
    Unknown,
    Conflict,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryCase {
    SpawnFailedPreAck {
        retries_remaining: u32,
    },
    PaneGonePostAck,
    ReviewShellGoneWithoutReceipt,
    QueuedStale {
        effect_exists: bool,
    },
    OutboxEffectAfterRestart {
        observation: Observation,
    },
    DigestConflict,
    DuplicateOwnedTabs {
        canonical_tab_id: String,
        stale_tab_ids: Vec<String>,
        active_run_tab_id: Option<String>,
    },
    ClosePending {
        observation: Observation,
    },
    ActiveTopologyDrift,
    LockExpiredWithUnknownLiveness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryPlan {
    RetryInterruptedAttempt,
    BlockRuntimeUnavailable,
    FailStartedWork,
    ReturnToHumanGate,
    ReclaimQueuedRun,
    KeepPending,
    AcknowledgeObservedEffect,
    ReissueSameEffect,
    BlockConflict,
    CloseExactTabs { tab_ids: Vec<String> },
    BlockNewEffects,
}

/// Convert one reconciled failure condition into a side-effect-free plan.
/// Unknown liveness always preserves state; cleanup is emitted only for exact
/// IDs whose stale ownership has already been proved by the caller.
pub fn plan_recovery(case: RecoveryCase) -> RecoveryPlan {
    match case {
        RecoveryCase::SpawnFailedPreAck {
            retries_remaining: 1..,
        } => RecoveryPlan::RetryInterruptedAttempt,
        RecoveryCase::SpawnFailedPreAck {
            retries_remaining: 0,
        } => RecoveryPlan::BlockRuntimeUnavailable,
        RecoveryCase::PaneGonePostAck => RecoveryPlan::FailStartedWork,
        RecoveryCase::ReviewShellGoneWithoutReceipt => RecoveryPlan::ReturnToHumanGate,
        RecoveryCase::QueuedStale {
            effect_exists: false,
        } => RecoveryPlan::ReclaimQueuedRun,
        RecoveryCase::QueuedStale {
            effect_exists: true,
        } => RecoveryPlan::KeepPending,
        RecoveryCase::OutboxEffectAfterRestart { observation } => match observation {
            Observation::Present => RecoveryPlan::AcknowledgeObservedEffect,
            Observation::Gone => RecoveryPlan::ReissueSameEffect,
            Observation::Unknown => RecoveryPlan::KeepPending,
            Observation::Conflict => RecoveryPlan::BlockConflict,
        },
        RecoveryCase::ClosePending { observation } => match observation {
            Observation::Gone => RecoveryPlan::AcknowledgeObservedEffect,
            Observation::Present => RecoveryPlan::ReissueSameEffect,
            Observation::Unknown => RecoveryPlan::KeepPending,
            Observation::Conflict => RecoveryPlan::BlockConflict,
        },
        RecoveryCase::DigestConflict => RecoveryPlan::BlockConflict,
        RecoveryCase::DuplicateOwnedTabs {
            canonical_tab_id,
            stale_tab_ids,
            active_run_tab_id,
        } => {
            let active_conflicts = active_run_tab_id
                .as_deref()
                .is_some_and(|active| active.is_empty() || active != canonical_tab_id);
            let mut unique_stale = BTreeSet::new();
            let stale_conflicts = stale_tab_ids.is_empty()
                || stale_tab_ids.iter().any(|tab_id| {
                    tab_id.is_empty()
                        || tab_id == &canonical_tab_id
                        || active_run_tab_id.as_ref() == Some(tab_id)
                        || !unique_stale.insert(tab_id.as_str())
                });
            if canonical_tab_id.is_empty() || active_conflicts || stale_conflicts {
                RecoveryPlan::BlockConflict
            } else {
                RecoveryPlan::CloseExactTabs {
                    tab_ids: stale_tab_ids,
                }
            }
        }
        RecoveryCase::ActiveTopologyDrift | RecoveryCase::LockExpiredWithUnknownLiveness => {
            RecoveryPlan::BlockNewEffects
        }
    }
}
