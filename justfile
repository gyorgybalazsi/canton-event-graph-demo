# End-to-end ticket purchase demo: Canton sandbox → Rust client → syncer →
# SurrealDB → surrealdb-explorer. `just demo` runs it all; see README.md.

# Recipes run in Nushell: one-line ones with `nu -c`, longer ones as Nushell
# scripts (the `#!/usr/bin/env -S nu` recipes).
set shell := ["nu", "--no-config-file", "-c"]

# The tools come from their own repositories, pinned to their release tags.
# Bump a tag to try a newer version (and the syncer crate tags in
# app/Cargo.toml and analytics/bench/Cargo.toml together with syncer_tag).
# The `analytics` recipes install canton-analytics and a dashboard-enabled
# explorer from the same syncer and explorer tags.
syncer_git := "https://github.com/gyorgybalazsi/canton-event-graph-syncer"
syncer_tag := "v0.2.0"
codegen_git := "https://github.com/gyorgybalazsi/daml-rust-codegen"
codegen_tag := "v0.1.0"
explorer_git := "https://github.com/gyorgybalazsi/surrealdb-explorer"
explorer_tag := "v0.2.0"
# Set EXPLORER_PATH to a local clone of surrealdb-explorer to install from it
# instead of GitHub (for example to try local changes).
explorer_path := env("EXPLORER_PATH", "")

demo := justfile_directory() / ".demo"
bin := demo / "bin"
# Starts, waits for and stops the background services; see scripts/demo.nu.
svc := "nu --no-config-file " + quote(justfile_directory() / "scripts/demo.nu")
# Runs a build with one progress line instead of cargo's full output.
build := "nu --no-config-file " + quote(justfile_directory() / "scripts/build.nu")
dar := justfile_directory() / "daml/.daml/dist/ticket-purchase-0.1.0.dar"

analytics := demo / "analytics"
analytics_bin := analytics / "bin"
plugins := analytics / "plugins"

ledger_port := "6865"
# Not 8000, so the demo doesn't collide with a SurrealDB server you already run.
surreal_port := env("DEMO_SURREAL_PORT", "8001")
surreal_http := "http://127.0.0.1:" + surreal_port
# The benchmark's own SurrealDB, apart from the demo's
bench_surreal_port := env("DEMO_BENCH_SURREAL_PORT", "8002")
# Demo-only credentials for the throwaway in-memory server on 127.0.0.1.
surreal_root_pass := "root"
syncer_pass := "syncer"
# Seconds `just demo` waits after each transaction of the first, guided purchase.
guided_pause := env("DEMO_PAUSE", "4")
# Runs SurrealQL as root on the demo server, prints the results as JSON and
# fails on an error.
sql := svc + " sql --url " + surreal_http + " --pass " + surreal_root_pass
# The Nushell the recipes are tested with; older ones may lack what they use.
nu_min := "0.117.0"

# List the recipes
default:
    @just --list --unsorted

# Run the whole demo: start everything, open the explorer on the empty graph, buy one ticket step by step, then two more
demo: tools surreal explorer build-dar sandbox setup sync
    @# SurrealDB and the explorer come first, so the explorer is open on the empty
    @# graph and everything after it, setup included, shows up as it happens.
    @just purchase 1 {{guided_pause}}
    @just purchase 2

# Stop everything and delete the demo's data and logs (keeps the installed tools)
demo-reset: stop
    @rm -rf '{{demo}}/logs' '{{demo}}/run' '{{demo}}/explorer' '{{analytics}}/tables' '{{analytics}}/explorer'
    @print "Reset. The sandbox and SurrealDB kept their data in memory, so it is gone too."

# Install the syncer, the code generator and the explorer into .demo/bin (skips what is already installed)
tools: _nu-version
    #!/usr/bin/env -S nu --no-config-file
    mkdir '{{bin}}'
    # cargo's install record: a tool at the pinned tag (or the explorer
    # installed from a local clone) is not fetched again.
    let record = ('{{demo}}/.crates.toml' | if ($in | path exists) { open --raw $in } else { "" })
    if not ($record =~ 'syncer .*tag={{syncer_tag}}') {
        {{build}} "syncer {{syncer_tag}}" cargo install --locked --root '{{demo}}' --git {{syncer_git}} --tag {{syncer_tag}} syncer
    }
    if not ($record =~ 'daml-rust-codegen .*tag={{codegen_tag}}') {
        {{build}} "daml-rust-codegen {{codegen_tag}}" cargo install --locked --root '{{demo}}' --git {{codegen_git}} --tag {{codegen_tag}} daml-rust-codegen
    }
    if ('{{explorer_path}}' | is-not-empty) {
        {{build}} "surrealdb-explorer from {{explorer_path}}" cargo install --locked --force --root '{{demo}}' --path '{{explorer_path}}'
    } else if not ($record =~ 'surrealdb-explorer .*(tag={{explorer_tag}}|path\+)') {
        {{build}} "surrealdb-explorer {{explorer_tag}}" cargo install --locked --root '{{demo}}' --git {{explorer_git}} --tag {{explorer_tag}} surrealdb-explorer
    }

