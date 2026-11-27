#!/usr/bin/env bash
# build.sh <label> <command...>: runs a cargo (or rustup) command quietly but
# shows where it is: one line that updates in place in a terminal, such as
#   ⧗ syncer v0.1.0: compiling, 213 crates so far, now wasmtime · 1m40s
# and ends with a ✓ line, or ✗ and the end of the build log on a failure.
# Without a terminal it prints a line per milestone instead. A build with
# nothing to do prints nothing. The full output of the last build is in
# .demo/logs/build.log. Works with macOS's bash 3.2 (so no `read -t`, whose
# timeout status differs there).
set -uo pipefail

label="$1"
shift
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root/.demo/logs"
log="$root/.demo/logs/build.log"

start=$SECONDS
tty=0
[[ -t 2 ]] && tty=1
crates=0
current=""
phase="starting"
said=""

elapsed() {
    local s=$((SECONDS - start))
    printf '%dm%02ds' $((s / 60)) $((s % 60))
}

describe() {
    case "$phase" in
        compiling)
            local n="$crates crates"
            ((crates == 1)) && n="1 crate"
            echo "compiling, $n so far, now $current" ;;
        *) echo "$phase" ;;
    esac
}

show() {
    if ((tty)); then
        printf '\r\033[K⧗ %s: %s · %s' "$label" "$(describe)" "$(elapsed)" >&2
    elif [[ "$phase" != "$said" && "$phase" != "linked" && "$phase" != "starting" ]] || ((phase_milestone)); then
        # Without a terminal: one line when the phase changes, and one per 100 crates.
        echo "⧗ $label: $(describe) · $(elapsed)" >&2
        said="$phase"
    fi
}

# Cargo's own progress bar and colours would garble the line and the log.
export CARGO_TERM_COLOR=never CARGO_TERM_PROGRESS_WHEN=never

# Reads the log lines that arrived since the last call and updates the phase.
seen=0
take_new_lines() {
    local total line
    total=$(wc -l <"$log" | tr -d ' ')
    ((total > seen)) || return 0
    while IFS= read -r line; do
        case "$line" in
            *"info: downloading component"* | *"info: installing component"* | *"info: syncing channel"*)
                phase="installing the Rust toolchain or target" ;;
            *"Updating "* | *"Fetching "* | *"Locking "*) phase="fetching sources" ;;
            *"Downloading "* | *"Downloaded "*) phase="downloading crates" ;;
            *"Compiling "*)
                phase="compiling"
                crates=$((crates + 1))
                current="${line#*Compiling }"
                current="${current%% *}"
                if ((crates % 100 == 0)); then phase_milestone=1; show; phase_milestone=0; fi ;;
            *"Finished "*) phase="linked" ;;
            *"Installing "* | *"Replacing "*) phase="installing" ;;
        esac
    done < <(sed -n "$((seen + 1)),${total}p" "$log")
    seen=$total
}

# The build runs in the background, writing to the log; this loop polls the
# log twice a second (no `read -t`, whose timeout status differs in bash 3.2).
phase_milestone=0
"$@" >"$log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null; ((tty)) && printf "\r\033[K" >&2; exit 130' INT TERM
while kill -0 "$pid" 2>/dev/null; do
    take_new_lines
    show
    sleep 0.5
done
wait "$pid"
status=$?
trap - INT TERM
take_new_lines

((tty)) && printf '\r\033[K' >&2
if [[ "$status" != 0 ]]; then
    echo "✗ $label failed after $(elapsed). The end of .demo/logs/build.log, without the progress lines:" >&2
    grep -vE '^ *(Updating|Fetching|Locking|Adding|Downloading|Downloaded|Compiling|Checking|Fresh) ' "$log" | tail -n 30 >&2
    exit "$status"
fi
if ((crates > 0)); then
    n="$crates crates"
    ((crates == 1)) && n="1 crate"
    echo "✓ $label: built in $(elapsed) ($n)" >&2
elif [[ "$phase" == "installing the Rust toolchain or target" ]]; then
    echo "✓ $label: done in $(elapsed)" >&2
fi
