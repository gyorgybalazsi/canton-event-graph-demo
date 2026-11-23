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

/// Builds a plugin for wasm32-wasip2 and returns the `.wasm` file.
fn build_plugin(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../plugins")
        .join(name);
    let target = dir.join("target");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-wasip2",
            "--target-dir",
        ])
        .arg(&target)
        .current_dir(&dir)
        .status()
        .expect("cargo runs");
    assert!(
        status.success(),
        "building {name} failed; is the wasm32-wasip2 target installed?"
    );
    target
        .join("wasm32-wasip2/release")
        .join(format!("{}.wasm", name.replace('-', "_")))
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
    let agreements = build_plugin("ticket-agreements");
    let by_offset = build_plugin("ticket-value-by-offset");
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
            let mut b = Runner::new(&engine, &by_offset, &tables, fetcher, options)
                .await
                .unwrap();
            let (sa, sb) = tokio::join!(a.catch_up(true), b.catch_up(true));
            assert_eq!(sa.unwrap().rows, 2);
            assert!(sb.unwrap().rows >= 1);

            // The buyer acted, the organizer and price come from the offer.
            let session = Session::open(&tables.join("ticket-agreements"))
                .await
                .unwrap();
            let frame = query(
                &session,
                "SELECT \"offset\", buyer, organizer, price FROM ticket_agreements ORDER BY 1",
            )
            .await;
            assert_eq!(numbers(&frame, "offset"), [Some(3.0), Some(5.0)]);
            assert_eq!(texts(&frame, "buyer"), ["Alice::1220bb", "Bob::1220dd"]);
            assert_eq!(
                texts(&frame, "organizer"),
                ["TicketWizard::1220cc", "TicketWizard::1220cc"]
            );
            assert_eq!(numbers(&frame, "price"), [Some(25.0), Some(40.5)]);
            let board = dashboard::load(&session).await.unwrap();
            for p in &board.panels {
                assert!(p.frame.is_ok(), "{}: {:?}", p.panel.title, p.frame);
            }
            // An offset axis: round buckets, not a time interval.
            let per_range = board.panels[1].frame.as_ref().unwrap();
            assert_eq!(numbers(per_range, "offset"), [Some(3.0), Some(5.0)]);

            let session = Session::open(&tables.join("ticket-value-by-offset"))
                .await
                .unwrap();
            let frame = query(
                &session,
                "SELECT offset_bucket, agreements, value FROM ticket_value_by_offset",
            )
            .await;
            assert_eq!(numbers(&frame, "offset_bucket"), [Some(0.0)]);
            assert_eq!(numbers(&frame, "agreements"), [Some(2.0)]);
            assert_eq!(numbers(&frame, "value"), [Some(65.5)]);
            let board = dashboard::load(&session).await.unwrap();
            for p in &board.panels {
                assert!(p.frame.is_ok(), "{}: {:?}", p.panel.title, p.frame);
            }
        })
        .await
        .unwrap();
    });
}
