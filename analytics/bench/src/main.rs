//! How fast is the analytics dashboard over a big ledger?
//!
//! `ticket-bench table` makes up a ledger of ticket purchases, one accepted
//! offer per offset, in the shape the ticket plugins' query returns. It feeds
//! the rows through each plugin's own `transform` and the analytics daemon's
//! own table writer (commits of a million offsets, then compaction), then
//! opens each table as the explorer does and times every dashboard panel.
//! Only SurrealDB and the ledger are left out, so the tables are what the
//! daemon would write for a ledger that size.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use analytics_reader::location::Location;
use analytics_reader::{Session, dashboard, quote};
use anyhow::{Context, Result, ensure};
use canton_analytics::plugin::{self, Plugin};
use canton_analytics::table::Table;
use clap::{Parser, Subcommand};
use deltalake::arrow::array::{
    Array, ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use deltalake::arrow::datatypes::SchemaRef;

#[derive(Parser)]
#[command(about = "Benchmarks the canton-analytics dashboard over a big made-up ticket ledger")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write the plugins' tables for a made-up ledger, then time their dashboards.
    Table {
        /// Folder for the tables; each plugin writes a sub-folder named after it.
        #[arg(long, default_value = "bench-tables")]
        tables: String,
        /// Offsets in the made-up ledger, one accepted offer each.
        #[arg(long, default_value_t = 10_000_000)]
        rows: i64,
        /// The plugins' `.wasm` files.
        #[arg(required = true)]
        plugins: Vec<PathBuf>,
    },
}

/// Offsets per commit.
const CHUNK: i64 = 1_000_000;
/// 2026-01-01T00:00:00Z, in microseconds: a Thursday.
const START: i64 = 1_767_225_600_000_000;
/// The made-up ledger spans a year, however many offsets it has.
const DAYS: usize = 365;
const DAY_MICROS: i64 = 86_400_000_000;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // On a worker thread: plugin calls use `block_in_place`.
    rt.block_on(async move {
        tokio::spawn(async move {
            match cli.command {
                Command::Table {
                    tables,
                    rows,
                    plugins,
                } => table(&tables, rows, &plugins).await,
            }
        })
        .await?
    })
}

async fn table(tables: &str, rows: i64, plugins: &[PathBuf]) -> Result<()> {
    ensure!(rows > 0, "--rows must be at least 1");
    let tables = Location::parse(tables, HashMap::new())?;
    let engine = plugin::engine()?;
    let calendar = Calendar::new(rows);
    for path in plugins {
        let mut plugin = tokio::task::block_in_place(|| Plugin::load(&engine, path))
            .with_context(|| format!("could not load {}", path.display()))?;
        let location = tables.join(&plugin.info.name);
        let mut table = Table::open_or_create(&location, &plugin.manifest, &plugin.schema).await?;
        ensure!(
            table.saved().await?.cursor == 0,
            "{location} already has rows: remove it first"
        );
        let input = plugin
            .inputs
            .first()
            .cloned()
            .context("the plugin reads no query")?;

        let started = Instant::now();
        let mut from = 0;
        while from < rows {
            let to = rows.min(from + CHUNK);
            let batch = agreements(&calendar, from + 1, to, input.clone())?;
            let out = tokio::task::block_in_place(|| plugin.transform(&[batch]))?;
            // Save the state with the last commit, as the daemon does.
            let state = if to == rows {
                Some(tokio::task::block_in_place(|| plugin.save_state())?)
            } else {
                None
            };
            table.commit(out, to, state.as_deref()).await?;
            from = to;
        }
        let wrote = started.elapsed();
        table.optimize().await?;
        let compacted = started.elapsed() - wrote;

        // What the explorer does when it opens the table.
        let opened = Instant::now();
        let session = Session::open(&location).await?;
        let open_ms = opened.elapsed().as_millis();
        let count = session
            .sql(&format!("SELECT count(*) FROM {}", quote(&session.name)))
            .await?;
        let count = count
            .first()
            .and_then(|b| b.column(0).as_any().downcast_ref::<Int64Array>())
            .filter(|c| !c.is_empty())
            .map_or(0, |c| c.value(0));
        println!(
            "{}: {rows} offsets through the plugin and into the table in {:.1} s \
             ({:.0} offsets/s), compacted in {:.1} s; the table has {count} rows",
            plugin.info.name,
            wrote.as_secs_f64(),
            rows as f64 / wrote.as_secs_f64(),
            compacted.as_secs_f64(),
        );
        let loaded = Instant::now();
        let d = dashboard::load(&session).await?;
        println!(
            "  dashboard: table opened in {open_ms} ms, all panels in {} ms",
            loaded.elapsed().as_millis()
        );
        for p in &d.panels {
            let outcome = match &p.frame {
                Ok(f) => format!("{} rows", f.rows()),
                Err(e) => format!("error: {e}"),
            };
            println!("    {:>5} ms  {} ({outcome})", p.millis, p.panel.title);
        }
    }
    Ok(())
}

