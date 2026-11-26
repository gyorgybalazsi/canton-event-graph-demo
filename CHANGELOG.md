# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Analytics: two canton-analytics plugins for the ticket model,
  `ticket-agreements` and `ticket-value`, each a column chart of the new
  agreements (or their value) per day or per ISO week (the dashboard's
  Day / Week switch); `just analytics` runs the daemon on the demo's ledger
  next to the explorer's dashboard, and `just analytics-bench` times it over
  a made-up year of 10 million purchases that vary by weekday and season.
- `just analytics-bench-surreal` loads a made-up ledger (default 1 million
  purchases, each the demo's whole flow of three transactions, browsable in
  the explorer's Canton view) into a SurrealDB of its own as the syncer
  writes it, times the daemon's catch-up from offset 0, and times the same
  per-day and per-week numbers in SurrealQL and in DataFusion, checking that
  they agree on the agreements and the value of every day and week.
- The ticket dashboards show headline figures above the chart (the newest
  day or week, the one before, the change, the best, the total), and each
  day's row keeps the ledger offsets of its first and last agreement, so a
  click on a column in the explorer shows that day's or week's transactions.
  The tables gained two columns: remove tables written by an earlier version
  (`just demo-reset`, or a fresh `just demo` with SurrealDB stopped).

### Fixed
- `just surreal` starts SurrealDB 4 with `surreal db start`; SurrealDB 4 moved
  the server command under `db`.
- `just analytics` checks that the demo's SurrealDB port is listening before
  creating its database user or starting the daemon, and tells you to run
  `just demo` if it is not.

### Changed
- The ticket plugins' query reads only an agreement's time and price; it no
  longer looks up each offer's organizer, which nothing charts (catch-up about
  15% faster on 50,000 purchases). They read those columns by name and give an
  error, not a crash, if one has the wrong type. The two plugin crates share
  their glue through `ticket_totals::plugin!`.
- `just analytics` restarts a running daemon, so it loads the plugins it has
  just built.
- `just analytics-bench` times the dashboards per day and per week.
- The README no longer calls surrealdb-explorer private: all three tools install
  from their public repositories. `EXPLORER_PATH` stays, for trying a local
  clone of the explorer.
- The analytics pins point at the merged commits: the syncer's analytics crates
  at `bdb8852` and the explorer's dashboard at `f51d815`, with fewer tick
  labels, abbreviated numbers, fainter major gridlines, no minor gridlines,
  darker chart axes and dashboard settings for the daemon's lag.
- The analytics explorer pin moves to `b533d93` (explorer main), which adds
  the Canton items to the record menu (lifecycle, show in table, filters,
  more copies) and copying or saving a picture of the whole graph canvas.

## [0.1.0] - 2026-11-23

The first release of the end-to-end demo: a Canton sandbox, a Rust ticket
purchase client, the syncer, SurrealDB and surrealdb-explorer, started with one
command.

### Added
- `just demo` installs the tools, starts SurrealDB and opens the explorer on the
  empty graph, builds the Daml model, starts the sandbox and the syncer, then buys one ticket step
  by step (`DEMO_PAUSE` seconds after each transaction, default 4) and two more
  at full speed.
- `app/` (`ticket-purchase`): a Rust client that submits the purchase flow to
  the sandbox, using types generated from `daml/Main.daml` by daml-rust-codegen
  at build time.
- Recipes: `tools`, `build-dar`, `sandbox`, `surreal`, `setup`, `sync`,
  `explorer`, `explorer-restart`, `explorer-limit n` (only the latest *n*
  transactions), `purchase`, `tickets` (each ticket agreement with the price
  paid, found by walking the graph), `show <offset>`, `status`, `logs`, `stop`
  and `demo-reset`.
- The explorer follows the newest transactions, and can filter the graph per
  party by each event's witness parties.
- `EXPLORER_PATH` installs the explorer from a local clone.
- MIT OR Apache-2.0 license files, `SECURITY.md` and Dependabot config.

### Notes
- The tools are pinned to their 0.1.0 release tags: canton-event-graph-syncer
  v0.1.0 (also the app's `client`, `ledger-api`, `daml-type-rep` and
  `derive-lapi-access` crates), daml-rust-codegen v0.1.0 and surrealdb-explorer
  v0.1.0.

[Unreleased]: https://github.com/gyorgybalazsi/canton-event-graph-demo/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/gyorgybalazsi/canton-event-graph-demo/releases/tag/v0.1.0
