//! L'exécuteur : point de passage unique de toute mutation. Orchestre dry-run, idempotence,
//! politique de confirmation et audit — une [`super::Command`] n'implémente que sa propre
//! logique métier, jamais ces préoccupations transverses.

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::actor::Actor;
use super::audit::{self, AuditOutcome};
use super::command::Command;
use super::error::AppError;
use super::pending::PendingActionId;
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub actor: Actor,
    pub dry_run: bool,
}

impl ExecutionContext {
    #[must_use]
    pub const fn new(actor: Actor, dry_run: bool) -> Self {
        Self { actor, dry_run }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<T> {
    /// La commande a été appliquée réellement.
    Applied(T),
    /// `dry_run` était actif : aucune écriture n'a eu lieu.
    DryRun,
    /// La clé d'idempotence correspondait à une exécution précédente : voici son résultat
    /// mémorisé, sans nouvel effet.
    AlreadyApplied(T),
    /// La commande nécessite une confirmation humaine et a été déposée en attente.
    PendingConfirmation(PendingActionId),
}

pub struct Executor<'a> {
    store: &'a mut Store,
}

impl<'a> Executor<'a> {
    pub fn new(store: &'a mut Store) -> Self {
        Self { store }
    }

    /// Exécute `command` selon le contexte fourni.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si la persistance échoue ou si la commande elle-même échoue.
    pub fn execute<C: Command>(
        &mut self,
        command: &C,
        ctx: &ExecutionContext,
    ) -> Result<Outcome<C::Output>, AppError> {
        if ctx.dry_run {
            return Ok(Outcome::DryRun);
        }

        // Transaction IMMEDIATE : sérialise les écrivains concurrents dès l'ouverture, avant
        // même la lecture de la clé d'idempotence — sans quoi deux appels concurrents avec la
        // même clé pourraient tous deux la trouver absente et appliquer la commande deux fois.
        let tx = self
            .store
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(key) = command.idempotency_key()
            && let Some(cached) = tx
                .query_row(
                    "SELECT output_json FROM idempotency_keys WHERE key = ?1",
                    [key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        {
            let output: C::Output = serde_json::from_str(&cached)?;
            tx.commit()?;
            return Ok(Outcome::AlreadyApplied(output));
        }

        let command_json = serde_json::to_string(command)?;

        if command.requires_confirmation() && matches!(ctx.actor, Actor::Agent { .. }) {
            let id = PendingActionId::new();
            let session = ctx.actor.session().unwrap_or_default();
            tx.execute(
                "INSERT INTO pending_actions (id, actor_session, command_name, command_json, created_at, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
                params![id.to_string(), session, C::NAME, command_json, OffsetDateTime::now_utc().format(&Rfc3339)?],
            )?;
            audit::append(
                &tx,
                &ctx.actor,
                C::NAME,
                &command_json,
                AuditOutcome::PendingConfirmation,
                None,
            )?;
            tx.commit()?;
            return Ok(Outcome::PendingConfirmation(id));
        }

        let output = command.apply(&tx)?;
        let output_json = serde_json::to_string(&output)?;

        if let Some(key) = command.idempotency_key() {
            tx.execute(
                "INSERT INTO idempotency_keys (key, command_name, output_json, recorded_at) VALUES (?1, ?2, ?3, ?4)",
                params![key, C::NAME, output_json, OffsetDateTime::now_utc().format(&Rfc3339)?],
            )?;
        }

        audit::append(
            &tx,
            &ctx.actor,
            C::NAME,
            &command_json,
            AuditOutcome::Applied,
            Some(&output_json),
        )?;

        tx.commit()?;
        Ok(Outcome::Applied(output))
    }

    /// Applique une commande précédemment déposée en attente par un agent — c'est l'humain qui
    /// confirme qui joue le rôle d'acteur pour l'audit, quelle que soit la session d'origine.
    ///
    /// # Errors
    ///
    /// Retourne [`AppError::PendingActionNotFound`] si `id` n'existe pas,
    /// [`AppError::PendingActionAlreadyResolved`] si elle a déjà été traitée, ou
    /// [`AppError::PendingActionKindMismatch`] si `C` ne correspond pas à la commande déposée.
    pub fn confirm<C: Command>(
        &mut self,
        id: PendingActionId,
    ) -> Result<Outcome<C::Output>, AppError> {
        let tx = self
            .store
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        let row: Option<(String, String, String)> = tx
            .query_row(
                "SELECT command_name, command_json, status FROM pending_actions WHERE id = ?1",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (command_name, command_json, status) =
            row.ok_or_else(|| AppError::PendingActionNotFound(id.to_string()))?;

        if status != "pending" {
            return Err(AppError::PendingActionAlreadyResolved(id.to_string()));
        }
        if command_name != C::NAME {
            return Err(AppError::PendingActionKindMismatch {
                id: id.to_string(),
                expected: C::NAME,
                actual: command_name,
            });
        }

        let command: C = serde_json::from_str(&command_json)?;
        let output = command.apply(&tx)?;
        let output_json = serde_json::to_string(&output)?;

        tx.execute(
            "UPDATE pending_actions SET status = 'confirmed', resolved_at = ?1 WHERE id = ?2",
            params![OffsetDateTime::now_utc().format(&Rfc3339)?, id.to_string()],
        )?;

        // La confirmation est par nature un acte humain : c'est elle qui autorise la mutation.
        audit::append(
            &tx,
            &Actor::Human,
            C::NAME,
            &command_json,
            AuditOutcome::Confirmed,
            Some(&output_json),
        )?;

        tx.commit()?;
        Ok(Outcome::Applied(output))
    }
}