# Build the ticket purchase DAR (needs the Daml SDK's dpm)
build-dar:
    #!/usr/bin/env -S nu --no-config-file
    print "▶ Building the ticket purchase model (daml/ → ticket-purchase-0.1.0.dar)"
    mkdir '{{demo}}'
    let result = (do { cd daml; ^dpm build } | complete)
    $result.stdout + $result.stderr | save -f '{{demo}}/build-dar.log'
    if $result.exit_code != 0 {
        print -e ($result.stdout + $result.stderr)
        exit 1
    }

# Start a Canton sandbox on port 6865 in the background
sandbox:
    @print "▶ Starting the Canton sandbox (port {{ledger_port}})"
    @{{svc}} check-port {{ledger_port}} sandbox "another sandbox?"
    @{{svc}} start sandbox daml dpm sandbox
    @{{svc}} wait-log sandbox "Canton sandbox is ready." 180

# Start an in-memory SurrealDB server and create the syncer's database user
surreal:
    #!/usr/bin/env -S nu --no-config-file
    print "▶ Starting SurrealDB (port {{surreal_port}}) and creating the syncer's database user"
    {{svc}} check-port {{surreal_port}} surreal "set DEMO_SURREAL_PORT"
    # A fresh in-memory database starts the ledger over, so the analytics
    # tables made from the last one no longer match it: start them over too.
    if (({{svc}} running surreal | complete).exit_code != 0) { rm -rf '{{analytics}}/tables' }
    {{svc}} start surreal . surreal db start --user root --pass {{surreal_root_pass}} --bind 127.0.0.1:{{surreal_port}} memory
    {{svc}} wait-http surreal {{surreal_http}}/health 30
    {{sql}} "DEFINE NAMESPACE IF NOT EXISTS canton; USE NS canton; DEFINE DATABASE IF NOT EXISTS ticket_purchase; USE DB ticket_purchase; DEFINE USER IF NOT EXISTS syncer ON DATABASE PASSWORD '{{syncer_pass}}' ROLES EDITOR;" | ignore

# Upload the DAR and allocate Alice, TicketWizard and ScroogeBank
setup:
    @print "▶ Uploading the model and allocating Alice, TicketWizard and ScroogeBank"
    @{{build}} "the ticket purchase app" cargo build -p ticket-purchase
    @cargo run --quiet -p ticket-purchase -- setup --dar '{{dar}}'

# Start the syncer in the background, following every party on the sandbox
sync:
    @print "▶ Starting the syncer: ledger → SurrealDB"
    @SURREAL_URL=ws://127.0.0.1:{{surreal_port}} SURREAL_PASSWORD={{syncer_pass}} {{svc}} start syncer . '{{bin}}/syncer' sync --config-file config/syncer.toml
    @{{svc}} wait-log syncer "Connected to SurrealDB" 60

# Buy tickets; `pause` waits that many seconds after each transaction
purchase count="3" pause="0":
    @print "▶ Buying {{count}} ticket(s)"
    @cargo run --quiet -p ticket-purchase -- purchase --count {{count}} --pause {{pause}}

# Open the explorer on the demo database, in the background
explorer:
    @print "▶ Opening the explorer (Canton view, refreshes by itself)"
    @just _explorer-config
    @{{svc}} start explorer '{{demo}}/explorer' '{{bin}}/surrealdb-explorer'
    @sleep 3sec

# Copy the explorer's settings into .demo/explorer on the first run; later runs keep what its Settings saved
_explorer-config:
    #!/usr/bin/env -S nu --no-config-file
    mkdir '{{demo}}/explorer'
    if not ('{{demo}}/explorer/config.toml' | path exists) {
        open --raw config/explorer.toml
            | str replace 'ws://127.0.0.1:8001' 'ws://127.0.0.1:{{surreal_port}}'
            | save '{{demo}}/explorer/config.toml'
    }

