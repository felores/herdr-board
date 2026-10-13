use rusqlite::{params, OptionalExtension};

use super::rows;
use super::Db;
use crate::model::{Card, IssueAuditEntry, IssueIdentity};
use crate::protocol::CardStatus;
use crate::{Error, Result};

pub(super) fn validate_issue(issue: &IssueIdentity) -> Result<()> {
    let valid_id =
        |id: &str| !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_graphic());
    if !valid_id(&issue.node_id) || !valid_id(&issue.repository_id) || issue.number < 1 {
        return Err(Error::BadRequest("invalid GitHub issue identity".into()));
    }
    let Some((owner, repo)) = issue.repository.split_once('/') else {
        return Err(Error::BadRequest("repository must be OWNER/REPO".into()));
    };
    let valid_segment = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c))
    };
    if !valid_segment(owner)
        || !valid_segment(repo)
        || issue.url
            != format!(
                "https://github.com/{}/issues/{}",
                issue.repository, issue.number
            )
    {
        return Err(Error::BadRequest(
            "invalid canonical GitHub issue URL".into(),
        ));
    }
    Ok(())
}

impl Db {
    pub(super) fn require_issue_transfer_project(
        &self,
        card: &Card,
        target_board: i64,
    ) -> Result<()> {
        if card.source_issue.is_some()
            && self.get_board(card.board_id)?.project_id != self.get_board(target_board)?.project_id
        {
            return Err(Error::InvalidState(
                "unlink the Issue before moving the card to another repository project".into(),
            ));
        }
        Ok(())
    }

    fn require_mutable_issue_card(&self, id: i64) -> Result<Card> {
        let card = self.require_card(id)?;
        if !matches!(
            card.status,
            CardStatus::Idle | CardStatus::Done | CardStatus::Failed
        ) || self.open_run_for_card(id)?.is_some()
        {
            return Err(Error::InvalidState(
                "cannot change an Issue binding while the card has open work".into(),
            ));
        }
        Ok(card)
    }

    fn write_issue(&self, id: i64, issue: &IssueIdentity) -> Result<Card> {
        validate_issue(issue)?;
        let card = self.require_mutable_issue_card(id)?;
        if self.get_board(card.board_id)?.scope_path.is_none() {
            return Err(Error::BadRequest(
                "issue card requires a repository-scoped board".into(),
            ));
        }
        self.require_issue_available(&issue.node_id, Some(id))?;
        self.conn.execute(
            "UPDATE cards SET issue_node_id=?2, issue_repository_id=?3,
                issue_repository=?4, issue_number=?5, issue_url=?6,
                issue_observed_at=datetime('now'), updated_at=datetime('now') WHERE id=?1",
            params![
                id,
                issue.node_id,
                issue.repository_id,
                issue.repository,
                issue.number,
                issue.url
            ],
        )?;
        self.require_card(id)
    }

    /// Link a legacy unbound card. A different immutable ID needs an explicit
    /// unlink first; identical replay returns the unchanged card.
    pub fn bind_card_issue(&self, id: i64, issue: &IssueIdentity) -> Result<Card> {
        let card = self.require_mutable_issue_card(id)?;
        if let Some(existing) = card.source_issue {
            if existing.identity == *issue {
                return self.require_card(id);
            }
            return Err(Error::InvalidState(
                "unlink the existing Issue before rebinding".into(),
            ));
        }
        self.write_issue(id, issue)
    }

    /// Refresh display coordinates only while the immutable Issue ID is the
    /// same. An inaccessible lookup must not call this method.
    pub fn refresh_card_issue(&self, id: i64, issue: &IssueIdentity) -> Result<Card> {
        let card = self.require_mutable_issue_card(id)?;
        if card
            .source_issue
            .as_ref()
            .map(|source| source.identity.node_id.as_str())
            != Some(issue.node_id.as_str())
        {
            return Err(Error::InvalidState(
                "Issue node ID changed; explicit unlink is required".into(),
            ));
        }
        self.write_issue(id, issue)
    }

    pub fn unlink_card_issue(&self, id: i64) -> Result<Card> {
        let card = self.require_mutable_issue_card(id)?;
        if card.source_issue.is_none() {
            return Ok(card);
        }
        self.conn.execute(
            "UPDATE cards SET issue_node_id=NULL, issue_repository_id=NULL,
                issue_repository=NULL, issue_number=NULL, issue_url=NULL,
                issue_observed_at=NULL, updated_at=datetime('now') WHERE id=?1",
            params![id],
        )?;
        self.require_card(id)
    }

    pub fn card_issue_history(&self, id: i64) -> Result<Vec<IssueAuditEntry>> {
        self.require_card(id)?;
        let mut stmt = self.conn.prepare(
            "SELECT id, card_id, action, old_node_id, new_node_id, changed_at
             FROM card_issue_history WHERE card_id=?1 ORDER BY id",
        )?;
        let entries = stmt
            .query_map(params![id], |row| {
                Ok(IssueAuditEntry {
                    id: row.get(0)?,
                    card_id: row.get(1)?,
                    action: row.get(2)?,
                    old_node_id: row.get(3)?,
                    new_node_id: row.get(4)?,
                    changed_at: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries)
    }

    pub fn active_card_for_issue(&self, node_id: &str) -> Result<Option<Card>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM cards WHERE issue_node_id=?1 AND archived_at IS NULL",
                params![node_id],
                rows::row_to_card,
            )
            .optional()?)
    }

    /// The database index is the final guard. This check provides an actionable
    /// error before attempting an insert or restoring an archived card.
    pub(super) fn require_issue_available(
        &self,
        node_id: &str,
        except_card: Option<i64>,
    ) -> Result<()> {
        let found = self.conn.query_row(
            "SELECT id FROM cards WHERE issue_node_id=?1 AND archived_at IS NULL AND id != COALESCE(?2, -1)",
            params![node_id, except_card],
            |row| row.get::<_, i64>(0),
        ).optional()?;
        if let Some(id) = found {
            return Err(Error::InvalidState(format!(
                "GitHub issue is already selected by active card #{id}"
            )));
        }
        Ok(())
    }
}
