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
//! Each purchase is the demo's whole flow (`daml/daml/Main.daml`), three
//! transactions as the syncer writes them, so the explorer's Canton view can
//! browse the result like a real ledger:
//!
//! 1. The bank issues `Cash` to the buyer.
//! 2. The organizer offers a `TicketOffer`.
//! 3. The buyer exercises `Accept`, which transfers the cash (a new `Cash` for
//!    the organizer, the old one archived) and creates the `TicketAgreement`.
//!
//! with the transactions, the parties that requested them, and the
//! `action`, `consequence`, `target`, `consumes` and `requested` links. Only
//! the `Accept` and its offer are what the plugins read; the rest is there so
//! the ledger looks, and costs, like a real one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use analytics_reader::frame::Frame;
use analytics_reader::location::Location;
use analytics_reader::{Session, dashboard};
use anyhow::{Context, Result, bail, ensure};
use canton_analytics::fetch::Fetcher;
use canton_analytics::plugin;
use canton_analytics::runner::{Options, Runner};
use canton_analytics::source::{self, Connection, Db, UserLevel};
use futures_util::future::join_all;
use futures_util::{StreamExt, stream};

use crate::{Calendar, Purchase, purchase};

/// Purchases per load statement.
const LOAD_BATCH: i64 = 2_000;
/// Load statements in flight at once.
const LOAD_PARALLEL: usize = 4;

/// The syncer's tables and indexes for what is loaded here (from its
/// `store::SCHEMA`). The offset indexes are built up front, not
/// `CONCURRENTLY`, as the tables are still empty.
const SCHEMA: &str = "
DEFINE TABLE IF NOT EXISTS transaction TYPE NORMAL SCHEMALESS;
DEFINE TABLE IF NOT EXISTS created TYPE NORMAL SCHEMALESS;
DEFINE TABLE IF NOT EXISTS exercised TYPE NORMAL SCHEMALESS;
DEFINE TABLE IF NOT EXISTS party TYPE NORMAL SCHEMALESS;
DEFINE TABLE IF NOT EXISTS sync_state TYPE NORMAL SCHEMALESS;
DEFINE TABLE IF NOT EXISTS package TYPE NORMAL SCHEMALESS;
UPSERT package:pkg CONTENT { package_id: 'pkg', package_name: 'ticket-purchase', package_version: '0.1.0' } RETURN NONE;
DEFINE TABLE IF NOT EXISTS action TYPE RELATION IN transaction OUT created | exercised SCHEMALESS;
DEFINE TABLE IF NOT EXISTS consequence TYPE RELATION IN exercised OUT created | exercised SCHEMALESS;
DEFINE TABLE IF NOT EXISTS target TYPE RELATION IN exercised OUT created SCHEMALESS;
DEFINE TABLE IF NOT EXISTS consumes TYPE RELATION IN exercised OUT created SCHEMALESS;
DEFINE TABLE IF NOT EXISTS requested TYPE RELATION IN party OUT transaction SCHEMALESS;
DEFINE INDEX IF NOT EXISTS action_in_out ON action FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS consequence_in_out ON consequence FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS target_in_out ON target FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS consumes_in_out ON consumes FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS requested_in_out ON requested FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS created_offset_node ON created FIELDS offset, node_id;
DEFINE INDEX IF NOT EXISTS created_template_name ON created FIELDS template_name;
DEFINE INDEX IF NOT EXISTS exercised_choice_name ON exercised FIELDS choice_name;
DEFINE INDEX IF NOT EXISTS exercised_target_contract ON exercised FIELDS target_contract_id;
DEFINE INDEX IF NOT EXISTS transaction_update_id ON transaction FIELDS update_id;
DEFINE INDEX IF NOT EXISTS transaction_offset ON transaction FIELDS offset;
DEFINE INDEX IF NOT EXISTS exercised_offset ON exercised FIELDS offset;
";

/// Ledger offsets per purchase: cash, offer, accept.
pub const OFFSETS_PER_PURCHASE: i64 = 3;

