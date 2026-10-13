use std::collections::{BTreeMap, BTreeSet};

use super::ControllerEvent;

/// Durable event metadata used for deterministic replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerEventEnvelope {
    pub event_id: String,
    pub idempotency_key: String,
    pub payload_digest: String,
    pub source: String,
    pub source_sequence: Option<u64>,
    pub ingest_sequence: u64,
    pub causation_id: Option<String>,
    pub event: ControllerEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EventOrderError {
    #[error(
        "idempotency key '{idempotency_key}' has conflicting payload digests '{first_digest}' and '{conflicting_digest}'"
    )]
    DigestConflict {
        idempotency_key: String,
        first_digest: String,
        conflicting_digest: String,
    },
    #[error("duplicate event id '{event_id}'")]
    DuplicateEventId { event_id: String },
    #[error("causation cycle contains event '{event_id}'")]
    CausationCycle { event_id: String },
}

/// Deduplicate and order a replay batch.
///
/// Causation always wins. Among currently-ready events, source sequence orders
/// observations from the same source; ingest sequence is the stable fallback.
/// Duplicate idempotency keys with an identical digest collapse to the first
/// ingested event. A changed digest is a conflict, never an overwrite.
pub fn order_controller_events(
    events: &[ControllerEventEnvelope],
) -> Result<Vec<ControllerEventEnvelope>, EventOrderError> {
    let mut by_key: BTreeMap<&str, (&str, usize)> = BTreeMap::new();
    let mut unique = Vec::new();
    let mut event_ids = BTreeSet::new();

    let mut indexes: Vec<_> = (0..events.len()).collect();
    indexes.sort_by_key(|&index| events[index].ingest_sequence);
    for index in indexes {
        let event = &events[index];
        if let Some((digest, _)) = by_key.get(event.idempotency_key.as_str()) {
            if *digest != event.payload_digest {
                return Err(EventOrderError::DigestConflict {
                    idempotency_key: event.idempotency_key.clone(),
                    first_digest: (*digest).to_string(),
                    conflicting_digest: event.payload_digest.clone(),
                });
            }
            continue;
        }
        if !event_ids.insert(event.event_id.as_str()) {
            return Err(EventOrderError::DuplicateEventId {
                event_id: event.event_id.clone(),
            });
        }
        by_key.insert(
            event.idempotency_key.as_str(),
            (event.payload_digest.as_str(), index),
        );
        unique.push(event.clone());
    }

    let known_ids: BTreeSet<_> = unique.iter().map(|event| event.event_id.as_str()).collect();
    let mut emitted = BTreeSet::new();
    let mut ordered = Vec::with_capacity(unique.len());
    while ordered.len() < unique.len() {
        let ready: Vec<_> = unique
            .iter()
            .filter(|event| !emitted.contains(event.event_id.as_str()))
            .filter(|event| {
                event
                    .causation_id
                    .as_deref()
                    .is_none_or(|cause| !known_ids.contains(cause) || emitted.contains(cause))
            })
            .filter(|event| {
                !unique.iter().any(|other| {
                    !emitted.contains(other.event_id.as_str())
                        && other.source == event.source
                        && other.causation_id.as_deref().is_none_or(|cause| {
                            !known_ids.contains(cause) || emitted.contains(cause)
                        })
                        && matches!(
                            (other.source_sequence, event.source_sequence),
                            (Some(other_sequence), Some(event_sequence))
                                if other_sequence < event_sequence
                        )
                })
            })
            .collect();
        if ready.is_empty() {
            let event_id = unique
                .iter()
                .find(|event| !emitted.contains(event.event_id.as_str()))
                .expect("unordered event remains")
                .event_id
                .clone();
            return Err(EventOrderError::CausationCycle { event_id });
        }
        let next = ready
            .into_iter()
            .min_by_key(|event| event.ingest_sequence)
            .expect("ready event exists");
        emitted.insert(next.event_id.as_str());
        ordered.push(next.clone());
    }
    Ok(ordered)
}