# Restart only the explorer (to pick up a newly installed build); the rest keeps running
explorer-restart:
    @{{svc}} stop explorer | ignore
    @just explorer

# Show only the latest `n` transactions in the explorer (reopens it)
explorer-limit n="5":
    #!/usr/bin/env -S nu --no-config-file
    print "▶ Showing only the latest {{n}} transactions in the explorer"
    {{svc}} stop explorer | ignore
    just _explorer-config
    let config = '{{demo}}/explorer/config.toml'
    open --raw $config
        | str replace --regex '(?m)^canton_transactions = .*$' 'canton_transactions = {{n}}'
        | save -f $config
    just explorer | ignore

# Each ticket agreement with the price paid for it, found by walking the event graph
tickets:
    #!/usr/bin/env -S nu --no-config-file
    print "▶ Ticket agreements and the price paid, from the event graph in SurrealDB"
    let rows = (
        {{sql}} "USE NS canton DB ticket_purchase;
            SELECT offset, string::split(create_arguments.owner, '::')[0] AS owner,
                (<-consequence<-exercised[WHERE choice_name = 'Accept']->target->created.create_arguments.price)[0] AS price
            FROM created WHERE template.entity_name = 'TicketAgreement' ORDER BY offset;"
        | from json | last | get result
    )
    for row in $rows { print $"  TX@($row.offset)  ($row.owner) paid ($row.price)" }

# The events of the transaction at `offset`: created contracts' and exercised choices' arguments
show offset:
    #!/usr/bin/env -S nu --no-config-file
    # Party ids without their fingerprint, and long hex ids (contract ids) cut.
    def short [] {
        let value = $in
        match ($value | describe | str replace --regex '<.*' '') {
            "string" => {
                let s = ($value | str replace --all --regex '::[0-9a-f]+' '')
                if ($s =~ '^[0-9a-f]{40,}$') { ($s | str substring 0..9) + "…" } else { $s }
            }
            "list" | "table" => { $value | each { short } }
            "record" => { $value | items {|key, item| {$key: ($item | short)} } | reduce --fold {} {|it, acc| $acc | merge $it } }
            _ => { $value }
        }
    }
    print "▶ Events of TX@{{offset}} (party ids shortened, contract ids cut)"
    let events = (
        {{sql}} "USE NS canton DB ticket_purchase;
            SELECT node_id, name, 'exercised' AS kind, choice_argument AS args, acting_parties AS parties FROM exercised WHERE offset = {{offset}};
            SELECT node_id, name, 'created' AS kind, create_arguments AS args, signatories AS parties FROM created WHERE offset = {{offset}};"
        | from json | last 2 | get result | flatten | sort-by node_id
    )
    for event in $events {
        let event = ($event | short)
        let parties = ($event.parties | str join ", ")
        let what = (if $event.kind == "exercised" { $"exercised by ($parties)" } else { $"created, signed by ($parties)" })
        print $"  node ($event.node_id)  ($event.name)  ($what)"
        if ($event.args | is-empty) {
            print "      (no argument)"
        } else {
            for arg in ($event.args | transpose key value) {
                let value = (if ($arg.value | describe) == "string" { $arg.value } else { $arg.value | to json --raw })
                print $"      ($arg.key): ($value)"
            }
        }
        print ""
    }

# Show what runs and how much the syncer has written
status:
    #!/usr/bin/env -S nu --no-config-file
    {{svc}} status
    if (({{svc}} listening {{surreal_port}} | complete).exit_code == 0) {
        [
            [what from];
            [transactions "transaction"]
            [Cash "created WHERE template.entity_name = 'Cash'"]
            [TicketOffer "created WHERE template.entity_name = 'TicketOffer'"]
            [TicketAgreement "created WHERE template.entity_name = 'TicketAgreement'"]
        ] | each {|row|
            let count = ({{sql}} $"USE NS canton DB ticket_purchase; RETURN count\(SELECT * FROM ($row.from)\);" | from json | last | get result)
            {what: $row.what, count: $count}
        } | print
    }

# Stop the analytics daemon and dashboard, the explorer, the syncer, SurrealDB and the sandbox
stop:
    @{{svc}} stop bench-surreal | ignore
    @{{svc}} stop dashboard
    @{{svc}} stop analytics
    @{{svc}} stop explorer
    @{{svc}} stop syncer
    @{{svc}} stop surreal
    @{{svc}} stop sandbox

