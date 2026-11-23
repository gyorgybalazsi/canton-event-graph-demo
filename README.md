# canton-event-graph-demo

An end-to-end demo of a Canton ledger's event graph. One command starts a
Canton sandbox, buys a few tickets on it from a small Rust program, syncs the
ledger into SurrealDB and opens a desktop window that draws the transactions
as a graph.

```
 daml/Main.daml ──dpm build──▶ ticket-purchase.dar ──daml-rust-codegen──▶ Rust types
                                      │                                      │
                                      ▼                                      ▼
                               Canton sandbox ◀──── commands ──── app/ (ticket-purchase)
                                      │
                                   syncer
                                      ▼
                                  SurrealDB ──────▶ surrealdb-explorer
```

The pieces come from their own repositories and are pinned to their release tags,
not copied:

| Piece | Repository | Used for |
|---|---|---|
| `daml-rust-codegen` | [daml-rust-codegen](https://github.com/gyorgybalazsi/daml-rust-codegen) | Generates the Rust types for the Daml model, at build time |
| `syncer`, and the `client` / `ledger-api` / `daml-type-rep` / `derive-lapi-access` crates | [canton-event-graph-syncer](https://github.com/gyorgybalazsi/canton-event-graph-syncer) | Streams the ledger into SurrealDB; the app talks to the ledger with the crates |
| `surrealdb-explorer` | [surrealdb-explorer](https://github.com/gyorgybalazsi/surrealdb-explorer) | Draws the event graph |

## The model

[`daml/daml/Main.daml`](daml/daml/Main.daml) has three templates:

- `Cash`, issued by a bank to an owner, with a `Transfer` choice;
- `TicketOffer`, from an organizer to a buyer at a price, with an `Accept`
  choice that takes the buyer's cash;
- `TicketAgreement`, the ticket, signed by both.

The app ([`app/src/main.rs`](app/src/main.rs)) buys tickets as three parties:
ScroogeBank issues cash to Alice, TicketWizard offers Alice a ticket, and Alice
accepts. Accepting is one transaction that archives the offer, transfers the
cash to TicketWizard and creates the ticket agreement, so in the explorer it is
a tree: `Accept` → `Transfer` → the new `Cash` and the archived old one, plus
the `TicketAgreement`.

## Prerequisites

- **Rust** via [rustup](https://rustup.rs) (the version is pinned in `rust-toolchain.toml`)
- **[protoc](https://protobuf.dev/installation/)**, for the Ledger API bindings (`brew install protobuf`)
- **Daml SDK 3.7.1** with `dpm` (`dpm install 3.7.1`), for the DAR and the sandbox
- **[SurrealDB](https://surrealdb.com/install) 4.x**, the `surreal` program
- **[just](https://github.com/casey/just)** (`brew install just`)
- **curl**, and network access to GitHub: `just tools` installs the three tools
  from their public repositories above

Ports 6865–6869 (sandbox) and 8001 (SurrealDB) must be free. The demo uses
8001 rather than SurrealDB's usual 8000 so it doesn't collide with a server
you already run; set `DEMO_SURREAL_PORT` to use another.

## Run it

```bash
just demo
```

The first run builds the three tools into `.demo/bin`, which takes a few
minutes. Then it prints one line per step:

1. starts an in-memory SurrealDB server in the background and creates the
   syncer's database user (`just surreal`);
2. opens the explorer on the still empty graph (`just explorer`);
3. builds the DAR and starts the sandbox in the background (`just build-dar`,
   `just sandbox`);
4. uploads the DAR and allocates Alice, TicketWizard and ScroogeBank
   (`just setup`);
5. starts the syncer (`just sync`);
6. buys one ticket step by step, waiting 4 seconds after each of its three
   transactions so you can watch each one arrive (`DEMO_PAUSE` changes the
   wait), then buys two more at full speed.

The explorer starts in its Canton view and refreshes by itself. Then try:

```bash
just purchase 5        # buy more tickets and watch them arrive
just explorer-limit 5  # show only the latest 5 transactions
just tickets           # each ticket agreement with the price paid, by walking the graph
just show 25           # the events of one transaction: payloads and choice arguments
just status            # what runs, and how much the syncer has written
```

In the explorer, open **▸ Query** under the graph and pick **Ticket purchase
demo query** from **Choose a saved query…**, then press **Run**: the table under
the query lists each accepted offer with its buyer and price, like
`just tickets`.

`just stop` stops everything, including the explorer and the analytics daemon. `just demo-reset` also
deletes the demo's logs and settings; the sandbox and SurrealDB keep their
data in memory, so the next `just demo` starts from an empty ledger.

## Recipes

| Recipe | What it does |
|---|---|
| `just demo` | Everything below, in order, with one guided purchase and two more |
| `just tools` | Installs `syncer`, `daml-rust-codegen` and `surrealdb-explorer` into `.demo/bin` at the pinned tags, skipping what is already installed (an explorer installed from a local clone counts) |
| `just build-dar` | Builds `daml/.daml/dist/ticket-purchase-0.1.0.dar` |
| `just sandbox` | Starts the Canton sandbox and waits until it is ready |
| `just surreal` | Starts SurrealDB in memory and creates the `canton` namespace, the `ticket_purchase` database and the `syncer` user |
| `just setup` | Uploads the DAR, allocates the parties (safe to repeat) |
| `just sync` | Starts the syncer, which follows every party on the sandbox |
| `just explorer` | Opens the explorer in the background |
| `just purchase [count] [pause]` | Buys `count` tickets (default 3), waiting `pause` seconds after each transaction (default 0) |
| `just explorer-restart` | Restarts only the explorer, for example after installing a new build with `just tools`; the sandbox, SurrealDB and the syncer keep running |
| `just explorer-limit [n]` | Reopens the explorer showing only the latest `n` transactions (default 5) |
| `just tickets` | Lists each ticket agreement with the price paid for it, found by walking from the agreement to the `Accept` choice and its offer |
| `just show <offset>` | Prints the events of the transaction at `offset` in node order: each created contract's arguments and signatories, and each exercised choice's argument and acting parties (needs `jq`) |
| `just status` | Shows what runs, and the transaction and contract counts in SurrealDB |
| `just logs [name]` | Follows the log of `sandbox`, `surreal`, `syncer` or `explorer` |
| `just stop` | Stops the explorer, the syncer, SurrealDB and the sandbox, and waits until their ports are free |
| `just demo-reset` | Stops everything and deletes `.demo/logs`, `.demo/run` and `.demo/explorer` |

The logs are in `.demo/logs/`. Everything the demo creates is under `.demo/`
(gitignored).

## Analytics

The [canton-analytics](https://github.com/gyorgybalazsi/canton-event-graph-syncer/tree/main/analytics)
daemon turns the event graph into Delta Lake tables through WebAssembly
plugins, and the explorer's **Dashboard** tab shows them. This repo has two
plugins for the ticket model, in [`analytics/plugins`](analytics/plugins):

- **ticket-agreements**: one row per accepted offer (`TicketOffer`'s `Accept`
  choice, which creates the `TicketAgreement`): offset, time, buyer,
  organizer and the offer's price. Its dashboard aggregates the raw rows.
- **ticket-value-by-offset**: the same agreements pre-aggregated per 10,000
  offsets, upserted as they come in, so its dashboard reads about 1,000 rows
  for a 10-million-offset ledger.

**Live, on the demo's ledger.** With `just demo` running:

```bash
just analytics      # starts the daemon in the background and opens the dashboard
just purchase 20    # buy more tickets; the dashboard picks them up within seconds
```

The first run installs `canton-analytics` and an explorer built with its
dashboard into `.demo/analytics/bin`, and needs the `wasm32-wasip2` Rust target
(the recipe adds it). The daemon signs in as a read-only `analytics` user.

**How fast over a big ledger.** `just analytics-bench` makes up a ledger of
10 million ticket purchases, one per offset, runs it through both plugins and
the daemon's own table writer (commits of a million offsets, then
compaction), and times every dashboard panel. It needs no sandbox or
SurrealDB:

```bash
just analytics-bench            # or: just analytics-bench 1000000
just analytics-tools && just analytics-dashboard .demo/analytics/bench-tables
```

On a 4-core cloud machine it wrote the 10 million rows in about 6 s, plus
2 s of compaction (300 MB of Parquet). The raw table's five panels, each
aggregating all 10 million rows, took about 390 ms together; the
pre-aggregated table's took 3 to 6 ms each. Each panel shows its query time in
the explorer too.

| Recipe | What it does |
|---|---|
| `just analytics` | Starts the daemon on the demo's SurrealDB (tables in `.demo/analytics/tables`) and opens the dashboard |
| `just analytics-dashboard [tables]` | Opens the dashboard-enabled explorer on a tables folder; the Graph tab shows the demo's ledger when SurrealDB runs |
| `just analytics-bench [rows]` | Writes the plugins' tables for a made-up ledger of `rows` offsets (default 10 million) and times their dashboards |
| `just analytics-tools` | Installs `canton-analytics` and the dashboard-enabled explorer, pinned to commits until the analytics module is tagged |
| `just analytics-plugins` | Builds the two plugins into `.demo/analytics/plugins` |

`cargo test --release -p ticket-bench` runs both plugins over two purchases in
the shape the syncer writes them, through the daemon, and checks the tables
and every dashboard panel.

## Layout

| Path | Contents |
|---|---|
| `daml/` | The Daml model, built into a DAR with `dpm build` |
| `app/` | The Rust client. `build.rs` runs `daml-rust-codegen` on the DAR, and `src/model.rs` includes the output, so the types always match the model |
| `config/` | The syncer's and the explorer's settings for the demo |
| `analytics/` | The ticket analytics plugins (`plugins/`, each its own workspace built for `wasm32-wasip2`), the plugin interface they build against (`wit/`, a copy of the syncer's), and the benchmark (`bench/`) |
| `scripts/demo.sh` | Starts, waits for and stops the background services |
| `justfile` | The recipes, and the pinned tags of the tools |

## Changing things

- **The model.** Edit `daml/daml/Main.daml` and run `just build-dar`; the next
  build of the app regenerates its types. If you rename a template or choice,
  the app stops compiling where it uses it, which is the point of generating
  the types.
- **Tool versions.** The release tags are at the top of the `justfile`. The app's
  ledger crates in `app/Cargo.toml` are pinned to the same
  canton-event-graph-syncer tag as the syncer; bump them together.
- **The explorer from a local clone.** `EXPLORER_PATH=~/surrealdb-explorer just tools`
  installs it from your clone instead of GitHub, for example to try local
  changes. `just tools` rebuilds it from the clone each time.
- **Building the app without the Daml SDK.** Point `TICKET_PURCHASE_GENERATED`
  at an already generated file, or `TICKET_PURCHASE_DAR` / `DAML_RUST_CODEGEN`
  at another DAR or codegen program (see `app/build.rs`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
