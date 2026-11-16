# End-to-end ticket purchase demo: Canton sandbox → Rust client → syncer →
# SurrealDB → surrealdb-explorer. `just demo` runs it all; see README.md.

set shell := ["bash", "-euo", "pipefail", "-c"]

# The tools come from their own repositories, pinned to known-good commits.
# Bump a rev to try a newer version (and the `client`/`ledger-api` revs in
# app/Cargo.toml together with SYNCER_REV).
syncer_git := "https://github.com/gyorgybalazsi/canton-event-graph-syncer"
syncer_rev := "9f25cb06ccf43af23b2f41bcad0355f17567d7db"
codegen_git := "https://github.com/gyorgybalazsi/daml-rust-codegen"
codegen_rev := "308c6bd279787c3697f051c18ba2e47a2f547bc3"
explorer_git := "https://github.com/gyorgybalazsi/surrealdb-explorer"
explorer_rev := "67786d9c1622442ff30e8c20acc3289647871065"
# Set EXPLORER_PATH to a local clone of surrealdb-explorer to install from it
# instead (needed while that repository is private).
explorer_path := env("EXPLORER_PATH", "")

demo := justfile_directory() / ".demo"
bin := demo / "bin"
svc := justfile_directory() / "scripts/demo.sh"
dar := justfile_directory() / "daml/.daml/dist/ticket-purchase-0.1.0.dar"

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
# rev, or the explorer installed from a local clone, is not fetched again).
installed := "grep -qsE"
# Prints the last statement's rows, one per line (as JSON without jq).

# List the recipes
default:
    @just --list --unsorted

# Run the whole demo: start everything, open the explorer on the empty graph, buy one ticket step by step, then two more
demo: tools build-dar sandbox surreal setup sync explorer
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
    @{{installed}} "syncer .*rev={{syncer_rev}}" "{{demo}}/.crates.toml" || \
        cargo install --quiet --locked --root "{{demo}}" --git {{syncer_git}} --rev {{syncer_rev}} syncer 2>&1 | {{quiet}}
    @{{installed}} "daml-rust-codegen .*rev={{codegen_rev}}" "{{demo}}/.crates.toml" || \
        cargo install --quiet --locked --root "{{demo}}" --git {{codegen_git}} --rev {{codegen_rev}} daml-rust-codegen 2>&1 | {{quiet}}
    @if [[ -n "{{explorer_path}}" ]]; then \
        cargo install --quiet --locked --force --root "{{demo}}" --path "{{explorer_path}}" 2>&1 | {{quiet}}; \
    elif ! {{installed}} "surrealdb-explorer .*(rev={{explorer_rev}}|path\+)" "{{demo}}/.crates.toml"; then \
        CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --quiet --locked --root "{{demo}}" --git {{explorer_git}} --rev {{explorer_rev}} surrealdb-explorer 2>&1 | {{quiet}} \
        || { echo "error: could not install surrealdb-explorer from GitHub (the repository may be private)." >&2; \
             echo "Install it from a local clone instead: EXPLORER_PATH=/path/to/surrealdb-explorer just tools" >&2; exit 1; }; \
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
    @"{{svc}}" start surreal . surreal start --user root --pass {{surreal_root_pass}} --bind 127.0.0.1:{{surreal_port}} memory
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

# Stop the explorer, the syncer, SurrealDB and the sandbox
stop:
    @"{{svc}}" stop explorer
    @"{{svc}}" stop syncer
    @"{{svc}}" stop surreal
    @"{{svc}}" stop sandbox

# Follow a service's log: sandbox, surreal, syncer or explorer
logs name="syncer":
    tail -f "{{demo}}/logs/{{name}}.log"

# Run SurrealQL as root on the demo server and fail on an error
_sql query:
    @out=$(curl -sS -u root:{{surreal_root_pass}} -H "Accept: application/json" \
        --data-binary "{{query}}" {{surreal_http}}/sql); \
    if echo "$out" | grep -q '"status":"ERR"'; then echo "$out" >&2; exit 1; fi; \
    echo "$out"
