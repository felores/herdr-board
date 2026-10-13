use std::collections::BTreeSet;

use rusqlite::{params, OptionalExtension};

use super::Db;
use crate::Result;

pub struct RuntimeLaunch<'a> {
    pub card_id: i64,
    pub run_id: i64,
    pub herdr_session: &'a str,
    pub workspace_id: &'a str,
    pub tab_id: &'a str,
    pub pane_id: &'a str,
    pub launch_cwd: Option<&'a str>,
    pub desired_tab_label: &'a str,
    pub stage: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCloseEffect {
    pub effect_id: String,
    pub card_id: i64,
    pub herdr_session: String,
    pub workspace_id: String,
    pub tab_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRenameEffect {
    pub effect_id: String,
    pub card_id: i64,
    pub herdr_session: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTopology {
    pub card_id: i64,
    pub herdr_session: String,
    pub workspace_id: String,
    pub canonical_tab_id: String,
    pub active_run_id: Option<i64>,
    pub active_run_pane_id: Option<String>,
    pub durable_pane_ids: Vec<String>,
}

impl Db {
    pub fn runtime_role_for_run(&self, run_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT json_extract(payload_json,'$.role') FROM controller_events
                 WHERE source='board' AND source_id=?1 ORDER BY ingest_sequence DESC LIMIT 1",
                [format!("run:{run_id}")],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub(super) fn record_runtime_enqueue_tx(
        tx: &rusqlite::Transaction<'_>,
        card_id: i64,
        run_id: i64,
        column_id: i64,
    ) -> Result<()> {
        let enabled: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM card_contracts WHERE card_id=?1)",
            [card_id],
            |row| row.get(0),
        )?;
        if !enabled {
            return Ok(());
        }
        let fresh: bool = tx.query_row(
            "SELECT fresh_session FROM columns WHERE id=?1",
            [column_id],
            |row| row.get(0),
        )?;
        let role = if fresh { "verification" } else { "executor" };
        let event_id = format!("run-enqueued:{run_id}");
        let payload = serde_json::json!({"run_id":run_id,"role":role}).to_string();
        tx.execute(
            "INSERT INTO controller_events(
               event_id,idempotency_key,source,source_id,card_id,observed_at,
               payload_digest,payload_json)
             VALUES(?1,?1,'board',?2,?3,datetime('now'),?4,?5)",
            params![event_id, format!("run:{run_id}"), card_id, role, payload],
        )?;
        Ok(())
    }

    pub fn runtime_label_description(&self, card_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT json_extract(contract_json,'$.runtime_label')
                 FROM card_contracts WHERE card_id=?1",
                [card_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Explicit lazy-migration boundary for a legacy card. Runtime behavior is
    /// never enabled from a label or title heuristic.
    pub fn activate_runtime_contract(&self, card_id: i64, description: &str) -> Result<()> {
        let contract = serde_json::json!({"runtime_label": description}).to_string();
        self.conn.execute(
            "INSERT INTO card_contracts(
               card_id,contract_version,revision,contract_json,outcome_code,projection_revision)
             VALUES(?1,'board-card/v1',1,?2,'runtime',1)
             ON CONFLICT(card_id) DO UPDATE SET
               contract_json=excluded.contract_json,revision=card_contracts.revision+1,
               projection_revision=card_contracts.projection_revision+1,updated_at=datetime('now')",
            params![card_id, contract],
        )?;
        Ok(())
    }

    pub fn record_runtime_launch(&self, launch: &RuntimeLaunch<'_>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO runtime_ownership(
               card_id,herdr_session,workspace_id,tab_id,canonical_pane_id,
               owned_child_pane_ids_json,launch_cwd,desired_tab_label,observed_tab_label,
               stage,active_run_id,topology_state,tab_policy,cleanup_state,revision)
             VALUES(?1,?2,?3,?4,?5,json_array(?5),?6,?7,NULL,?8,?9,'converged','required','none',1)
             ON CONFLICT(card_id) DO UPDATE SET
               herdr_session=excluded.herdr_session, workspace_id=excluded.workspace_id,
               tab_id=excluded.tab_id, canonical_pane_id=excluded.canonical_pane_id,
               owned_child_pane_ids_json=excluded.owned_child_pane_ids_json,
               launch_cwd=excluded.launch_cwd, desired_tab_label=excluded.desired_tab_label,
               observed_tab_label=excluded.observed_tab_label, stage=excluded.stage,
               active_run_id=excluded.active_run_id, topology_state='converged',
               tab_policy='required', cleanup_state='none', last_close_effect_id=NULL,
               last_close_ack_at=NULL, revision=runtime_ownership.revision+1,
               updated_at=datetime('now')",
            params![
                launch.card_id,
                launch.herdr_session,
                launch.workspace_id,
                launch.tab_id,
                launch.pane_id,
                launch.launch_cwd,
                launch.desired_tab_label,
                launch.stage,
                launch.run_id,
            ],
        )?;
        let effect_id = format!("tab-rename:{}:{}", launch.card_id, launch.run_id);
        let payload = serde_json::json!({
            "herdr_session": launch.herdr_session,
            "workspace_id": launch.workspace_id,
            "tab_id": launch.tab_id,
            "label": launch.desired_tab_label,
        })
        .to_string();
        tx.execute(
            "INSERT OR IGNORE INTO effect_outbox(
               effect_id,card_id,causation_event_id,effect_kind,idempotency_key,payload_json)
             VALUES(?1,?2,?3,'tab_rename',?1,?4)",
            params![
                effect_id,
                launch.card_id,
                format!("run:{}:launched", launch.run_id),
                payload
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn runtime_topologies(&self) -> Result<Vec<RuntimeTopology>> {
        let ownership = {
            let mut statement = self.conn.prepare(
                "SELECT card_id,herdr_session,workspace_id,tab_id,active_run_id
                 FROM runtime_ownership WHERE tab_id IS NOT NULL ORDER BY card_id",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        ownership
            .into_iter()
            .map(
                |(card_id, herdr_session, workspace_id, canonical_tab_id, active_run_id)| {
                    let runs = self.list_runs(card_id)?;
                    let active_run_pane_id = active_run_id
                        .and_then(|id| runs.iter().find(|run| run.id == id))
                        .and_then(|run| run.herdr_pane_id.clone());
                    let mut durable_pane_ids = BTreeSet::new();
                    for run in runs.iter().filter(|run| {
                        run.launch_spec.is_some()
                            && run.session.as_deref().unwrap_or("default") == herdr_session
                            && run.herdr_workspace_id.as_deref() == Some(workspace_id.as_str())
                    }) {
                        if let Some(pane_id) =
                            run.herdr_pane_id.as_deref().filter(|id| !id.is_empty())
                        {
                            durable_pane_ids.insert(pane_id.to_string());
                        }
                        if let Some(pane_id) = run
                            .herdr_anchor_pane_id
                            .as_deref()
                            .filter(|id| !id.is_empty())
                        {
                            durable_pane_ids.insert(pane_id.to_string());
                        }
                    }
                    Ok(RuntimeTopology {
                        card_id,
                        herdr_session,
                        workspace_id,
                        canonical_tab_id,
                        active_run_id,
                        active_run_pane_id,
                        durable_pane_ids: durable_pane_ids.into_iter().collect(),
                    })
                },
            )
            .collect()
    }

    pub fn plan_stale_runtime_tab_closes(
        &self,
        topology: &RuntimeTopology,
        active_run_tab_id: Option<&str>,
        stale_tab_ids: &[String],
    ) -> Result<()> {
        use crate::engine::{plan_recovery, RecoveryCase, RecoveryPlan};

        let plan = plan_recovery(RecoveryCase::DuplicateOwnedTabs {
            canonical_tab_id: topology.canonical_tab_id.clone(),
            stale_tab_ids: stale_tab_ids.to_vec(),
            active_run_tab_id: active_run_tab_id.map(str::to_string),
        });
        let RecoveryPlan::CloseExactTabs { tab_ids } = plan else {
            return Err(crate::Error::InvalidState(format!(
                "runtime tab cleanup for card {} is ambiguous",
                topology.card_id
            )));
        };
        let tx = self.conn.unchecked_transaction()?;
        for tab_id in tab_ids {
            let effect_id = format!("tab-close-stale:{}:{tab_id}", topology.card_id);
            let payload = serde_json::json!({
                "herdr_session": topology.herdr_session,
                "workspace_id": topology.workspace_id,
                "tab_id": tab_id,
            })
            .to_string();
            tx.execute(
                "INSERT OR IGNORE INTO effect_outbox(
                   effect_id,card_id,causation_event_id,effect_kind,idempotency_key,payload_json)
                 VALUES(?1,?2,?3,'tab_close',?1,?4)",
                params![
                    effect_id,
                    topology.card_id,
                    format!("startup-cleanup:card:{}", topology.card_id),
                    payload
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn plan_runtime_close_tx(
        tx: &rusqlite::Transaction<'_>,
        card_id: i64,
        run_id: i64,
        next_run_id: Option<i64>,
    ) -> Result<()> {
        let owned: Option<(String, String, String, String, i64)> = tx
            .query_row(
                "SELECT herdr_session,workspace_id,tab_id,stage,revision
                 FROM runtime_ownership
                 WHERE card_id=?1 AND active_run_id=?2 AND tab_id IS NOT NULL",
                params![card_id, run_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((session, workspace, tab_id, stage, revision)) = owned else {
            return Ok(());
        };
        if stage == "executing" {
            if let Some(next_run_id) = next_run_id {
                let next_role: Option<String> = tx
                    .query_row(
                        "SELECT json_extract(payload_json,'$.role') FROM controller_events
                         WHERE source_id=?1 ORDER BY ingest_sequence DESC LIMIT 1",
                        [format!("run:{next_run_id}")],
                        |row| row.get(0),
                    )
                    .optional()?;
                if next_role.as_deref() == Some("executor") {
                    return Ok(());
                }
            }
        }
        let effect_id = format!("tab-close:{card_id}:{run_id}:{revision}");
        let payload = serde_json::json!({
            "herdr_session": session,
            "workspace_id": workspace,
            "tab_id": tab_id,
        })
        .to_string();
        tx.execute(
            "INSERT OR IGNORE INTO effect_outbox(
               effect_id,card_id,causation_event_id,effect_kind,idempotency_key,payload_json)
             VALUES(?1,?2,?3,'tab_close',?1,?4)",
            params![
                effect_id,
                card_id,
                format!("run:{run_id}:terminal"),
                payload
            ],
        )?;
        tx.execute(
            "UPDATE runtime_ownership SET stage='done',active_run_id=NULL,
             tab_policy='close_pending',cleanup_state='pending',last_close_effect_id=?1,
             revision=revision+1,updated_at=datetime('now') WHERE card_id=?2",
            params![effect_id, card_id],
        )?;
        Ok(())
    }

    pub fn pending_runtime_closes(&self) -> Result<Vec<RuntimeCloseEffect>> {
        let mut stmt = self.conn.prepare(
            "SELECT effect_id,card_id,payload_json FROM effect_outbox
             WHERE effect_kind='tab_close' AND status IN ('pending','failed')
             ORDER BY effect_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let payload: String = row.get(2)?;
                let value: serde_json::Value = serde_json::from_str(&payload).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(RuntimeCloseEffect {
                    effect_id: row.get(0)?,
                    card_id: row.get(1)?,
                    herdr_session: value["herdr_session"].as_str().unwrap_or("default").into(),
                    workspace_id: value["workspace_id"].as_str().unwrap_or_default().into(),
                    tab_id: value["tab_id"].as_str().unwrap_or_default().into(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn pending_runtime_renames(&self) -> Result<Vec<RuntimeRenameEffect>> {
        let mut stmt = self.conn.prepare(
            "SELECT effect_id,card_id,payload_json FROM effect_outbox
             WHERE effect_kind='tab_rename' AND status IN ('pending','failed')
             ORDER BY effect_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let payload: String = row.get(2)?;
                let value: serde_json::Value = serde_json::from_str(&payload).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(RuntimeRenameEffect {
                    effect_id: row.get(0)?,
                    card_id: row.get(1)?,
                    herdr_session: value["herdr_session"].as_str().unwrap_or("default").into(),
                    workspace_id: value["workspace_id"].as_str().unwrap_or_default().into(),
                    tab_id: value["tab_id"].as_str().unwrap_or_default().into(),
                    label: value["label"].as_str().unwrap_or_default().into(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn acknowledge_runtime_rename(&self, effect: &RuntimeRenameEffect) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE effect_outbox SET status='acknowledged',attempt_count=attempt_count+1,
             last_attempt_at=datetime('now'),acknowledged_at=datetime('now'),
             acknowledgement_json=?2,last_error=NULL,updated_at=datetime('now')
             WHERE effect_id=?1",
            params![
                effect.effect_id,
                serde_json::json!({"label": effect.label}).to_string()
            ],
        )?;
        tx.execute(
            "UPDATE runtime_ownership SET observed_tab_label=?1,revision=revision+1,
             updated_at=datetime('now')
             WHERE card_id=?2 AND herdr_session=?3 AND workspace_id=?4 AND tab_id=?5",
            params![
                effect.label,
                effect.card_id,
                effect.herdr_session,
                effect.workspace_id,
                effect.tab_id
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn fail_runtime_effect(&self, effect_id: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE effect_outbox SET status='failed',attempt_count=attempt_count+1,
             last_attempt_at=datetime('now'),last_error=?2,updated_at=datetime('now')
             WHERE effect_id=?1",
            params![effect_id, error],
        )?;
        Ok(())
    }

    pub fn acknowledge_runtime_close(&self, effect_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE effect_outbox SET status='acknowledged',attempt_count=attempt_count+1,
             last_attempt_at=datetime('now'),acknowledged_at=datetime('now'),
             acknowledgement_json='{\"gone\":true}',last_error=NULL,updated_at=datetime('now')
             WHERE effect_id=?1",
            [effect_id],
        )?;
        tx.execute(
            "UPDATE runtime_ownership SET tab_id=NULL,canonical_pane_id=NULL,
             owned_child_pane_ids_json='[]',tab_policy='forbidden',cleanup_state='none',
             last_close_ack_at=datetime('now'),revision=revision+1,updated_at=datetime('now')
             WHERE last_close_effect_id=?1",
            [effect_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn fail_runtime_close(&self, effect_id: &str, error: &str) -> Result<()> {
        self.fail_runtime_effect(effect_id, error)?;
        self.conn.execute(
            "UPDATE runtime_ownership SET cleanup_state='failed',updated_at=datetime('now')
             WHERE last_close_effect_id=?1",
            [effect_id],
        )?;
        Ok(())
    }
}
