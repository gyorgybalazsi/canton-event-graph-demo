//! Ticket agreements per day, for a column chart of each day's or week's
//! new agreements.
//!
//! Both ticket plugins read the same rows: each accepted ticket offer (the
//! `TicketOffer` template's `Accept` choice, which creates the
//! `TicketAgreement`) with its time and price. A plugin picks a [`Measure`]
//! and writes one row per day (UTC): how many agreements that day, or their
//! value. Its chart adds the days up to the end of each day or week, whichever
//! the dashboard's Day / Week switch picks (`{period}`).
//!
//! The table is written in merge mode, keyed on the day. A step rewrites the
//! days it touched, and gives the quiet days between them a row of 0, so a
//! week's total never misses a day. The state is the per-day totals, since a
//! step may add to a day that already has a row.
//!
//! Both plugins run the same query, so the daemon fetches it once for both.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::builder::{Float64Builder, Int64Builder, TimestampMicrosecondBuilder};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, TimestampMicrosecondType};
use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};

/// Each accepted ticket offer with an offset in `($from, $to]`, with the
/// offer's terms: the offer is the choice's target contract. The fields are
/// the syncer's.
pub const QUERY: &str = "\
SELECT offset, transaction_effective_at AS agreed_at, acting_parties[0] AS buyer, \
    type::record('created', target_contract_id).create_arguments.organizer AS organizer, \
    <float> type::record('created', target_contract_id).create_arguments.price AS price \
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'TicketOffer' \
    AND offset > $from AND offset <= $to;";

/// How far the database has got: the syncer's own mark, else the newest offset.
pub const HIGH_WATER: &str = "\
LET $synced = IF (INFO FOR DB).tables.sync_state != NONE { \
    (SELECT VALUE last_offset FROM ONLY sync_state:stream) } ELSE { NONE };
RETURN $synced ?? math::max(SELECT VALUE offset FROM transaction) ?? 0;";

const DAY_US: i64 = 86_400_000_000;

/// The table's key: the day's first moment, UTC.
pub const DAY: &str = "day_start";

/// The start of the day that holds `us` (microseconds since 1970, UTC).
pub fn day_start(us: i64) -> i64 {
    us.div_euclid(DAY_US) * DAY_US
}

/// What a plugin counts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Measure {
    /// How many offers were accepted.
    Agreements,
    /// The accepted offers' prices, added up.
    Value,
}

impl Measure {
    pub fn column(self) -> &'static str {
        match self {
            Measure::Agreements => "agreements",
            Measure::Value => "value",
        }
    }

    fn data_type(self) -> DataType {
        match self {
            Measure::Agreements => DataType::Int64,
            Measure::Value => DataType::Float64,
        }
    }
}

/// One plugin: its table's name, and what goes in it.
pub struct Spec {
    pub name: &'static str,
    pub measure: Measure,
}

impl Spec {
    pub fn description(&self) -> &'static str {
        match self.measure {
            Measure::Agreements => {
                "New ticket agreements each day or week (UTC)."
            }
            Measure::Value => {
                "Total value of the new ticket agreements each day or week (UTC)."
            }
        }
    }

    /// The dashboard's one chart.
    pub fn chart_title(&self) -> &'static str {
        match self.measure {
            Measure::Agreements => "New ticket agreements",
            Measure::Value => "Value of new ticket agreements",
        }
    }

    /// Each day's or week's amount, one column per period. The reader
    /// fills `{period}` from the dashboard's Day / Week switch; weeks start
    /// on Monday (2001-01-01 was one), as ISO weeks do.
    pub fn chart_sql(&self) -> String {
        format!(
            "SELECT date_bin({{period}}, {DAY}, TIMESTAMP '2001-01-01T00:00:00Z') AS period_start, \
             sum({m}) AS \"{m}\" FROM {t} GROUP BY 1 ORDER BY 1",
            m = self.measure.column(),
            t = self.name.replace('-', "_"),
        )
    }

    pub fn output_schema(&self) -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(DAY, timestamp(), false),
            Field::new(self.measure.column(), self.measure.data_type(), false),
        ]))
    }

    /// The output table's columns, as an Arrow IPC stream with no rows.
    pub fn schema_ipc(&self) -> Vec<u8> {
        ipc(&RecordBatch::new_empty(self.output_schema())).expect("the schema encodes")
    }
}

