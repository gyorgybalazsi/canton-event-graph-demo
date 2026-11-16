//! A thin layer over the Ledger API: connect, make sure the model and parties
//! exist, and submit one command at a time, waiting for its transaction.

use anyhow::{Context, Result, anyhow, bail};
use client::party_management::allocate_parties::allocate_parties;
use client::party_management::get_parties::get_parties;
use client::upload_dar::{list_dars, package_id_from_dar, upload_dars};
use client::utils::build_filters_by_party;
use daml_type_rep::template_id::TemplateId;
use ledger_api::v2::command_service_client::CommandServiceClient;
use ledger_api::v2::event::Event;
use ledger_api::v2::{
    Command, Commands, EventFormat, SubmitAndWaitForTransactionRequest, TransactionFormat,
    TransactionShape, command,
};
use std::path::Path;
use tonic::transport::Channel;

use crate::model::{MODULE, PACKAGE};

/// The user the commands are submitted as. The sandbox runs without
/// authentication, so it only shows up as the submitting user in the ledger.
const USER_ID: &str = "ticket-purchase-demo";

/// Uploads the DAR and checks that the participant now knows its package.
pub async fn upload_model(url: &str, dar: &Path) -> Result<String> {
    let package_id = package_id_from_dar(dar)?;
    upload_dars(Path::new(url), &[dar.to_path_buf()]).await?;
    let known = list_dars(Path::new(url)).await?;
    if !known.contains(&package_id) {
        bail!(
            "the participant does not list package {package_id} after uploading {}",
            dar.display()
        );
    }
    Ok(package_id)
}

/// Returns the party allocated with `hint`, allocating it on the first run.
pub async fn party(url: &str, hint: &str) -> Result<String> {
    let prefix = format!("{hint}::");
    let existing = get_parties(url.to_string(), None, None).await?;
    if let Some(party) = existing.into_iter().find(|p| p.starts_with(&prefix)) {
        return Ok(party);
    }
    allocate_parties(url.to_string(), None, vec![hint.to_string()])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("allocating party {hint} returned nothing"))
}

/// What a submitted command did: its transaction and the contracts it created.
pub struct Submitted {
    pub offset: i64,
    pub update_id: String,
    /// `(template name, contract id)` of each created contract, in event order.
    pub created: Vec<(String, String)>,
}

impl Submitted {
    /// The id of the first contract of `template` the transaction created.
    pub fn created(&self, template: &str) -> Result<&str> {
        self.created
            .iter()
            .find(|(t, _)| t == template)
            .map(|(_, cid)| cid.as_str())
            .ok_or_else(|| anyhow!("transaction {} created no {template}", self.update_id))
    }
}

pub struct Ledger {
    commands: CommandServiceClient<Channel>,
}

impl Ledger {
    pub async fn connect(url: &str) -> Result<Self> {
        let channel = Channel::from_shared(url.to_string())?
            .connect()
            .await
            .with_context(|| format!("could not connect to the ledger at {url}"))?;
        Ok(Ledger {
            commands: CommandServiceClient::new(channel),
        })
    }

    pub fn template(entity: &str) -> ledger_api::v2::Identifier {
        TemplateId::new(PACKAGE, MODULE, entity).to_template_id()
    }

    /// Submits one command as `party` and waits for its transaction.
    pub async fn submit(&mut self, party: &str, command: command::Command) -> Result<Submitted> {
        let parties = vec![party.to_string()];
        let request = SubmitAndWaitForTransactionRequest {
            commands: Some(Commands {
                act_as: parties.clone(),
                user_id: USER_ID.to_string(),
                command_id: uuid::Uuid::new_v4().to_string(),
                commands: vec![Command {
                    command: Some(command),
                }],
                ..Default::default()
            }),
            transaction_format: Some(TransactionFormat {
                event_format: Some(EventFormat {
                    filters_by_party: build_filters_by_party(&parties),
                    filters_for_any_party: None,
                    verbose: false,
                }),
                transaction_shape: TransactionShape::LedgerEffects as i32,
            }),
        };
        let tx = self
            .commands
            .submit_and_wait_for_transaction(request)
            .await?
            .into_inner()
            .transaction
            .ok_or_else(|| anyhow!("the ledger returned no transaction"))?;
        let created = tx
            .events
            .iter()
            .filter_map(|e| match &e.event {
                Some(Event::Created(c)) => Some((
                    c.template_id
                        .as_ref()
                        .map(|t| t.entity_name.clone())
                        .unwrap_or_default(),
                    c.contract_id.clone(),
                )),
                _ => None,
            })
            .collect();
        Ok(Submitted {
            offset: tx.offset,
            update_id: tx.update_id,
            created,
        })
    }
}
