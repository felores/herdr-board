//! The real CLI and daemon, with only the read-only GitHub lookup stubbed.
//! All state and the fake gh binary live in the sandbox's disposable /tmp.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use super::{json_output, TestDaemon};
use board_core::client::BoardClient;

#[test]
fn selecting_a_writable_repo_issue_creates_one_bound_card() {
    let td = TestDaemon::start(&[]);
    let project = td._dir.path().join("repo");
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let run_git = |args: &[&str]| {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(args)
            .status()
            .unwrap()
            .success());
    };
    run_git(&["init", "-q"]);
    run_git(&[
        "remote",
        "add",
        "fork",
        "https://github.com/owner/project.git",
    ]);
    let scope = project.to_str().unwrap();
    td.client().board_open(scope).unwrap();

    let fake_bin = td._dir.path().join("fake-bin");
    std::fs::create_dir(&fake_bin).unwrap();
    let fake_gh = fake_bin.join("gh");
    std::fs::write(&fake_gh, r##"#!/bin/sh
case "$4" in
  *'node(id:'*)
    printf '%s\n' '{"data":{"node":{"id":"I_kwDOexample","number":43,"url":"https://github.com/owner/project/issues/43","repository":{"id":"R_kgDOexample","nameWithOwner":"owner/project","viewerPermission":"WRITE"}}}}'
    ;;
  *)
    printf '%s\n' '{"data":{"repository":{"id":"R_kgDOexample","nameWithOwner":"owner/project","viewerPermission":"WRITE","issue":{"id":"I_kwDOexample","number":42,"url":"https://github.com/owner/project/issues/42"}}}}'
    ;;
esac
"##).unwrap();
    std::fs::set_permissions(&fake_gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());
    let args = [
        "--project",
        scope,
        "card",
        "create",
        "--title",
        "selected",
        "--issue",
        "owner/project#42",
        "--json",
    ];
    let created = json_output(&td.board_in_with_env(&project, &args, &[("PATH", &path)]));
    assert_eq!(created["source_issue"]["node_id"], "I_kwDOexample");
    assert_eq!(created["source_issue"]["repository_id"], "R_kgDOexample");
    let replay = json_output(&td.board_in_with_env(&project, &args, &[("PATH", &path)]));
    assert_eq!(replay["id"], created["id"]);
    let cards = json_output(&td.board_in_with_env(
        &project,
        &["--project", scope, "card", "list", "--json"],
        &[],
    ));
    assert_eq!(cards.as_array().unwrap().len(), 1);

    let id = created["id"].as_i64().unwrap().to_string();
    let unlinked = json_output(&td.board_in_with_env(
        &project,
        &[
            "--project",
            scope,
            "card",
            "issue",
            "unlink",
            &id,
            "--yes",
            "--json",
        ],
        &[],
    ));
    assert!(unlinked.get("source_issue").is_none());
    let bound = json_output(&td.board_in_with_env(
        &project,
        &[
            "--project",
            scope,
            "card",
            "issue",
            "bind",
            &id,
            "owner/project#42",
            "--json",
        ],
        &[("PATH", &path)],
    ));
    assert_eq!(bound["source_issue"]["number"], 42);
    let refreshed = json_output(&td.board_in_with_env(
        &project,
        &[
            "--project",
            scope,
            "card",
            "issue",
            "refresh",
            &id,
            "--json",
        ],
        &[("PATH", &path)],
    ));
    assert_eq!(refreshed["source_issue"]["number"], 43);
    assert_eq!(
        refreshed["source_issue"]["node_id"],
        created["source_issue"]["node_id"]
    );
    // Live read-only observation must not rewrite the stored Issue identity.
    std::fs::write(&fake_gh, r##"#!/bin/sh
printf '%s\n' '{"data":{"node":{"id":"I_kwDOexample","number":8,"url":"https://github.com/other/project/issues/8","state":"CLOSED","repository":{"id":"R_other","nameWithOwner":"other/project"}}}}'
"##).unwrap();
    let inspected = json_output(&td.board_in_with_env(
        &project,
        &[
            "--project",
            scope,
            "card",
            "issue",
            "inspect",
            &id,
            "--json",
        ],
        &[("PATH", &path)],
    ));
    assert_eq!(inspected["issue"]["authority"], "github");
    assert_eq!(inspected["issue"]["state"], "closed");
    assert_eq!(
        inspected["issue"]["identity"]["repository"],
        "other/project"
    );
    assert_eq!(
        inspected["diagnostics"],
        serde_json::json!(["transferred", "repository_mismatch", "renumbered"])
    );
    assert_eq!(inspected["verification"], "not_assessed");
    assert_eq!(inspected["delivery"], "not_assessed");
    assert!(inspected["issue"]["observed_at_epoch_seconds"]
        .as_u64()
        .is_some());
    assert_eq!(
        td.client()
            .card_get(id.parse().unwrap())
            .unwrap()
            .card
            .source_issue
            .unwrap()
            .identity
            .repository,
        "owner/project"
    );
    let history = json_output(&td.board_in_with_env(
        &project,
        &[
            "--project",
            scope,
            "card",
            "issue",
            "history",
            &id,
            "--json",
        ],
        &[],
    ));
    let actions: Vec<_> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["bind", "unlink", "bind", "refresh"]);

    std::fs::write(
        &fake_gh,
        "#!/bin/sh\nprintf '%s\\n' '{\"data\":{\"node\":null}}'\n",
    )
    .unwrap();
    let unavailable = json_output(&td.board_in_with_env(
        &project,
        &[
            "--project",
            scope,
            "card",
            "issue",
            "inspect",
            &id,
            "--json",
        ],
        &[("PATH", &path)],
    ));
    assert_eq!(unavailable["issue"]["state"], "unknown");
    assert_eq!(
        unavailable["diagnostics"],
        serde_json::json!(["missing_or_inaccessible"])
    );
    std::fs::write(&fake_gh, "#!/bin/sh\nexit 2\n").unwrap();
    let outage = json_output(&td.board_in_with_env(
        &project,
        &[
            "--project",
            scope,
            "card",
            "issue",
            "inspect",
            &id,
            "--json",
        ],
        &[("PATH", &path)],
    ));
    assert_eq!(outage["issue"]["state"], "unknown");
    assert_eq!(
        outage["diagnostics"],
        serde_json::json!(["github_unavailable"])
    );
    assert!(outage["issue"]["observed_at_epoch_seconds"].is_null());
    // Restore a null-node response for the existing mutating refresh failure assertion.
    std::fs::write(
        &fake_gh,
        "#!/bin/sh\nprintf '%s\\n' '{\"data\":{\"node\":null}}'\n",
    )
    .unwrap();
    let inaccessible = td.board_in_with_env(
        &project,
        &[
            "--project",
            scope,
            "card",
            "issue",
            "refresh",
            &id,
            "--json",
        ],
        &[("PATH", &path)],
    );
    assert!(!inaccessible.status.success());
    let shown = json_output(&td.board_in_with_env(
        &project,
        &["--project", scope, "card", "show", &id, "--json"],
        &[],
    ));
    assert_eq!(shown["card"]["source_issue"]["number"], 43);
}