fn timestamp() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

/// The query's columns.
pub fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("offset", DataType::Int64, true),
        Field::new("agreed_at", timestamp(), true),
        Field::new("buyer", DataType::Utf8, true),
        Field::new("organizer", DataType::Utf8, true),
        Field::new("price", DataType::Float64, true),
    ]))
}

pub fn input_schema_ipc() -> Vec<u8> {
    ipc(&RecordBatch::new_empty(input_schema())).expect("the schema encodes")
}

fn ipc(batch: &RecordBatch) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut writer = StreamWriter::try_new(&mut out, &batch.schema()).map_err(|e| e.to_string())?;
    writer.write(batch).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    drop(writer);
    Ok(out)
}

/// One day's agreements: how many, and their value.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Day {
    agreements: i64,
    value: f64,
}

thread_local! {
    /// Day start → that day's agreements.
    static DAYS: RefCell<BTreeMap<i64, Day>> = const { RefCell::new(BTreeMap::new()) };
}

pub fn load_state(state: &[u8]) -> Result<(), String> {
    let rows: Vec<(i64, i64, f64)> =
        serde_json::from_slice(state).map_err(|e| format!("bad state: {e}"))?;
    let days = rows
        .into_iter()
        .map(|(start, agreements, value)| (start, Day { agreements, value }))
        .collect();
    DAYS.set(days);
    Ok(())
}

pub fn save_state() -> Vec<u8> {
    DAYS.with_borrow(|days| {
        let rows: Vec<(i64, i64, f64)> = days
            .iter()
            .map(|(start, d)| (*start, d.agreements, d.value))
            .collect();
        serde_json::to_vec(&rows).expect("the state encodes")
    })
}

/// (time, price) of each accepted offer. An offer without a time can't be
/// put in a day and is left out; one without a price counts with 0.
fn agreements(bytes: &[u8]) -> Result<Vec<(i64, f64)>, String> {
    let reader = StreamReader::try_new(bytes, None).map_err(|e| format!("accepted offers: {e}"))?;
    let mut out = Vec::new();
    for batch in reader {
        let b = batch.map_err(|e| format!("accepted offers: {e}"))?;
        let at = b.column(1).as_primitive::<TimestampMicrosecondType>();
        let price = b.column(4).as_primitive::<Float64Type>();
        for i in 0..b.num_rows() {
            if at.is_valid(i) {
                let p = if price.is_valid(i) {
                    price.value(i)
                } else {
                    0.0
                };
                out.push((at.value(i), p));
            }
        }
    }
    Ok(out)
}

