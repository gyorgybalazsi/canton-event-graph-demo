# End-to-end ticket purchase demo: Canton sandbox → Rust client → syncer →
# SurrealDB → surrealdb-explorer. `just demo` runs it all; see README.md.

set shell := ["bash", "-euo", "pipefail", "-c"]

# The tools come from their own repositories, pinned to their release tags.
# Bump a tag to try a newer version (and the `client`/`ledger-api` tags in
# app/Cargo.toml together with syncer_tag).
syncer_git := "https://github.com/gyorgybalazsi/canton-event-graph-syncer"
syncer_tag := "v0.1.0"
codegen_git := "https://github.com/gyorgybalazsi/daml-rust-codegen"
codegen_tag := "v0.1.0"
explorer_git := "https://github.com/gyorgybalazsi/surrealdb-explorer"
explorer_tag := "v0.1.0"
# The analytics daemon and a dashboard-enabled explorer for the `analytics`
# recipes. Pinned to commits on main until the analytics module gets a release
# tag; bump `rev` in analytics/bench/Cargo.toml together with analytics_rev.
analytics_rev := "615c8e6"
explorer_dashboard_rev := "d41f3f4"
# Set EXPLORER_PATH to a local clone of surrealdb-explorer to install from it
# instead of GitHub (for example to try local changes).
explorer_path := env("EXPLORER_PATH", "")

demo := justfile_directory() / ".demo"
bin := demo / "bin"
svc := justfile_directory() / "scripts/demo.sh"
dar := justfile_directory() / "daml/.daml/dist/ticket-purchase-0.1.0.dar"

analytics := demo / "analytics"
analytics_bin := analytics / "bin"
plugins := analytics / "plugins"

ledger_port := "6865"
# Not 8000, so the demo doesn't collide with a SurrealDB server you already run.
surreal_port := env("DEMO_SURREAL_PORT", "8001")
surreal_http := "http://127.0.0.1:" + surreal_port
# Demo-only credentials for the throwaway in-memory server on 127.0.0.1.
surreal_root_pass := "root"
syncer_pass := "syncer"
# Seconds `just demo` waits after each transaction of the first, guided purchase.
guided_pause := env("DEMO_PAUSE", "4")
# Hides cargo's "already installed" and toolchain notes; real errors still show.
quiet := "{ grep -v -e 'Ignored package' -e 'is already installed' -e 'toolchain' -e '^$' >&2 || true; }"
# Whether cargo's install record in .demo matches a pattern (a tool at the pinned
# tag, or the explorer installed from a local clone, is not fetched again).
installed := "grep -qsE"
# Prints the last statement's rows, one per line (as JSON without jq).

# List the recipes
default:
    @just --list --unsorted

# Run the whole demo: start everything, open the explorer on the empty graph, buy one ticket step by step, then two more
# SurrealDB and the explorer come first, so the explorer is open on the empty
# graph and everything after it, setup included, shows up as it happens.
demo: tools surreal explorer build-dar sandbox setup sync
    @just purchase 1 {{guided_pause}}
    @just purchase 2

# Stop everything and delete the demo's data and logs (keeps the installed tools)
demo-reset: stop
    @rm -rf "{{demo}}/logs" "{{demo}}/run" "{{demo}}/explorer"
    @echo "Reset. The sandbox and SurrealDB kept their data in memory, so it is gone too."

# Install the syncer, the code generator and the explorer into .demo/bin (skips what is already installed)
tools:
    @echo "▶ Installing syncer, daml-rust-codegen and surrealdb-explorer (pinned versions)"
    @mkdir -p "{{bin}}"
    @{{installed}} "syncer .*tag={{syncer_tag}}" "{{demo}}/.crates.toml" || \
        cargo install --quiet --locked --root "{{demo}}" --git {{syncer_git}} --tag {{syncer_tag}} syncer 2>&1 | {{quiet}}
    @{{installed}} "daml-rust-codegen .*tag={{codegen_tag}}" "{{demo}}/.crates.toml" || \
        cargo install --quiet --locked --root "{{demo}}" --git {{codegen_git}} --tag {{codegen_tag}} daml-rust-codegen 2>&1 | {{quiet}}
    @if [[ -n "{{explorer_path}}" ]]; then \
        cargo install --quiet --locked --force --root "{{demo}}" --path "{{explorer_path}}" 2>&1 | {{quiet}}; \
    elif ! {{installed}} "surrealdb-explorer .*(tag={{explorer_tag}}|path\+)" "{{demo}}/.crates.toml"; then \
        cargo install --quiet --locked --root "{{demo}}" --git {{explorer_git}} --tag {{explorer_tag}} surrealdb-explorer 2>&1 | {{quiet}}; \
    fi

