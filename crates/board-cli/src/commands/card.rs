use anyhow::{anyhow, bail, Result};
use board_core::client::BoardClient;
use board_core::protocol::{
    BoardSnapshot, BoundCardCreateParams, CardCreateParams, CardIssueParams, CardMoveParams,
    CardUpdateParams, Patch,
};

use crate::args::{CardCmd, IssueCmd};
use crate::commands::board::resolve_board_in_list;
use crate::commands::run::{cmd_card_comment, cmd_card_run};
use crate::context::Ctx;
use crate::helpers::{confirm_action, parse_effort, parse_space_kind, parse_visibility};
use crate::render::{emit, emit_line};
use crate::scope::{resolve_column_in, resolved_scope_path};

pub(crate) fn cmd_card(sub: CardCmd, ctx: &mut Ctx) -> Result<()> {
    // Do not even auto-start boardd for a refused non-interactive deletion.
    if let CardCmd::Delete { confirm, .. } = &sub {
        confirm_action("card deletion", confirm.yes)?;
    }
    if let CardCmd::Comment {
        sub: crate::args::CommentCmd::Delete { confirm, .. },
    } = &sub
    {
        confirm_action("comment deletion", confirm.yes)?;
    }
    if let CardCmd::Issue {
        sub: IssueCmd::Unlink { confirm, .. },
    } = &sub
    {
        confirm_action("Issue unlink", confirm.yes)?;
    }

    let json = ctx.json();
    match sub {
        CardCmd::Create {
            title,
            issue,
            description,
            column,
            harness,
            model,
            effort,
            permission,
            session,
            space_kind,
            space_ref,
            space_cwd,
        } => {
            let column_id = ctx.optional_column_id(column.as_deref())?;
            let source_issue = issue
                .as_deref()
                .map(|reference| super::github_issue::resolve(ctx, reference))
                .transpose()?;
            let p = CardCreateParams {
                title,
                board_id: Some(ctx.board_id()?),
                description,
                column_id,
                harness,
                model,
                effort: parse_effort(effort)?,
                permission_mode: permission,
                session,
                space_kind: space_kind.as_deref().map(parse_space_kind).transpose()?,
                space_ref,
                space_cwd,
                position: None,
            };
            let card = match source_issue {
                Some(issue) => ctx
                    .client()?
                    .card_create_bound(&BoundCardCreateParams { card: p, issue })?,
                None => ctx.client()?.card_create(&p)?,
            };
            emit_line(
                &card,
                json,
                format!(
                    "Created card #{} \"{}\" in column {}",
                    card.id, card.title, card.column_id
                ),
            )
        }
        CardCmd::Edit {
            id,
            title,
            description,
            clear_description,
            harness,
            clear_harness,
            model,
            clear_model,
            effort,
            clear_effort,
            permission,
            clear_permission,
            session,
            clear_session,
            space_kind,
            space_ref,
            clear_space_ref,
            space_cwd,
            clear_space_cwd,
        } => {
            if clear_harness {
                bail!("--clear-harness is not supported: harness is required")
            }
            let p = CardUpdateParams {
                id,
                title,
                description: if clear_description {
                    Some(String::new())
                } else {
                    description
                },
                harness,
                model: Patch::from_flags(clear_model, model),
                effort: Patch::from_flags(clear_effort, parse_effort(effort)?),
                permission_mode: Patch::from_flags(clear_permission, permission),
                session: Patch::from_flags(clear_session, session),
                space_kind: space_kind.as_deref().map(parse_space_kind).transpose()?,
                space_ref: Patch::from_flags(clear_space_ref, space_ref),
                space_cwd: Patch::from_flags(clear_space_cwd, space_cwd),
            };
            let card = ctx.client()?.card_update(&p)?;
            emit_line(&card, json, format!("Updated card #{}", card.id))
        }
        CardCmd::Delete { id, .. } => {
            let result = ctx.client()?.card_delete(id)?;
            emit_line(&result, json, format!("Deleted card #{id}"))
        }
        CardCmd::Duplicate { id } => {
            let card = ctx.client()?.card_duplicate(id)?;
            emit_line(
                &card,
                json,
                format!("Duplicated card #{id} as #{} \"{}\"", card.id, card.title),
            )
        }
        CardCmd::Archive { id } => card_archive(ctx, id, true),
        CardCmd::Restore { id } => card_archive(ctx, id, false),
        CardCmd::Show { id } => {
            let detail = ctx.client()?.card_get(id)?;
            emit(&detail, json)
        }
        CardCmd::List { column, visibility } => {
            let column_id = ctx.optional_column_id(column.as_deref())?;
            let board_id = ctx.board_id()?;
            let visibility = parse_visibility(visibility)?;
            let cards =
                ctx.client()?
                    .card_list_for_board_visible(Some(board_id), column_id, visibility)?;
            emit(&cards, json)
        }
        CardCmd::Move {
            id,
            column,
            position,
            destination_board,
            to_project,
        } => cmd_move(
            ctx,
            id,
            &column,
            to_project.as_deref(),
            destination_board.as_deref(),
            position,
        ),
        CardCmd::Comment { sub } => cmd_card_comment(sub, ctx),
        CardCmd::Run { sub } => cmd_card_run(sub, ctx),
        CardCmd::Issue { sub } => cmd_issue(sub, ctx),
    }
}