/// The per-day numbers the plugins' tables hold, asked of SurrealDB itself:
/// each day's start (seconds since 1970), agreements and value.
const SURREAL_DAYS: &str = "\
SELECT period, count() AS agreements, math::sum(price) AS value FROM ( \
    SELECT time::unix(time::floor(transaction_effective_at, 1d)) AS period, \
        <float> type::record('created', target_contract_id).create_arguments.price AS price \
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'TicketOffer' \
) GROUP BY period ORDER BY period;";

/// The same by Monday-to-Sunday week. `time::floor` counts weeks from
/// 1970-01-01, a Thursday, so the times are moved three days on and the
/// weeks back again.
const SURREAL_WEEKS: &str = "\
SELECT period, count() AS agreements, math::sum(price) AS value FROM ( \
    SELECT time::unix(time::floor(transaction_effective_at + 3d, 1w) - 3d) AS period, \
        <float> type::record('created', target_contract_id).create_arguments.price AS price \
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'TicketOffer' \
) GROUP BY period ORDER BY period;";

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
        "load: {} purchases ({} transactions, {} records and links) into SurrealDB in {:.1} s \
         ({:.0} purchases/s)",
        a.rows,
        OFFSETS_PER_PURCHASE * a.rows,
        RECORDS_PER_PURCHASE * a.rows,
        loaded,
        a.rows as f64 / loaded
    );

    let (seconds, rows) = catch_up(&db, &tables, &a.plugins).await?;
    println!(
        "catch-up: the daemon read {} offsets from SurrealDB through {} plugins into Delta in {:.1} s \
         ({:.0} offsets/s, {:.0} purchases/s); the tables have {rows} rows",
        OFFSETS_PER_PURCHASE * a.rows,
        a.plugins.len(),
        seconds,
        (OFFSETS_PER_PURCHASE * a.rows) as f64 / seconds,
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
    // Every party first, so the batches below never write the same record
    // and can run side by side.
    let mut parties = std::collections::BTreeSet::from([BANK.to_string()]);
    for n in 1..=rows {
        let p = purchase(&calendar, n);
        parties.insert(p.buyer);
        parties.insert(p.organizer);
    }
    let parties: Vec<String> = parties.iter().map(|p| party_row(p)).collect();
    db.query(format!(
        "INSERT INTO party [{}] RETURN NONE;",
        parties.join(",")
    ))
    .await?
    .check()
    .context("loading the parties")?;
    let batches: Vec<(i64, i64)> = (0..)
        .map(|i| (1 + i * LOAD_BATCH, rows.min((i + 1) * LOAD_BATCH)))
        .take_while(|(from, _)| *from <= rows)
        .collect();
    let mut next_report = rows / 10;
    let mut loads = stream::iter(batches)
        .map(|(from, to)| {
            let calendar = &calendar;
            async move {
                db.query(purchases_sql(calendar, from, to))
                    .await?
                    .check()
                    .with_context(|| format!("loading purchases {from} to {to}"))?;
                anyhow::Ok(to)
            }
        })
        .buffered(LOAD_PARALLEL);
    while let Some(to) = loads.next().await {
        let to = to?;
        if to >= next_report && to < rows {
            eprintln!(
                "  loaded {to} of {rows} ({:.0} s)",
                started.elapsed().as_secs_f64()
            );
            next_report += rows / 10;
        }
    }
    db.query("UPSERT sync_state:stream CONTENT { last_offset: $offset }")
        .bind(("offset", OFFSETS_PER_PURCHASE * rows))
        .await?
        .check()?;
    Ok(started.elapsed().as_secs_f64())
}

/// Records and links each purchase writes: 3 transactions, 4 contracts,
/// 3 choices, and 15 links (3 `action`, 3 `requested`, 4 `consequence`,
/// 3 `target`, 2 `consumes`). Parties come on top, once each.
const RECORDS_PER_PURCHASE: i64 = 25;

/// The demo's bank, which issues every buyer's cash.
const BANK: &str = "ScroogeBank::1220aa";

/// The contract ids of purchase `n`: its cash, the cash the organizer gets,
/// the offer and the agreement, in the ledger's hex style.
fn contract_ids(n: i64) -> [String; 4] {
    [0, 1, 2, 3].map(|kind| format!("0{kind}{n:016x}"))
}

