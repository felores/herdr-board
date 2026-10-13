use super::ReviewClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AggregateRole {
    #[default]
    Intake,
    Ready,
    Queued,
    Executing,
    Verifying,
    HumanGate,
    Blocked,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkState {
    #[default]
    NotStarted,
    Started,
    Interrupted,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingGate {
    pub request_id: String,
    pub artifact_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControllerAggregate {
    pub role: AggregateRole,
    pub work_state: WorkState,
    pub open_run_id: Option<i64>,
    pub prompt_acknowledged: bool,
    pub retry_budget: u32,
    pub pending_gate: Option<PendingGate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Approve,
    RequestChanges,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerEvent {
    EvidenceObserved,
    RunEnqueued {
        run_id: i64,
        verifying: bool,
    },
    PromptAcknowledged {
        run_id: i64,
    },
    SpawnFailedPreAck {
        run_id: i64,
    },
    PaneExitedPostAck {
        run_id: i64,
    },
    RunDoneOk {
        run_id: i64,
    },
    RunDoneFail {
        run_id: i64,
    },
    GateOpened(PendingGate),
    GateDecided {
        request_id: String,
        artifact_digest: String,
        decision: GateDecision,
    },
    ReviewClassified(ReviewClass),
    DigestConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectIntent {
    RetrySpawn { run_id: i64 },
    EvaluateCandidates,
    CreateFollowUps { note_indexes: Vec<usize> },
    CreateRevision { note_indexes: Vec<usize> },
    PresentFeedbackClassification { note_indexes: Vec<usize> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerReduction {
    pub aggregate: ControllerAggregate,
    pub effects: Vec<EffectIntent>,
    pub diagnostics: Vec<&'static str>,
}

/// Reduce one accepted event. Stale run IDs and gate decisions are retained as
/// diagnostics and never mutate the aggregate.
pub fn reduce_controller(
    current: &ControllerAggregate,
    event: &ControllerEvent,
) -> ControllerReduction {
    let mut aggregate = current.clone();
    let mut effects = Vec::new();
    let mut diagnostics = Vec::new();

    match event {
        ControllerEvent::EvidenceObserved => {}
        ControllerEvent::RunEnqueued { run_id, verifying } => {
            if current.open_run_id.is_some() {
                diagnostics.push("run_already_open");
            } else {
                aggregate.role = if *verifying {
                    AggregateRole::Verifying
                } else {
                    AggregateRole::Queued
                };
                aggregate.work_state = WorkState::NotStarted;
                aggregate.open_run_id = Some(*run_id);
                aggregate.prompt_acknowledged = false;
            }
        }
        ControllerEvent::PromptAcknowledged { run_id } => {
            if current.open_run_id == Some(*run_id) {
                aggregate.role = match current.role {
                    AggregateRole::Verifying => AggregateRole::Verifying,
                    _ => AggregateRole::Executing,
                };
                aggregate.work_state = WorkState::Started;
                aggregate.prompt_acknowledged = true;
            } else {
                diagnostics.push("stale_run_event");
            }
        }
        ControllerEvent::SpawnFailedPreAck { run_id } => {
            if current.open_run_id != Some(*run_id) || current.prompt_acknowledged {
                diagnostics.push("stale_run_event");
            } else {
                aggregate.work_state = WorkState::Interrupted;
                aggregate.open_run_id = None;
                if current.retry_budget > 0 {
                    aggregate.role = AggregateRole::Ready;
                    aggregate.retry_budget -= 1;
                    effects.push(EffectIntent::RetrySpawn { run_id: *run_id });
                } else {
                    aggregate.role = AggregateRole::Blocked;
                    diagnostics.push("runtime_unavailable");
                }
            }
        }
        ControllerEvent::PaneExitedPostAck { run_id } => {
            if current.open_run_id == Some(*run_id) && current.prompt_acknowledged {
                aggregate.role = AggregateRole::Blocked;
                aggregate.work_state = WorkState::Failed;
                aggregate.open_run_id = None;
                aggregate.prompt_acknowledged = false;
            } else {
                diagnostics.push("stale_run_event");
            }
        }
        ControllerEvent::RunDoneOk { run_id } => {
            if current.open_run_id == Some(*run_id) {
                aggregate.role = AggregateRole::Ready;
                aggregate.work_state = WorkState::Succeeded;
                aggregate.open_run_id = None;
                aggregate.prompt_acknowledged = false;
                effects.push(EffectIntent::EvaluateCandidates);
            } else {
                diagnostics.push("stale_run_event");
            }
        }
        ControllerEvent::RunDoneFail { run_id } => {
            if current.open_run_id == Some(*run_id) {
                aggregate.role = AggregateRole::Blocked;
                aggregate.work_state = WorkState::Failed;
                aggregate.open_run_id = None;
                aggregate.prompt_acknowledged = false;
            } else {
                diagnostics.push("stale_run_event");
            }
        }
        ControllerEvent::GateOpened(gate) => {
            aggregate.role = AggregateRole::HumanGate;
            aggregate.pending_gate = Some(gate.clone());
        }
        ControllerEvent::GateDecided {
            request_id,
            artifact_digest,
            decision,
        } => match &current.pending_gate {
            Some(gate)
                if gate.request_id == *request_id && gate.artifact_digest == *artifact_digest =>
            {
                aggregate.pending_gate = None;
                match decision {
                    GateDecision::Approve => {
                        aggregate.role = AggregateRole::Ready;
                        effects.push(EffectIntent::EvaluateCandidates);
                    }
                    GateDecision::RequestChanges => {
                        aggregate.role = AggregateRole::Ready;
                        effects.push(EffectIntent::CreateRevision {
                            note_indexes: Vec::new(),
                        });
                    }
                    GateDecision::Block => aggregate.role = AggregateRole::Blocked,
                }
            }
            _ => diagnostics.push("gate_decision_mismatch"),
        },
        ControllerEvent::ReviewClassified(class) => match class {
            ReviewClass::ApproveOnly => effects.push(EffectIntent::EvaluateCandidates),
            ReviewClass::ApproveWithFollowUp { note_indexes } => {
                effects.push(EffectIntent::CreateFollowUps {
                    note_indexes: note_indexes.clone(),
                });
                effects.push(EffectIntent::EvaluateCandidates);
            }
            ReviewClass::RevisionRequired { note_indexes } => {
                aggregate.role = AggregateRole::Ready;
                effects.push(EffectIntent::CreateRevision {
                    note_indexes: note_indexes.clone(),
                });
            }
            ReviewClass::Reject => aggregate.role = AggregateRole::Blocked,
            ReviewClass::NeedsHumanClassification { note_indexes } => {
                aggregate.role = AggregateRole::HumanGate;
                effects.push(EffectIntent::PresentFeedbackClassification {
                    note_indexes: note_indexes.clone(),
                });
            }
            ReviewClass::DigestConflict => {
                aggregate.role = AggregateRole::Blocked;
                diagnostics.push("digest_conflict");
            }
        },
        ControllerEvent::DigestConflict => {
            aggregate.role = AggregateRole::Blocked;
            diagnostics.push("digest_conflict");
        }
    }

    ControllerReduction {
        aggregate,
        effects,
        diagnostics,
    }
}
