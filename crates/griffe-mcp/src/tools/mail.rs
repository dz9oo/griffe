//! Courrier : lecture du compte, armement d'une lettre, envoi du jour.
//! Pas de mot de passe, pas de passage au serveur : la fenêtre ou `griffe courrier flush` poste.

use griffe_core::app::Executor;
use griffe_core::mail::{
    ArmOutbound, SaveLetterface, SaveMailSignature, SetAutomaticSend, profile, trial_letter,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, outcome_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ArmArgs {
    /// Adresse du destinataire.
    to: String,
    /// Sujet.
    subject: String,
    /// Corps, en texte seul.
    body: String,
    /// Ancre affichée avec la lettre. Absente : aucune.
    anchor: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LetterfaceArgs {
    /// `vert`, `encre` ou `sceau`.
    ink: String,
    /// Ligne sous le nom. Vide : pas de ligne.
    metier: String,
    /// Adresse en bas de la lettre. Vide : pas de ligne.
    site: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SignatureArgs {
    /// Formule de fin. Vide : Bien à vous, le nom, la société.
    signature: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TrialArgs {
    /// Destinataire. Absent : l'adresse qui signe.
    to: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AutomaticArgs {
    /// Vrai : les lettres dues aujourd'hui partent pendant que le coffre est ouvert.
    enabled: bool,
    #[serde(default)]
    dry_run: bool,
}

#[tool_router(router = mail_router, vis = "pub(crate)")]
impl FreeflowServer {
    #[tool(
        name = "mail.show",
        description = "Compte d'envoi. Le mot de passe n'est jamais renvoyé.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn mail_show(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match profile(store.connection()) {
            Ok(account) => ok_json(serde_json::json!({
                "from_name": account.from_name,
                "from_address": account.from_address,
                "host": account.host,
                "port": account.port,
                "tls": account.tls.as_str(),
                "username": account.username,
                "preset": account.preset.as_str(),
                "has_secret": account.has_secret,
                "auto_send": account.auto_send,
                "ready": account.ready,
                "signature": account.signature,
                "link_ink": account.link_ink.as_str(),
                "metier": account.metier,
                "site": account.site,
                "probe_ok": account.probe.as_ref().map(|probe| probe.ok),
                "probe_detail": account.probe.as_ref().map(|probe| probe.detail.clone()),
            })),
            Err(error) => err_text(error.to_string()),
        }
    }

    #[tool(
        name = "mail.arm",
        description = "Arme une lettre. Un agent la laisse en attente de confirmation. Rien n'est posté ici.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn mail_arm(&self, Parameters(args): Parameters<ArmArgs>) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = ArmOutbound {
            kind: "letter".into(),
            anchor: args.anchor,
            to_address: args.to,
            subject: args.subject,
            body: args.body,
            delay_secs: 0,
            session_token: None,
            follow_subject: None,
            follow_subject_id: None,
            follow_cycle: None,
            follow_step: None,
            client_id: None,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(error) => err_text(error.to_string()),
        }
    }

    #[tool(
        name = "mail.save_letterface",
        description = "Enregistre la couleur des liens, le métier et le site de la lettre.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn mail_save_letterface(
        &self,
        Parameters(args): Parameters<LetterfaceArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = SaveLetterface {
            ink: args.ink,
            metier: args.metier,
            site: args.site,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(error) => err_text(error.to_string()),
        }
    }

    #[tool(
        name = "mail.save_signature",
        description = "Enregistre la formule qui ferme les lettres. Vide : Bien à vous, le nom, la société.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn mail_save_signature(
        &self,
        Parameters(args): Parameters<SignatureArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = SaveMailSignature {
            signature: args.signature,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(error) => err_text(error.to_string()),
        }
    }

    #[tool(
        name = "mail.trial",
        description = "Arme la lettre d'essai, le premier message pour un prénom fictif. Un agent la laisse en attente. Rien n'est posté ici.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn mail_trial(&self, Parameters(args): Parameters<TrialArgs>) -> CallToolResult {
        let mut store = self.store.lock().await;
        let account = match profile(store.connection()) {
            Ok(account) => account,
            Err(error) => return err_text(error.to_string()),
        };
        if !account.ready {
            return err_text("Le courrier n'est pas encore branché.".to_string());
        }
        let letter = match trial_letter(store.connection()) {
            Ok(letter) => letter,
            Err(error) => return err_text(error.to_string()),
        };
        let cmd = ArmOutbound {
            kind: "trial".into(),
            anchor: Some("essai".into()),
            to_address: args.to.unwrap_or(account.from_address),
            subject: letter.subject,
            body: letter.body,
            delay_secs: 0,
            session_token: None,
            follow_subject: None,
            follow_subject_id: None,
            follow_cycle: None,
            follow_step: None,
            client_id: None,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(error) => err_text(error.to_string()),
        }
    }

    #[tool(
        name = "mail.automatic",
        description = "Active ou éteint l'envoi des lettres dues aujourd'hui. Un agent laisse le geste en attente.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn mail_automatic(&self, Parameters(args): Parameters<AutomaticArgs>) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = SetAutomaticSend {
            enabled: args.enabled,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(error) => err_text(error.to_string()),
        }
    }
}