fn cmd_issue(sub: IssueCmd, ctx: &mut Ctx) -> Result<()> {
    let json = ctx.json();
    match sub {
        IssueCmd::Inspect { card_id } => {
            let detail = ctx.client()?.card_get(card_id)?;
            if detail.card.board_id != ctx.board_id()? {
                bail!("card #{card_id} does not belong to the selected Board project");
            }
            let mut projection = super::github_issue::inspect(&detail)?;
            projection["card"]["project_scope"] = serde_json::json!(ctx.board()?.board.scope_path);
            emit_line(
                &projection,
                json,
                super::github_issue::inspection_line(&projection),
            )
        }
        IssueCmd::Bind { card_id, issue } => {
            require_selected_card(ctx, card_id)?;
            let issue = super::github_issue::resolve(ctx, &issue)?;
            let card = ctx
                .client()?
                .card_issue_bind(&CardIssueParams { card_id, issue })?;
            emit_line(&card, json, format!("Bound Issue to card #{card_id}"))
        }
        IssueCmd::Refresh { card_id } => {
            let card = require_selected_card(ctx, card_id)?;
            let source = card
                .source_issue
                .ok_or_else(|| anyhow!("card #{card_id} has no Issue"))?;
            let issue = super::github_issue::resolve_node(ctx, &source.identity.node_id)?;
            let card = ctx
                .client()?
                .card_issue_refresh(&CardIssueParams { card_id, issue })?;
            emit_line(&card, json, format!("Refreshed Issue on card #{card_id}"))
        }
        IssueCmd::Unlink { card_id, .. } => {
            require_selected_card(ctx, card_id)?;
            let card = ctx.client()?.card_issue_unlink(card_id)?;
            emit_line(&card, json, format!("Unlinked Issue from card #{card_id}"))
        }
        IssueCmd::History { card_id } => {
            require_selected_card(ctx, card_id)?;
            let entries = ctx.client()?.card_issue_history(card_id)?;
            emit(&entries, json)
        }
    }
}

fn require_selected_card(ctx: &mut Ctx, id: i64) -> Result<board_core::model::Card> {
    let board_id = ctx.board_id()?;
    let card = ctx.client()?.card_get(id)?.card;
    if card.board_id != board_id {
        bail!("card #{id} does not belong to the selected Board project");
    }
    Ok(card)
}

fn card_archive(ctx: &mut Ctx, id: i64, archived: bool) -> Result<()> {
    let json = ctx.json();
    let card = ctx.client()?.card_archive(id, archived)?;
    let action = if archived { "Archived" } else { "Restored" };
    emit_line(&card, json, format!("{action} card #{}", card.id))
}

