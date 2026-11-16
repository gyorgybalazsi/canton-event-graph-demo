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
explorer_rev := "f8995b8a27fb1d8d46087ff80f4f9ed209a849bf"
# Set EXPLORER_PATH to a local clone of surrealdb-explorer to install from it instead.
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

# List the recipes
default:
    @just --list --unsorted

# Run the whole demo: tools, DAR, sandbox, SurrealDB, parties, syncer, purchases, explorer
demo: tools build-dar sandbox surreal setup sync purchase explorer

# Stop everything and delete the demo's data and logs (keeps the installed tools)
demo-reset: stop
    rm -rf "{{demo}}/logs" "{{demo}}/run" "{{demo}}/explorer"
    @echo "Reset. The sandbox and SurrealDB kept their data in memory, so it is gone too."

# Install the syncer, the code generator and the explorer into .demo/bin
tools:
    @mkdir -p "{{bin}}"
    cargo install --locked --root "{{demo}}" --git {{syncer_git}} --rev {{syncer_rev}} syncer
    cargo install --locked --root "{{demo}}" --git {{codegen_git}} --rev {{codegen_rev}} daml-rust-codegen
    if [[ -n "{{explorer_path}}" ]]; then \
        cargo install --locked --root "{{demo}}" --path "{{explorer_path}}"; \
    else \
        CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --root "{{demo}}" --git {{explorer_git}} --rev {{explorer_rev}} surrealdb-explorer; \
    fi

# Build the ticket purchase DAR (needs the Daml SDK's dpm)
build-dar:
    cd daml && dpm build

# Start a Canton sandbox on port 6865 in the background
sandbox:
    @if "{{svc}}" listening {{ledger_port}} && ! "{{svc}}" status | grep -q "sandbox: running"; then \
        echo "error: something else already listens on port {{ledger_port}} (another sandbox?)" >&2; exit 1; \
    fi
    "{{svc}}" start sandbox daml dpm sandbox
    "{{svc}}" wait-log sandbox "Canton sandbox is ready." 180

# Start an in-memory SurrealDB server and create the syncer's database user
surreal:
    @if "{{svc}}" listening {{surreal_port}} && ! "{{svc}}" status | grep -q "surreal: running"; then \
        echo "error: something else already listens on port {{surreal_port}}; set DEMO_SURREAL_PORT" >&2; exit 1; \
    fi
    "{{svc}}" start surreal . surreal start --user root --pass {{surreal_root_pass}} --bind 127.0.0.1:{{surreal_port}} memory
    "{{svc}}" wait-http surreal {{surreal_http}}/health 30
    just _sql "DEFINE NAMESPACE IF NOT EXISTS canton; USE NS canton; \
        DEFINE DATABASE IF NOT EXISTS ticket_purchase; USE DB ticket_purchase; \
        DEFINE USER IF NOT EXISTS syncer ON DATABASE PASSWORD '{{syncer_pass}}' ROLES EDITOR;"

# Upload the DAR and allocate Alice, TicketWizard and ScroogeBank
setup:
    cargo run --quiet -p ticket-purchase -- setup --dar "{{dar}}"

# Start the syncer in the background, following every party on the sandbox
sync:
    SURREAL_URL=ws://127.0.0.1:{{surreal_port}} SURREAL_PASSWORD={{syncer_pass}} \
        "{{svc}}" start syncer . "{{bin}}/syncer" sync --config-file config/syncer.toml
    "{{svc}}" wait-log syncer "Connected to SurrealDB" 60

# Buy tickets (run again while the explorer is open to watch them arrive)
purchase count="3":
    cargo run --quiet -p ticket-purchase -- purchase --count {{count}}

# Open the explorer on the demo database (closes when you close the window)
explorer:
    @mkdir -p "{{demo}}/explorer"
    @[[ -f "{{demo}}/explorer/config.toml" ]] || \
        sed 's#ws://127.0.0.1:8001#ws://127.0.0.1:{{surreal_port}}#' config/explorer.toml > "{{demo}}/explorer/config.toml"
    cd "{{demo}}/explorer" && "{{bin}}/surrealdb-explorer"

# Show what runs and how much the syncer has written
status:
    @"{{svc}}" status
    @if "{{svc}}" listening {{surreal_port}}; then just _sql "USE NS canton DB ticket_purchase; \
        SELECT count() AS transactions FROM transaction GROUP ALL; \
        SELECT template_name, count() AS contracts FROM created GROUP BY template_name;"; fi

# Stop the syncer, SurrealDB and the sandbox
stop:
    "{{svc}}" stop syncer
    "{{svc}}" stop surreal
    "{{svc}}" stop sandbox

# Follow a service's log: sandbox, surreal or syncer
logs name="syncer":
    tail -f "{{demo}}/logs/{{name}}.log"

# Run SurrealQL as root on the demo server and fail on an error
_sql query:
    @out=$(curl -sS -u root:{{surreal_root_pass}} -H "Accept: application/json" \
        --data-binary "{{query}}" {{surreal_http}}/sql); \
    if echo "$out" | grep -q '"status":"ERR"'; then echo "$out" >&2; exit 1; fi; \
    echo "$out"
