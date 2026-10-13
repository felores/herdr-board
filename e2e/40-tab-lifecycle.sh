#!/usr/bin/env bash
# 40-tab-lifecycle.sh — full opted-in executor/verification topology,
# protected legacy-duplicate cleanup, rename outbox, and close retry on restart.
set -euo pipefail
. "$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)/lib.sh"

export E2E_FAKE_ENV="FAKE_AGENT_SLEEP=8"
e2e_boot
e2e_ws_create lifecycle; WS_ID="$E2E_WS"
HUMAN_ID="$(col_create '{"name":"Human Gate","trigger":"manual"}')"
BLOCKED_ID="$(col_create '{"name":"Blocked","trigger":"manual"}')"
VERIFY_ID="$(col_create "{\"name\":\"Verification\",\"trigger\":\"auto\",\"fresh_session\":true,\"on_success_column_id\":$BLOCKED_ID}")"
EXEC_ID="$(col_create "{\"name\":\"Execute\",\"trigger\":\"auto\",\"fresh_session\":false,\"on_success_column_id\":$VERIFY_ID}")"
card_json="$($BOARD_BIN card new --title 'Legacy title' --description 'runtime lifecycle' \
  --harness fake --space-kind workspace --space-ref "$WS_ID" --json)"
CARD_ID="$(printf '%s' "$card_json" | jget id)"

step "Opt in the fixture while boardd is stopped (single-writer preserved)"
e2e_daemon_stop
python3 - "$BOARD_DB" "$CARD_ID" <<'PY'
import json, sqlite3, sys
conn=sqlite3.connect(sys.argv[1])
payload=json.dumps({"runtime_label":"tab lifecycle repair"}, separators=(",",":"))
conn.execute("INSERT INTO card_contracts(card_id,contract_version,revision,contract_json,outcome_code,projection_revision) VALUES(?, 'board-card/v1', 1, ?, 'runtime', 1)", (int(sys.argv[2]), payload))
conn.commit()
PY
e2e_daemon_start