/// Purchases `from..=to` as the syncer writes them. Purchase `n` is three
/// transactions: the cash at offset `3n - 2`, the offer at `3n - 1` (one and
/// two minutes before the purchase) and the accept at `3n`.
fn purchases_sql(calendar: &Calendar, from: i64, to: i64) -> String {
    let all: Vec<(i64, Purchase)> = (from..=to).map(|n| (n, purchase(calendar, n))).collect();
    let mut tx = Vec::new();
    let mut created = Vec::new();
    let mut exercised = Vec::new();
    let mut action = Vec::new();
    let mut requested = Vec::new();
    let mut consequence = Vec::new();
    let mut target = Vec::new();
    let mut consumes = Vec::new();
    for (n, p) in &all {
        let [cash, paid, offer, agreement] = contract_ids(*n);
        let (o_cash, o_offer, o_accept) = (3 * n - 2, 3 * n - 1, 3 * n);
        let (org, buyer) = (p.organizer.as_str(), p.buyer.as_str());
        let price = format!("{:.2}dec", p.price);
        let at_cash = rfc3339(p.at - 120_000_000);
        let at_offer = rfc3339(p.at - 60_000_000);
        let at = rfc3339(p.at);

        // 1. The bank issues the cash to the buyer.
        tx.push(transaction(o_cash, &at_cash));
        created.push(contract(
            &cash,
            "Cash",
            o_cash,
            0,
            &[BANK],
            &[BANK, buyer],
            &at_cash,
            &format!("{{ issuer: '{BANK}', owner: '{buyer}', amount: {price} }}"),
        ));
        action.push(link(
            [o_cash, 0],
            &format!("transaction:{o_cash}"),
            &created_ref(&cash),
        ));
        requested.push(requested_link(BANK, o_cash));

        // 2. The organizer offers the ticket.
        tx.push(transaction(o_offer, &at_offer));
        created.push(contract(
            &offer,
            "TicketOffer",
            o_offer,
            0,
            &[org],
            &[org, buyer],
            &at_offer,
            &format!("{{ organizer: '{org}', buyer: '{buyer}', price: {price} }}"),
        ));
        action.push(link(
            [o_offer, 0],
            &format!("transaction:{o_offer}"),
            &created_ref(&offer),
        ));
        requested.push(requested_link(org, o_offer));

        // 3. The buyer accepts: node 0 Accept, 1 Transfer, 2 the organizer's
        // cash, 3 the old cash's Archive, 4 the agreement.
        tx.push(transaction(o_accept, &at));
        let witnesses = [org, buyer, BANK];
        exercised.push(choice(
            o_accept,
            0,
            "Accept",
            "TicketOffer",
            &offer,
            buyer,
            true,
            4,
            &witnesses,
            &at,
            &format!("{{ cashId: '{cash}' }}"),
            &agreement,
        ));
        exercised.push(choice(
            o_accept,
            1,
            "Transfer",
            "Cash",
            &cash,
            buyer,
            false,
            3,
            &witnesses,
            &at,
            &format!("{{ newOwner: '{org}' }}"),
            "",
        ));
        created.push(contract(
            &paid,
            "Cash",
            o_accept,
            2,
            &[BANK],
            &witnesses,
            &at,
            &format!("{{ issuer: '{BANK}', owner: '{org}', amount: {price} }}"),
        ));
        exercised.push(choice(
            o_accept, 3, "Archive", "Cash", &cash, buyer, true, 3, &witnesses, &at, "{}", "",
        ));
        created.push(contract(
            &agreement,
            "TicketAgreement",
            o_accept,
            4,
            &[org, buyer],
            &[org, buyer],
            &at,
            &format!("{{ organizer: '{org}', owner: '{buyer}' }}"),
        ));
        let node = |i: i64| format!("exercised:[{o_accept}, {i}]");
        action.push(link(
            [o_accept, 0],
            &format!("transaction:{o_accept}"),
            &node(0),
        ));
        requested.push(requested_link(buyer, o_accept));
        consequence.push(link([o_accept, 1], &node(0), &node(1)));
        consequence.push(link([o_accept, 2], &node(1), &created_ref(&paid)));
        consequence.push(link([o_accept, 3], &node(1), &node(3)));
        consequence.push(link([o_accept, 4], &node(0), &created_ref(&agreement)));
        for (i, contract) in [(0, &offer), (1, &cash), (3, &cash)] {
            target.push(link([o_accept, i], &node(i), &created_ref(contract)));
        }
        for (i, contract) in [(0, &offer), (3, &cash)] {
            consumes.push(link([o_accept, i], &node(i), &created_ref(contract)));
        }
    }

    let mut sql = String::with_capacity(all.len() * 6_000);
    for (table, rows) in [
        ("transaction", &tx),
        ("created", &created),
        ("exercised", &exercised),
    ] {
        sql.push_str(&format!(
            "INSERT INTO {table} [{}] RETURN NONE;\n",
            rows.join(",")
        ));
    }
    for (table, links) in [
        ("action", &action),
        ("requested", &requested),
        ("consequence", &consequence),
        ("target", &target),
        ("consumes", &consumes),
    ] {
        sql.push_str(&format!(
            "INSERT RELATION INTO {table} [{}] RETURN NONE;\n",
            links.join(",")
        ));
    }
    sql
}

