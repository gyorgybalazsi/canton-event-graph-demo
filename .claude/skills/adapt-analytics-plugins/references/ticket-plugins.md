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
| `FIRST_OFFSET`, `LAST_OFFSET` | No | Keep. The explorer finds a chart column's transactions by these column names. |
| `Measure` | Yes | One variant per measure. `column()` names the output column, `data_type()` gives its Arrow type (`Int64` for counts, `Float64` for sums), `whole()` says how the dashboard rounds it (money as `round(..)`). |
| `Spec::description` | Yes | The plugin's one-line description, shown in the catalog. |
| `Spec::headline_title`, `Spec::chart_title` | Yes | The two panels' titles. |
| `Spec::headline_sql` | No | Keep: last full period, the one before, the change, newest so far, best, total, named with `{period_name}`. Change only to drop or add a figure. |
| `Spec::chart_sql` | No | Keep: it already uses the measure's column, the table name, and returns `first_offset`/`last_offset` per column for the click-through. Change it to add a breakdown (`GROUP BY 1, <dimension>`) or another panel. |
| `Spec::table` | No | Keep: the plugin name with `-` as `_`. |
| `Spec::output_schema` | Partly | `day_start`, the measure, `first_offset`, `last_offset`. Add a dimension column if the key gets one. |
| `input_schema` | Yes | One field per column the query returns, by its `AS` name, with the type from `references/event-graph.md`. |
| `ipc`, `timestamp` | No | Keep. |
| `Day` struct, `DAYS` | Yes | The running totals per key: one field per measure, plus `offsets` (first and last). Key the `BTreeMap` on `(i64, String)` for a dimension. |
| `SavedDay`, `load_state`, `save_state` | Yes | Encode the new state, offsets included; keep the encoding sorted and stable (a `Vec` of tuples from the `BTreeMap`, as now). |
| `agreements` | Yes | Reads the query's rows from Arrow, `offset` first. Rename it after the event; read each column by name with the right `as_primitive_opt::<..>()` / `as_string_opt::<i32>()`, and decide what a null means (skip the row, or count it as 0). A row without an offset or a time is skipped. |
| `column` | Mostly | Change the "accepted offers" wording in its error. |
| `transform` | Partly | Keep the shape: check `results.len()`, read all rows first, then update the state (widening the day's offsets), then emit every key from the earliest touched day to the newest, quiet days as 0 with null offsets. Change the per-row update and the per-measure builders. |
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
