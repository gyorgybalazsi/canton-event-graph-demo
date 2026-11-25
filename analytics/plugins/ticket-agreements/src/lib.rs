//! The ticket analytics plugin `ticket-agreements`: how many new ticket agreements each day or week.
//! The `ticket-totals` crate does the work; this crate only picks what it counts.

ticket_totals::plugin!("ticket-agreements", ticket_totals::Measure::Agreements);
