---
name: adapt-analytics-plugins
description: Adapt this repo's ticket analytics plugins (WebAssembly, analytics/plugins) to another Daml model, so the canton-analytics daemon and the explorer's Dashboard tab chart that model's business events. Use when asked to make dashboard plugins, charts or analytics tables for a different Daml model, or to change what the ticket plugins count.
---

# Adapt the analytics plugins to another Daml model

The plugins in `analytics/plugins` turn the event graph that the syncer
writes into SurrealDB into small Delta Lake tables, and say which charts the
explorer's **Dashboard** tab draws from them. They are written for the ticket
model in `daml/daml/Main.daml`: they count each accepted `TicketOffer`
(its `Accept` choice) per day, and add up the offers' prices.

Adapting them to another model means changing four things, and nothing else:

1. **What is read**: the SurrealQL query, and the Arrow schema of its rows.
2. **What is kept**: the per-key totals in the plugin's memory.
3. **What is written**: the output table's columns and its key.
4. **What is shown**: the dashboard panels' SQL and titles.

The plugin interface (`analytics/wit/analytics.wit`), the high-water query,
the Arrow and IPC plumbing and the `plugin!` macro pattern stay as they are.

Read these before you change code:

- `analytics/wit/analytics.wit`: the contract with the daemon. Its doc
  comments are the rules.
- `analytics/plugins/ticket-totals/src/lib.rs`: the shared code; every
  ticket-specific line is in it.
- [references/event-graph.md](references/event-graph.md): the SurrealDB
  records the syncer writes, and how Daml values are stored in them.
- [references/ticket-plugins.md](references/ticket-plugins.md): each
  ticket-specific line in the plugins, and what it becomes for a new model.

## Step 1: Pick the business event and the numbers

Read the target model's `.daml` files (or ask for them). With the person,
settle on:

- **The event** that marks one business result: usually one choice being
  exercised (`Accept`, `Settle`, `Approve`), sometimes one template being
  created. Prefer the choice that completes the deal over the one that
  proposes it. A consuming choice on an offer or proposal is the usual pick.
- **The template that owns it.** Choice names repeat across templates, so
  always filter on the template too.
- **The measures**: a count, and sums of numeric fields (an amount, a
  quantity). Say where each field lives: in the choice's argument, in the
  contract it was exercised on (its `create_arguments`), or in a contract it
  created.
- **The time**: the transaction's effective time
  (`exercised.transaction_effective_at`, `created.created_at`). Days are UTC.
- **The key**: per day is the default. A breakdown (per instrument, per
  counterparty) makes the key `(day, <dimension>)`; keep dimensions with
  few values, since the dashboard reads the whole table.

If the request is vague ("analytics for my model"), propose one event and
one or two measures, say which you chose and why, and go ahead.

## Step 2: Write and try the query

The daemon runs the query once per step with `$from` (exclusive) and `$to`
(inclusive) bound to ledger offsets. Start from the ticket query:

```sql
SELECT offset, transaction_effective_at AS agreed_at,
    <float> type::record('created', target_contract_id).create_arguments.price AS price
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'TicketOffer'
    AND offset > $from AND offset <= $to;
```

Rules (details in [references/event-graph.md](references/event-graph.md)):

- Always keep `offset > $from AND offset <= $to`, on the table you select
  from. Every other filter is up to you.
- Give every column a flat name with `AS`. The daemon reads each row's
  fields by name, so a nested path such as `create_arguments.amount` must be
  aliased (`create_arguments.amount AS amount`).
- Filter on `template.entity_name` (and `template.module_name` if two modules
  share a name). Add `template.package_name` to tell packages apart; do not
  filter on `package_id`, which changes with every package version.
- A field of the exercised contract is
  `type::record('created', target_contract_id).create_arguments.<field>`.
  A field of a contract the choice created can be read through
  `result_contract_ids`, for example
  `type::record('created', result_contract_ids[0]).create_arguments.<field>`,
  when the choice returns that contract id.
- Daml `Numeric`/`Decimal` is stored as a SurrealDB decimal. Declare the
  input column `Float64` (a `<float>` cast is optional). `Int` is an
  integer (`Int64`), `Party`, `Text`, `ContractId` and enums are strings
  (`Utf8`), `Time` and `Date` are datetimes (`Timestamp(µs, "UTC")`).
- Contracts loaded from the ACS have `offset = -1`, so they never fall in a
  step's range and are not counted. Say so if it matters for the model.
- More than one statement is allowed (one Arrow schema each, in order), for
  example one for creates and one for exercises. Prefer one statement when
  you can: plugins with the same query and range share one fetch.

