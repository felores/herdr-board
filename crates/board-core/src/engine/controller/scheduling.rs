/// Already-evaluated candidate facts. The daemon owns the evaluators and I/O;
/// this type makes the eligibility and ordering policy explicit and testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub card_id: i64,
    pub priority_rank: i64,
    pub deadline: Option<i64>,
    pub ready_at: i64,
    pub dependencies_satisfied: bool,
    pub blockers_resolved: bool,
    pub gate_resolved: bool,
    pub no_open_run: bool,
    pub workspace_converged: bool,
    pub locks_available: bool,
    pub adapter_available: bool,
}

impl Candidate {
    pub fn eligible(
        card_id: i64,
        priority_rank: i64,
        deadline: Option<i64>,
        ready_at: i64,
    ) -> Self {
        Self {
            card_id,
            priority_rank,
            deadline,
            ready_at,
            dependencies_satisfied: true,
            blockers_resolved: true,
            gate_resolved: true,
            no_open_run: true,
            workspace_converged: true,
            locks_available: true,
            adapter_available: true,
        }
    }

    fn is_eligible(&self) -> bool {
        self.dependencies_satisfied
            && self.blockers_resolved
            && self.gate_resolved
            && self.no_open_run
            && self.workspace_converged
            && self.locks_available
            && self.adapter_available
    }
}

/// Select the next card without mutating the supplied set.
pub fn select_candidate(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates
        .iter()
        .filter(|candidate| candidate.is_eligible())
        .min_by_key(|candidate| {
            (
                candidate.priority_rank,
                candidate.deadline.is_none(),
                candidate.deadline.unwrap_or(i64::MAX),
                candidate.ready_at,
                candidate.card_id,
            )
        })
}
