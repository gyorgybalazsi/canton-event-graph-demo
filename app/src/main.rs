//! The ticket purchase flow, submitted to a Canton sandbox with Rust types
//! generated from the Daml model.
//!
//! `setup` uploads the model and allocates the three parties; `purchase` then
//! runs the purchase itself, as many times as asked:
//!
//! 1. ScroogeBank issues cash to Alice (`Cash`).
//! 2. TicketWizard offers Alice a ticket for the same price (`TicketOffer`).
//! 3. Alice accepts the offer, paying with the cash (`Accept`). In one
//!    transaction this transfers the cash to TicketWizard and creates the
//!    `TicketAgreement`.

mod ledger;
mod model;

use anyhow::Result;
use clap::{Parser, Subcommand};
use daml_type_rep::built_in_types::{DamlContractId, DamlDecimal, DamlParty};
use daml_type_rep::lapi_access::{LapiAccess, ToCreateArguments};
use ledger::Ledger;
use ledger_api::v2::{CreateCommand, ExerciseCommand, command};
use model::{Accept, Cash, TicketOffer};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

const BUYER: &str = "Alice";
const ORGANIZER: &str = "TicketWizard";
const BANK: &str = "ScroogeBank";

#[derive(Parser)]
#[command(
    name = "ticket-purchase",
    about = "Ticket purchase flow on a Canton ledger"
)]
struct Cli {
    /// Ledger API address of the participant
    #[arg(long, global = true, default_value = "http://localhost:6865")]
    url: String,
    #[command(subcommand)]
    command: Step,
}

#[derive(Subcommand)]
enum Step {
    /// Upload the model's DAR and allocate the parties (safe to run again)
    Setup {
        #[arg(long, default_value = "daml/.daml/dist/ticket-purchase-0.1.0.dar")]
        dar: PathBuf,
    },
    /// Buy tickets: issue cash, offer a ticket, accept the offer
    Purchase {
        /// How many tickets to buy
        #[arg(long, default_value_t = 3)]
        count: u32,
        /// Price of the first ticket; each further one costs 5 more
        #[arg(long, default_value_t = 10.0)]
        price: f64,
        /// Seconds to wait after each transaction, so a viewer can follow
        /// each one arriving in the explorer (0: no pause)
        #[arg(long, default_value_t = 0)]
        pause: u64,
    },
}

struct Parties {
    buyer: String,
    organizer: String,
    bank: String,
}

impl Parties {
    async fn get(url: &str) -> Result<Self> {
        Ok(Parties {
            buyer: ledger::party(url, BUYER).await?,
            organizer: ledger::party(url, ORGANIZER).await?,
            bank: ledger::party(url, BANK).await?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let cli = Cli::parse();
    match cli.command {
        Step::Setup { dar } => setup(&cli.url, &dar).await,
        Step::Purchase {
            count,
            price,
            pause,
        } => purchase(&cli.url, count, price, pause).await,
    }
}

async fn setup(url: &str, dar: &std::path::Path) -> Result<()> {
    let package_id = ledger::upload_model(url, dar).await?;
    println!("  model uploaded (package {})", short(&package_id));
    let p = Parties::get(url).await?;
    println!(
        "  parties: {}, {}, {}",
        short(&p.buyer),
        short(&p.organizer),
        short(&p.bank)
    );
    Ok(())
}

async fn purchase(url: &str, count: u32, first_price: f64, pause: u64) -> Result<()> {
    let p = Parties::get(url).await?;
    let mut ledger = Ledger::connect(url).await?;
    let wait = || tokio::time::sleep(std::time::Duration::from_secs(pause));

    for i in 0..count {
        let price = first_price + 5.0 * f64::from(i);
        println!("Ticket {} of {count}, price {price}", i + 1);

        // 1. The bank issues the cash to the buyer.
        let cash = Cash {
            issuer: DamlParty::new(&p.bank),
            owner: DamlParty::new(&p.buyer),
            amount: DamlDecimal::new(price),
        };
        let tx = ledger
            .submit(&p.bank, create("Cash", cash.to_create_arguments()))
            .await?;
        let cash_id = tx.created("Cash")?.to_string();
        println!(
            "  1/3  TX@{:<4} {BANK} issues {price} cash to {BUYER}",
            tx.offset
        );
        wait().await;

        // 2. The organizer offers the buyer a ticket.
        let offer = TicketOffer {
            organizer: DamlParty::new(&p.organizer),
            buyer: DamlParty::new(&p.buyer),
            price: DamlDecimal::new(price),
        };
        let tx = ledger
            .submit(
                &p.organizer,
                create("TicketOffer", offer.to_create_arguments()),
            )
            .await?;
        let offer_id = tx.created("TicketOffer")?.to_string();
        println!(
            "  2/3  TX@{:<4} {ORGANIZER} offers {BUYER} a ticket for {price}",
            tx.offset
        );
        wait().await;

        // 3. The buyer accepts, paying with the cash.
        let accept = Accept {
            cash_id: DamlContractId::new(cash_id),
        };
        let tx = ledger
            .submit(
                &p.buyer,
                command::Command::Exercise(ExerciseCommand {
                    template_id: Some(Ledger::template("TicketOffer")),
                    contract_id: offer_id,
                    choice: "Accept".to_string(),
                    choice_argument: Some(accept.to_lapi_value()),
                }),
            )
            .await?;
        tx.created("TicketAgreement")?;
        println!(
            "  3/3  TX@{:<4} {BUYER} accepts: cash goes to {ORGANIZER}, ticket agreement created",
            tx.offset
        );
        wait().await;
    }
    Ok(())
}

/// A party or package id shortened for display: `Alice::1220ab…`.
fn short(id: &str) -> String {
    match id.split_once("::") {
        Some((name, fingerprint)) => {
            format!("{name}::{}…", &fingerprint[..fingerprint.len().min(6)])
        }
        None => format!("{}…", &id[..id.len().min(12)]),
    }
}

fn create(entity: &str, arguments: ledger_api::v2::Record) -> command::Command {
    command::Command::Create(CreateCommand {
        template_id: Some(Ledger::template(entity)),
        create_arguments: Some(arguments),
    })
}
