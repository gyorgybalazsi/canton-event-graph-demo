#!/usr/bin/env bash
# build.sh <label> <command...>: runs a cargo (or rustup) command quietly but
# shows where it is: one line that updates in place in a terminal, such as
#   ⧗ syncer v0.1.0: compiling, 213 crates so far, now wasmtime · 1m40s
# and ends with a ✓ line, or ✗ and the end of the build log on a failure.
# Without a terminal it prints a line per milestone instead. A build with
# nothing to do prints nothing. The full output of the last build is in
# .demo/logs/build.log. Works with macOS's bash 3.2.
set -uo pipefail

label="$1"
shift
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root/.demo/logs"
log="$root/.demo/logs/build.log"
: >"$log"

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
    elif [[ "$phase" != "$said" && "$phase" != "linked" ]] || ((phase_milestone)); then
        # Without a terminal: one line when the phase changes, and one per 100 crates.
        echo "⧗ $label: $(describe) · $(elapsed)" >&2
        said="$phase"
    fi
}

# Cargo's own progress bar and colours would garble the line and the log.
export CARGO_TERM_COLOR=never CARGO_TERM_PROGRESS_WHEN=never

exec 3< <("$@" 2>&1; echo "__build_status=$?")
status=1
while true; do
    phase_milestone=0
    if IFS= read -r -t 1 line <&3; then
        case "$line" in
            __build_status=*) status="${line#__build_status=}"; break ;;
        esac
        echo "$line" >>"$log"
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
                ((crates % 100 == 0)) && phase_milestone=1 ;;
            *"Finished "*) phase="linked" ;;
            *"Installing "* | *"Replacing "*) phase="installing" ;;
            *) continue ;;
        esac
        show
    elif (($? > 128)); then
        # No new line within a second (linking, a big crate): keep the clock moving.
        ((tty)) && show
    else
        break
    fi
done
exec 3<&-

((tty)) && printf '\r\033[K' >&2
if [[ "$status" != 0 ]]; then
    echo "✗ $label failed after $(elapsed). The end of .demo/logs/build.log, without the progress lines:" >&2
    grep -vE '^ *(Updating|Fetching|Locking|Adding|Downloading|Downloaded|Compiling|Checking|Fresh) ' "$log" | tail -n 30 >&2
    exit "$status"
fi
if ((crates > 0)); then
    echo "✓ $label: built in $(elapsed) ($crates crates)" >&2
elif [[ "$phase" == "installing the Rust toolchain or target" ]]; then
    echo "✓ $label: done in $(elapsed)" >&2
fi
