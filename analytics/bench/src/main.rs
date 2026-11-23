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
/// 2026-01-01T00:00:00Z, in microseconds.
const START: i64 = 1_767_225_600_000_000;
/// One offset every 3 seconds: 10 million offsets span about a year.
const STEP: i64 = 3_000_000;

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
            let batch = agreements(from + 1, to, input.clone())?;
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

/// The accepted offers at offsets `first..=last`, one per offset, in the
/// columns the plugins' query returns: 50,000 buyers, 50 organizers, prices
/// from 20 to 300.
fn agreements(first: i64, last: i64, schema: SchemaRef) -> Result<RecordBatch> {
    let offsets: Vec<i64> = (first..=last).collect();
    let at: Vec<i64> = offsets.iter().map(|o| START + o * STEP).collect();
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
        .map(|&o| 20.0 + (mix(o, 3) % 2_800) as f64 / 10.0)
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
