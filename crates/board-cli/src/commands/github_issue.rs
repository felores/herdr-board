//! Resolve a selected GitHub Issue before sending a card creation request.
//! Only `gh` reads credentials; neither the request nor the database stores them.

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use board_core::model::IssueIdentity;
use board_core::protocol::CardDetail;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::context::Ctx;

const QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){id nameWithOwner viewerPermission issue(number:$number){id number url}}}";
const NODE_QUERY: &str = "query($id:ID!){node(id:$id){... on Issue{id number url state repository{id nameWithOwner viewerPermission}}}}";

/// A single live, read-only observation. Unknown is deliberately distinct from
/// open: a null GraphQL node cannot distinguish a deleted Issue from lack of access.
/// Board execution and client-declared evidence do not establish verification
/// or Git delivery. Consumers must not promote this view into either authority.
pub(super) fn inspect(detail: &CardDetail) -> Result<Value> {
    let source = detail
        .card
        .source_issue
        .as_ref()
        .ok_or_else(|| anyhow!("card #{} has no bound GitHub Issue", detail.card.id))?;
    let checked_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut diagnostics: Vec<&str> = Vec::new();
    let mut issue = json!({"authority": "github", "state": "unknown", "identity": null,
        "observed_at_epoch_seconds": null});
    let output = Command::new("gh")
        .args([
            "api",
            "graphql",
            "-f",
            &format!("query={NODE_QUERY}"),
            "-f",
            &format!("id={}", source.identity.node_id),
        ])
        .output();
    match output {
        Err(_) => diagnostics.push("github_unavailable"),
        Ok(output) if !output.status.success() => diagnostics.push("github_unavailable"),
        Ok(output) => match serde_json::from_slice::<Value>(&output.stdout) {
            Err(_) => diagnostics.push("github_unavailable"),
            Ok(response) if response.get("errors").is_some() => {
                diagnostics.push("github_unavailable")
            }
            Ok(response) => {
                let node = &response["data"]["node"];
                if node.is_null() {
                    diagnostics.push("missing_or_inaccessible");
                } else if node["id"].as_str() != Some(&source.identity.node_id) {
                    diagnostics.push("identity_mismatch");
                } else if let (
                    Some(repo_id),
                    Some(repository),
                    Some(number),
                    Some(url),
                    Some(state),
                ) = (
                    node["repository"]["id"].as_str(),
                    node["repository"]["nameWithOwner"].as_str(),
                    node["number"].as_i64(),
                    node["url"].as_str(),
                    node["state"].as_str(),
                ) {
                    if number < 1
                        || url != format!("https://github.com/{repository}/issues/{number}")
                        || !matches!(state, "OPEN" | "CLOSED")
                    {
                        diagnostics.push("invalid_github_response");
                    } else {
                        issue = json!({"authority": "github", "state": state.to_ascii_lowercase(),
                            "identity": IssueIdentity {node_id: source.identity.node_id.clone(),
                                repository_id: repo_id.to_owned(), repository: repository.to_owned(),
                                number, url: url.to_owned()}, "observed_at_epoch_seconds": checked_at});
                        if repo_id != source.identity.repository_id {
                            diagnostics.push("transferred");
                            diagnostics.push("repository_mismatch");
                        } else if repository != source.identity.repository {
                            diagnostics.push("repository_renamed");
                        }
                        if number != source.identity.number {
                            diagnostics.push("renumbered");
                        }
                        if state == "CLOSED" && detail.runs.iter().any(|run| run.ended_at.is_none())
                        {
                            diagnostics.push("closed_with_open_run");
                        }
                    }
                } else {
                    diagnostics.push("invalid_github_response");
                }
            }
        },
    }
    Ok(json!({
        "card": {"authority": "board", "id": detail.card.id, "board_id": detail.card.board_id,
            "status": detail.card.status, "archived_at": detail.card.archived_at,
            "open_run_ids": detail.runs.iter().filter(|r| r.ended_at.is_none()).map(|r| r.id).collect::<Vec<_>>()},
        "binding": {"authority": "board", "identity": source.identity, "observed_at": source.observed_at},
        "issue": issue, "diagnostics": diagnostics,
        "checked_at_epoch_seconds": checked_at,
        "verification": "not_assessed", "delivery": "not_assessed"
    }))
}

