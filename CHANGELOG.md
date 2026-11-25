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

### Fixed
- `just surreal` starts SurrealDB 4 with `surreal db start`; SurrealDB 4 moved
  the server command under `db`.
- `just analytics` checks that the demo's SurrealDB port is listening before
  creating its database user or starting the daemon, and tells you to run
  `just demo` if it is not.

### Changed
- The README no longer calls surrealdb-explorer private: all three tools install
  from their public repositories. `EXPLORER_PATH` stays, for trying a local
  clone of the explorer.
- The analytics dashboard's explorer pin is updated to `2ba607c`, with fewer
  tick labels, abbreviated numbers, fainter major gridlines, no minor
  gridlines and darker chart axes.

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
