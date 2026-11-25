//! `ticket-bench surreal`: the same made-up year of purchases, but loaded
//! into SurrealDB in the shape the syncer writes, so the whole path is timed:
//!
//! 1. **Load**: the purchases into SurrealDB, with the syncer's tables and
//!    indexes for them.
//! 2. **Catch-up**: the analytics daemon's own runner reads them from
//!    SurrealDB, through the plugins, into Delta tables, as a daemon started
//!    on an existing ledger would.
//! 3. **Head to head**: the same per-day and per-week numbers asked of
//!    SurrealDB directly (SurrealQL over the raw records) and of the tables
//!    (DataFusion, the dashboard's own queries).
//!
//! Each purchase is two records: the `TicketOffer` contract (in `created`)
//! and its `Accept` choice (in `exercised`), at two offsets. The cash and the
//! ticket contracts the real model also makes are left out: the plugins
//! don't read them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use analytics_reader::location::Location;
use analytics_reader::{Session, dashboard};
use anyhow::{Context, Result, bail, ensure};
use canton_analytics::fetch::Fetcher;
use canton_analytics::plugin;
use canton_analytics::runner::{Options, Runner};
use canton_analytics::source::{self, Connection, Db, UserLevel};
use futures_util::future::join_all;

use crate::{Calendar, Purchase, purchase};

/// Purchases per load statement.
const LOAD_BATCH: i64 = 5_000;

/// The syncer's tables and indexes for what is loaded here (from its
/// `store::SCHEMA`). The offset index is built up front, not `CONCURRENTLY`,
/// as the tables are still empty.
const SCHEMA: &str = "
DEFINE TABLE IF NOT EXISTS created TYPE NORMAL SCHEMALESS;
DEFINE TABLE IF NOT EXISTS exercised TYPE NORMAL SCHEMALESS;
DEFINE TABLE IF NOT EXISTS sync_state TYPE NORMAL SCHEMALESS;
DEFINE INDEX IF NOT EXISTS created_offset_node ON created FIELDS offset, node_id;
DEFINE INDEX IF NOT EXISTS created_template_name ON created FIELDS template_name;
DEFINE INDEX IF NOT EXISTS exercised_choice_name ON exercised FIELDS choice_name;
DEFINE INDEX IF NOT EXISTS exercised_target_contract ON exercised FIELDS target_contract_id;
DEFINE INDEX IF NOT EXISTS exercised_offset ON exercised FIELDS offset;
";

/// The per-day numbers the plugins' tables hold, asked of SurrealDB itself.
const SURREAL_DAYS: &str = "\
SELECT day, count() AS agreements, math::sum(price) AS value FROM ( \
    SELECT time::floor(transaction_effective_at, 1d) AS day, \
        <float> type::record('created', target_contract_id).create_arguments.price AS price \
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'TicketOffer' \
) GROUP BY day ORDER BY day;";

/// The same by Monday-to-Sunday week. `time::floor` counts weeks from
/// 1970-01-01, a Thursday, so the times are moved three days on and the
/// weeks back again.
const SURREAL_WEEKS: &str = "\
SELECT week, count() AS agreements, math::sum(price) AS value FROM ( \
    SELECT time::floor(transaction_effective_at + 3d, 1w) - 3d AS week, \
        <float> type::record('created', target_contract_id).create_arguments.price AS price \
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'TicketOffer' \
) GROUP BY week ORDER BY week;";

pub struct Args {
    pub url: String,
    pub namespace: String,
    pub database: String,
    pub user: String,
    pub pass: String,
    pub rows: i64,
    pub tables: String,
    pub plugins: Vec<PathBuf>,
}

