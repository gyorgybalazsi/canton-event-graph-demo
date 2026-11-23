# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-23

The first release of the end-to-end demo: a Canton sandbox, a Rust ticket
purchase client, the syncer, SurrealDB and surrealdb-explorer, started with one
command.

### Added
- `just demo` builds the Daml model, starts SurrealDB and opens the explorer on
  the empty graph, starts the sandbox and the syncer, then buys one ticket step
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

### Changed
- The tools are pinned to their 0.1.0 release tags: canton-event-graph-syncer
  v0.1.0 (also the app's `client`, `ledger-api`, `daml-type-rep` and
  `derive-lapi-access` crates), daml-rust-codegen v0.1.0 and surrealdb-explorer
  v0.1.0. Before, they were pinned to commits.
- tonic 0.14, matching the syncer.
- Output is one line per step instead of cargo, rustup and SurrealDB output,
  with short party ids.

### Fixed
- `just tickets` printed `price: null`; it now prints one line per ticket.
- `just tools` no longer fetches a tool again when it is already installed at
  the pinned version, or when the explorer was installed from a local clone.
- `just stop` waits up to 30 s for each service's processes to exit, so
  `just demo` straight after `just demo-reset` no longer fails with "port
  already in use".

[Unreleased]: https://github.com/gyorgybalazsi/canton-event-graph-demo/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/gyorgybalazsi/canton-event-graph-demo/releases/tag/v0.1.0