/// One step: adds the step's agreements to their days, and returns every
/// day from the earliest it touched to the newest, quiet days as 0.
pub fn transform(spec: &Spec, results: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let [accepted] = results else {
        return Err(format!("expected 1 query result, got {}", results.len()));
    };
    // Read everything before touching the state, so an error changes nothing.
    let rows = agreements(accepted)?;

    let days: Vec<(i64, Day)> = DAYS.with_borrow_mut(|days| {
        let mut earliest = None::<i64>;
        for (at, price) in rows {
            let start = day_start(at);
            let d = days.entry(start).or_default();
            d.agreements += 1;
            d.value += price;
            earliest = Some(earliest.map_or(start, |e| e.min(start)));
        }
        let Some(earliest) = earliest else {
            return Vec::new();
        };
        let newest = *days.keys().next_back().expect("a day was just added");
        (0..)
            .map(|i| earliest + i * DAY_US)
            .take_while(|start| *start <= newest)
            .map(|start| (start, days.get(&start).copied().unwrap_or_default()))
            .collect()
    });

    let mut key = TimestampMicrosecondBuilder::new().with_timezone("UTC");
    days.iter().for_each(|(start, _)| key.append_value(*start));
    let measure: ArrayRef = match spec.measure {
        Measure::Agreements => {
            let mut b = Int64Builder::new();
            days.iter().for_each(|(_, d)| b.append_value(d.agreements));
            Arc::new(b.finish())
        }
        Measure::Value => {
            let mut b = Float64Builder::new();
            days.iter().for_each(|(_, d)| b.append_value(d.value));
            Arc::new(b.finish())
        }
    };
    let batch = RecordBatch::try_new(spec.output_schema(), vec![Arc::new(key.finish()), measure])
        .map_err(|e| e.to_string())?;
    ipc(&batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::types::Int64Type;
    use arrow_array::{Float64Array, Int64Array, StringArray, TimestampMicrosecondArray};

    /// 2026-11-02, a Monday, 00:00 UTC.
    const MONDAY: i64 = 1_793_577_600_000_000;
    const HOUR: i64 = 3_600_000_000;

    fn step(times_prices: &[(i64, f64)]) -> Vec<u8> {
        let n = times_prices.len();
        let columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from((1..=n as i64).collect::<Vec<_>>())),
            Arc::new(
                TimestampMicrosecondArray::from(
                    times_prices.iter().map(|t| t.0).collect::<Vec<_>>(),
                )
                .with_timezone("UTC"),
            ),
            Arc::new(StringArray::from(vec!["Alice::1220bb"; n])),
            Arc::new(StringArray::from(vec!["TicketWizard::1220cc"; n])),
            Arc::new(Float64Array::from(
                times_prices.iter().map(|t| t.1).collect::<Vec<_>>(),
            )),
        ];
        ipc(&RecordBatch::try_new(input_schema(), columns).unwrap()).unwrap()
    }

    /// Runs one step and returns its (day start, amount) rows.
    fn run(spec: &Spec, times_prices: &[(i64, f64)]) -> Vec<(i64, f64)> {
        let out = transform(spec, &[step(times_prices)]).unwrap();
        let mut rows = Vec::new();
        for batch in StreamReader::try_new(out.as_slice(), None).unwrap() {
            let b = batch.unwrap();
            let keys = b.column(0).as_primitive::<TimestampMicrosecondType>();
            for i in 0..b.num_rows() {
                let v = match spec.measure {
                    Measure::Agreements => b.column(1).as_primitive::<Int64Type>().value(i) as f64,
                    Measure::Value => b.column(1).as_primitive::<Float64Type>().value(i),
                };
                rows.push((keys.value(i), v));
            }
        }
        rows
    }

    #[test]
    fn days_are_utc() {
        assert_eq!(day_start(MONDAY + 23 * HOUR), MONDAY);
        assert_eq!(day_start(MONDAY - 1), MONDAY - DAY_US);
    }

    #[test]
    fn days_add_up_and_quiet_days_get_a_zero() {
        DAYS.set(BTreeMap::new());
        let spec = Spec {
            name: "t",
            measure: Measure::Agreements,
        };
        // Two on Monday, none on Tuesday, one on Wednesday.
        let rows = run(
            &spec,
            &[
                (MONDAY + HOUR, 25.0),
                (MONDAY + 2 * HOUR, 40.5),
                (MONDAY + 2 * DAY_US, 10.0),
            ],
        );
        assert_eq!(
            rows,
            [
                (MONDAY, 2.0),
                (MONDAY + DAY_US, 0.0),
                (MONDAY + 2 * DAY_US, 1.0)
            ]
        );
        // A later step rewrites only the days from the one it touched.
        let rows = run(&spec, &[(MONDAY + 2 * DAY_US + HOUR, 5.0)]);
        assert_eq!(rows, [(MONDAY + 2 * DAY_US, 2.0)]);
    }

    #[test]
    fn value_survives_a_restart() {
        DAYS.set(BTreeMap::new());
        let spec = Spec {
            name: "t",
            measure: Measure::Value,
        };
        let rows = run(&spec, &[(MONDAY + HOUR, 25.0), (MONDAY + DAY_US, 40.5)]);
        assert_eq!(rows, [(MONDAY, 25.0), (MONDAY + DAY_US, 40.5)]);
        let saved = save_state();
        DAYS.set(BTreeMap::new());
        load_state(&saved).unwrap();
        let rows = run(&spec, &[(MONDAY + DAY_US + HOUR, 4.5)]);
        assert_eq!(rows, [(MONDAY + DAY_US, 45.0)]);
    }

    #[test]
    fn a_step_without_agreements_writes_nothing() {
        DAYS.set(BTreeMap::new());
        let spec = Spec {
            name: "t",
            measure: Measure::Value,
        };
        assert!(run(&spec, &[]).is_empty());
    }
}
