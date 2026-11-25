# Analytics benchmark results

Measured numbers for the two ticket analytics benchmarks, kept here to use in
talks and in a click-through demo video. Each run says where and when it was
measured. Add new runs below; don't overwrite old ones.

## The finding in one line

The analytics daemon caught up on a ledger of a million purchases in under half
a minute. After that, each dashboard chart answered in a few milliseconds,
about 1,000 times faster than asking SurrealDB for the same numbers.

## Demo B: from SurrealDB, `just analytics-bench-surreal`

### 1 million purchases on György's Mac, 2026-11-25

A purchase is two ledger offsets: the `TicketOffer` contract is created, then
its `Accept` choice is exercised. So 1 million purchases make 2 million
offsets. The data is loaded into a SurrealDB of the benchmark's own, on disk
(SurrealKV).

| Step | Time | Rate |
|---|---|---|
| Load into SurrealDB, in the syncer's shape | 53.3 s | 18,751 purchases/s |
| Daemon catch-up from offset 0, both plugins, into Delta tables | 27.2 s | 73,455 offsets/s (36,728 purchases/s) |
| SurrealQL `GROUP BY` over the raw records, per day | 4,744 ms | 365 rows |
| SurrealQL `GROUP BY` over the raw records, per week | 4,699 ms | 53 rows |
| DataFusion over the Delta table, ticket-agreements per day | 4 ms | 365 rows |
| DataFusion, ticket-agreements per week | under 1 ms | 53 rows |
| DataFusion, ticket-value per day | under 1 ms | 365 rows |
| DataFusion, ticket-value per week | under 1 ms | 53 rows |

Cross-check: both gave the same agreements and value for every day and week.

The raw output:

```text
load: 1000000 purchases (2000000 records) into SurrealDB in 53.3 s (18751 purchases/s)
catch-up: the daemon read 2000000 offsets from SurrealDB through 2 plugins into Delta in 27.2 s (73455 offsets/s, 36728 purchases/s); the tables have 1128 rows
head to head: the same numbers from SurrealDB (raw records) and from the Delta tables (one row per day, kept up to date by the plugins)
  SurrealQL, per day:    4744 ms (365 rows)
  SurrealQL, per week:    4699 ms (53 rows)
  DataFusion, ticket-agreements per Day:    4 ms (365 rows)
  DataFusion, ticket-agreements per Week:    0 ms (53 rows)
  DataFusion, ticket-value per Day:    0 ms (365 rows)
  DataFusion, ticket-value per Week:    0 ms (53 rows)
  both give the same agreements and value for every day and week
```

### 20,000 purchases on a 4-core cloud machine, 2026-11-25

These used an in-memory SurrealDB and a release build, with the plugins from
#24.

| Step | Time |
|---|---|
| Load | 3.3 s (about 6,000 purchases/s) |
| Catch-up, 40,000 offsets | 0.4 s (about 100,000 offsets/s) |
| SurrealQL per day / week | 265 / 304 ms |
| DataFusion per chart | 3–5 ms |

### What the numbers say

- **Catching up is quick.** A daemon started on an existing ledger of a
  million purchases is up to date in under half a minute. After that it reads
  only new offsets, so in live use the delay before a purchase shows up is set
  by the dashboard's lag settings, not by how fast the daemon works.
- **SurrealDB slows down as the ledger grows.** The same chart took about
  0.27 s at 20,000 purchases and 4.7 s at 1 million, roughly in step with the
  ledger. At 10 million it would be around 45–50 s. That figure is an
  extrapolation, not a measurement.
- **The Delta tables don't.** They hold one row per day, which the plugins keep
  up to date, so a chart reads a few hundred rows however big the ledger
  gets: a few milliseconds at most.
- **The gap is about 1,000 times at 1 million purchases**, and it widens as
  the ledger grows.
- **Loading is the syncer's job**, done once as the ledger comes in. The Mac
  loaded about 3 times faster than the cloud machine.

## Demo A: tables only, `just analytics-bench`

### 10 million purchases on a 4-core cloud machine, 2026-11-24

There's no SurrealDB in this one: the made-up rows go straight through the
plugins and the daemon's table writer.

| Step | Time |
|---|---|
| Each plugin, 10 million offsets into its table | about 5 s |
| Each chart afterwards | 6–7 ms |

## For the demo video

Captions that fit on screen:

1. "1 million ticket purchases: 2 million ledger offsets in SurrealDB"
2. "Analytics daemon catches up: 27 seconds"
3. "Same chart straight from SurrealDB: 4.7 seconds"
4. "From the Delta tables: under 5 milliseconds"
5. "About 1,000 times faster, and the gap grows with the ledger"
6. "Both give the same numbers for every day and week"

Suggested beats:

1. **Terminal:** run `just analytics-bench-surreal 100000`. That size loads in
   seconds, which is better on camera. Show the load, catch-up and head-to-head
   lines as they print, and pause on the head-to-head block.
2. **Cut to a caption card** with the 1-million figures from the table above,
   since that run takes about 1.5 minutes.
3. **Explorer:** `just analytics-tools && just analytics-dashboard
   .demo/analytics/bench-surreal-tables`. Open the Dashboard tab, show the
   column chart, and switch Day ↔ Week. Each chart shows its own query time.
4. **Close on the one-line finding** above.

Caveats to keep in the narration:

- The data is made up. It follows the demo's ticket model and varies by
  weekday, season and on-sale spikes.
- SurrealQL is timed over raw records and DataFusion over pre-aggregated
  tables. That is the point of the comparison, not an engine-against-engine
  benchmark.
- "1128 rows" in the catch-up line counts rows written, including days
  rewritten at commit boundaries. The tables themselves hold one row per day.