pub async fn run(a: Args) -> Result<()> {
    ensure!(a.rows > 0, "--rows must be at least 1");
    let db = source::connect(&Connection {
        url: a.url.clone(),
        namespace: a.namespace.clone(),
        database: a.database.clone(),
        // An in-process database has no users to sign in as.
        username: if a.url.starts_with("mem://") {
            String::new()
        } else {
            a.user.clone()
        },
        password: a.pass.clone(),
        user_level: UserLevel::Root,
    })
    .await
    .with_context(|| format!("could not connect to SurrealDB at {}", a.url))?;
    let tables = Location::parse(&a.tables, HashMap::new())?;

    let loaded = load(&db, a.rows).await?;
    println!(
        "load: {} purchases ({} records) into SurrealDB in {:.1} s ({:.0} purchases/s)",
        a.rows,
        2 * a.rows,
        loaded,
        a.rows as f64 / loaded
    );

    let (seconds, rows) = catch_up(&db, &tables, &a.plugins).await?;
    println!(
        "catch-up: the daemon read {} offsets from SurrealDB through {} plugins into Delta in {:.1} s \
         ({:.0} offsets/s, {:.0} purchases/s); the tables have {rows} rows",
        2 * a.rows,
        a.plugins.len(),
        seconds,
        2.0 * a.rows as f64 / seconds,
        a.rows as f64 / seconds,
    );

    head_to_head(&db, &tables, a.rows).await
}

/// Loads `rows` purchases and the syncer's high-water mark; returns seconds.
async fn load(db: &Db, rows: i64) -> Result<f64> {
    db.query(SCHEMA).await?.check()?;
    let mut existing = db
        .query("SELECT VALUE count() FROM exercised GROUP ALL")
        .await?
        .check()?;
    let existing: Vec<i64> = existing.take(0)?;
    if existing.first().copied().unwrap_or(0) > 0 {
        bail!("the database already has exercised choices: use an empty database");
    }
    let calendar = Calendar::new(rows);
    let started = Instant::now();
    let mut next_report = rows / 10;
    let mut from = 1;
    while from <= rows {
        let to = rows.min(from + LOAD_BATCH - 1);
        db.query(purchases_sql(&calendar, from, to))
            .await?
            .check()
            .with_context(|| format!("loading purchases {from} to {to}"))?;
        if to >= next_report && to < rows {
            eprintln!(
                "  loaded {to} of {rows} ({:.0} s)",
                started.elapsed().as_secs_f64()
            );
            next_report += rows / 10;
        }
        from = to + 1;
    }
    db.query("UPSERT sync_state:stream CONTENT { last_offset: $offset }")
        .bind(("offset", 2 * rows))
        .await?
        .check()?;
    Ok(started.elapsed().as_secs_f64())
}

/// Purchases `from..=to` as the syncer writes them: purchase `n` offers its
/// ticket at offset `2n - 1` and accepts it at offset `2n`, a minute later.
fn purchases_sql(calendar: &Calendar, from: i64, to: i64) -> String {
    const OFFER: &str = "{ package_id: 'pkg', module_name: 'Main', entity_name: 'TicketOffer' }";
    let all: Vec<(i64, Purchase)> = (from..=to).map(|n| (n, purchase(calendar, n))).collect();
    let mut sql = String::with_capacity(all.len() * 700);
    sql.push_str("INSERT INTO created [");
    for (n, p) in &all {
        let cid = contract_id(*n);
        sql.push_str(&format!(
            "{{ id: '{cid}', contract_id: '{cid}', name: 'TicketOffer@{offset}', template_name: 'Main.TicketOffer', \
             template: {OFFER}, offset: {offset}, node_id: 0, signatories: ['{org}'], created_at: d'{at}', \
             create_arguments: {{ organizer: '{org}', buyer: '{buyer}', price: {price:.2}dec }} }},",
            offset = 2 * n - 1,
            org = p.organizer,
            buyer = p.buyer,
            at = rfc3339(p.at - 60_000_000),
            price = p.price,
        ));
    }
    sql.push_str("];\nINSERT INTO exercised [");
    for (n, p) in &all {
        sql.push_str(&format!(
            "{{ id: [{offset}, 0], name: 'Accept@{offset}', choice_name: 'Accept', template: {OFFER}, \
             target_contract_id: '{cid}', acting_parties: ['{buyer}'], consuming: true, offset: {offset}, \
             node_id: 0, transaction_effective_at: d'{at}', choice_argument: {{}} }},",
            offset = 2 * n,
            cid = contract_id(*n),
            buyer = p.buyer,
            at = rfc3339(p.at),
        ));
    }
    sql.push_str("];");
    sql
}

