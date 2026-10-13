/// Outcome written by the review authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptOutcome {
    Approved,
    ChangesRequested,
    Rejected,
}

/// Structured meaning attached to one review note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewNoteMarker {
    Informational,
    FollowUp,
    RevisionRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewNote {
    pub text: String,
    pub marker: Option<ReviewNoteMarker>,
}

/// Deterministic four-way review classification plus the two fail-closed
/// states required before a class can be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewClass {
    ApproveOnly,
    ApproveWithFollowUp { note_indexes: Vec<usize> },
    RevisionRequired { note_indexes: Vec<usize> },
    Reject,
    NeedsHumanClassification { note_indexes: Vec<usize> },
    DigestConflict,
}

pub fn classify_review(
    outcome: ReceiptOutcome,
    artifact_digest_matches: bool,
    notes: &[ReviewNote],
) -> ReviewClass {
    if !artifact_digest_matches {
        return ReviewClass::DigestConflict;
    }
    if outcome == ReceiptOutcome::Rejected {
        return ReviewClass::Reject;
    }

    let indexes = |marker| {
        notes
            .iter()
            .enumerate()
            .filter_map(|(index, note)| (note.marker == marker).then_some(index))
            .collect::<Vec<_>>()
    };
    let revision = indexes(Some(ReviewNoteMarker::RevisionRequired));
    if outcome == ReceiptOutcome::ChangesRequested {
        return ReviewClass::RevisionRequired {
            note_indexes: (0..notes.len()).collect(),
        };
    }
    if !revision.is_empty() {
        return ReviewClass::RevisionRequired {
            note_indexes: revision,
        };
    }
    let uncertain = indexes(None);
    if !uncertain.is_empty() {
        return ReviewClass::NeedsHumanClassification {
            note_indexes: uncertain,
        };
    }
    let follow_up = indexes(Some(ReviewNoteMarker::FollowUp));
    if !follow_up.is_empty() {
        return ReviewClass::ApproveWithFollowUp {
            note_indexes: follow_up,
        };
    }
    ReviewClass::ApproveOnly
}
