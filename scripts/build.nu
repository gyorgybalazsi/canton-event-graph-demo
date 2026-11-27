#!/usr/bin/env nu
# nu scripts/build.nu <label> <command...>: runs a cargo (or rustup) command
# quietly but shows where it is: one line that updates in place in a terminal,
# such as
#   ⧗ syncer v0.1.0: compiling, 213 crates so far, now wasmtime · 1m40s
# and ends with a ✓ line, or ✗ and the errors from the build log on a failure.
# Without a terminal it prints a line per phase instead. A build with nothing
# to do prints nothing. The full output of the last build is in
# .demo/logs/build.log.

# What a line of cargo or rustup output says about the build.
def phase-of [line: string] {
    if ($line =~ 'info: (downloading|installing) component|info: syncing channel') {
        "installing the Rust toolchain or target"
    } else if ($line =~ '^\s*(Updating|Fetching|Locking) ') {
        "fetching sources"
    } else if ($line =~ '^\s*(Downloading|Downloaded) ') {
        "downloading crates"
    } else if ($line =~ '^\s*Compiling ') {
        "compiling"
    } else if ($line =~ '^\s*Finished ') {
        "linked"
    } else if ($line =~ '^\s*(Installing|Replacing) ') {
        "installing"
    } else {
        null
    }
}

def crates [n: int] { if $n == 1 { "1 crate" } else { $"($n) crates" } }

def describe [state: record] {
    if $state.phase == "compiling" {
        $"compiling, (crates $state.crates) so far, now ($state.current)"
    } else {
        $state.phase
    }
}

def elapsed [start: datetime] {
    let s = ((date now) - $start) / 1sec | math floor
    $"($s // 60)m($s mod 60 | fill --alignment right --character '0' --width 2)s"
}

# Folds new log lines into the state.
def update [state: record, lines: list<string>] {
    $lines | reduce --fold $state {|line, acc|
        let phase = (phase-of $line)
        if $phase == null {
            $acc
        } else if $phase == "compiling" {
            let name = ($line | str trim | split row " " | get 1)
            $acc | merge {phase: $phase, crates: ($acc.crates + 1), current: $name}
        } else {
            $acc | merge {phase: $phase}
        }
    }
}

# The repository: this file's folder's parent, fixed when the script is parsed.
const root = path self ..

@example "install the syncer at its release tag" { nu scripts/build.nu "syncer v0.1.0" cargo install --locked --git https://github.com/gyorgybalazsi/canton-event-graph-syncer --tag v0.1.0 syncer }
def --wrapped main [label: string, ...args] {
    # Arguments that look like numbers or booleans arrive typed: back to text.
    let command = ($args | each { into string })
    let logs = ($root | path join .demo logs)
    mkdir $logs
    let log = ($logs | path join build.log)
    "" | save -f $log

    # Cargo's own progress bar and colours would garble the line and the log.
    $env.CARGO_TERM_COLOR = "never"
    $env.CARGO_TERM_PROGRESS_WHEN = "never"
    let tty = (is-terminal --stderr)
    let start = (date now)

    # The build runs as a background job writing to the log and mails its exit
    # code to this thread (job 0) when it ends. Until then this loop reads the
    # log twice a second, so the clock moves on during a long link too.
    job spawn --auto-kill --description $label {
        let code = (try { ^($command | first) ...($command | skip 1) o+e> $log; 0 } catch {|err| $err.exit_code? | default 1 })
        {exit_code: $code} | job send 0
    } | ignore

    mut state = {phase: "starting", crates: 0, current: "", seen: 0, said: ""}
    mut code: any = null
    loop {
        $code = (try { job recv --timeout 500ms | get exit_code } catch { null })
        let done = $code != null
        let lines = (open --raw $log | decode utf-8 | lines)
        let new = ($lines | skip $state.seen)
        let before = $state.crates
        $state = (update $state $new | merge {seen: ($lines | length)})
        if $tty {
            if $state.phase != "starting" {
                print -e -n $"\r\e[K⧗ ($label): (describe $state) · (elapsed $start)"
            }
        } else if (($state.phase != $state.said and $state.phase not-in ["starting" "linked"])
                or ($before // 100) != ($state.crates // 100)) {
            # Without a terminal: a line when the phase changes, and one per 100 crates.
            print -e $"⧗ ($label): (describe $state) · (elapsed $start)"
            $state = ($state | merge {said: $state.phase})
        }
        if $done { break }
    }
    if $tty { print -e -n "\r\e[K" }

    if $code != 0 {
        print -e $"✗ ($label) failed after (elapsed $start). The end of .demo/logs/build.log, without the progress lines:"
        let errors = (open --raw $log | decode utf-8 | lines
            | where {|line| (phase-of $line) == null or $line =~ 'error|warning' }
            | last 30 | str join "\n")
        print -e $errors
        exit ($code | into int)
    }
    if $state.crates > 0 {
        print -e $"✓ ($label): built in (elapsed $start) \((crates $state.crates)\)"
    } else if $state.phase == "installing the Rust toolchain or target" {
        print -e $"✓ ($label): done in (elapsed $start)"
    }
}