/// The demo's template, as the syncer records it.
fn template(entity: &str) -> String {
    format!(
        "{{ package_id: 'pkg', package_name: 'ticket-purchase', package: package:pkg, \
         module_name: 'Main', entity_name: '{entity}' }}"
    )
}

fn transaction(offset: i64, at: &str) -> String {
    format!(
        "{{ id: {offset}, offset: {offset}, name: 'TX@{offset}', update_id: 'update-{offset}', \
         command_id: 'bench-{offset}', workflow_id: '', synchronizer_id: 'bench::1220ff', \
         effective_at: d'{at}', record_time: d'{at}' }}"
    )
}

#[allow(clippy::too_many_arguments)]
fn contract(
    cid: &str,
    entity: &str,
    offset: i64,
    node: i64,
    signatories: &[&str],
    witnesses: &[&str],
    at: &str,
    arguments: &str,
) -> String {
    format!(
        "{{ id: '{cid}', contract_id: '{cid}', name: '{entity}@{offset}', \
         template_name: 'Main.{entity}', template: {t}, signatories: {s}, witness_parties: {w}, \
         offset: {offset}, node_id: {node}, created_at: d'{at}', create_arguments: {arguments}, \
         from_acs: false }}",
        t = template(entity),
        s = list(signatories),
        w = list(witnesses),
    )
}

#[allow(clippy::too_many_arguments)]
fn choice(
    offset: i64,
    node: i64,
    choice: &str,
    entity: &str,
    target: &str,
    actor: &str,
    consuming: bool,
    last_descendant: i64,
    witnesses: &[&str],
    at: &str,
    argument: &str,
    result: &str,
) -> String {
    let result = if result.is_empty() {
        "[]".to_string()
    } else {
        format!("['{result}']")
    };
    format!(
        "{{ id: [{offset}, {node}], name: '{choice}@{offset}', choice_name: '{choice}', \
         template: {t}, target_contract_id: '{target}', acting_parties: ['{actor}'], \
         witness_parties: {w}, consuming: {consuming}, result_contract_ids: {result}, \
         offset: {offset}, node_id: {node}, last_descendant_node_id: {last_descendant}, \
         transaction_effective_at: d'{at}', choice_argument: {argument} }}",
        t = template(entity),
        w = list(witnesses),
    )
}

fn party_row(party: &str) -> String {
    let name = party.split("::").next().unwrap_or(party);
    format!("{{ id: '{party}', party_id: '{party}', name: '{name}' }}")
}

fn created_ref(cid: &str) -> String {
    format!("created:⟨{cid}⟩")
}

/// A link with the syncer's id: `[offset, node]`.
fn link(key: [i64; 2], from: &str, to: &str) -> String {
    format!("{{ id: [{}, {}], in: {from}, out: {to} }}", key[0], key[1])
}

fn requested_link(party: &str, offset: i64) -> String {
    format!("{{ id: ['{party}', {offset}], in: party:⟨{party}⟩, out: transaction:{offset} }}")
}

