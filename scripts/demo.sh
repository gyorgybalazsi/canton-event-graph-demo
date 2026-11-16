#!/usr/bin/env bash
# Starts, waits for and stops the demo's background services (sandbox,
# SurrealDB, syncer). Each one runs with its output in .demo/logs/<name>.log
# and its process id in .demo/run/<name>.pid. Works on macOS and Linux.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
logs="$root/.demo/logs"
run="$root/.demo/run"
mkdir -p "$logs" "$run"

die() { echo "error: $*" >&2; exit 1; }

# Whether something listens on 127.0.0.1:<port>.
listening() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

running() {
    local pidfile="$run/$1.pid"
    [[ -f "$pidfile" ]] && kill -0 "$(cat "$pidfile")" 2>/dev/null
}

# start <name> <dir> <command...>: runs the command in <dir> in the background.
start() {
    local name="$1" dir="$2"
    shift 2
    if running "$name"; then
        echo "$name is already running (pid $(cat "$run/$name.pid"))"
        return
    fi
    (cd "$dir" && exec nohup "$@" >"$logs/$name.log" 2>&1) &
    echo $! >"$run/$name.pid"
    echo "started $name (pid $(cat "$run/$name.pid")), log: .demo/logs/$name.log"
}

# wait-log <name> <text> <seconds>: waits until the log contains the text.
wait_log() {
    local name="$1" text="$2" seconds="$3"
    for _ in $(seq "$seconds"); do
        grep -qF "$text" "$logs/$name.log" 2>/dev/null && return
        running "$name" || { tail -n 20 "$logs/$name.log" >&2; die "$name stopped while starting"; }
        sleep 1
    done
    tail -n 20 "$logs/$name.log" >&2
    die "$name did not print \"$text\" within $seconds s"
}

# wait-http <name> <url> <seconds>: waits until the URL answers.
wait_http() {
    local name="$1" url="$2" seconds="$3"
    for _ in $(seq "$seconds"); do
        curl -sf "$url" >/dev/null && return
        running "$name" || { tail -n 20 "$logs/$name.log" >&2; die "$name stopped while starting"; }
        sleep 1
    done
    die "$name did not answer at $url within $seconds s"
}

# Kills a process and everything it started (dpm starts a JVM, for example).
kill_tree() {
    local pid="$1" child
    for child in $(pgrep -P "$pid" 2>/dev/null); do kill_tree "$child"; done
    kill "$pid" 2>/dev/null || true
}

stop() {
    local name="$1" pidfile="$run/$1.pid"
    if running "$name"; then
        kill_tree "$(cat "$pidfile")"
        echo "stopped $name"
    fi
    rm -f "$pidfile"
}

status() {
    local name
    for name in sandbox surreal syncer; do
        if running "$name"; then
            echo "$name: running (pid $(cat "$run/$name.pid"))"
        else
            echo "$name: stopped"
        fi
    done
}

cmd="${1:-}"
shift || true
case "$cmd" in
    listening) listening "$@" ;;
    start) start "$@" ;;
    wait-log) wait_log "$@" ;;
    wait-http) wait_http "$@" ;;
    stop) stop "$@" ;;
    status) status ;;
    *) die "usage: $0 listening|start|wait-log|wait-http|stop|status ..." ;;
esac