tab_id_for_label() {
  local label="$1"
  hrpc tab.list "{\"workspace_id\":\"$WS_ID\"}" | python3 -c '
import json,sys
tabs=[t for t in json.load(sys.stdin).get("tabs",[]) if t.get("label")==sys.argv[1]]
if len(tabs)!=1: raise SystemExit(1)
print(tabs[0]["tab_id"])' "$label"
}
wait_label() {
  local label="$1" tab="" i
  for i in $(seq 1 100); do
    tab="$(tab_id_for_label "$label" 2>/dev/null || true)"
    [ -n "$tab" ] && { printf '%s\n' "$tab"; return 0; }
    sleep 0.1
  done
  return 1
}
assert_zero_card_tabs() {
  local count
  count="$(hrpc tab.list "{\"workspace_id\":\"$WS_ID\"}" | python3 -c '
import json,sys
prefix=f"card-{sys.argv[1]}"
print(sum(t.get("label","").startswith(prefix) for t in json.load(sys.stdin).get("tabs",[])))' "$CARD_ID")"
  [ "$count" = 0 ] || fail "expected zero terminal card tabs, got $count"
}

step "Execute→Verification→Blocked uses compact role labels and terminal cleanup"
e2e_board_herdr_mutate -- move "$CARD_ID" Execute --json >/dev/null
EXEC_LABEL="card-$CARD_ID tab lifecycle repair"
VERIFY_LABEL="$EXEC_LABEL verification"
EXEC_TAB="$(wait_label "$EXEC_LABEL")" || fail "executor label not projected"
step "Drift the exact owned label; the next durable rename effect repairs it"
mut "tab.rename exact executor tab $EXEC_TAB to injected-drift"
e2e_herdr_mutate -- tab rename "$EXEC_TAB" injected-drift >/dev/null
VERIFY_TAB="$(wait_label "$VERIFY_LABEL")" || fail "verification label not projected by outbox"
[ "$VERIFY_TAB" != "$EXEC_TAB" ] || fail "Verification reused the executor tab"
wait_runs "$CARD_ID" 2 >/dev/null || fail "Execute→Verification cycle did not finish"
card="$($BOARD_BIN card show "$CARD_ID" --json)"
python3 - "$card" "$BLOCKED_ID" <<'PY'
import json,sys
v=json.loads(sys.argv[1]); assert v["card"]["column_id"]==int(sys.argv[2]); assert v["card"]["status"]=="idle"
PY
assert_zero_card_tabs

step "Route the next cycle to Human Gate"
brpc column.update "{\"id\":$VERIFY_ID,\"on_success_column_id\":$HUMAN_ID}" >/dev/null
e2e_board_herdr_mutate -- move "$CARD_ID" Execute --json >/dev/null
EXEC_TAB="$(wait_label "$EXEC_LABEL")" || fail "second executor tab not projected"

step "Create one exact historical duplicate and restart; canonical active tab is protected"
stale="$(e2e_herdr_mutate -- tab create --workspace "$WS_ID" --label "legacy-card-$CARD_ID" --no-focus)"
STALE_TAB="$(printf '%s' "$stale" | jget tab_id)"
STALE_PANE="$(printf '%s' "$stale" | jget pane_id)"
e2e_daemon_stop
python3 - "$BOARD_DB" "$CARD_ID" "$STALE_PANE" "$WS_ID" <<'PY'
import sqlite3,sys
db,card,pane,workspace=sys.argv[1:]
conn=sqlite3.connect(db)
source=conn.execute("SELECT column_id,harness,argv_json,prompt_snapshot,system_prompt_snapshot,launch_spec_json,session FROM runs WHERE card_id=? ORDER BY id DESC LIMIT 1",(int(card),)).fetchone()
conn.execute("INSERT INTO runs(card_id,column_id,harness,argv_json,prompt_snapshot,system_prompt_snapshot,launch_spec_json,herdr_workspace_id,herdr_pane_id,session,started_at,ended_at,outcome) VALUES(?,?,?,?,?,?,?,?,?,?,datetime('now'),datetime('now'),'ok')",(int(card),*source[:6],workspace,pane,source[6]))
conn.commit()
PY
e2e_daemon_start
for _ in $(seq 1 50); do
  tabs="$(hrpc tab.list "{\"workspace_id\":\"$WS_ID\"}")"
  if python3 - "$tabs" "$STALE_TAB" "$EXEC_TAB" <<'PY'
import json,sys
ids={t["tab_id"] for t in json.loads(sys.argv[1]).get("tabs",[])}
assert sys.argv[2] not in ids and sys.argv[3] in ids
PY
  then break; fi
  sleep 0.1
done
tabs="$(hrpc tab.list "{\"workspace_id\":\"$WS_ID\"}")"
python3 - "$tabs" "$STALE_TAB" "$EXEC_TAB" <<'PY'
import json,sys
ids={t["tab_id"] for t in json.loads(sys.argv[1]).get("tabs",[])}
assert sys.argv[2] not in ids
assert sys.argv[3] in ids
PY

step "Verification close fails while the private socket is hidden, then restart retries it"
VERIFY_TAB="$(wait_label "$VERIFY_LABEL")" || fail "second Verification tab not projected"
BLOCKED_SOCKET="$E2E_SESSION_SOCKET.close-blocked"
mut "temporarily hide private session socket to force tab.close failure"
mv -- "$E2E_SESSION_SOCKET" "$BLOCKED_SOCKET"
wait_runs "$CARD_ID" 5 >/dev/null || fail "Human Gate cycle did not finish"
card="$($BOARD_BIN card show "$CARD_ID" --json)"
python3 - "$card" "$HUMAN_ID" <<'PY'
import json,sys
v=json.loads(sys.argv[1]); assert v["card"]["column_id"]==int(sys.argv[2]); assert v["card"]["status"]=="idle"
PY
e2e_daemon_stop
python3 - "$BOARD_DB" "$CARD_ID" <<'PY'
import sqlite3,sys
conn=sqlite3.connect(sys.argv[1])
rows=conn.execute("SELECT status FROM effect_outbox WHERE card_id=? AND effect_kind='tab_close' ORDER BY created_at,effect_id",(int(sys.argv[2]),)).fetchall()
assert rows and rows[-1][0]=='failed'
PY
mv -- "$BLOCKED_SOCKET" "$E2E_SESSION_SOCKET"
e2e_daemon_start
for _ in $(seq 1 50); do
  count="$(hrpc tab.list "{\"workspace_id\":\"$WS_ID\"}" | python3 -c '
import json,sys
prefix=f"card-{sys.argv[1]}"
print(sum(t.get("label","").startswith(prefix) for t in json.load(sys.stdin).get("tabs",[])))' "$CARD_ID")"
  [ "$count" = 0 ] && break
  sleep 0.1
done
assert_zero_card_tabs
python3 - "$BOARD_DB" "$CARD_ID" <<'PY'
import sqlite3,sys
conn=sqlite3.connect(sys.argv[1])
rows=conn.execute("SELECT status FROM effect_outbox WHERE card_id=? AND effect_kind='tab_close' ORDER BY created_at,effect_id",(int(sys.argv[2]),)).fetchall()
assert rows and rows[-1][0]=='acknowledged'
PY
ok "full lifecycle converged through Blocked and Human Gate with exact cleanup and restart retry"
