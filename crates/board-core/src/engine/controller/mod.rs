//! Pure decisions for the deterministic controller.
//!
//! This module owns no persistence, clocks, Herdr clients, or daemon tasks. It
//! accepts already-observed facts and returns deterministic state/effect
//! intents for the daemon to commit and execute later through the outbox.

mod events;
mod recovery;
mod reducer;
mod review;
mod scheduling;
mod topology;

pub use events::{order_controller_events, ControllerEventEnvelope, EventOrderError};
pub use recovery::{plan_recovery, Observation, RecoveryCase, RecoveryPlan};
pub use reducer::{
    reduce_controller, AggregateRole, ControllerAggregate, ControllerEvent, ControllerReduction,
    EffectIntent, GateDecision, PendingGate, WorkState,
};
pub use review::{classify_review, ReceiptOutcome, ReviewClass, ReviewNote, ReviewNoteMarker};
pub use scheduling::{select_candidate, Candidate};
pub use topology::{decide_tab_policy, TabPolicy};
