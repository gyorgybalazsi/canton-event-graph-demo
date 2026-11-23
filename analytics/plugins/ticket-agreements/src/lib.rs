//! Ticket agreements: one row per accepted ticket offer (the `TicketOffer`
//! template's `Accept` choice, which creates the `TicketAgreement`), with the
//! buyer, the organizer and the price from the offer.
//!
//! The table is written in append mode and the plugin keeps no state: each
//! step's rows go to the table as they are. Its dashboard aggregates the raw
//! rows, so it shows how fast DataFusion is over a big table; the
//! `ticket-value-by-offset` plugin keeps the same numbers pre-aggregated.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};

wit_bindgen::generate!({
    world: "analytics-plugin",
    path: "../../wit",
});

use exports::canton::analytics::plugin::{
    ColumnFormat, Guest, Panel, PanelKind, PluginInfo, QuerySource, ValueFormat, WriteMode,
};

struct Plugin;

export!(Plugin);

/// Each accepted ticket offer (the `TicketOffer` template's `Accept` choice)
/// with an offset in `($from, $to]`, and the offer's terms: the offer is the
/// choice's target contract. The fields are the syncer's.
const QUERY: &str = "\
SELECT offset, transaction_effective_at AS agreed_at, acting_parties[0] AS buyer, \
    type::record('created', target_contract_id).create_arguments.organizer AS organizer, \
    <float> type::record('created', target_contract_id).create_arguments.price AS price \
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'TicketOffer' \
    AND offset > $from AND offset <= $to;";

/// How far the database has got: the syncer's own mark, else the newest offset.
const HIGH_WATER: &str = "\
LET $synced = IF (INFO FOR DB).tables.sync_state != NONE { \
    (SELECT VALUE last_offset FROM ONLY sync_state:stream) } ELSE { NONE };
RETURN $synced ?? math::max(SELECT VALUE offset FROM transaction) ?? 0;";

/// The table's columns, which are also the query's: the plugin passes the
/// rows through.
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("offset", DataType::Int64, true),
        Field::new(
            "agreed_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
        Field::new("buyer", DataType::Utf8, true),
        Field::new("organizer", DataType::Utf8, true),
        Field::new("price", DataType::Float64, true),
    ]))
}

fn ipc(batches: &[RecordBatch], schema: &SchemaRef) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut writer = StreamWriter::try_new(&mut out, schema).map_err(|e| e.to_string())?;
    for batch in batches {
        writer.write(batch).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())?;
    drop(writer);
    Ok(out)
}

fn transform(results: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let [accepted] = results else {
        return Err(format!("expected 1 query result, got {}", results.len()));
    };
    let schema = output_schema();
    let reader = StreamReader::try_new(accepted.as_slice(), None)
        .map_err(|e| format!("accepted offers: {e}"))?;
    let mut batches = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| format!("accepted offers: {e}"))?;
        // The runner builds the rows with the schema the plugin declared.
        batches.push(
            RecordBatch::try_new(schema.clone(), batch.columns().to_vec())
                .map_err(|e| e.to_string())?,
        );
    }
    ipc(&batches, &schema)
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
            name: "ticket-agreements".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Ticket agreements: buyer, organizer and price of each accepted offer, one row each."
                .into(),
        }
    }

    fn schema() -> Vec<u8> {
        let schema = output_schema();
        ipc(&[], &schema).expect("the schema encodes")
    }

    fn source() -> QuerySource {
        let schema = output_schema();
        QuerySource {
            query: QUERY.into(),
            inputs: vec![ipc(&[], &schema).expect("encodes")],
            high_water: HIGH_WATER.into(),
        }
    }

    fn mode() -> WriteMode {
        WriteMode::Append
    }

    fn dashboard() -> Vec<Panel> {
        use ValueFormat::{Id, Number, Timestamp};
        vec![
            panel(
                "Totals",
                PanelKind::Stats,
                "SELECT count(*) AS \"Agreements\", sum(price) AS \"Total value\", \
                 avg(price) AS \"Mean price\", max(\"offset\") AS \"Newest offset\" \
                 FROM ticket_agreements",
                None,
                &[("Total value", Number), ("Mean price", Number)],
            ),
            panel(
                "Agreement value per offset range",
                PanelKind::Timeseries,
                "SELECT \"offset\" - \"offset\" % {bucket} AS \"offset\", sum(price) AS value \
                 FROM ticket_agreements GROUP BY 1 ORDER BY 1",
                Some("offset"),
                &[],
            ),
            panel(
                "Agreements over time",
                PanelKind::Timeseries,
                "SELECT date_bin({bucket}, agreed_at) AS time, count(*) AS agreements \
                 FROM ticket_agreements WHERE agreed_at IS NOT NULL GROUP BY 1 ORDER BY 1",
                Some("agreed_at"),
                &[],
            ),
            panel(
                "Organizers by value",
                PanelKind::Bar,
                "SELECT organizer, sum(price) AS value FROM ticket_agreements \
                 GROUP BY organizer ORDER BY value DESC LIMIT 10",
                None,
                &[("organizer", Id)],
            ),
            panel(
                "Latest agreements",
                PanelKind::Table,
                "SELECT \"offset\", agreed_at, buyer, organizer, price \
                 FROM ticket_agreements ORDER BY \"offset\" DESC LIMIT 100",
                None,
                &[("agreed_at", Timestamp), ("buyer", Id), ("organizer", Id)],
            ),
        ]
    }

    fn load_state(_state: Vec<u8>) -> Result<(), String> {
        Ok(())
    }

    fn save_state() -> Vec<u8> {
        Vec::new()
    }

    fn transform(results: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
        transform(&results)
    }
}
