#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="$ROOT/var/run"
LOG_DIR="$ROOT/var/log"
PY_PID_FILE="$RUN_DIR/python-backend.pid"
RS_PID_FILE="$RUN_DIR/rust-gateway.pid"
PY_LOG="$LOG_DIR/python-backend.log"
RS_LOG="$LOG_DIR/rust-gateway.log"

PY_HOST="${PY_HOST:-127.0.0.1}"
PY_PORT="${PY_PORT:-18080}"
RS_HOST="${RS_HOST:-127.0.0.1}"
RS_PORT="${RS_PORT:-50071}"

mkdir -p "$RUN_DIR" "$LOG_DIR"

if [[ -t 1 ]]; then
  C_RESET="$(printf '\033[0m')"
  C_GREEN="$(printf '\033[32m')"
  C_BGREEN="$(printf '\033[1;32m')"
  C_YELLOW="$(printf '\033[33m')"
  C_RED="$(printf '\033[31m')"
  C_CYAN="$(printf '\033[36m')"
  C_DIM="$(printf '\033[2m')"
else
  C_RESET=""
  C_GREEN=""
  C_BGREEN=""
  C_YELLOW=""
  C_RED=""
  C_CYAN=""
  C_DIM=""
fi

msg() { echo -e "${C_GREEN}$*${C_RESET}"; }
warn() { echo -e "${C_YELLOW}$*${C_RESET}"; }
err() { echo -e "${C_RED}$*${C_RESET}"; }

backend_health() {
  curl -fsS "http://${PY_HOST}:${PY_PORT}/health" >/dev/null 2>&1
}

stream_health() {
  (
    cd "$ROOT" &&
      TC_TLS_ENABLED=false \
      TC_GRPC_ENDPOINT="http://${RS_HOST}:${RS_PORT}" \
      ./rust/terminal-client/target/release/stream_probe >/dev/null 2>&1
  )
}

pid_alive() {
  local pid="$1"
  kill -0 "$pid" >/dev/null 2>&1
}

read_pid() {
  local file="$1"
  if [[ -f "$file" ]]; then
    cat "$file"
  fi
}

kill_pid_graceful_then_hard() {
  local pid="$1"
  local name="$2"
  if ! pid_alive "$pid"; then
    return
  fi
  kill "$pid" >/dev/null 2>&1 || true
  sleep 1
  if pid_alive "$pid"; then
    kill -9 "$pid" >/dev/null 2>&1 || true
  fi
  msg "$name stopped (pid=$pid)"
}

port_listener_pids() {
  local port="$1"
  lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true
}

kill_residual_listeners() {
  local py_pids rs_pids
  py_pids="$(port_listener_pids "$PY_PORT")"
  rs_pids="$(port_listener_pids "$RS_PORT")"

  if [[ -n "$py_pids" ]]; then
    warn "killing residual backend listeners on :$PY_PORT -> $py_pids"
    # shellcheck disable=SC2086
    kill -9 $py_pids >/dev/null 2>&1 || true
  fi
  if [[ -n "$rs_pids" ]]; then
    warn "killing residual gateway listeners on :$RS_PORT -> $rs_pids"
    # shellcheck disable=SC2086
    kill -9 $rs_pids >/dev/null 2>&1 || true
  fi
}

