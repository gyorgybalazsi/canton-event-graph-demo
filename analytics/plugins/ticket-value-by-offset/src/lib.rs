//! Ticket agreements pre-aggregated: one row per range of 10,000 offsets,
//! with how many ticket offers were accepted in it and their total value.
//!
//! The table is written in merge mode, keyed on `offset_bucket`: each step
//! writes a fresh row for every range it touched, and the runner replaces
//! that range's old row. A ledger of 10 million offsets makes 1,000 rows, so
//! the dashboard reads almost nothing however big the ledger gets. It runs
//! the same query as `ticket-agreements`, so the runner fetches it once for
//! both.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::builder::{Float64Builder, Int64Builder, TimestampMicrosecondBuilder};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int64Type, TimestampMicrosecondType};
use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use serde::{Deserialize, Serialize};

wit_bindgen::generate!({
    world: "analytics-plugin",
    path: "../../wit",
});

use exports::canton::analytics::plugin::{
    ColumnFormat, Guest, Panel, PanelKind, PluginInfo, QuerySource, ValueFormat, WriteMode,
};

struct Plugin;

export!(Plugin);

/// Offsets per row.
const WIDTH: i64 = 10_000;

/// Each accepted ticket offer (the `TicketOffer` template's `Accept` choice)
/// with an offset in `($from, $to]`, and the offer's terms: the offer is the
/// choice's target contract. The fields are the syncer's.
const QUERY: &str = "\
SELECT offset, transaction_effective_at AS agreed_at, acting_parties[0] AS buyer, \
    type::record('created', target_contract_id).create_arguments.organizer AS organizer, \
    <float> type::record('created', target_contract_id).create_arguments.price AS price \
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'TicketOffer' \
    AND offset > $from AND offset <= $to;";

const HIGH_WATER: &str = "\
LET $synced = IF (INFO FOR DB).tables.sync_state != NONE { \
    (SELECT VALUE last_offset FROM ONLY sync_state:stream) } ELSE { NONE };
RETURN $synced ?? math::max(SELECT VALUE offset FROM transaction) ?? 0;";

/// One range's running totals.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
struct Bucket {
    agreements: i64,
    value: f64,
    first_at: Option<i64>,
    last_at: Option<i64>,
}

thread_local! {
    static BUCKETS: RefCell<BTreeMap<i64, Bucket>> = const { RefCell::new(BTreeMap::new()) };
}

fn timestamp() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("offset", DataType::Int64, true),
        Field::new("agreed_at", timestamp(), true),
        Field::new("buyer", DataType::Utf8, true),
        Field::new("organizer", DataType::Utf8, true),
        Field::new("price", DataType::Float64, true),
    ]))
}

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("offset_bucket", DataType::Int64, false),
        Field::new("agreements", DataType::Int64, false),
        Field::new("value", DataType::Float64, false),
        Field::new("first_at", timestamp(), true),
        Field::new("last_at", timestamp(), true),
    ]))
}

fn ipc(batch: &RecordBatch) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut writer = StreamWriter::try_new(&mut out, &batch.schema()).map_err(|e| e.to_string())?;
    writer.write(batch).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    drop(writer);
    Ok(out)
}

/// One accepted offer: (offset, time, price).
type Agreement = (i64, Option<i64>, f64);

fn agreements(bytes: &[u8]) -> Result<Vec<Agreement>, String> {
    let reader = StreamReader::try_new(bytes, None).map_err(|e| format!("accepted offers: {e}"))?;
    let mut out = Vec::new();
    for batch in reader {
        let b = batch.map_err(|e| format!("accepted offers: {e}"))?;
        let offset = b.column(0).as_primitive::<Int64Type>();
        let at = b.column(1).as_primitive::<TimestampMicrosecondType>();
        let price = b.column(4).as_primitive::<Float64Type>();
        for i in 0..b.num_rows() {
            if offset.is_null(i) {
                continue;
            }
            out.push((
                offset.value(i),
                at.is_valid(i).then(|| at.value(i)),
                if price.is_valid(i) {
                    price.value(i)
                } else {
                    0.0
                },
            ));
        }
    }
    Ok(out)
}