/// A contract id in the ledger's hex style.
fn contract_id(n: i64) -> String {
    format!("00{n:016x}")
}

/// Runs every plugin's runner from SurrealDB to the high-water mark, all at
/// once and sharing fetches, as the daemon does; returns seconds and the
/// rows in the tables.
async fn catch_up(db: &Db, tables: &Location, plugins: &[PathBuf]) -> Result<(f64, i64)> {
    let engine = plugin::engine()?;
    let fetcher = Fetcher::new(db.clone());
    let mut runners = Vec::new();
    for path in plugins {
        let runner = Runner::new(&engine, path, tables, fetcher.clone(), Options::default())
            .await
            .with_context(|| format!("could not start {}", path.display()))?;
        ensure!(
            runner.committed == 0,
            "{} already has rows: remove the tables first",
            runner.table.location
        );
        runners.push(runner);
    }
    let started = Instant::now();
    let summaries = join_all(runners.iter_mut().map(|r| r.catch_up(true))).await;
    let seconds = started.elapsed().as_secs_f64();
    let mut rows = 0;
    for s in summaries {
        rows += s?.rows as i64;
    }
    Ok((seconds, rows))
}

/// The day and week numbers from SurrealDB and from the tables, timed, and
/// checked against each other.
async fn head_to_head(db: &Db, tables: &Location, rows: i64) -> Result<()> {
    println!(
        "head to head: the same numbers from SurrealDB (raw records) and from the Delta tables"
    );
    let mut counted = Vec::new();
    for (name, sql) in [("day", SURREAL_DAYS), ("week", SURREAL_WEEKS)] {
        let started = Instant::now();
        let mut response = db.query(sql).await?.check()?;
        let agreements: Vec<i64> = response.take("agreements")?;
        let millis = started.elapsed().as_millis();
        let total: i64 = agreements.iter().sum();
        ensure!(
            total == rows,
            "SurrealDB counted {total} agreements by {name}, not {rows}"
        );
        println!(
            "  SurrealQL, per {name}: {millis:>7} ms ({} rows)",
            agreements.len()
        );
        counted.push(agreements);
    }
    for table in ["ticket-agreements", "ticket-value"] {
        let session = Session::open(&tables.join(table)).await?;
        for (period, surreal) in [dashboard::Period::Day, dashboard::Period::Week]
            .into_iter()
            .zip(&counted)
        {
            let started = Instant::now();
            let board = dashboard::load_with(&session, period).await?;
            let millis = started.elapsed().as_millis();
            let panel = board.panels.first().context("no panel")?;
            let frame = panel.frame.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "  DataFusion, {table} per {period:?}: {millis:>4} ms ({} rows)",
                frame.rows()
            );
            // The daemon's tables must say what SurrealDB says, period by period.
            if table == "ticket-agreements" {
                let column = frame.column("agreements").context("no agreements column")?;
                let ours: Vec<i64> = (0..frame.rows())
                    .map(|i| column.values.number(i).unwrap_or(0.0) as i64)
                    .collect();
                ensure!(
                    &ours == surreal,
                    "the tables' agreements per {period:?} differ from SurrealDB's"
                );
            }
        }
    }
    println!("  both give the same agreements for every day and week");
    Ok(())
}

/// `2026-01-01T09:00:00.000000Z` for microseconds since 1970 (UTC).
fn rfc3339(us: i64) -> String {
    let (days, rest) = (us.div_euclid(86_400_000_000), us.rem_euclid(86_400_000_000));
    let (y, m, d) = civil_from_days(days);
    let s = rest / 1_000_000;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:06}Z",
        s / 3_600,
        s % 3_600 / 60,
        s % 60,
        rest % 1_000_000
    )
}

/// Days since 1970-01-01 to (year, month, day), from Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    #[test]
    fn times_print_as_surrealql_datetimes() {
        assert_eq!(
            rfc3339(1_767_225_600_000_000),
            "2026-01-01T00:00:00.000000Z"
        );
        assert_eq!(
            rfc3339(1_767_225_600_000_000 + 86_399_000_001),
            "2026-01-01T23:59:59.000001Z"
        );
    }
}