resolve_path() {
  local path="$1"
  if [[ "$path" = /* ]]; then
    echo "$path"
  else
    echo "$ROOT/$path"
  fi
}

audit_db_path() {
  local configured="${AUDIT_SQLITE_PATH:-data/audit/audit_events.sqlite3}"
  resolve_path "$configured"
}

recover_corrupt_audit_db() {
  local db
  db="$(audit_db_path)"
  if [[ ! -f "$db" ]]; then
    return
  fi
  local ts backup
  ts="$(date +%Y%m%d-%H%M%S)"
  backup="${db}.corrupt.${ts}.bak"
  warn "detected corrupted sqlite audit DB; backing up to $backup"
  mv "$db" "$backup"
}

file_size_bytes() {
  local file="$1"
  if [[ -f "$file" ]]; then
    stat -f%z "$file" 2>/dev/null || echo 0
  else
    echo 0
  fi
}

new_log_since_offset_contains() {
  local file="$1"
  local offset="$2"
  local needle="$3"
  if [[ ! -f "$file" ]]; then
    return 1
  fi
  tail -c +"$((offset + 1))" "$file" 2>/dev/null | grep -q "$needle"
}

ensure_binaries() {
  if [[ ! -x "$ROOT/rust/terminal-client/target/release/stream_probe" ]]; then
    warn "building stream_probe..."
    (cd "$ROOT/rust/terminal-client" && cargo build --release --bin stream_probe)
  fi
  if [[ ! -x "$ROOT/rust/vps1-serving/target/release/vps1-serving" ]]; then
    warn "building vps1-serving..."
    (cd "$ROOT/rust/vps1-serving" && cargo build --release)
  fi
  if [[ ! -x "$ROOT/rust/terminal-client/target/release/terminal-client" ]]; then
    warn "building terminal-client..."
    (cd "$ROOT/rust/terminal-client" && cargo build --release)
  fi
}

start_backend() {
  if backend_health; then
    msg "backend already healthy on ${PY_HOST}:${PY_PORT}"
    return
  fi

  local stale_pid
  stale_pid="$(read_pid "$PY_PID_FILE" || true)"
  if [[ -n "$stale_pid" ]] && pid_alive "$stale_pid"; then
    warn "backend process exists but health failed (pid=$stale_pid); restarting"
    kill_pid_graceful_then_hard "$stale_pid" "backend"
  fi

  local attempt
  for attempt in 1 2; do
    local log_offset
    log_offset="$(file_size_bytes "$PY_LOG")"

    msg "starting backend (attempt $attempt)..."
    (
      cd "$ROOT"
      nohup env PYTHONPATH=src python3 -m quant_platform.main --host "$PY_HOST" --port "$PY_PORT" >>"$PY_LOG" 2>&1 < /dev/null &
      echo $! >"$PY_PID_FILE"
    )

    sleep 1
    if backend_health; then
      msg "backend started"
      return
    fi

    if new_log_since_offset_contains "$PY_LOG" "$log_offset" "database disk image is malformed"; then
      recover_corrupt_audit_db
      local maybe_pid
      maybe_pid="$(read_pid "$PY_PID_FILE" || true)"
      if [[ -n "$maybe_pid" ]] && pid_alive "$maybe_pid"; then
        kill_pid_graceful_then_hard "$maybe_pid" "backend"
      fi
      rm -f "$PY_PID_FILE"
      continue
    fi
    break
  done

  rm -f "$PY_PID_FILE"
  err "backend failed to start"
  tail -n 40 "$PY_LOG" || true
  exit 1
}

start_gateway() {
  if stream_health; then
    msg "gateway stream healthy on ${RS_HOST}:${RS_PORT}"
    return
  fi

  local stale_pid
  stale_pid="$(read_pid "$RS_PID_FILE" || true)"
  if [[ -n "$stale_pid" ]] && pid_alive "$stale_pid"; then
    warn "gateway process exists but stream check failed (pid=$stale_pid); restarting"
    kill_pid_graceful_then_hard "$stale_pid" "gateway"
  fi

  msg "starting gateway..."
  (
    cd "$ROOT"
    nohup env \
      RUST_REQUIRE_MTLS=false \
      RUST_AUDIT_ENABLED=false \
      RUST_GATEWAY_HOST="$RS_HOST" \
      RUST_GATEWAY_PORT="$RS_PORT" \
      PYTHON_BACKEND_URL="http://${PY_HOST}:${PY_PORT}" \
      ./rust/vps1-serving/target/release/vps1-serving >>"$RS_LOG" 2>&1 < /dev/null &
    echo $! >"$RS_PID_FILE"
  )

  sleep 1
  if stream_health; then
    msg "gateway started"
  else
    err "gateway failed to start"
    tail -n 40 "$RS_LOG" || true
    exit 1
  fi
}

start_all() {
  ensure_binaries
  start_backend
  start_gateway
  msg "stack is up"
}

stop_one() {
  local file="$1"
  local name="$2"
  local pid
  pid="$(read_pid "$file" || true)"
  if [[ -z "$pid" ]]; then
    warn "$name not running (no pid file)"
    return
  fi

  if pid_alive "$pid"; then
    kill_pid_graceful_then_hard "$pid" "$name"
  else
    warn "$name pid file was stale"
  fi

  rm -f "$file"
}

stop_all() {
  stop_one "$RS_PID_FILE" "gateway"
  stop_one "$PY_PID_FILE" "backend"
  kill_residual_listeners
}

restart_all() {
  stop_all
  start_all
}

kill_all() {
  warn "force killing all project runtime processes..."
  stop_all
  # Best effort cleanup by process names as fallback.
  pkill -9 -f "quant_platform.main" >/dev/null 2>&1 || true
  pkill -9 -f "/rust/vps1-serving/target/release/vps1-serving" >/dev/null 2>&1 || true
  pkill -9 -f "/rust/terminal-client/target/release/terminal-client" >/dev/null 2>&1 || true
  rm -f "$PY_PID_FILE" "$RS_PID_FILE"
  msg "kill-all complete"
}

status() {
  local b="down"
  local g="down"
  local bcolor="$C_RED"
  local gcolor="$C_RED"
  if backend_health; then
    b="healthy"
    bcolor="$C_GREEN"
  fi
  if stream_health; then
    g="healthy"
    gcolor="$C_GREEN"
  fi

  echo -e "backend: ${bcolor}${b}${C_RESET} (${PY_HOST}:${PY_PORT})"
  echo -e "gateway: ${gcolor}${g}${C_RESET} (${RS_HOST}:${RS_PORT})"
  echo "backend pid: $(read_pid "$PY_PID_FILE" || echo '-')"
  echo "gateway pid: $(read_pid "$RS_PID_FILE" || echo '-')"
}

doctor() {
  echo "== status =="
  status
  echo
  echo "== listeners =="
  lsof -nP -iTCP:"$PY_PORT" -sTCP:LISTEN || true
  lsof -nP -iTCP:"$RS_PORT" -sTCP:LISTEN || true
  echo
  echo "== backend log (tail) =="
  tail -n 40 "$PY_LOG" || true
  echo
  echo "== gateway log (tail) =="
  tail -n 40 "$RS_LOG" || true
}

tail_logs() {
  msg "tailing logs (Ctrl+C to return)"
  touch "$PY_LOG" "$RS_LOG"
  tail -n 50 -f "$PY_LOG" "$RS_LOG"
}

run_client() {
  if ! backend_health || ! stream_health; then
    warn "stack not healthy; starting first..."
    start_all
  fi

  (
    cd "$ROOT"
    env \
      TC_TLS_ENABLED=false \
      TC_GRPC_ENDPOINT="http://${RS_HOST}:${RS_PORT}" \
      TC_REFRESH_SECONDS=15 \
      ./rust/terminal-client/target/release/terminal-client
  )
}

draw_header() {
  clear
  echo -e "${C_BGREEN}╔══════════════════════════════════════════════════════════════════════╗${C_RESET}"
  echo -e "${C_BGREEN}║                 QUANT TERMINAL CONTROL PANEL (OPS)                  ║${C_RESET}"
  echo -e "${C_BGREEN}╚══════════════════════════════════════════════════════════════════════╝${C_RESET}"
  echo -e "${C_DIM}project:${C_RESET} $ROOT"
  echo -e "${C_DIM}time:${C_RESET}    $(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo
  status
  echo
}

menu() {
  while true; do
    draw_header
    echo -e "${C_CYAN}[S]${C_RESET} Start stack      ${C_CYAN}[T]${C_RESET} Stop stack"
    echo -e "${C_CYAN}[R]${C_RESET} Restart stack    ${C_CYAN}[K]${C_RESET} Kill all processes"
    echo -e "${C_CYAN}[C]${C_RESET} Launch client    ${C_CYAN}[D]${C_RESET} Doctor"
    echo -e "${C_CYAN}[L]${C_RESET} Tail logs        ${C_CYAN}[Q]${C_RESET} Exit"
    echo
    read -r -n1 -p "Select action: " choice
    echo
    local choice_lc
    choice_lc="$(printf '%s' "$choice" | tr '[:upper:]' '[:lower:]')"
    case "$choice_lc" in
      s) start_all ;;
      t) stop_all ;;
      r) restart_all ;;
      k) kill_all ;;
      c) run_client ;;
      d) doctor ;;
      l) tail_logs ;;
      q) break ;;
      *) warn "invalid option" ;;
    esac
    echo
    read -r -p "Press Enter to continue... " _
  done
}

cmd="${1:-menu}"
case "$cmd" in
  start) start_all ;;
  stop) stop_all ;;
  restart) restart_all ;;
  kill-all|killall|kill) kill_all ;;
  status) status ;;
  doctor) doctor ;;
  logs) tail_logs ;;
  client) run_client ;;
  menu) menu ;;
  *)
    echo "usage: $0 [start|stop|restart|kill-all|status|doctor|logs|client|menu]"
    exit 2
    ;;
esac