# Follow a service's log: sandbox, surreal, syncer, explorer, analytics, dashboard or bench-surreal
logs name="syncer":
    ^tail -f '{{demo}}/logs/{{name}}.log'

# Fail with a clear message when Nushell is older than the recipes need
_nu-version:
    #!/usr/bin/env -S nu --no-config-file
    let have = ((version).version | split row "." | into int)
    let need = ('{{nu_min}}' | split row "." | into int)
    let older = ($have | zip $need | reduce --fold null {|pair, acc| if $acc != null { $acc } else if $pair.0 < $pair.1 { true } else if $pair.0 > $pair.1 { false } else { null } })
    if $older == true {
        print -e $"error: the recipes need Nushell {{nu_min}} or newer, and this is ((version).version) \(brew upgrade nushell\)"
        exit 1
    }

# --- Analytics: the canton-analytics daemon and the explorer's dashboard ---

# Install canton-analytics and a dashboard-enabled explorer into .demo/analytics/bin (skips what is already installed)
analytics-tools: _nu-version
    #!/usr/bin/env -S nu --no-config-file
    mkdir '{{analytics_bin}}'
    let record = ('{{analytics}}/.crates.toml' | if ($in | path exists) { open --raw $in } else { "" })
    if not ($record =~ 'canton-analytics .*tag={{syncer_tag}}') {
        {{build}} "canton-analytics {{syncer_tag}}" cargo install --locked --root '{{analytics}}' --git {{syncer_git}} --tag {{syncer_tag}} canton-analytics
    }
    if not ($record =~ 'surrealdb-explorer .*tag={{explorer_tag}}') {
        {{build}} "explorer {{explorer_tag}} with its dashboard" cargo install --locked --root '{{analytics}}' --git {{explorer_git}} --tag {{explorer_tag}} --features dashboard surrealdb-explorer
    }

# Build the ticket analytics plugins (WebAssembly) into .demo/analytics/plugins
analytics-plugins:
    @print "▶ Building the ticket analytics plugins"
    @{{build}} "the wasm32-wasip2 target" rustup target add wasm32-wasip2
    @mkdir '{{plugins}}'
    @cd analytics/plugins; {{build}} "the plugins" cargo build --locked --release --target wasm32-wasip2
    @for p in [ticket_agreements ticket_value] { cp $"analytics/plugins/target/wasm32-wasip2/release/($p).wasm" '{{plugins}}/' }

# Run the analytics daemon on the demo's SurrealDB in the background, then open the dashboard (start the demo first)
analytics: analytics-tools analytics-plugins
    #!/usr/bin/env -S nu --no-config-file
    print "▶ Starting canton-analytics: SurrealDB → Delta tables in .demo/analytics/tables"
    if (({{svc}} listening {{surreal_port}} | complete).exit_code != 0) {
        print -e "SurrealDB isn't running (nothing on port {{surreal_port}}): start the demo first with 'just demo'"
        exit 1
    }
    {{sql}} "USE NS canton DB ticket_purchase; DEFINE USER IF NOT EXISTS analytics ON DATABASE PASSWORD 'analytics' ROLES VIEWER;" | ignore
    # Restart a running daemon, so it loads the plugins just built; it carries on from its tables.
    {{svc}} stop analytics | ignore
    with-env {
        SURREAL_URL: "ws://127.0.0.1:{{surreal_port}}", SURREAL_NAMESPACE: canton, SURREAL_DATABASE: ticket_purchase,
        SURREAL_USER: analytics, SURREAL_PASSWORD: analytics
    } {
        {{svc}} start analytics '{{analytics}}' '{{analytics_bin}}/canton-analytics' run --plugins-dir '{{plugins}}' --tables '{{analytics}}/tables' --max-wait 2
    }
    {{svc}} wait-log analytics "plugin ready" 60
    just analytics-dashboard

# Open the dashboard-enabled explorer on a tables folder (default: the daemon's), in the background
analytics-dashboard tables=(analytics / "tables"):
    #!/usr/bin/env -S nu --no-config-file
    print "▶ Opening the explorer on {{tables}} (Dashboard tab; the Graph tab shows the demo's ledger)"
    # Without the demo's SurrealDB running (say, for the benchmark's tables), no database: it opens on the dashboard.
    let server = (if (({{svc}} listening {{surreal_port}} | complete).exit_code == 0) { "ws://127.0.0.1:{{surreal_port}}" } else { "" })
    just _dashboard '{{tables}}' $server ticket_purchase 50

