# GitHub issue and Board execution lifecycle

Status: implementation plan. Only repository Issue settings and the global agent rule are in effect. The Board bindings, admission control, and reconciliation specified here are not shipped.

2026-10-03 revision: card granularity changed at the owner's direction. A card represents one **initiative** (epic or roadmap item), never one task or issue; issues carry the task detail underneath it via milestone or `epic:*` label, and runs and gates attach to the initiative card. The one-active-card-per-issue rule below is superseded and any implementation must follow the initiative model instead.

2026-10-03 in-flight work examined: the dirty checkout on `docs/herdr-compatibility-guardrails` carries an uncommitted **schema v16** adding `evidence` and `decisions` tables (38 files, ~482 insertions; new `evidence.rs` across board-core/board-daemon/board-cli). The epic binding must NOT reuse v16: it targets **schema v17** and integrates after that work lands.

Owner decision 2026-10-03: implementation of the epic binding starts **only after** the v16 (evidence/decisions) work is committed and lands. No parallel worktree until then.

## Outcome and ownership

Make the active-work view complete without creating another backlog. Repository product files own direction and outcomes; a GitHub Issue owns one durable repository deliverable; the repository Harness ledger owns feature verification; Git and GitHub own changes, PRs, checks, and delivery. Herdr Board alone owns the bird's-eye initiative view: one card per initiative with its operational phase, runs, gates, and execution history; it is never a task backlog. Herdr supplies process observations, not completion proof. Portfolio Intelligence is read-only and may report mismatches but never create or close work. Directus retains only shared AOS engagements, commitments, jobs, and audit; `os_tasks` is not a mirror of Issues or cards.

The owner of Board schema, daemon, CLI, and operational reconciliation is this repository. The global agent instruction carries only the short routing rule. The FeloVault AOS architecture remains the cross-system authority: `agencia/wiki/arquitectura/aos-arquitectura.md`. Do not implement a second writer or controller in `aos-core`, a Portfolio adapter, or the global prompt.

## Observed baseline and constraints

- The FeloVault Board currently holds cards for work in other Git roots; the installed CLI/daemon report version `0.16.1`. Its current `card create/edit` contract has no typed GitHub Issue binding. Card #33 names Sinapso issues #2, #3, and #4 in prose, while one Issue should represent one deliverable. Those three issues became visible again when Issues were enabled on `felores/sinapso`.
- Issues are enabled on `felores/sinapso` (private) and `felores/herdr-board` (public fork). The latter's `main` is 20 commits behind upstream `nelsonPires5/herdr-board/main` as observed on 2026-09-26; this is not a reason to fast-forward or touch the dirty checkout automatically.
- The active herdr-board checkout is dirty across Board schema, daemon, docs, and tests. It contains another change in progress. Do not overwrite it, commit it, run host Board/Herdr tests on it, or claim this plan is implemented. Reconcile ownership and choose an isolated integration branch/worktree at an agreed base before product-code edits. Existing schema v16 work must be examined before defining a new migration.
- Card #7 and the approved design referenced by card #14 cover the broader deterministic Board controller, admission, exact Herdr identity, locks, recovery, and Human Gate UX. This plan adds GitHub bindings and an explicit repository-work lifecycle to that design; it neither silently supersedes nor claims completion of those slices.

## Selection and identity contract

1. A product proposal enters `STRATEGY.md`, `PRODUCT.md`, or `ROADMAP.md` only through its owning repository procedure. When a repository deliverable is accepted, create or use one Issue in the writable canonical repository. Bugs, features, and maintenance share this rule; minor direct in-session fixes need no Issue. Never turn every roadmap row or brainstorm into an Issue.
2. Resolve the intended repository explicitly. `gh` may infer the upstream from a fork's `origin`, so calls must supply `--repo OWNER/NAME` after verifying the writable remote and access. The Issue key is GitHub's immutable issue `node_id` plus the current repository ID and number for display. Resolve transfers or renames by the immutable ID, never by title, label, number alone, or free-text URL.
3. Selecting work to execute creates or reuses one active Board card per **initiative** (epic or roadmap item); opening an Issue alone never dispatches a card. Issues bind underneath the initiative via milestone or `epic:*` label, and runs and gates attach to that initiative card; never create one card per task or issue. The card references the repository Git root and may carry Harness feature, roadmap item, plan, and PR refs, without duplicating their state. A card can instead have no issues for Vault, personal, AOS, decision, or explicitly scoped direct work. Split legacy card #33 into separate deliverables only after inspecting its history and exact Issue identities. Preserve history and do not close Issues during migration.
4. Any material *delegated agent launch* selected by the operator must receive a Board card/run before dispatch. Direct read-only probes and small direct in-session edits are not represented as unearned Board runs. The planned admission mechanism must also cover direct Herdr launches and report agents or AOS jobs outside its scope as observed exceptions, not fabricate retroactive Board runs. Board's completion channel plus repository evidence, not pane idle or Issue closure, proves operational completion.
5. An Issue closes only after a commit, PR, or diff proving its deliverable is linked in a GitHub comment; no Board transition or Harness `passing` alone closes it. A card may reach Done while delivery remains pending, but the unified view must show that mismatch explicitly. Never merge, push, publish, or close an Issue as a side effect of a card transition.

