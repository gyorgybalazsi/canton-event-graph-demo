# The ticket plugins, item by item

`analytics/plugins` is a Cargo workspace of its own, built for
`wasm32-wasip2`:

| Crate | What it is |
|---|---|
| `ticket-totals` | A plain library with all the logic, and the `plugin!` macro that makes a crate into a plugin. Its unit tests run on the host. |
| `ticket-agreements` | One line: `ticket_totals::plugin!("ticket-agreements", ticket_totals::Measure::Agreements);`. A `cdylib`, so it builds to `ticket_agreements.wasm`. |
| `ticket-value` | The same, with `Measure::Value`. |

Both plugins run the same query, so the daemon fetches each range once for
both. Keep that pattern for a new model: one totals crate, one thin crate
per measure.

## What each item in `ticket-totals/src/lib.rs` does, and what to do with it

| Item | Ticket-specific? | For a new model |
|---|---|---|
| Crate docs (`//!`) | Yes | Describe the new event, measures and key. |
| `QUERY` | Yes | The new query (SKILL.md, step 2). Keep the offset range filter. |
| `plugin!` macro | No, except `mode()` | Keep it. Change `mode()` only if the key changes, and `PanelKind`/`time_column` only if the chart's kind changes. The `path: "../../wit"` assumes the crate sits next to the ticket ones. |
| `HIGH_WATER` | No | Keep as is. |
| `DAY_US`, `DAY`, `day_start` | No | Keep. `DAY` (`day_start`) is the key column and the chart's time column. |
| `Measure` | Yes | One variant per measure. `column()` names the output column, `data_type()` gives its Arrow type (`Int64` for counts, `Float64` for sums). |
| `Spec::description` | Yes | The plugin's one-line description, shown in the catalog. |
| `Spec::chart_title` | Yes | The panel's title. |
| `Spec::chart_sql` | No | Keep: it already uses the measure's column and the table name. Change it to add a breakdown (`GROUP BY 1, <dimension>`) or another panel. |
| `Spec::output_schema` | Partly | `day_start` plus the measure. Add a dimension column if the key gets one. |
| `input_schema` | Yes | One field per column the query returns, by its `AS` name, with the type from `references/event-graph.md`. |
| `ipc`, `timestamp` | No | Keep. |
| `Day` struct, `DAYS` | Yes | The running totals per key: one field per measure. Key the `BTreeMap` on `(i64, String)` for a dimension. |
| `load_state`, `save_state` | Yes | Encode the new state; keep the encoding sorted and stable (a `Vec` of tuples from the `BTreeMap`, as now). |
| `agreements` | Yes | Reads the query's rows from Arrow. Rename it after the event; read each column by name with the right `as_primitive_opt::<..>()` / `as_string_opt::<i32>()`, and decide what a null means (skip the row, or count it as 0). |
| `column` | Mostly | Change the "accepted offers" wording in its error. |
| `transform` | Partly | Keep the shape: check `results.len()`, read all rows first, then update the state, then emit every key from the earliest touched day to the newest, quiet days as 0. Change the per-row update and the per-measure builders. |
| `tests` | Yes | Same cases with the new columns (SKILL.md, step 4). |

## Elsewhere in the repo

| File | What to change |
|---|---|
| `analytics/plugins/Cargo.toml` | `members`, and the totals crate under `[workspace.dependencies]`. |
| `analytics/plugins/<crate>/Cargo.toml` | Package name and description; the dependency on the new totals crate. |
| `justfile`, recipe `analytics-plugins` | The `for p in ...` list of `.wasm` files to copy (crate names with `_`). |
| `justfile`, recipe `analytics` | The database name (`SURREAL_DATABASE`) if the model's ledger is synced into another database. |
| `README.md`, section Analytics | What the new plugins count, and their charts. |
| `analytics/bench` | Ticket-only (its fixtures and made-up ledger are tickets). Add a test for the new plugins next to `tests/plugins.rs` with its own `.surql` fixture; leave the benchmark itself unless asked. |