# Build the ticket purchase DAR (needs the Daml SDK's dpm)
build-dar:
    @echo "▶ Building the ticket purchase model (daml/ → ticket-purchase-0.1.0.dar)"
    @mkdir -p "{{demo}}"
    @cd daml && dpm build > "{{demo}}/build-dar.log" 2>&1 || { cat "{{demo}}/build-dar.log" >&2; exit 1; }

# Start a Canton sandbox on port 6865 in the background
sandbox:
    @echo "▶ Starting the Canton sandbox (port {{ledger_port}})"
    @if "{{svc}}" listening {{ledger_port}} && [[ "$("{{svc}}" status)" != *"sandbox: running"* ]]; then \
        echo "error: something else already listens on port {{ledger_port}} (another sandbox?)" >&2; exit 1; \
    fi
    @"{{svc}}" start sandbox daml dpm sandbox
    @"{{svc}}" wait-log sandbox "Canton sandbox is ready." 180

# Start an in-memory SurrealDB server and create the syncer's database user
surreal:
    @echo "▶ Starting SurrealDB (port {{surreal_port}}) and creating the syncer's database user"
    @if "{{svc}}" listening {{surreal_port}} && [[ "$("{{svc}}" status)" != *"surreal: running"* ]]; then \
        echo "error: something else already listens on port {{surreal_port}}; set DEMO_SURREAL_PORT" >&2; exit 1; \
    fi
    @"{{svc}}" start surreal . surreal db start --user root --pass {{surreal_root_pass}} --bind 127.0.0.1:{{surreal_port}} memory
    @"{{svc}}" wait-http surreal {{surreal_http}}/health 30
    @just _sql "DEFINE NAMESPACE IF NOT EXISTS canton; USE NS canton; \
        DEFINE DATABASE IF NOT EXISTS ticket_purchase; USE DB ticket_purchase; \
        DEFINE USER IF NOT EXISTS syncer ON DATABASE PASSWORD '{{syncer_pass}}' ROLES EDITOR;" > /dev/null

# Upload the DAR and allocate Alice, TicketWizard and ScroogeBank
setup:
    @echo "▶ Uploading the model and allocating Alice, TicketWizard and ScroogeBank"
    @cargo run --quiet -p ticket-purchase -- setup --dar "{{dar}}"

# Start the syncer in the background, following every party on the sandbox
sync:
    @echo "▶ Starting the syncer: ledger → SurrealDB"
    @SURREAL_URL=ws://127.0.0.1:{{surreal_port}} SURREAL_PASSWORD={{syncer_pass}} \
        "{{svc}}" start syncer . "{{bin}}/syncer" sync --config-file config/syncer.toml
    @"{{svc}}" wait-log syncer "Connected to SurrealDB" 60

# Buy tickets; `pause` waits that many seconds after each transaction
purchase count="3" pause="0":
    @echo "▶ Buying {{count}} ticket(s)"
    @cargo run --quiet -p ticket-purchase -- purchase --count {{count}} --pause {{pause}}

# Open the explorer on the demo database, in the background
explorer:
    @echo "▶ Opening the explorer (Canton view, refreshes by itself)"
    @mkdir -p "{{demo}}/explorer"
    @[[ -f "{{demo}}/explorer/config.toml" ]] || \
        sed 's#ws://127.0.0.1:8001#ws://127.0.0.1:{{surreal_port}}#' config/explorer.toml > "{{demo}}/explorer/config.toml"
    @"{{svc}}" start explorer "{{demo}}/explorer" "{{bin}}/surrealdb-explorer"
    @sleep 3