## Typed interfaces and permissions

- Add an optional typed issue binding on a card in `board-core`/SQLite/protocol: an epic reference (milestone or `epic:*` label) plus explicit issue refs. Accept a canonical issue URL or `OWNER/REPO#N` at CLI admission; resolve via the GitHub API, persist immutable ID and current repository ID/name/number, validate the card's target Git root against the registered repo, and enforce one active card per initiative among non-archived cards. Allow an explicit unlink/rebind action with audit history; no silent reassignment. Existing free-text cards remain valid and unchanged.
- Board daemon is the sole SQLite writer. New card, binding, run, gate, and dedupe operations use its existing atomic operation path. GitHub resolution happens outside database locks. On ambiguous network timeout, re-read by immutable ID before a retry. Cache observations with timestamps and expiry; never convert a stale cache into Issue closure or dispatch authorization.
- The CLI/Board detail shows the link, `verified_at`, and any `missing`, `inaccessible`, `transferred`, `closed_with_open_run`, or `delivered_with_open_issue` diagnostic. GitHub credentials are used by a least-privilege adapter outside stored card payloads; never persist tokens or include them in prompts. Do not require GitHub access to create an unbound local card.
- Portfolio may join read-only Board snapshots with registered repository and Git observations. It labels each claim by authority, freshness, and evidence reference; it does not create cards from unbound Issues or roadmap items. An eventual webhook is an optional observation transport, not a second lifecycle writer. The first version reconciles bound Issues at explicit selection and bounded refresh; no repository-wide polling or issue auto-import.
- Global agent rule must state routing and completion proof, then link to this plan and the canonical architecture. Repository `AGENTS.md` files add only local exceptions or verification commands, not copies of this entire process. Keep public GitHub-facing prose in English.

## Epic binding data model (schema v17)

All changes are additive and ship after the in-flight v16 (evidence/decisions) work lands.

```sql
-- cards gain an optional initiative identity (one active card per initiative)
ALTER TABLE cards ADD COLUMN epic_kind      TEXT;   -- 'milestone' | 'label' | NULL
ALTER TABLE cards ADD COLUMN epic_slug      TEXT;   -- milestone title slug or label name
ALTER TABLE cards ADD COLUMN repo_full_name TEXT;   -- 'OWNER/REPO' the epic resolves in

CREATE TABLE card_issues (
  id             INTEGER PRIMARY KEY,
  card_id        INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
  issue_node_id  TEXT NOT NULL,         -- GitHub immutable id
  repo_full_name TEXT NOT NULL,
  issue_number   INTEGER NOT NULL,
  state          TEXT NOT NULL,         -- cached open|closed observation
  verified_at    TEXT,
  UNIQUE(card_id, issue_node_id)
);

CREATE UNIQUE INDEX idx_cards_epic_active
  ON cards(epic_kind, epic_slug)
  WHERE epic_slug IS NOT NULL AND archived_at IS NULL;
```

- **Roll-up:** card progress = closed/total counts from `card_issues`, refreshed at bind time and on bounded explicit refresh only; no repository-wide polling.
- **Milestone vs label:** `epic_kind = 'milestone'` binds by milestone number and pulls its issue set; `epic_kind = 'label'` binds by `epic:<slug>`. Both cache per-issue rows into `card_issues` for roll-up and diagnostics.
- **CLI surface:** `board card create --milestone <n>|--epic <slug>`, `board card bind-epic`, `board card issues`, `board card sync-issues` (bounded refresh). The detail view shows the epic, roll-up `k/n`, per-issue state and the existing diagnostics vocabulary (`missing`, `inaccessible`, `transferred`, `closed_with_open_run`, `delivered_with_open_issue`).
- **Daemon:** sole SQLite writer; GitHub resolution outside database locks; ambiguous timeouts re-read by immutable ID before retry. Unbound cards remain valid and usable.

