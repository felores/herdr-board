//! Conservative, one-pass restart reconciliation.
//!
//! Resolution and runtime snapshot I/O are injectable and complete before any
//! scheduler/store mutation. Uncertain observations are never treated as pane
//! exit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::spawner::RuntimeHandle;
use board_core::engine::AgentSignal;
use board_core::model::{Card, Run};
use board_core::protocol::RunOutcome;
use board_herdr::AgentStatus;

use crate::dispatch;
use crate::session::SessionRegistry;
use crate::state::{ActiveRun, Daemon};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    Default(PathBuf),
    Resolved(PathBuf),
    Unresolved,
}

impl SessionTarget {
    fn socket(&self) -> Option<&Path> {
        match self {
            Self::Default(path) | Self::Resolved(path) => Some(path),
            Self::Unresolved => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    Deadline,
    Malformed,
    Transport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    panes: Vec<RuntimePane>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimePane {
    pane_id: String,
    workspace_id: String,
    tab_id: String,
    status: AgentStatus,
}

impl RuntimeSnapshot {
    #[cfg(test)]
    fn new(panes: impl IntoIterator<Item = (impl Into<String>, AgentStatus)>) -> Self {
        Self {
            panes: panes
                .into_iter()
                .map(|(id, status)| RuntimePane {
                    pane_id: id.into(),
                    workspace_id: String::new(),
                    tab_id: String::new(),
                    status,
                })
                .collect(),
        }
    }

    fn observe(&self, pane_id: &str) -> RuntimeProbe {
        self.panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .map(|pane| RuntimeProbe::Alive(pane.status))
            .unwrap_or(RuntimeProbe::Gone)
    }

    fn tab_for_pane(&self, pane_id: &str, workspace_id: &str) -> Option<&str> {
        self.panes
            .iter()
            .find(|pane| pane.pane_id == pane_id && pane.workspace_id == workspace_id)
            .map(|pane| pane.tab_id.as_str())
            .filter(|tab_id| !tab_id.is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeProbe {
    Alive(AgentStatus),
    Gone,
    Unknown,
}

pub trait SessionResolver: Send + Sync {
    fn resolve_target(&self, session: Option<&str>) -> SessionTarget;
}

pub trait Runtime: Send + Sync {
    fn snapshot(&self, target: &SessionTarget) -> Result<RuntimeSnapshot, ProbeFailure>;
}

pub trait ReconcileClock: Send + Sync {
    fn now(&self) -> Instant;
    fn wall_now_ms(&self) -> i64;
}

#[derive(Default)]
pub struct SystemClock;

impl ReconcileClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wall_now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
}

impl SessionResolver for SessionRegistry {
    fn resolve_target(&self, session: Option<&str>) -> SessionTarget {
        match session {
            None => SessionTarget::Default(self.default_socket().to_path_buf()),
            Some(_) => self
                .resolve(session)
                .map(|resolved| SessionTarget::Resolved(resolved.socket))
                .unwrap_or(SessionTarget::Unresolved),
        }
    }
}

#[derive(Default)]
pub struct HerdrRuntime;

impl Runtime for HerdrRuntime {
    fn snapshot(&self, target: &SessionTarget) -> Result<RuntimeSnapshot, ProbeFailure> {
        let socket = target.socket().ok_or(ProbeFailure::Transport)?;
        // Only a valid snapshot omitting a pane classifies as `Gone`, so an
        // incompatible socket must fail the gate rather than produce one.
        let mut client =
            crate::herdr_conn::connect_checked(socket).map_err(classify_herdr_error)?;
        let snapshot = client.session_snapshot().map_err(classify_herdr_error)?;
        let statuses = crate::herdr_snapshot::snapshot_pane_statuses(snapshot.clone());
        let panes = snapshot
            .panes
            .into_iter()
            .map(|pane| RuntimePane {
                status: statuses
                    .get(&pane.pane_id)
                    .copied()
                    .unwrap_or(pane.agent_status),
                pane_id: pane.pane_id,
                workspace_id: pane.workspace_id,
                tab_id: pane.tab_id,
            })
            .collect();
        Ok(RuntimeSnapshot { panes })
    }
}

fn classify_herdr_error(error: board_herdr::HerdrError) -> ProbeFailure {
    match error {
        board_herdr::HerdrError::Deadline { .. } => ProbeFailure::Deadline,
        board_herdr::HerdrError::Decode(_) => ProbeFailure::Malformed,
        _ => ProbeFailure::Transport,
    }
}

/// Reconcile the durable open-run set once. Worker failure, deadline, malformed
/// reply, unresolved session, and missing persisted pane identity all classify
/// as `Unknown`; only a valid snapshot omitting the exact pane is `Gone`.
pub async fn reconcile_once(
    d: &Arc<Daemon>,
    resolver: Arc<dyn SessionResolver>,
    runtime: Arc<dyn Runtime>,
    clock: Arc<dyn ReconcileClock>,
) {
    dispatch::process_runtime_effects(d);
    reconcile_duplicate_tabs(d, resolver.clone(), runtime.clone()).await;
    dispatch::process_runtime_effects(d);
    let active = match d.store.active_runs() {
        Ok(active) => active,
        Err(_) => {
            tracing::warn!(
                error_category = "database",
                "reconciliation: active_runs failed"
            );
            return;
        }
    };
    for (run, card) in active {
        let target = resolver.resolve_target(run.session.as_deref());
        let pane_id = run.herdr_pane_id.clone();
        let probe = match (&target, pane_id.as_deref()) {
            (SessionTarget::Unresolved, _) | (_, None) => RuntimeProbe::Unknown,
            (_, Some(pane_id)) => {
                let runtime = runtime.clone();
                let worker_target = target.clone();
                let probed_pane = pane_id.to_string();
                let probe = tokio::task::spawn_blocking(move || {
                    runtime
                        .snapshot(&worker_target)
                        .map(|snapshot| snapshot.observe(&probed_pane))
                })
                .await;
                // Both collapses stay conservative (`Unknown` never finalizes a
                // run), but `classify_herdr_error` already computed the precise
                // reason — losing it unlogged made "remains unknown" unanswerable.
                match probe {
                    Ok(Ok(probe)) => probe,
                    Ok(Err(failure)) => {
                        tracing::warn!(
                            run_id = run.id,
                            failure = ?failure,
                            "reconciliation: runtime probe failed; treating the run as unknown"
                        );
                        RuntimeProbe::Unknown
                    }
                    Err(_) => {
                        tracing::warn!(
                            run_id = run.id,
                            error_category = "task",
                            "reconciliation: runtime probe task failed"
                        );
                        RuntimeProbe::Unknown
                    }
                }
            }
        };
        apply_observation(d, run, card, target, probe, clock.as_ref());
    }
}

async fn reconcile_duplicate_tabs(
    d: &Arc<Daemon>,
    resolver: Arc<dyn SessionResolver>,
    runtime: Arc<dyn Runtime>,
) {
    let topologies = match d.store.lock().runtime_topologies() {
        Ok(topologies) => topologies,
        Err(error) => {
            tracing::warn!(error = %error, "reconciliation: runtime topology load failed");
            return;
        }
    };
    for topology in topologies {
        let target = resolver.resolve_target(
            (topology.herdr_session != "default").then_some(topology.herdr_session.as_str()),
        );
        if matches!(target, SessionTarget::Unresolved) {
            continue;
        }
        let worker_target = target.clone();
        let runtime = runtime.clone();
        let snapshot =
            match tokio::task::spawn_blocking(move || runtime.snapshot(&worker_target)).await {
                Ok(Ok(snapshot)) => snapshot,
                _ => continue,
            };
        if !snapshot.panes.iter().any(|pane| {
            pane.workspace_id == topology.workspace_id && pane.tab_id == topology.canonical_tab_id
        }) {
            continue;
        }
        let active_run_tab_id = match topology.active_run_pane_id.as_deref() {
            Some(pane_id) => match snapshot.tab_for_pane(pane_id, &topology.workspace_id) {
                Some(tab_id) => Some(tab_id.to_string()),
                None => continue,
            },
            None if topology.active_run_id.is_some() => continue,
            None => None,
        };
        let known_panes = topology
            .durable_pane_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let candidate_tabs = snapshot
            .panes
            .iter()
            .filter(|pane| {
                pane.workspace_id == topology.workspace_id
                    && pane.tab_id != topology.canonical_tab_id
                    && known_panes.contains(pane.pane_id.as_str())
            })
            .map(|pane| pane.tab_id.clone())
            .collect::<BTreeSet<_>>();
        let stale_tab_ids = candidate_tabs
            .into_iter()
            .filter(|tab_id| {
                let panes = snapshot
                    .panes
                    .iter()
                    .filter(|pane| {
                        pane.workspace_id == topology.workspace_id && pane.tab_id == *tab_id
                    })
                    .collect::<Vec<_>>();
                !panes.is_empty()
                    && panes.iter().all(|pane| {
                        known_panes.contains(pane.pane_id.as_str())
                            && !matches!(pane.status, AgentStatus::Working | AgentStatus::Blocked)
                    })
            })
            .collect::<Vec<_>>();
        if stale_tab_ids.is_empty() {
            continue;
        }
        if let Err(error) = d.store.lock().plan_stale_runtime_tab_closes(
            &topology,
            active_run_tab_id.as_deref(),
            &stale_tab_ids,
        ) {
            tracing::warn!(card_id = topology.card_id, error = %error, "reconciliation: duplicate tab cleanup blocked");
        }
    }
}

fn same_open_run(d: &Arc<Daemon>, observed: &Run, card_id: i64) -> Option<Run> {
    let db = d.store.lock();
    let current = db.get_run(observed.id).ok()?;
    let card = db.get_card(card_id).ok()??;
    (current.started_at.is_some()
        && current.ended_at.is_none()
        && current.card_id == observed.card_id
        && card.id == current.card_id
        && current.session == observed.session
        && current.herdr_pane_id == observed.herdr_pane_id)
        .then_some(current)
}

fn apply_observation(
    d: &Arc<Daemon>,
    run: Run,
    card: Card,
    target: SessionTarget,
    probe: RuntimeProbe,
    clock: &dyn ReconcileClock,
) {
    match probe {
        RuntimeProbe::Unknown => {
            tracing::warn!("reconciliation: run {} remains unknown", run.id);
        }
        RuntimeProbe::Gone => {
            // Revalidate the exact runtime identity after external I/O. The
            // finalizer performs its own atomic open-run check as well.
            if same_open_run(d, &run, card.id).is_none() {
                return;
            }
            let message = "daemon restart: pane exited".to_string();
            if let Err(_error) = dispatch::finalize_run(
                d,
                run.id,
                RunOutcome::Fail,
                Some(message.clone()),
                Some(message),
                false,
                false,
            ) {
                tracing::error!(
                    run_id = run.id,
                    card_id = card.id,
                    error_category = "database",
                    "reconciliation could not finalize a gone run; it stays open"
                );
            }
        }
        RuntimeProbe::Alive(status) => adopt_alive(d, run, card, target, status, clock),
    }
}

fn adopt_alive(
    d: &Arc<Daemon>,
    run: Run,
    card: Card,
    target: SessionTarget,
    status: AgentStatus,
    clock: &dyn ReconcileClock,
) {
    let Some(current_run) = same_open_run(d, &run, card.id) else {
        return;
    };
    let adopted_at = clock.now();
    let wall_now_ms = clock.wall_now_ms();
    let deadline = ActiveRun::reconstruct_deadline(
        adopted_at,
        wall_now_ms,
        current_run.timeout_deadline_at_ms,
    );
    let socket = match target {
        SessionTarget::Default(_) => None,
        SessionTarget::Resolved(socket) => Some(socket),
        SessionTarget::Unresolved => return,
    };
    let handle = RuntimeHandle {
        pane_id: current_run.herdr_pane_id.clone(),
        workspace_id: current_run.herdr_workspace_id.clone(),
        tab_id: None,
        anchor_pane_id: current_run.herdr_anchor_pane_id.clone(),
        pid: None,
        herdr_socket: socket,
        captured_session_id: current_run.session_id.clone(),
    };
    {
        let mut sched = d.sched.lock().unwrap();
        if sched.active.contains_key(&run.id) {
            return;
        }
        sched.active.insert(
            run.id,
            ActiveRun {
                card_id: card.id,
                handle,
                started: adopted_at,
                timeout_deadline: deadline,
                idle_since: None,
                awaiting_since: ActiveRun::reconstruct_awaiting_since(
                    adopted_at,
                    wall_now_ms,
                    current_run.timeout_paused_at_ms,
                ),
                is_local: false,
                pane_id: current_run.herdr_pane_id.clone(),
            },
        );
    }
    d.refresh_watch();
    let signal = match status {
        AgentStatus::Done => Some(AgentSignal::Done),
        AgentStatus::Blocked => Some(AgentSignal::Blocked),
        AgentStatus::Working => Some(AgentSignal::Working),
        _ => None,
    };
    if let Some(signal) = signal {
        crate::watchers::apply_signal(d, run.id, card.id, signal);
    }
}

#[cfg(test)]
mod tests;
