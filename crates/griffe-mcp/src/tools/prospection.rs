//! Outils `prospect.*` — miroir de `freeflow prospect ...` (CLI, lot 7). Lot 16 : ajout de
//! `show`/`list`/`update`/`archive`/`unarchive`/`delete`/`references` et de
//! `interactions.list`/`interactions.update`/`interactions.delete` — `log_interaction` reste tel
//! quel (asymétrie assumée avec les nouveaux verbes au pluriel `interactions.*`, cohérente avec
//! la décision de ne pas renommer les verbes de transition existants).

use griffe_core::app::Executor;
use griffe_core::domain::{
    ClientId, InteractionId, InteractionKind, OpportunityId, OpportunityStage, Probability,
};
use griffe_core::prospection::{
    self, OpportunityFilter, interaction_by_id, late_actions, list_interactions,
    list_opportunities_with, opportunity_by_id, opportunity_references, pipeline_by_stage,
    weighted_pipeline, without_next_action,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{
    err_text, ok_json, ok_or_return, outcome_json, parse_loss_reason, resolve_opportunity,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateOpportunityArgs {
    /// Référence d'une fiche déjà enregistrée : UUID, préfixe d'UUID, ou nom. Incompatible
    /// avec `prospect`.
    #[serde(default)]
    client: Option<String>,
    /// Nom du prospect : crée la fiche s'il n'existe pas (elle n'apparaît dans Clients qu'au
    /// premier devis ou à la première facture). Incompatible avec `client`.
    #[serde(default)]
    prospect: Option<String>,
    representative: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    street: Option<String>,
    postal_code: Option<String>,
    city: Option<String>,
    /// Code pays ISO 3166-1 alpha-2, ex. `FR`.
    country: Option<String>,
    name: String,
    /// Montant estimé, en centimes d'euro.
    amount_cents: i64,
    /// Probabilité de gain, en pourcentage entier (0-100).
    probability_percent: u8,
    /// Date de prochaine action, au format `AAAA-MM-JJ`.
    next_action: String,
    source: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct OpportunityRefArgs {
    /// Référence de l'opportunité : UUID, préfixe d'UUID, ou nom.
    opportunity: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct OpportunityRefMutationArgs {
    opportunity: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListOpportunitiesArgs {
    /// Inclut les opportunités closes (gagnées/perdues) si vrai (défaut : faux).
    #[serde(default)]
    closed: bool,
    /// Inclut les opportunités archivées si vrai (défaut : faux).
    #[serde(default)]
    archived: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UpdateOpportunityArgs {
    opportunity: String,
    /// Chaque champ fourni remplace la valeur actuelle ; un champ omis est conservé tel quel.
    name: Option<String>,
    amount_cents: Option<i64>,
    probability_percent: Option<u8>,
    next_action: Option<String>,
    source: Option<String>,
    prospect_name: Option<String>,
    representative: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    street: Option<String>,
    postal_code: Option<String>,
    city: Option<String>,
    country: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AdvanceOpportunityArgs {
    opportunity: String,
    /// Étape cible : `qualification`, `discovery`, `proposal`, `negotiation`, `won`, ou `lost`.
    to: String,
    next_action: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WinOpportunityArgs {
    opportunity: String,
    /// Date de début de la mission créée, au format `AAAA-MM-JJ`.
    started_on: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReopenOpportunityArgs {
    opportunity: String,
    /// Prochain pas, au format `AAAA-MM-JJ`.
    next_action: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LoseOpportunityArgs {
    opportunity: String,
    /// `budget`, `timing`, `competitor`, `no-response`, `scope-mismatch`, ou `other:<détail>`.
    reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LogInteractionArgs {
    opportunity: String,
    /// `call`, `email`, `meeting`, ou `note`.
    kind: String,
    note: String,
    /// Par défaut, l'instant présent — au format `AAAA-MM-JJ` pour journaliser un échange
    /// survenu plus tôt.
    occurred_on: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UpdateInteractionArgs {
    /// Identifiant de l'interaction (UUID) — voir `prospect.interactions.list`.
    id: String,
    kind: Option<String>,
    note: Option<String>,
    occurred_on: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct InteractionIdArgs {
    id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LateArgs {
    /// Date du jour, au format `AAAA-MM-JJ` — sert de référence pour détecter le retard.
    today: String,
}

fn midnight_utc(date: time::Date) -> time::OffsetDateTime {
    date.with_hms(0, 0, 0)
        .expect("minuit est toujours une heure valide")
        .assume_utc()
}

fn opportunity_or_not_found(
    store: &griffe_core::store::Store,
    id: OpportunityId,
) -> Result<griffe_core::domain::Opportunity, CallToolResult> {
    match opportunity_by_id(store.connection(), id) {
        Ok(Some(o)) => Ok(o),
        Ok(None) => Err(err_text(format!("opportunité introuvable : {id}"))),
        Err(e) => Err(err_text(e.to_string())),
    }
}

#[tool_router(router = prospection_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Crée une nouvelle opportunité, en étape Qualification. Passez `client` pour rattacher
    /// une fiche existante, ou `prospect` pour créer un prospect (invisible dans Clients tant
    /// qu'il n'a ni devis ni facture).
    #[tool(
        name = "prospect.create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_create(
        &self,
        Parameters(args): Parameters<CreateOpportunityArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let probability = ok_or_return!(
            "probability_percent",
            Probability::new(args.probability_percent)
        );
        let next_action_at = ok_or_return!(
            "next_action",
            griffe_core::domain::parse_date(&args.next_action)
        );
        if args.client.is_some() && args.prospect.is_some() {
            return err_text("fournir `client` ou `prospect`, pas les deux");
        }
        let address = match (args.street, args.postal_code, args.city, args.country) {
            (None, None, None, None) => None,
            (Some(street), Some(postal_code), Some(city), Some(country)) => {
                Some(griffe_core::domain::Address {
                    street,
                    postal_code,
                    city,
                    country,
                })
            }
            _ => {
                return err_text(
                    "les 4 champs d'adresse (street, postal_code, city, country) vont ensemble",
                );
            }
        };
        let outcome = if let Some(prospect_name) = args.prospect {
            let cmd = prospection::CreateProspect {
                prospect_name,
                address,
                representative: args.representative,
                email: args.email,
                phone: args.phone,
                name: args.name,
                amount: griffe_core::domain::Money::from_cents(args.amount_cents),
                probability,
                next_action_at,
                source: args.source,
            };
            Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run))
        } else if let Some(client) = args.client {
            let client_id: ClientId =
                ok_or_return!("client", crate::support::resolve_client(&store, &client));
            let cmd = prospection::CreateOpportunity {
                client_id,
                name: args.name,
                amount: griffe_core::domain::Money::from_cents(args.amount_cents),
                probability,
                next_action_at,
                source: args.source,
            };
            Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run))
        } else {
            return err_text("fournir `client` ou `prospect`");
        };
        match outcome {
            Ok(o) => ok_json(outcome_json(&o)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Affiche une opportunité.
    #[tool(
        name = "prospect.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_show(
        &self,
        Parameters(args): Parameters<OpportunityRefArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        match opportunity_or_not_found(&store, id) {
            Ok(o) => ok_json(o),
            Err(result) => result,
        }
    }

    /// Liste les opportunités ouvertes et non archivées (`closed`/`archived: true` pour élargir).
    #[tool(
        name = "prospect.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_list(
        &self,
        Parameters(args): Parameters<ListOpportunitiesArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let filter = OpportunityFilter {
            include_closed: args.closed,
            include_archived: args.archived,
        };
        match list_opportunities_with(store.connection(), filter) {
            Ok(opportunities) => ok_json(opportunities),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Ce qui référence encore une opportunité (des devis) — à consulter avant `prospect.delete`.
    #[tool(
        name = "prospect.references",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_references(
        &self,
        Parameters(args): Parameters<OpportunityRefArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        match opportunity_references(store.connection(), id) {
            Ok(refs) => ok_json(refs),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Modifie une opportunité ouverte — seuls les champs fournis changent. Ne peut pas changer
    /// l'étape (`prospect.advance`/`win`/`lose`) ni le motif de perte.
    #[tool(
        name = "prospect.update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_update(
        &self,
        Parameters(args): Parameters<UpdateOpportunityArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let current = match opportunity_or_not_found(&store, id) {
            Ok(o) => o,
            Err(result) => return result,
        };
        let next_action_at = match args.next_action {
            Some(s) => Some(ok_or_return!(
                "next_action",
                griffe_core::domain::parse_date(&s)
            )),
            None => current.next_action_at,
        };
        let probability = match args.probability_percent {
            Some(p) => ok_or_return!("probability_percent", Probability::new(p)),
            None => current.probability,
        };
        let name = args.name.unwrap_or(current.name);
        let amount = args
            .amount_cents
            .map_or(current.amount, griffe_core::domain::Money::from_cents);
        let source = args.source.or(current.source);
        let party_touched = args.prospect_name.is_some()
            || args.representative.is_some()
            || args.email.is_some()
            || args.phone.is_some()
            || args.street.is_some();
        let outcome = if party_touched {
            let party =
                match griffe_core::clients::client_by_id(store.connection(), current.client_id) {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        return err_text(format!("fiche introuvable : {}", current.client_id));
                    }
                    Err(e) => return err_text(e.to_string()),
                };
            let address = match (args.street, args.postal_code, args.city, args.country) {
                (None, None, None, None) => party.address,
                (Some(street), Some(postal_code), Some(city), Some(country)) => {
                    Some(griffe_core::domain::Address {
                        street,
                        postal_code,
                        city,
                        country,
                    })
                }
                _ => {
                    return err_text(
                        "les 4 champs d'adresse (street, postal_code, city, country) vont ensemble",
                    );
                }
            };
            let cmd = prospection::UpdateProspect {
                id,
                revision: current.revision,
                client_revision: party.revision,
                name,
                amount,
                probability,
                next_action_at,
                source,
                prospect_name: args.prospect_name.unwrap_or(party.name),
                address,
                representative: args.representative,
                email: args.email,
                phone: args.phone,
            };
            Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run))
        } else {
            let cmd = prospection::UpdateOpportunity {
                id,
                revision: current.revision,
                name,
                amount,
                probability,
                next_action_at,
                source,
            };
            Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run))
        };
        match outcome {
            Ok(o) => ok_json(outcome_json(&o)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Retire une opportunité des listes actives sans la supprimer — un axe distinct de
    /// gagnée/perdue.
    #[tool(
        name = "prospect.archive",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_archive(
        &self,
        Parameters(args): Parameters<OpportunityRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let current = match opportunity_or_not_found(&store, id) {
            Ok(o) => o,
            Err(result) => return result,
        };
        let cmd = prospection::ArchiveOpportunity {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Réintègre une opportunité archivée dans les listes actives.
    #[tool(
        name = "prospect.unarchive",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_unarchive(
        &self,
        Parameters(args): Parameters<OpportunityRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let current = match opportunity_or_not_found(&store, id) {
            Ok(o) => o,
            Err(result) => return result,
        };
        let cmd = prospection::UnarchiveOpportunity {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime une opportunité pour de bon — refusée si un devis la référence (voir
    /// `prospect.references`, ou archivez-la à la place). Effet destructeur : déclenché par un
    /// agent, attend toujours une confirmation humaine.
    #[tool(
        name = "prospect.delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn prospect_delete(
        &self,
        Parameters(args): Parameters<OpportunityRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let current = match opportunity_or_not_found(&store, id) {
            Ok(o) => o,
            Err(result) => return result,
        };
        let cmd = prospection::DeleteOpportunity {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Fait avancer une opportunité vers une autre étape ouverte.
    #[tool(
        name = "prospect.advance",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_advance(
        &self,
        Parameters(args): Parameters<AdvanceOpportunityArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let opportunity_id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let to: OpportunityStage = ok_or_return!("to", args.to.parse());
        let next_action_at = ok_or_return!(
            "next_action",
            griffe_core::domain::parse_date(&args.next_action)
        );
        let cmd = prospection::AdvanceOpportunity {
            opportunity_id,
            to,
            next_action_at,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Gagne une opportunité : crée la mission forfait correspondante.
    #[tool(
        name = "prospect.win",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_win(
        &self,
        Parameters(args): Parameters<WinOpportunityArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let opportunity_id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let started_on = ok_or_return!(
            "started_on",
            griffe_core::domain::parse_date(&args.started_on)
        );
        let cmd = prospection::WinOpportunity {
            opportunity_id,
            started_on,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Rouvre une conversation arrêtée. Les lettres et l'estimation restent.
    #[tool(
        name = "prospect.reopen",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_reopen(
        &self,
        Parameters(args): Parameters<ReopenOpportunityArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let opportunity_id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let next_action_at = ok_or_return!(
            "next_action",
            griffe_core::domain::parse_date(&args.next_action)
        );
        let cmd = prospection::ReopenOpportunity {
            opportunity_id,
            next_action_at,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Perd une opportunité, avec un motif structuré.
    #[tool(
        name = "prospect.lose",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_lose(
        &self,
        Parameters(args): Parameters<LoseOpportunityArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let opportunity_id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let reason = ok_or_return!("reason", parse_loss_reason(&args.reason));
        let cmd = prospection::LoseOpportunity {
            opportunity_id,
            reason,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Journalise une interaction (téléphone, e-mail, rencontre physique, visioconférence, note).
    #[tool(
        name = "prospect.log_interaction",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_log_interaction(
        &self,
        Parameters(args): Parameters<LogInteractionArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let opportunity_id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        let kind: InteractionKind = ok_or_return!("kind", args.kind.parse());
        let occurred_at = match &args.occurred_on {
            Some(s) => Some(midnight_utc(ok_or_return!(
                "occurred_on",
                griffe_core::domain::parse_date(s)
            ))),
            None => None,
        };
        let cmd = prospection::LogInteraction {
            opportunity_id,
            kind,
            note: args.note,
            occurred_at,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les interactions d'une opportunité.
    #[tool(
        name = "prospect.interactions.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_interactions_list(
        &self,
        Parameters(args): Parameters<OpportunityRefArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!(
            "opportunity",
            resolve_opportunity(&store, &args.opportunity)
        );
        match list_interactions(store.connection(), id) {
            Ok(interactions) => ok_json(interactions),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Modifie une interaction existante.
    #[tool(
        name = "prospect.interactions.update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_interactions_update(
        &self,
        Parameters(args): Parameters<UpdateInteractionArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id: InteractionId = ok_or_return!("id", args.id.parse());
        let current = match interaction_by_id(store.connection(), id) {
            Ok(Some(i)) => i,
            Ok(None) => return err_text(format!("interaction introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let kind = match &args.kind {
            Some(s) => ok_or_return!("kind", s.parse()),
            None => current.kind,
        };
        let occurred_at = match &args.occurred_on {
            Some(s) => midnight_utc(ok_or_return!(
                "occurred_on",
                griffe_core::domain::parse_date(s)
            )),
            None => current.occurred_at,
        };
        let cmd = prospection::UpdateInteraction {
            id,
            revision: current.revision,
            kind,
            note: args.note.unwrap_or(current.note),
            occurred_at,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime une interaction.
    #[tool(
        name = "prospect.interactions.delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn prospect_interactions_delete(
        &self,
        Parameters(args): Parameters<InteractionIdArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id: InteractionId = ok_or_return!("id", args.id.parse());
        let current = match interaction_by_id(store.connection(), id) {
            Ok(Some(i)) => i,
            Ok(None) => return err_text(format!("interaction introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = prospection::DeleteInteraction {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Opportunités ouvertes dont la prochaine action est en retard par rapport à `today`.
    #[tool(
        name = "prospect.late",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_late(&self, Parameters(args): Parameters<LateArgs>) -> CallToolResult {
        let today = ok_or_return!("today", griffe_core::domain::parse_date(&args.today));
        let store = self.store.lock().await;
        match late_actions(store.connection(), today) {
            Ok(opportunities) => ok_json(opportunities),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Opportunités ouvertes sans prochaine action (filet de sécurité).
    #[tool(
        name = "prospect.orphans",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_orphans(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match without_next_action(store.connection()) {
            Ok(opportunities) => ok_json(opportunities),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Pipeline pondéré (montant × probabilité) et répartition par étape.
    #[tool(
        name = "prospect.pipeline",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_pipeline(&self) -> CallToolResult {
        let store = self.store.lock().await;
        let weighted = ok_or_return!("pipeline", weighted_pipeline(store.connection()));
        let by_stage = ok_or_return!("pipeline", pipeline_by_stage(store.connection()));
        ok_json(json!({
            "weighted_total_cents": weighted.cents(),
            "by_stage": by_stage.iter().map(|s| json!({
                "stage": s.stage.as_str(),
                "count": s.count,
                "weighted_amount_cents": s.weighted_amount.cents(),
            })).collect::<Vec<_>>(),
        }))
    }
}
