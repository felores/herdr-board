use super::AggregateRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabPolicy {
    Required,
    Inspectable,
    Forbidden,
}

/// Desired tab policy is derived from the aggregate role. Labels never enter
/// this decision because they are presentation, not ownership evidence.
pub fn decide_tab_policy(role: AggregateRole, keep_inspectable_tab: bool) -> TabPolicy {
    match role {
        AggregateRole::Queued | AggregateRole::Executing | AggregateRole::Verifying => {
            TabPolicy::Required
        }
        AggregateRole::HumanGate if keep_inspectable_tab => TabPolicy::Inspectable,
        AggregateRole::Intake
        | AggregateRole::Ready
        | AggregateRole::HumanGate
        | AggregateRole::Blocked
        | AggregateRole::Done => TabPolicy::Forbidden,
    }
}
