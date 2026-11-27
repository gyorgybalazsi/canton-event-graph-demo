#!/usr/bin/env nu
# Starts, waits for and stops the demo's background services (sandbox,
# SurrealDB, syncer, explorer, analytics, dashboard, bench-surreal), and runs
# SurrealQL on the demo's server. Each service runs with its output in
# .demo/logs/<name>.log and its process id in .demo/run/<name>.pid.
#
#   nu scripts/demo.nu start <name> <dir> <command...>
#   nu scripts/demo.nu wait-log <name> <text> <seconds>
#   nu scripts/demo.nu wait-http <name> <url> <seconds>
#   nu scripts/demo.nu stop <name>
#   nu scripts/demo.nu status
#   nu scripts/demo.nu running <name>      (exit code 0 if it runs)
#   nu scripts/demo.nu listening <port>    (exit code 0 if something listens)
#   nu scripts/demo.nu check-port <port> <name> <hint>   (fails if something
#                                          other than <name> listens there)
#   nu scripts/demo.nu sql --url <http> --pass <pw> <query>   (JSON on stdout)

const services = [sandbox surreal syncer explorer analytics dashboard bench-surreal]

# The repository: this file's folder's parent, fixed when the script is parsed.
const root = path self ..
const logs = ($root | path join .demo logs)
const run = ($root | path join .demo run)

def log-file [name: string] { $logs | path join $"($name).log" }
def pid-file [name: string] { $run | path join $"($name).pid" }

def die [message: string] {
    print -e $"error: ($message)"
    exit 1
}

def pid-of [name: string] {
    let file = (pid-file $name)
    if not ($file | path exists) { return null }
    open --raw $file | decode utf-8 | str trim | into int
}

def is-running [name: string] {
    let pid = (pid-of $name)
    $pid != null and (ps | where pid == $pid | is-not-empty)
}

# Whether something listens on 127.0.0.1:<port>. curl reports "couldn't
# connect" (exit code 7) when nothing does.
def is-listening [port: int] {
    ("" | ^curl -s --max-time 2 $"telnet://127.0.0.1:($port)" | complete).exit_code != 7
}

# A process and everything it started (dpm starts a JVM, for example).
def tree [pid: int] {
    let all = (ps | select pid ppid)
    mut found = [$pid]
    mut frontier = [$pid]
    while ($frontier | is-not-empty) {
        let parents = $frontier
        let children = ($all | where ppid in $parents | get pid)
        $found = ($found | append $children)
        $frontier = $children
    }
    $found
}

def show-log-tail [name: string] {
    let file = (log-file $name)
    if ($file | path exists) {
        print -e (open --raw $file | decode utf-8 | lines | last 20 | str join "\n")
    }
}

def log-contains [name: string, text: string] {
    let file = (log-file $name)
    ($file | path exists) and (open --raw $file | decode utf-8 | str contains $text)
}

# The demo's background services; `nu scripts/demo.nu --help` lists the commands.
def main [] {
    help main
}

# Runs a command in <dir> in the background, detached from this shell.
@category service
@example "start SurrealDB" { nu scripts/demo.nu start surreal . surreal db start --bind 127.0.0.1:8001 memory }
def --wrapped "main start" [name: string, dir: string, ...args] {
    # Arguments that look like numbers or booleans arrive typed: back to text.
    let command = ($args | each { into string })
    mkdir $logs $run
    if (is-running $name) {
        print $"($name) is already running \(pid (pid-of $name)\)"
        return
    }
    # The one job Nushell can't do: start a process that outlives it. `sh`
    # only backgrounds it and prints its pid.
    let pid = (^sh -c '(cd "$1" && shift 2 && exec nohup "$@" < /dev/null > "$0" 2>&1) & echo $!' (log-file $name) $dir $name ...$command | str trim | into int)
    $pid | into string | save -f (pid-file $name)
    print $"started ($name) \(pid ($pid)\), log: .demo/logs/($name).log"
}

# Waits until the service's log contains the text.
@category service
@example "wait for the sandbox" { nu scripts/demo.nu wait-log sandbox "Canton sandbox is ready." 180 }
def "main wait-log" [name: string, text: string, seconds: int] {
    for _ in 1..$seconds {
        if (log-contains $name $text) { return }
        if not (is-running $name) {
            show-log-tail $name
            die $"($name) stopped while starting"
        }
        sleep 1sec
    }
    show-log-tail $name
    die $"($name) did not print \"($text)\" within ($seconds) s"
}

# Waits until the URL answers.
@category service
def "main wait-http" [name: string, url: string, seconds: int] {
    for _ in 1..$seconds {
        let up = (try { http get $url | ignore; true } catch { false })
        if $up { return }
        if not (is-running $name) {
            show-log-tail $name
            die $"($name) stopped while starting"
        }
        sleep 1sec
    }
    die $"($name) did not answer at ($url) within ($seconds) s"
}

# Stops a service and waits until all its processes have exited, so its ports
# are free when this returns (the sandbox's JVM takes a few seconds).
@category service
def "main stop" [name: string] {
    if (is-running $name) {
        let pids = (tree (pid-of $name))
        for pid in $pids { try { kill $pid } }
        mut alive = true
        for _ in 1..30 {
            if (ps | where pid in $pids | is-empty) {
                $alive = false
                break
            }
            sleep 1sec
        }
        if $alive {
            for pid in $pids { try { kill --force $pid } }
        }
        print $"stopped ($name)"
    }
    rm -f (pid-file $name)
}

# Which services run, as a table.
@category service
def "main status" [] {
    $services | each {|name|
        let running = (is-running $name)
        {service: $name, state: (if $running { "running" } else { "stopped" }), pid: (if $running { pid-of $name } else { null })}
    } | print
}

# Exits with 1 unless the service runs.
@category service
def "main running" [name: string] {
    if not (is-running $name) { exit 1 }
}

# Exits with 1 unless something listens on 127.0.0.1:<port>.
@category port
def "main listening" [port: int] {
    if not (is-listening $port) { exit 1 }
}

# Fails when something other than the named service listens on the port.
@category port
@example "before starting SurrealDB" { nu scripts/demo.nu check-port 8001 surreal "set DEMO_SURREAL_PORT" }
def "main check-port" [port: int, name: string, hint: string] {
    if (is-listening $port) and not (is-running $name) {
        die $"something else already listens on port ($port) \(($hint)\)"
    }
}

# Runs SurrealQL as root and prints the statements' results as JSON; fails on
# an error in any statement.
@category surrealdb
@example "count the transactions" { nu scripts/demo.nu sql --url http://127.0.0.1:8001 --pass root "USE NS canton DB ticket_purchase; RETURN count(SELECT * FROM transaction);" }
def "main sql" [query: string, --url: string, --pass: string] {
    let results = (
        http post --user root --password $pass --headers {Accept: application/json}
            $"($url)/sql" $query
    )
    let errors = ($results | where status == "ERR")
    if ($errors | is-not-empty) {
        print -e ($errors | to json)
        exit 1
    }
    $results | to json --raw
}
