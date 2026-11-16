//! Rust types for the ticket purchase Daml model, generated at build time by
//! daml-rust-codegen from the model's DAR (see `build.rs`).

#[allow(clippy::all)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/ticket_purchase.rs"));
}

pub use generated::ticket_purchase::main::{Accept, Cash, TicketOffer};

/// The model's package, referenced by name so the app works with any build of the DAR.
pub const PACKAGE: &str = "#ticket-purchase";
/// The Daml module the templates are in.
pub const MODULE: &str = "Main";