/// A small, fast, repeatable pseudo-random number for offset `n`.
fn mix(n: i64, salt: u64) -> u64 {
    let mut x = (n as u64) ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// How busy each day of the made-up year is, relative to an average day:
/// weekends sell more, summer and December are high season, the business
/// grows through the year, every day is a bit off, and now and then a big
/// event goes on sale.
fn busyness(day: usize) -> f64 {
    // START is a Thursday; Monday is 0.
    let weekday = (day + 3) % 7;
    let week = [0.75, 0.8, 0.85, 1.0, 1.35, 1.6, 1.1][weekday];
    let year = day as f64 / DAYS as f64;
    let summer = 0.3 * (-((year - 0.55) / 0.12).powi(2)).exp();
    let december = 0.4 * (-((year - 0.95) / 0.03).powi(2)).exp();
    let growth = 0.7 + 0.6 * year;
    let noise = 0.75 + (mix(day as i64, 7) % 500) as f64 / 1_000.0;
    let on_sale = if mix(day as i64, 8) % 23 == 0 { 2.5 } else { 1.0 };
    week * (1.0 + summer + december) * growth * noise * on_sale
}

/// How much dearer than usual a day's tickets are: big events cost more.
fn price_level(day: usize) -> f64 {
    let level = 0.8 + (mix(day as i64, 9) % 400) as f64 / 1_000.0;
    if mix(day as i64, 8) % 23 == 0 { level * 1.5 } else { level }
}

/// Which day each offset falls on: `ends[d]` is the last offset of day `d`.
struct Calendar {
    ends: Vec<i64>,
}

impl Calendar {
    /// Spreads `rows` offsets over the year by each day's busyness.
    fn new(rows: i64) -> Calendar {
        let weights: Vec<f64> = (0..DAYS).map(busyness).collect();
        let total: f64 = weights.iter().sum();
        let mut sum = 0.0;
        let ends = weights
            .iter()
            .map(|w| {
                sum += w;
                (rows as f64 * sum / total).round() as i64
            })
            .collect::<Vec<_>>();
        Calendar { ends }
    }

    /// Offset `o`'s day, and its time: the day's offsets are spread evenly
    /// over the day.
    fn place(&self, o: i64) -> (usize, i64) {
        let day = self.ends.partition_point(|&end| end < o).min(DAYS - 1);
        let first = if day == 0 { 1 } else { self.ends[day - 1] + 1 };
        let count = (self.ends[day] - first + 1).max(1);
        let at = START + day as i64 * DAY_MICROS + (o - first) * DAY_MICROS / count;
        (day, at)
    }
}

/// The accepted offers at offsets `first..=last`, one per offset, in the
/// columns the plugins' query returns: 50,000 buyers, 50 organizers, prices
/// from 20 to 300 times the day's price level.
fn agreements(calendar: &Calendar, first: i64, last: i64, schema: SchemaRef) -> Result<RecordBatch> {
    let offsets: Vec<i64> = (first..=last).collect();
    let placed: Vec<(usize, i64)> = offsets.iter().map(|&o| calendar.place(o)).collect();
    let at: Vec<i64> = placed.iter().map(|&(_, at)| at).collect();
    let buyer: Vec<String> = offsets
        .iter()
        .map(|&o| format!("Buyer{}::1220aa", mix(o, 1) % 50_000))
        .collect();
    let organizer: Vec<String> = offsets
        .iter()
        .map(|&o| format!("Organizer{}::1220bb", mix(o, 2) % 50))
        .collect();
    let price: Vec<f64> = offsets
        .iter()
        .zip(&placed)
        .map(|(&o, &(day, _))| {
            let base = 20.0 + (mix(o, 3) % 2_800) as f64 / 10.0;
            (base * price_level(day) * 100.0).round() / 100.0
        })
        .collect();
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(offsets)),
        Arc::new(TimestampMicrosecondArray::from(at).with_timezone("UTC")),
        Arc::new(StringArray::from(buyer)),
        Arc::new(StringArray::from(organizer)),
        Arc::new(Float64Array::from(price)),
    ];
    Ok(RecordBatch::try_new(schema, columns)?)
}

#[cfg(test)]
mod tests {
    use super::{Calendar, DAY_MICROS, DAYS, START};

    #[test]
    fn every_offset_lands_on_its_day_in_order() {
        let rows = 100_000;
        let calendar = Calendar::new(rows);
        assert_eq!(*calendar.ends.last().unwrap(), rows);
        let mut last = i64::MIN;
        for o in 1..=rows {
            let (day, at) = calendar.place(o);
            assert!(at > last, "offset {o} goes back in time");
            assert_eq!((at - START) / DAY_MICROS, day as i64);
            last = at;
        }
        assert!(calendar.place(rows).0 == DAYS - 1);
    }

    #[test]
    fn days_differ() {
        let calendar = Calendar::new(10_000_000);
        let counts: Vec<i64> = calendar.ends.windows(2).map(|w| w[1] - w[0]).collect();
        let (lo, hi) = (counts.iter().min().unwrap(), counts.iter().max().unwrap());
        assert!(*hi > 3 * lo, "quietest day {lo}, busiest {hi}");
    }
}