# Benchmark A: write the plugins' tables for a made-up ledger (default 10 million offsets) and time the dashboards
analytics-bench rows="10000000": analytics-plugins
    @rm -rf '{{analytics}}/bench-tables'
    @{{build}} "the benchmark" cargo build --release -p ticket-bench
    cargo run --quiet --release -p ticket-bench -- table --tables '{{analytics}}/bench-tables' --rows {{rows}} '{{plugins}}/ticket_agreements.wasm' '{{plugins}}/ticket_value.wasm'
    @print "Look at them with: just analytics-tools && just analytics-dashboard {{analytics}}/bench-tables"

# Benchmark B: time the daemon's catch-up from a fresh SurrealDB with a made-up ledger (default 1 million purchases)
analytics-bench-surreal rows="1000000": analytics-plugins
    #!/usr/bin/env -S nu --no-config-file
    # Loads the made-up ledger in the shape the syncer writes it, times the daemon's
    # catch-up into Delta tables, then asks SurrealDB and the tables for the same numbers.
    # A benchmark SurrealDB left over from an interrupted run: its data is thrown away anyway.
    {{svc}} stop bench-surreal | ignore
    {{svc}} check-port {{bench_surreal_port}} bench-surreal "set DEMO_BENCH_SURREAL_PORT"
    rm -rf '{{analytics}}/bench-surreal' '{{analytics}}/bench-surreal-tables'
    {{build}} "the benchmark" cargo build --release -p ticket-bench
    just _bench-surreal
    # The benchmark's SurrealDB stops whether the run passes or fails.
    try {
        cargo run --quiet --release -p ticket-bench -- surreal --url ws://127.0.0.1:{{bench_surreal_port}} --pass {{surreal_root_pass}} --rows {{rows}} --tables '{{analytics}}/bench-surreal-tables' '{{plugins}}/ticket_agreements.wasm' '{{plugins}}/ticket_value.wasm'
    } finally {
        {{svc}} stop bench-surreal | ignore
    }
    print "Browse its ledger and tables with: just analytics-tools && just analytics-bench-surreal-db"

# Start the last benchmark's SurrealDB again and open the explorer on it: its ledger in the Graph tab, its tables in the Dashboard tab
analytics-bench-surreal-db:
    @if not ('{{analytics}}/bench-surreal/db' | path exists) { print -e "No benchmark database yet: run just analytics-bench-surreal first"; exit 1 }
    @just _bench-surreal
    @print "▶ Opening the explorer on the benchmark's ledger (Graph tab) and its tables (Dashboard tab); just stop stops both"
    @# 48 transactions: a multiple of the benchmark's three per purchase, so each row is cash, offer, accept.
    @just _dashboard '{{analytics}}/bench-surreal-tables' ws://127.0.0.1:{{bench_surreal_port}} ticket_bench 48

# The dashboard-enabled explorer on a tables folder and a database ("" for none), in the background; replaces one already open
_dashboard tables server database transactions:
    #!/usr/bin/env -S nu --no-config-file
    {{svc}} stop dashboard | ignore
    mkdir '{{analytics}}/explorer' '{{tables}}'
    let location = ('{{tables}}' | path expand)
    let config = (
        open --raw config/explorer.toml
            | str replace 'ws://127.0.0.1:8001' '{{server}}'
            | str replace 'database = "ticket_purchase"' 'database = "{{database}}"'
            | str replace 'canton_transactions = 50' 'canton_transactions = {{transactions}}'
    )
    $config + $"\n[analytics]\nlocation = \"($location)\"\n" | save -f '{{analytics}}/explorer/config.toml'
    {{svc}} start dashboard '{{analytics}}/explorer' '{{analytics_bin}}/surrealdb-explorer'

# Start the benchmark's SurrealDB on disk (SurrealKV, as a real syncer's database would be), in the background
_bench-surreal:
    @mkdir '{{analytics}}/bench-surreal'
    @{{svc}} start bench-surreal '{{analytics}}/bench-surreal' surreal db start --user root --pass {{surreal_root_pass}} --bind 127.0.0.1:{{bench_surreal_port}} surrealkv://db
    @{{svc}} wait-http bench-surreal http://127.0.0.1:{{bench_surreal_port}}/health 30