fn transform(results: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let [accepted] = results else {
        return Err(format!("expected 1 query result, got {}", results.len()));
    };
    // Read everything before touching the state, so an error changes nothing.
    let all = agreements(accepted)?;

    let touched: BTreeMap<i64, Bucket> = BUCKETS.with_borrow_mut(|buckets| {
        let mut touched = BTreeMap::new();
        for (offset, at, price) in all {
            let key = offset - offset.rem_euclid(WIDTH);
            let b = buckets.entry(key).or_default();
            b.agreements += 1;
            b.value += price;
            if let Some(at) = at {
                b.first_at = Some(b.first_at.map_or(at, |t| t.min(at)));
                b.last_at = Some(b.last_at.map_or(at, |t| t.max(at)));
            }
            touched.insert(key, b.clone());
        }
        touched
    });

    let mut key = Int64Builder::new();
    let mut count = Int64Builder::new();
    let mut value = Float64Builder::new();
    let mut first_at = TimestampMicrosecondBuilder::new().with_timezone("UTC");
    let mut last_at = TimestampMicrosecondBuilder::new().with_timezone("UTC");
    for (k, b) in &touched {
        key.append_value(*k);
        count.append_value(b.agreements);
        value.append_value(b.value);
        first_at.append_option(b.first_at);
        last_at.append_option(b.last_at);
    }
    let columns: Vec<ArrayRef> = vec![
        Arc::new(key.finish()),
        Arc::new(count.finish()),
        Arc::new(value.finish()),
        Arc::new(first_at.finish()),
        Arc::new(last_at.finish()),
    ];
    ipc(&RecordBatch::try_new(output_schema(), columns).map_err(|e| e.to_string())?)
}

fn panel(
    title: &str,
    kind: PanelKind,
    sql: &str,
    time_column: Option<&str>,
    formats: &[(&str, ValueFormat)],
) -> Panel {
    Panel {
        title: title.into(),
        kind,
        sql: sql.into(),
        time_column: time_column.map(Into::into),
        formats: formats
            .iter()
            .map(|(c, f)| ColumnFormat {
                column: (*c).into(),
                format: *f,
            })
            .collect(),
    }
}

impl Guest for Plugin {
    fn info() -> PluginInfo {
        PluginInfo {
            name: "ticket-value-by-offset".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description:
                "Accepted ticket offers and their value per 10,000 offsets, kept up to date per range."
                    .into(),
        }
    }

    fn schema() -> Vec<u8> {
        ipc(&RecordBatch::new_empty(output_schema())).expect("the schema encodes")
    }

    fn source() -> QuerySource {
        QuerySource {
            query: QUERY.into(),
            inputs: vec![ipc(&RecordBatch::new_empty(input_schema())).expect("encodes")],
            high_water: HIGH_WATER.into(),
        }
    }

    fn mode() -> WriteMode {
        WriteMode::Merge(vec!["offset_bucket".into()])
    }

    fn dashboard() -> Vec<Panel> {
        use ValueFormat::{Number, Timestamp};
        vec![
            panel(
                "Totals",
                PanelKind::Stats,
                "SELECT sum(agreements) AS \"Agreements\", sum(value) AS \"Total value\", \
                 count(*) AS \"Offset ranges\", max(last_at) AS \"Last agreement\" \
                 FROM ticket_value_by_offset",
                None,
                &[("Total value", Number), ("Last agreement", Timestamp)],
            ),
            panel(
                "Agreement value per offset range",
                PanelKind::Timeseries,
                "SELECT offset_bucket - offset_bucket % {bucket} AS offset_bucket, sum(value) AS value \
                 FROM ticket_value_by_offset GROUP BY 1 ORDER BY 1",
                Some("offset_bucket"),
                &[],
            ),
            panel(
                "Busiest offset ranges",
                PanelKind::Table,
                &format!(
                    "SELECT concat(offset_bucket, ' – ', offset_bucket + {}) AS offsets, \
                     agreements, value, first_at, last_at \
                     FROM ticket_value_by_offset ORDER BY agreements DESC LIMIT 20",
                    WIDTH - 1
                ),
                None,
                &[("first_at", Timestamp), ("last_at", Timestamp)],
            ),
        ]
    }

    fn load_state(state: Vec<u8>) -> Result<(), String> {
        let buckets: BTreeMap<i64, Bucket> =
            serde_json::from_slice(&state).map_err(|e| format!("bad state: {e}"))?;
        BUCKETS.set(buckets);
        Ok(())
    }

    fn save_state() -> Vec<u8> {
        BUCKETS.with_borrow(|buckets| serde_json::to_vec(buckets).expect("the state encodes"))
    }

    fn transform(results: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
        transform(&results)
    }
}
