//! Run lifecycle: enqueue, promote (spawn), and finalize (done / fail / timeout
//! / lost / cancel), plus the transition + auto-chain logic. All effects the
//! pure engine only *decides* are executed here.

mod enqueue;
mod finalize;
mod launch_plan;
mod ownership;
mod pass;
mod space;
#[cfg(test)]
mod tests;

pub(crate) use enqueue::{antigravity_validation_config, enqueue_run, prepare_enqueue_values};
pub(crate) use finalize::{finalize_run, finalize_run_timeout};
pub(crate) use launch_plan::board_env;
pub(crate) use ownership::{owned_pane_ids, reconstruct_owned_tab_id, OwnedPanes};
pub(crate) use pass::dispatch_pass;
pub(crate) use space::{resolve_space, validate_space_resolvable, workspace_cwd};

use board_core::db::EnqueueRun;
use board_core::harness::HarnessError;
use board_core::Error;

pub(crate) struct PreparedEnqueue {
    pub(crate) card_id: i64,
    pub(crate) column_id: i64,
    pub(crate) harness: String,
    pub(crate) argv_json: String,
    pub(crate) prompt: String,
    pub(crate) system_prompt: String,
    pub(crate) launch_spec_json: String,
    pub(crate) session_id: Option<String>,
    pub(crate) session: Option<String>,
}

impl PreparedEnqueue {
    pub(crate) fn borrowed(&self) -> EnqueueRun<'_> {
        EnqueueRun {
            card_id: self.card_id,
            column_id: self.column_id,
            harness: &self.harness,
            argv_json: &self.argv_json,
            prompt_snapshot: &self.prompt,
            system_prompt_snapshot: Some(&self.system_prompt),
            launch_spec_json: Some(&self.launch_spec_json),
            session_id: self.session_id.as_deref(),
            session: self.session.as_deref(),
        }
    }
}

pub(crate) fn map_harness_err(e: HarnessError) -> Error {
    match e {
        HarnessError::UnknownHarness(h) => Error::BadRequest(format!("unknown harness: {h}")),
        HarnessError::MissingMintedSession => {
            Error::BadRequest("mint session requested without a uuid".into())
        }
        HarnessError::MissingForkTargetSession => {
            Error::BadRequest("Pi fork requested without a new session uuid".into())
        }
        HarnessError::PiPermissionModeUnsupported => {
            Error::BadRequest("pi does not support permission modes".into())
        }
        // OpenCode effort rides the herdr-board agent's per-model variant, so a
        // model is required up front: failing loudly beats dropping the effort.
        HarnessError::OpenCodeEffortRequiresModel => Error::BadRequest(
            "opencode effort requires a model: effort rides the herdr-board agent's \
             per-model variant, so set a model on the card or column"
                .into(),
        ),
        // Both are refusals about an existing run, not malformed requests: the
        // run exists and is valid, it just cannot be reopened. Code 3.
        HarnessError::ResumeUnsupported(harness) => Error::InvalidState(format!(
            "harness '{harness}' cannot resume a recorded conversation, so this run's \
             closed pane cannot be reopened; retry the card instead to start a new run"
        )),
        // A legacy all-in-one command line cannot be re-threaded onto a resume
        // without re-sending the task, so refuse rather than corrupt the argv.
        HarnessError::ResumeLegacyArgv(harness) => Error::InvalidState(format!(
            "harness '{harness}' recorded a legacy all-in-one command line that embeds the \
             task text, so this run cannot be reopened without re-running it; retry the card \
             to start a new run instead"
        )),
        HarnessError::MissingResumeSession => Error::InvalidState(
            "this run recorded no harness conversation id, so there is nothing to resume; \
             retry the card instead to start a new run"
                .into(),
        ),
    }
}

fn effect_socket(
    d: &std::sync::Arc<crate::state::Daemon>,
    herdr_session: &str,
) -> Option<std::path::PathBuf> {
    if herdr_session == "default" {
        None
    } else {
        d.session_registry.as_ref().and_then(|registry| {
            registry
                .resolve(Some(herdr_session))
                .ok()
                .map(|resolved| resolved.socket)
        })
    }
}

/// Execute durable exact-tab rename and close effects. Failed/uncertain
/// effects remain in the outbox for the next supervisor pass; close Gone is
/// an idempotent success.
pub(crate) fn process_runtime_effects(d: &std::sync::Arc<crate::state::Daemon>) {
    let renames = match d.store.lock().pending_runtime_renames() {
        Ok(effects) => effects,
        Err(error) => {
            tracing::warn!(error = %error, "loading runtime rename effects failed");
            return;
        }
    };
    for effect in renames {
        let socket = effect_socket(d, &effect.herdr_session);
        if effect.herdr_session != "default" && socket.is_none() {
            let _ = d.store.lock().fail_runtime_effect(
                &effect.effect_id,
                "named Herdr session is unavailable during rename reconciliation",
            );
            continue;
        }
        match d.spawner.rename_card_tab(
            socket.as_deref(),
            &effect.workspace_id,
            &effect.tab_id,
            &effect.label,
        ) {
            Ok(()) => {
                if let Err(error) = d.store.lock().acknowledge_runtime_rename(&effect) {
                    tracing::warn!(effect_id = %effect.effect_id, error = %error, "acknowledging tab rename failed");
                }
            }
            Err(error) => {
                let _ = d
                    .store
                    .lock()
                    .fail_runtime_effect(&effect.effect_id, &format!("{error:#}"));
            }
        }
    }

    let effects = match d.store.lock().pending_runtime_closes() {
        Ok(effects) => effects,
        Err(error) => {
            tracing::warn!(error = %error, "loading runtime close effects failed");
            return;
        }
    };
    for effect in effects {
        let socket = effect_socket(d, &effect.herdr_session);
        if effect.herdr_session != "default" && socket.is_none() {
            let _ = d.store.lock().fail_runtime_close(
                &effect.effect_id,
                "named Herdr session is unavailable during close reconciliation",
            );
            continue;
        }
        match d
            .spawner
            .close_card_tab(socket.as_deref(), &effect.workspace_id, &effect.tab_id)
        {
            Ok(()) => {
                if let Err(error) = d.store.lock().acknowledge_runtime_close(&effect.effect_id) {
                    tracing::warn!(effect_id = %effect.effect_id, error = %error, "acknowledging tab close failed");
                }
            }
            Err(error) => {
                let _ = d
                    .store
                    .lock()
                    .fail_runtime_close(&effect.effect_id, &format!("{error:#}"));
            }
        }
    }
}
