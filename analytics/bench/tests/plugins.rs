//! The ticket plugins over two purchases in the shape the syncer writes,
//! in an in-memory SurrealDB, through the analytics daemon into local Delta
//! tables, then read back as the explorer's dashboard does.
//!
//! Builds the plugins first, so it needs the `wasm32-wasip2` target
//! (`rustup target add wasm32-wasip2`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use analytics_reader::frame::Frame;
use analytics_reader::location::Location;
use analytics_reader::{Session, dashboard};
use canton_analytics::fetch::Fetcher;
use canton_analytics::plugin;
use canton_analytics::runner::{Options, Runner};
use canton_analytics::source::{self, Connection, UserLevel};

const PURCHASES: &str = include_str!("purchases.surql");

/// Builds the plugins' workspace for wasm32-wasip2 and returns each
/// plugin's `.wasm` file.
fn build_plugins(names: [&str; 2]) -> [PathBuf; 2] {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .status()
        .expect("cargo runs");
    assert!(
        status.success(),
        "building the plugins failed; is the wasm32-wasip2 target installed?"
    );
    names.map(|name| {
        dir.join("target/wasm32-wasip2/release")
            .join(format!("{}.wasm", name.replace('-', "_")))
    })
}

async fn query(session: &Session, sql: &str) -> Frame {
    Frame::from_batches(&session.sql(sql).await.unwrap(), |_| Default::default()).unwrap()
}

fn numbers(frame: &Frame, column: &str) -> Vec<Option<f64>> {
    let c = frame.column(column).unwrap();
    (0..frame.rows()).map(|i| c.values.number(i)).collect()
}

fn texts(frame: &Frame, column: &str) -> Vec<String> {
    let c = frame.column(column).unwrap();
    (0..frame.rows())
        .map(|i| c.values.text(i).unwrap_or_default())
        .collect()
}

#[test]
fn ticket_plugins_read_the_syncers_purchases() {
    let [agreements, value] = build_plugins(["ticket-agreements", "ticket-value"]);
    let dir = std::env::temp_dir().join(format!("ticket-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tables = Location::parse(&dir.to_string_lossy(), HashMap::new()).unwrap();

    // Embedded SurrealDB needs more stack than test threads have.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 << 20)
        .build()
        .unwrap();
    rt.block_on(async move {
        tokio::spawn(async move {
            let db = source::connect(&Connection {
                url: "mem://".into(),
                namespace: "canton".into(),
                database: "ticket_purchase".into(),
                username: String::new(),
                password: String::new(),
                user_level: UserLevel::Database,
            })
            .await
            .unwrap();
            db.query(PURCHASES).await.unwrap().check().unwrap();
            let engine = plugin::engine().unwrap();
            let fetcher = Fetcher::new(db);
            let options = Options {
                batch: 2,
                min_rows: 1,
                ..Options::default()
            };
            let mut a = Runner::new(
                &engine,
                &agreements,
                &tables,
                fetcher.clone(),
                options.clone(),
            )
            .await
            .unwrap();
            let mut b = Runner::new(&engine, &value, &tables, fetcher, options)
                .await
                .unwrap();
            let (sa, sb) = tokio::join!(a.catch_up(true), b.catch_up(true));
            assert!(sa.unwrap().rows >= 1);
            assert!(sb.unwrap().rows >= 1);

            // Both purchases were on 2 November: one day, two agreements
            // worth 25 + 40.5 (the prices come from the offers).
            for (table, column, total) in [
                ("ticket-agreements", "agreements", 2.0),
                ("ticket-value", "value", 65.5),
            ] {
                let session = Session::open(&tables.join(table)).await.unwrap();
                let frame = query(
                    &session,
                    &format!(
                        "SELECT day_start, {column} FROM {} ORDER BY 1",
                        table.replace('-', "_")
                    ),
                )
                .await;
                assert_eq!(numbers(&frame, column), [Some(total)], "{table}");
                assert_eq!(
                    texts(&frame, "day_start"),
                    ["1793577600000000"],
                    "{table}: 2026-11-02T00:00Z"
                );
                // One chart: a column per day (or week) with its amount.
                let board = dashboard::load(&session).await.unwrap();
                assert_eq!(board.panels.len(), 1);
                let chart = board.panels[0].frame.as_ref().unwrap();
                assert_eq!(numbers(chart, column), [Some(total)], "{table}");
                // The same by week, as the dashboard's Week switch asks.
                let board = dashboard::load_with(&session, dashboard::Period::Week)
                    .await
                    .unwrap();
                assert_eq!(board.panels[0].bucket_seconds, Some(7 * 86_400));
                let chart = board.panels[0].frame.as_ref().unwrap();
                assert_eq!(numbers(chart, column), [Some(total)], "{table}");
            }
        })
        .await
        .unwrap();
    });
}
