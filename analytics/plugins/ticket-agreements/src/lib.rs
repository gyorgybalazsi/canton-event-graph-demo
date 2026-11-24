//! The ticket analytics plugin `ticket-agreements`: see the `ticket-totals` crate,
//! which does the work. This crate only picks what it counts.

wit_bindgen::generate!({
    world: "analytics-plugin",
    path: "../../wit",
});

use exports::canton::analytics::plugin::{
    Guest, Panel, PanelKind, PluginInfo, QuerySource, WriteMode,
};
use ticket_totals::{Measure, Spec};

const SPEC: Spec = Spec {
    name: "ticket-agreements",
    measure: Measure::Agreements,
};

struct Plugin;

export!(Plugin);

impl Guest for Plugin {
    fn info() -> PluginInfo {
        PluginInfo {
            name: SPEC.name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: SPEC.description().into(),
        }
    }

    fn schema() -> Vec<u8> {
        SPEC.schema_ipc()
    }

    fn source() -> QuerySource {
        QuerySource {
            query: ticket_totals::QUERY.into(),
            inputs: vec![ticket_totals::input_schema_ipc()],
            high_water: ticket_totals::HIGH_WATER.into(),
        }
    }

    fn mode() -> WriteMode {
        WriteMode::Merge(vec![ticket_totals::DAY.into()])
    }

    fn dashboard() -> Vec<Panel> {
        vec![Panel {
            title: SPEC.chart_title().into(),
            kind: PanelKind::Timeseries,
            sql: SPEC.chart_sql(),
            time_column: Some(ticket_totals::DAY.into()),
            formats: Vec::new(),
        }]
    }

    fn load_state(state: Vec<u8>) -> Result<(), String> {
        ticket_totals::load_state(&state)
    }

    fn save_state() -> Vec<u8> {
        ticket_totals::save_state()
    }

    fn transform(results: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
        ticket_totals::transform(&SPEC, &results)
    }
}