# Restart only the explorer (to pick up a newly installed build); the rest keeps running
explorer-restart:
    @"{{svc}}" stop explorer > /dev/null
    @just explorer

# Show only the latest `n` transactions in the explorer (reopens it)
explorer-limit n="5":
    @echo "▶ Showing only the latest {{n}} transactions in the explorer"
    @"{{svc}}" stop explorer > /dev/null
    @sed -i.bak 's/^canton_transactions = .*/canton_transactions = {{n}}/' "{{demo}}/explorer/config.toml"
    @just explorer > /dev/null

# Each ticket agreement with the price paid for it, found by walking the event graph
tickets:
    @echo "▶ Ticket agreements and the price paid, from the event graph in SurrealDB"
    @just _sql "USE NS canton DB ticket_purchase; \
        SELECT offset, string::split(create_arguments.owner, '::')[0] AS owner, \
            (<-consequence<-exercised[WHERE choice_name = 'Accept']->target->created.create_arguments.price)[0] AS price \
        FROM created WHERE template.entity_name = 'TicketAgreement' ORDER BY offset;" \
        | if command -v jq >/dev/null; then \
            jq -r '.[-1].result[] | "  TX@\(.offset)  \(.owner) paid \(.price)"'; else cat; fi

# The events of the transaction at `offset`: created contracts' arguments and exercised choices' arguments
show offset:
    @echo "▶ Events of TX@{{offset}} (party ids shortened, contract ids cut)"
    @just _sql "USE NS canton DB ticket_purchase; \
        SELECT node_id, name, 'exercised' AS kind, choice_argument AS args, acting_parties AS parties FROM exercised WHERE offset = {{offset}}; \
        SELECT node_id, name, 'created' AS kind, create_arguments AS args, signatories AS parties FROM created WHERE offset = {{offset}};" \
        | jq -r 'def short: walk(if type == "string" then (gsub("::[0-9a-f]+"; "") | if test("^[0-9a-f]{40,}$") then .[0:10] + "…" else . end) else . end); [.[-2].result[], .[-1].result[]] | sort_by(.node_id)[] | short | "  node \(.node_id)  \(.name)  " + (if .kind == "exercised" then "exercised by \(.parties | join(", "))" else "created, signed by \(.parties | join(", "))" end) + "\n" + (if (.args | length) == 0 then "      (no argument)" else (.args | to_entries | map("      \(.key): \(.value | if type == "string" then . else tojson end)") | join("\n")) end) + "\n"'

# Show what runs and how much the syncer has written
status:
    @"{{svc}}" status
    @if "{{svc}}" listening {{surreal_port}}; then \
        for what in "transactions:transaction" "Cash:created WHERE template.entity_name = 'Cash'" \
            "TicketOffer:created WHERE template.entity_name = 'TicketOffer'" \
            "TicketAgreement:created WHERE template.entity_name = 'TicketAgreement'"; do \
            n=$(just _sql "USE NS canton DB ticket_purchase; RETURN count(SELECT * FROM ${what#*:});" \
                | grep -o '"result":[0-9]*' | tail -n 1 | cut -d: -f2); \
            printf '%-16s %s\n' "${what%%:*}" "${n:-0}"; \
        done; \
    fi

# Stop the analytics daemon and dashboard, the explorer, the syncer, SurrealDB and the sandbox
stop:
    @"{{svc}}" stop dashboard
    @"{{svc}}" stop analytics
    @"{{svc}}" stop explorer
    @"{{svc}}" stop syncer
    @"{{svc}}" stop surreal
    @"{{svc}}" stop sandbox

# Follow a service's log: sandbox, surreal, syncer, explorer, analytics or dashboard
logs name="syncer":
    tail -f "{{demo}}/logs/{{name}}.log"

# Run SurrealQL as root on the demo server and fail on an error
_sql query:
    @out=$(curl -sS -u root:{{surreal_root_pass}} -H "Accept: application/json" \
        --data-binary "{{query}}" {{surreal_http}}/sql); \
    if echo "$out" | grep -q '"status":"ERR"'; then echo "$out" >&2; exit 1; fi; \
    echo "$out"

# --- Analytics: the canton-analytics daemon and the explorer's dashboard ---