## Implementation slices and focused proofs

1. **Configuration and rule:** Verify enabled Issues in the two approved repos; clarify the global rule, including explicit `--repo` for forks. Direct inspection proves both settings and the prompt diff. Done for this slice does not mean runtime integration works.
2. **Epic binding:** On an isolated Board worktree at a base agreed with the v16 evidence/decisions owner, implement the initiative binding per the data model above: schema v17, core operations, protocol, CLI, and detail view. Proofs at the CLI boundary against a real public API or a controlled GitHub adapter: bind a milestone epic, bind a label epic, roll-up counts match the issue set, repeat bind without duplicate, reject a second active card for the same initiative, preserve unbound legacy cards, handle milestone rename and inaccessible issues. Follow Board's sandbox-first test gate. Schema and external API changes require review before release.
3. **Admission and reconciliation:** Integrate the existing card #14 controller plan instead of writing a second one. Add safe selection/reuse, direct-launch admission and mismatched-state diagnostics. E2E in the disposable Herdr sandbox proves no material delegated launch without a bound card/run, no false completion from an idle pane, and no duplicate launch after daemon restart. A GitHub outage leaves an existing bound card visible and blocked from authority-sensitive transitions; offline direct work remains possible only within the explicit unbound policy.
4. **Portfolio projection:** Add read-only joins with source references and freshness; prove that an Issue without a card is listed as backlog, not running, that a pane without an admitted run is labeled observed/external, and that no observation writes to Board, repo ledger, GitHub, or Directus.
5. **Pilot and migration:** Pilot one initiative (its issues under a milestone or `epic:*` label) on Sinapso and one on herdr-board after their respective repositories and identities are resolved. Reconcile legacy card #33 against issues #2/#3/#4 without losing comments or run history. Audit mismatched links before any bulk conversion. Verify a full journey: Issue accepted under an initiative -> initiative card selected -> admitted run -> receipt/verification -> commit or PR evidence -> Issue closure by authorized actor -> card roll-up and portfolio display consistent facts. Closing and publishing remain explicit human-authorized operations.

## Human and safety gates

- The operator authorized enabling Issues on both repositories. This does not authorize publishing private Issue content to the public fork, retroactive Issue closure, pushing code, merging upstream, migrating cards, or deploying the installed Board.
- Do not modify the current dirty herdr-board checkout's existing files until its other work is reconciled. Use an isolated integration base and account for upstream/fork divergence before choosing a merge target. Never overwrite someone else's branch or edits.
- Product decisions, ambiguous repository ownership, security/privacy, schema rollout, spending, publishing, and destructive migration stay human-gated. Board may suggest, not infer consent.

## Acceptance

The system is complete only when every material delegated run has a Board identity, an initiative resolves to one active card and its issues roll up under it without making Board the Issue authority, unbound cards remain usable, the portfolio distinguishes pending/active/observed/verified/delivered with evidence, a GitHub/Herdr outage does not fabricate success or duplicate effects, and the Sinapso pilot closes with linked Git proof. Until the controller and its primary-path checks land, report this integration as **planned**, not operational.

## Schema version collision (2026-10-12)

Three separate checkouts each claim SQLite schema v16 with different content:

- This repository's `docs/herdr-compatibility-guardrails` work adds the `evidence` and `decisions` durability surfaces.
- The installed plugin checkout (`~/.config/herdr/plugins/github/herdr-board-948ba5448416`) implements v16 as the deterministic controller state plane, including `human_gate_decisions`.
- The treehouse worktree (`feat/issue-board-lifecycle-20260926`) carries a third v16 plus v17 for the Issue binding.

The live Board database is at `user_version=16` and already contains `human_gate_decisions`, which comes from the plugin implementation. Installing a binary built from this repository's v16 would skip its `if version < 16` migration because the version check already passes, so its `evidence` and `decisions` tables would never be created and the daemon would run against a schema it wrongly believes is current.

Version numbering and ownership of v16 remain undecided. No install, release, or merge of any of the three variants may happen until that is settled, and no variant may silently reuse another variant's number or migration predicate.