Try the query on a real or test database before writing Rust. Replace
`$from` and `$to` with numbers, and run it in the explorer's query panel or
with `surreal sql` against the syncer's database. Check that every column
has values of the type you expect, and that the row count matches the
events you know of.

## Step 3: Change the code

Keep the ticket plugins as they are, as a working reference, unless the
person asks to replace them. Make new crates next to them, named after the
model (below, `loan` stands for it):

1. Copy `ticket-totals` to `loan-totals`, and each thin plugin crate
   (`ticket-agreements`, `ticket-value`) to a crate per measure
   (`loan-count`, `loan-volume`). Rename the packages, the macro calls
   (`loan_totals::plugin!("loan-count", loan_totals::Measure::Count)`), the
   `[workspace.dependencies]` entry and the `members` list in
   `analytics/plugins/Cargo.toml`.
2. In `loan-totals/src/lib.rs`, change exactly the places listed in
   [references/ticket-plugins.md](references/ticket-plugins.md): `QUERY`,
   `input_schema`, the function that reads the query's rows, `Measure`
   (variants, columns, types), `Spec::description`, `Spec::chart_title`,
   the per-key state struct and its `load_state`/`save_state` encoding, and
   the unit tests.
3. If the key gains a dimension, change `mode()` in the macro to
   `WriteMode::Merge(vec![DAY.into(), "<dimension>".into()])`, add the
   column to `output_schema`, key the state map on `(day, dimension)`, and
   fill quiet days per dimension that already has a row.
4. Panels: one chart per plugin is the default. Keep `{period}` and the
   Monday origin `TIMESTAMP '2001-01-01T00:00:00Z'` so the Day / Week switch
   works. Panel kinds and their columns are in the WIT file: `bar`
   `(label, value)`, `timeseries` `(time, value)` or
   `(time, value, series)` for a breakdown, `stats` one row, `table` any.
   The table's SQL name is the plugin name with `-` as `_`.
5. Update the plugin list in the justfile's `analytics-plugins` recipe (the
   `for p in ...` loop) and the README's Analytics section.
6. Build once without `--locked` (step 4) so `analytics/plugins/Cargo.lock`
   gains the new crates, and commit it: the justfile builds with `--locked`.

Keep these properties, which the daemon relies on:

- `transform` reads and checks every input before it changes the state, so
  a failed call changes nothing.
- `transform` is deterministic: the same state and inputs give the same rows
  and the same new state. No clocks, no randomness, no `HashMap` iteration
  order in the output or the saved state (use `BTreeMap`).
- In merge mode a step returns a full row for every key it touched, with the
  running total, not the step's increment: the row replaces the old one.
- The state holds everything a later step needs to rewrite a row.
- A step with no events returns no rows.

## Step 4: Test

1. Host unit tests: `cd analytics/plugins && cargo test -p loan-totals`.
   Adapt the ticket tests: days add up, quiet days get a 0, the state
   survives `save_state`/`load_state`, a wrong column type is an error that
   leaves the state empty, an empty step writes nothing.
2. Build the components: `cd analytics/plugins && cargo build --release
   --target wasm32-wasip2` (needs `rustup target add wasm32-wasip2`).
3. End to end: `analytics/bench` runs the ticket plugins through the real
   daemon over `analytics/bench/tests/purchases.surql`, a few records in the
   syncer's shape. For the new plugins, write a similar `.surql` fixture with
   a few of the model's events (only the fields the query reads, plus
   `offset`, and a `sync_state:stream` with the last offset) and a test like
   `analytics/bench/tests/plugins.rs`. Run it with
   `cargo test --release -p ticket-bench`. Leave the ticket benchmark alone
   unless asked.
4. Live: run the daemon on the syncer's database with the new `.wasm` files
   (see the canton-analytics README for `canton-analytics run` and its
   `SURREAL_*` variables; in this repo `just analytics` does it for the
   demo's ledger) and open the dashboard.

If the query or the output schema of a table that already exists changes,
stop the daemon and delete that plugin's table folder before running it
again: the table keeps its old rows, columns and cursor otherwise.

## Done means

- The query was tried on data and returns the expected rows and types.
- `cargo test` passes for the new totals crate, and the workspace builds for
  `wasm32-wasip2`.
- Each new plugin has a dashboard panel whose SQL names its table.
- The justfile and the README list the new plugins.
- The summary to the person names the event, the measures, the key and any
  events that are not counted (for example contracts loaded from the ACS).