# Install canton-analytics and a dashboard-enabled explorer into .demo/analytics/bin (skips what is already installed)
analytics-tools:
    @echo "▶ Installing canton-analytics and the explorer with its dashboard (pinned commits)"
    @mkdir -p "{{analytics_bin}}"
    @{{installed}} "canton-analytics .*#{{analytics_rev}}" "{{analytics}}/.crates.toml" || \
        cargo install --quiet --locked --root "{{analytics}}" --git {{syncer_git}} --rev {{analytics_rev}} canton-analytics 2>&1 | {{quiet}}
    @{{installed}} "surrealdb-explorer .*#{{explorer_dashboard_rev}}" "{{analytics}}/.crates.toml" || \
        cargo install --quiet --locked --root "{{analytics}}" --git {{explorer_git}} --rev {{explorer_dashboard_rev}} \
            --features dashboard surrealdb-explorer 2>&1 | {{quiet}}

# Build the ticket analytics plugins (WebAssembly) into .demo/analytics/plugins
analytics-plugins:
    @echo "▶ Building the ticket analytics plugins"
    @rustup target add wasm32-wasip2 2>&1 | {{quiet}}
    @mkdir -p "{{plugins}}"
    @for p in ticket-agreements ticket-value-by-offset; do \
        (cd analytics/plugins/$p && cargo build --quiet --locked --release --target wasm32-wasip2 --target-dir target) \
        && cp analytics/plugins/$p/target/wasm32-wasip2/release/${p//-/_}.wasm "{{plugins}}/"; \
    done

# Run the analytics daemon on the demo's SurrealDB in the background, then open the dashboard (start the demo first)
analytics: analytics-tools analytics-plugins
    @echo "▶ Starting canton-analytics: SurrealDB → Delta tables in .demo/analytics/tables"
    @just _sql "USE NS canton DB ticket_purchase; \
        DEFINE USER IF NOT EXISTS analytics ON DATABASE PASSWORD 'analytics' ROLES VIEWER;" > /dev/null
    @SURREAL_URL=ws://127.0.0.1:{{surreal_port}} SURREAL_NAMESPACE=canton SURREAL_DATABASE=ticket_purchase \
        SURREAL_USER=analytics SURREAL_PASSWORD=analytics \
        "{{svc}}" start analytics "{{analytics}}" "{{analytics_bin}}/canton-analytics" run \
            --plugins-dir "{{plugins}}" --tables "{{analytics}}/tables" --max-wait 2
    @"{{svc}}" wait-log analytics "plugin ready" 60
    @just analytics-dashboard

# Open the dashboard-enabled explorer on a tables folder (default: the daemon's), in the background
analytics-dashboard tables=(analytics / "tables"):
    @echo "▶ Opening the explorer on {{tables}} (Dashboard tab; the Graph tab shows the demo's ledger)"
    @"{{svc}}" stop dashboard > /dev/null
    @mkdir -p "{{analytics}}/explorer" "{{tables}}"
    @# Without the demo's SurrealDB running (say, for the benchmark's tables), no database: it opens on the dashboard.
    @if "{{svc}}" listening {{surreal_port}}; then server=ws://127.0.0.1:{{surreal_port}}; else server=; fi; \
        sed "s#ws://127.0.0.1:8001#$server#" config/explorer.toml > "{{analytics}}/explorer/config.toml"
    @printf '\n[analytics]\nlocation = "%s"\n' "$(cd "{{tables}}" && pwd)" >> "{{analytics}}/explorer/config.toml"
    @"{{svc}}" start dashboard "{{analytics}}/explorer" "{{analytics_bin}}/surrealdb-explorer"

# Benchmark: write the ticket plugins' tables for a made-up ledger (default 10 million offsets) and time their dashboards
analytics-bench rows="10000000": analytics-plugins
    @rm -rf "{{analytics}}/bench-tables"
    cargo run --quiet --release -p ticket-bench -- table --tables "{{analytics}}/bench-tables" --rows {{rows}} \
        "{{plugins}}/ticket_agreements.wasm" "{{plugins}}/ticket_value_by_offset.wasm"
    @echo "Look at them with: just analytics-tools && just analytics-dashboard {{analytics}}/bench-tables"