pub(super) fn inspection_line(projection: &Value) -> String {
    format!(
        "card #{}: GitHub Issue {} ({})",
        projection["card"]["id"],
        projection["issue"]["state"].as_str().unwrap_or("unknown"),
        projection["diagnostics"]
            .as_array()
            .map(|d| d
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_default()
    )
}

fn parse_issue_ref(reference: &str) -> Result<(&str, &str, i64)> {
    let (repository, number) = if let Some(rest) = reference.strip_prefix("https://github.com/") {
        let (repository, number) = rest
            .split_once("/issues/")
            .ok_or_else(|| anyhow!("expected a canonical GitHub Issue URL"))?;
        (repository, number)
    } else {
        reference
            .split_once('#')
            .ok_or_else(|| anyhow!("expected OWNER/REPO#NUMBER or a GitHub Issue URL"))?
    };
    let (owner, repo) = repository
        .split_once('/')
        .ok_or_else(|| anyhow!("expected OWNER/REPO"))?;
    let valid = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
    };
    if !valid(owner) || !valid(repo) {
        bail!("expected a GitHub OWNER/REPO without extra path segments");
    }
    let number: i64 = number.parse().context("invalid GitHub Issue number")?;
    if number < 1 {
        bail!("GitHub Issue number must be positive");
    }
    Ok((owner, repo, number))
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        bail!("the selected Board project must be a Git repository");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn remote_repository(url: &str) -> Option<&str> {
    let suffix = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    Some(suffix.strip_suffix(".git").unwrap_or(suffix))
}

fn require_matching_git_root(scope: &str, repository: &str) -> Result<()> {
    let path = Path::new(scope);
    let root = git_output(path, &["rev-parse", "--show-toplevel"])?;
    if path.canonicalize()? != Path::new(&root).canonicalize()? {
        bail!("the selected Board project is not the Git root for this Issue");
    }
    let remotes = git_output(path, &["remote"])?;
    for remote in remotes.lines() {
        let url = git_output(path, &["remote", "get-url", "--push", remote])?;
        if remote_repository(&url).is_some_and(|name| name.eq_ignore_ascii_case(repository)) {
            return Ok(());
        }
    }
    bail!("the Issue repository does not match a push remote of the selected Board project")
}

pub(super) fn resolve(ctx: &mut Ctx<'_>, reference: &str) -> Result<IssueIdentity> {
    let (owner, name, number) = parse_issue_ref(reference)?;
    let scope = ctx
        .board()?
        .board
        .scope_path
        .as_deref()
        .ok_or_else(|| {
            anyhow!("select a repository-scoped Board project before linking a GitHub Issue")
        })?
        .to_string();
    require_matching_git_root(&scope, &format!("{owner}/{name}"))?;
    let output = Command::new("gh")
        .args([
            "api",
            "graphql",
            "-f",
            &format!("query={QUERY}"),
            "-f",
            &format!("owner={owner}"),
            "-f",
            &format!("name={name}"),
            "-F",
            &format!("number={number}"),
        ])
        .output()
        .context("GitHub CLI unavailable; sign in with `gh auth login`")?;
    if !output.status.success() {
        bail!("GitHub Issue cannot be resolved or is inaccessible; no card was created");
    }
    let response: Value =
        serde_json::from_slice(&output.stdout).context("invalid GitHub Issue response")?;
    let repository = &response["data"]["repository"];
    if !matches!(
        repository["viewerPermission"].as_str(),
        Some("WRITE" | "MAINTAIN" | "ADMIN")
    ) {
        bail!("the selected GitHub repository is not writable by this account");
    }
    let issue = &repository["issue"];
    let required = |value: &Value, field| -> Result<String> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("GitHub Issue missing {field}"))
    };
    let identity = IssueIdentity {
        node_id: required(&issue["id"], "node ID")?,
        repository_id: required(&repository["id"], "repository ID")?,
        repository: required(&repository["nameWithOwner"], "repository")?,
        number: issue["number"]
            .as_i64()
            .ok_or_else(|| anyhow!("GitHub Issue missing number"))?,
        url: required(&issue["url"], "canonical URL")?,
    };
    require_matching_git_root(&scope, &identity.repository)?;
    Ok(identity)
}

pub(super) fn resolve_node(ctx: &mut Ctx<'_>, node_id: &str) -> Result<IssueIdentity> {
    let scope = ctx
        .board()?
        .board
        .scope_path
        .as_deref()
        .ok_or_else(|| {
            anyhow!("select a repository-scoped Board project before refreshing an Issue")
        })?
        .to_string();
    let output = Command::new("gh")
        .args([
            "api",
            "graphql",
            "-f",
            &format!("query={NODE_QUERY}"),
            "-f",
            &format!("id={node_id}"),
        ])
        .output()
        .context("GitHub CLI unavailable; sign in with `gh auth login`")?;
    if !output.status.success() {
        bail!(
            "GitHub Issue cannot be resolved or is inaccessible; the existing binding is unchanged"
        );
    }
    let response: Value =
        serde_json::from_slice(&output.stdout).context("invalid GitHub Issue response")?;
    let node = &response["data"]["node"];
    if node["id"].as_str() != Some(node_id) {
        bail!("GitHub did not return the bound Issue node ID; the binding is unchanged");
    }
    let repository = &node["repository"];
    if !matches!(
        repository["viewerPermission"].as_str(),
        Some("WRITE" | "MAINTAIN" | "ADMIN")
    ) {
        bail!("the Issue repository is not writable by this account; the binding is unchanged");
    }
    let required = |value: &Value, field| -> Result<String> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("GitHub Issue missing {field}"))
    };
    let identity = IssueIdentity {
        node_id: node_id.to_owned(),
        repository_id: required(&repository["id"], "repository ID")?,
        repository: required(&repository["nameWithOwner"], "repository")?,
        number: node["number"]
            .as_i64()
            .ok_or_else(|| anyhow!("GitHub Issue missing number"))?,
        url: required(&node["url"], "canonical URL")?,
    };
    require_matching_git_root(&scope, &identity.repository)?;
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_explicit_issue_identity() {
        assert_eq!(
            parse_issue_ref("owner/project#42").unwrap(),
            ("owner", "project", 42)
        );
        assert_eq!(
            parse_issue_ref("https://github.com/owner/project/issues/42").unwrap(),
            ("owner", "project", 42)
        );
        for invalid in [
            "42",
            "owner/project#0",
            "owner/project#42/extra",
            "owner/project/extra#42",
            "https://evil.example/owner/project/issues/42",
        ] {
            assert!(parse_issue_ref(invalid).is_err(), "accepted {invalid}");
        }
    }
}