fn list(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("'{i}'")).collect();
    format!("[{}]", quoted.join(", "))
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

/// One period's numbers: its start (seconds since 1970), how many
/// agreements, and their value.
type Period = (i64, i64, f64);

/// The day and week numbers from SurrealDB and from the tables, timed, and
/// checked against each other: the same agreements and value for every day
/// and week.
async fn head_to_head(db: &Db, tables: &Location, rows: i64) -> Result<()> {
    println!(
        "head to head: the same numbers from SurrealDB (raw records) and from the Delta tables \
         (one row per day, kept up to date by the plugins)"
    );
    let mut from_surreal: Vec<Vec<Period>> = Vec::new();
    for (name, sql) in [("day", SURREAL_DAYS), ("week", SURREAL_WEEKS)] {
        let started = Instant::now();
        let mut response = db.query(sql).await?.check()?;
        let period: Vec<i64> = response.take("period")?;
        let agreements: Vec<i64> = response.take("agreements")?;
        let value: Vec<f64> = response.take("value")?;
        let millis = started.elapsed().as_millis();
        let total: i64 = agreements.iter().sum();
        ensure!(
            total == rows,
            "SurrealDB counted {total} agreements by {name}, not {rows}"
        );
        println!(
            "  SurrealQL, per {name}: {millis:>7} ms ({} rows)",
            period.len()
        );
        from_surreal.push(
            period
                .into_iter()
                .zip(agreements)
                .zip(value)
                .map(|((p, a), v)| (p, a, v))
                .collect(),
        );
    }
    for (table, column) in [
        ("ticket-agreements", "agreements"),
        ("ticket-value", "value"),
    ] {
        let session = Session::open(&tables.join(table)).await?;
        for (period, surreal) in [dashboard::Period::Day, dashboard::Period::Week]
            .into_iter()
            .zip(&from_surreal)
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
            let ours = periods(frame, column)?;
            let theirs: Vec<(i64, f64)> = surreal
                .iter()
                .map(|&(p, a, v)| (p, if column == "agreements" { a as f64 } else { v }))
                .collect();
            ensure!(
                same(&ours, &theirs),
                "{table} per {period:?} differs from SurrealDB: {}",
                first_difference(&ours, &theirs)
            );
        }
    }
    println!("  both give the same agreements and value for every day and week");
    Ok(())
}

/// A chart's (period start in seconds, amount) for the periods with any
/// agreements: the tables also hold quiet days as 0, SurrealDB leaves them out.
fn periods(frame: &Frame, column: &str) -> Result<Vec<(i64, f64)>> {
    let start = frame
        .column("period_start")
        .context("no period_start column")?;
    let amount = frame
        .column(column)
        .with_context(|| format!("no {column} column"))?;
    Ok((0..frame.rows())
        .filter_map(|i| {
            let at = start.values.number(i)?.round() as i64;
            let v = amount.values.number(i).unwrap_or(0.0);
            (v != 0.0).then_some((at, v))
        })
        .collect())
}

/// Equal periods and amounts; values may differ by rounding in the sums.
fn same(a: &[(i64, f64)], b: &[(i64, f64)]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.0 == y.0 && (x.1 - y.1).abs() <= 0.005 + 1e-9 * y.1.abs())
}

fn first_difference(a: &[(i64, f64)], b: &[(i64, f64)]) -> String {
    match a.iter().zip(b).find(|(x, y)| !same(&[**x], &[**y])) {
        Some((x, y)) => format!("tables {x:?}, SurrealDB {y:?}"),
        None => format!(
            "{} periods in the tables, {} in SurrealDB",
            a.len(),
            b.len()
        ),
    }
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
    use super::{rfc3339, same};

    #[test]
    fn quiet_and_rounded_periods_compare_equal() {
        assert!(same(
            &[(0, 2.0), (86_400, 65.5)],
            &[(0, 2.0), (86_400, 65.500_000_1)]
        ));
        assert!(!same(&[(0, 2.0)], &[(86_400, 2.0)]));
        assert!(!same(&[(0, 2.0)], &[(0, 2.0), (86_400, 1.0)]));
        assert!(!same(&[(0, 65.5)], &[(0, 65.6)]));
    }

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