/// Move a card, resolving the destination column against the board the card is
/// landing on.
///
/// Destination precedence: `--to-project` (with an optional `--to-board`
/// id-or-name within it), else an explicit `--destination-board`, else the
/// global `--board` selector (deprecated — it warns only when it actually
/// crosses boards), else the global `--project` (deprecated — it warns only
/// when it actually leaves the project), else the card's own board. Every arm
/// is non-selecting: card moves never touch the daemon's selection or recency.
/// A named destination costs two RPCs (`project.get`/`board.open`/`board.get`
/// plus `card.move`); a fallback that ends up crossing boards or projects,
/// and the "stay on the card's board" case, need one extra `card.get`,
/// because protocol v1 has no way to learn a card's board without it and no
/// way to move a card by column *name*. A `card.move` that accepted a column
/// name would collapse every case to one round-trip.
pub(crate) fn cmd_move(
    ctx: &mut Ctx,
    card_id: i64,
    column: &str,
    to_project: Option<&str>,
    destination: Option<&str>,
    position: Option<i64>,
) -> Result<()> {
    let json = ctx.json();
    if position.is_some_and(|p| p < 0) {
        bail!("--position must be zero-based (0 = first card)")
    }
    let board = match to_project {
        Some(path) => {
            let scope = resolved_scope_path(path)?;
            destination_board_in_project(ctx.client()?, &scope, destination)?
        }
        None => match destination {
            Some(selector) => destination_board_legacy(ctx.client()?, selector)?,
            None => match ctx.selector() {
                Some(selector) => {
                    let board = destination_board_legacy(ctx.client()?, selector)?;
                    let source_board_id = ctx.client()?.card_get(card_id)?.card.board_id;
                    if source_board_id != board.board.id {
                        eprintln!(
                            "board: warning: `move` is using the global --board {selector} as the move \
                             destination; this fallback is deprecated, pass --destination-board \
                             {selector} to cross boards"
                        );
                    }
                    board
                }
                None => match ctx.project_selector() {
                    Some(path) => {
                        let scope = resolved_scope_path(path)?;
                        let board = destination_board_in_project(ctx.client()?, &scope, None)?;
                        let source_board_id = ctx.client()?.card_get(card_id)?.card.board_id;
                        if source_board_id != board.board.id {
                            eprintln!(
                                "board: warning: `move` is using the global --project as the move \
                                 destination; pass --to-project to cross projects"
                            );
                        }
                        board
                    }
                    None => {
                        let board_id = ctx.client()?.card_get(card_id)?.card.board_id;
                        ctx.client()?.board_get_by_id(board_id)?
                    }
                },
            },
        },
    };
    let column_id = resolve_column_in(&board, column)?;
    let card = ctx.client()?.card_move(&CardMoveParams {
        id: card_id,
        column_id,
        board_id: Some(board.board.id),
        position,
    })?;
    emit_line(
        &card,
        json,
        format!(
            "Moved card #{} to column {} [{}]",
            card.id, card.column_id, card.status
        ),
    )
}

/// Resolve a move destination inside a project: an explicit `--to-board`
/// reference as id-or-name within the project, else the project's selected
/// board, else its first board (`main`). Never touches selection or recency.
fn destination_board_in_project(
    c: &mut board_core::client::UnixClient,
    scope: &str,
    destination: Option<&str>,
) -> Result<BoardSnapshot> {
    let detail = c.project_get(scope)?;
    let board = match destination {
        Some(reference) => {
            resolve_board_in_list(&detail.boards, reference, &format!("project {scope:?}"))?
        }
        None => detail
            .selected_board
            .clone()
            .or_else(|| {
                detail
                    .boards
                    .iter()
                    .find(|board| board.name.eq_ignore_ascii_case("main"))
                    .cloned()
            })
            .or_else(|| detail.boards.first().cloned())
            .ok_or_else(|| anyhow!("project {scope:?} has no boards"))?,
    };
    c.board_get_by_id(board.id)
}

/// Resolve the legacy `<ID|PATH>` move destination: an id in any project,
/// else `board.open` for a canonical scope path. Non-selecting.
fn destination_board_legacy(
    c: &mut board_core::client::UnixClient,
    selector: &str,
) -> Result<BoardSnapshot> {
    if let Ok(id) = selector.parse::<i64>() {
        return c.board_get_by_id(id);
    }
    let scope = resolved_scope_path(selector)?;
    c.board_open(&scope)
}
