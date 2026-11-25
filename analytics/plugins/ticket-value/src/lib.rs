//! The ticket analytics plugin `ticket-value`: the total value of the new ticket agreements each day or week.
//! The `ticket-totals` crate does the work; this crate only picks what it counts.

ticket_totals::plugin!("ticket-value", ticket_totals::Measure::Value);
