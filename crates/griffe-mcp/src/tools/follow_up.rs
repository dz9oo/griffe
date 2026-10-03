//! Relances : file, brouillon, attestation d'envoi. Jamais d'envoi SMTP.

use griffe_core::app::Executor;
use griffe_core::clock::today_local;
use griffe_core::domain::phrase_from_editor;
use griffe_core::domain::{FollowUpSubject, SnoozePreset, parse_date, snooze_date};
use griffe_core::follow_up::{
    ArrangeProspectPhrases, CreateProspectGenre, DropProspectGenre, GenreWordDraft, KeepGenreWords,
    MarkFollowUpSent, MomentDraft, PhraseRewrite, PrepareFollowUp, RetractLastFollowUp,
    RewriteGenreWords, RewriteProspectPhrases, SetDossierGenre, SetFollowUpDate, SetFollowUpSender,
    SkipFollowUpStep, SnoozeFollowUp, card_for, follow_up_board, follow_up_queue, prospect_genres,
    prospect_phrases,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, outcome_json};

fn today_or(today: Option<String>) -> Result<time::Date, String> {
    match today {
        Some(s) => parse_date(&s).map_err(|e| e.to_string()),
        None => Ok(today_local()),
    }
}

fn resolve_subject(
    store: &griffe_core::store::Store,
    reference: &str,
) -> Result<FollowUpSubject, String> {
    use griffe_core::reference::{self, RefMatch};
    match reference::resolve_follow_up_subject(store.connection(), reference) {
        Ok(RefMatch::Unique(s)) => Ok(s),
        Ok(RefMatch::NotFound) => Err(format!("aucune relance ne correspond à « {reference} »")),
        Ok(RefMatch::Ambiguous(c)) => {
            let list = c
                .iter()
                .map(|(id, label)| format!("  {id} — {label}"))
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "« {reference} » désigne plusieurs relances :\n{list}"
            ))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TodayArgs {
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RefArgs {
    /// Opportunité ou facture (UUID, préfixe, nom ou numéro).
    reference: String,
    today: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LetterArgs {
    /// Opportunité ou facture (UUID, préfixe, nom ou numéro).
    reference: String,
    today: Option<String>,
    /// Sujet. Absent : modèle de cadence (prepare) ou dernier brouillon (sent).
    subject_line: Option<String>,
    /// Corps. Absent : modèle de cadence (prepare) ou dernier brouillon (sent).
    body: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RewritePhraseArgs {
    /// Identifiant du moment (`hello`, `bump`, `value`, `close`).
    key: String,
    /// Nom du moment, tel qu'on le lit.
    label: String,
    /// Sujet. `<prénom>`, ou le jeton `{{prenom}}`.
    subject: String,
    /// Corps. Mêmes balises que le sujet.
    body: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AddMomentArgs {
    /// Nom du moment. Défaut : « Nouveau moment ».
    label: Option<String>,
    /// Sujet. Défaut : `<sujet>`.
    subject: Option<String>,
    /// Corps. Défaut : une lettre courte.
    body: Option<String>,
    /// Jours après le précédent. Défaut : 7.
    days: Option<i64>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct MomentKeyArgs {
    /// Identifiant du moment.
    key: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct MoveMomentArgs {
    /// Identifiant du moment.
    key: String,
    /// `earlier` pour monter, `later` pour descendre.
    direction: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SetGapArgs {
    /// Identifiant du moment.
    key: String,
    /// Jours après le précédent.
    days: i64,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SenderArgs {
    email: String,
    name: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SnoozeArgs {
    reference: String,
    /// `AAAA-MM-JJ`. Ignoré si `preset` est fourni (`tomorrow`, `next_week`, `monday`).
    until: Option<String>,
    preset: Option<String>,
    today: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GenreNameArgs {
    name: String,
    /// Identifiant du genre dont on copie les mots. Absent : les phrases par défaut.
    copy_from: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GenreIdArgs {
    id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RewriteGenreArgs {
    /// Identifiant du genre.
    genre_id: String,
    /// Identifiant du moment.
    key: String,
    subject: String,
    body: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SetGenreArgs {
    reference: String,
    /// Nom du genre. Vide : le dossier redevient sans genre.
    name: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct KeepWordsArgs {
    reference: String,
    today: Option<String>,
    /// Absent : ce champ n'a pas été modifié, on garde le modèle.
    subject_line: Option<String>,
    body: Option<String>,
    /// Quand le dossier n'a pas de genre.
    genre_name: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ScheduleArgs {
    reference: String,
    on: String,
    today: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[tool_router(router = follow_up_router, vis = "pub(crate)")]
impl FreeflowServer {
    #[tool(
        name = "follow_up.queue",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn follow_up_queue_tool(
        &self,
        Parameters(args): Parameters<TodayArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match follow_up_queue(store.connection(), today) {
            Ok(cards) => ok_json(cards),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.board",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn follow_up_board_tool(
        &self,
        Parameters(args): Parameters<TodayArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match follow_up_board(store.connection(), today) {
            Ok(cards) => ok_json(cards),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Les phrases de prospection, dans l'ordre. Le coffre garde `{{prenom}}`, `{{sujet}}`,
    /// `{{montant}}`, `{{moi}}`, `{{societe}}`.
    #[tool(
        name = "follow_up.phrases",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn follow_up_phrases(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match prospect_phrases(store.connection()) {
            Ok(phrases) => ok_json(phrases),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Ajoute un moment à la fin. Les conversations déjà engagées finissent leur série.
    #[tool(
        name = "follow_up.add_moment",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_add_moment(
        &self,
        Parameters(args): Parameters<AddMomentArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let mut drafts = match living_drafts(&store) {
            Ok(drafts) => drafts,
            Err(e) => return err_text(e),
        };
        let offset_days = args.days.unwrap_or(if drafts.is_empty() { 0 } else { 7 });
        drafts.push(MomentDraft {
            key: None,
            label: args.label.unwrap_or_else(|| "Nouveau moment".to_string()),
            offset_days,
            subject: args.subject.unwrap_or_else(|| "{{sujet}}".to_string()),
            body: args.body.unwrap_or_else(|| NEW_MOMENT_BODY.to_string()),
            revision: None,
        });
        execute_arrange(&mut store, drafts, &self.ctx(args.dry_run))
    }

    /// Retire un moment. Il en reste au moins un.
    #[tool(
        name = "follow_up.drop_moment",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_drop_moment(
        &self,
        Parameters(args): Parameters<MomentKeyArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let mut drafts = match living_drafts(&store) {
            Ok(drafts) => drafts,
            Err(e) => return err_text(e),
        };
        let Some(index) = moment_index(&drafts, &args.key) else {
            return err_text("Ce moment n'existe pas.");
        };
        drafts.remove(index);
        zero_first(&mut drafts);
        execute_arrange(&mut store, drafts, &self.ctx(args.dry_run))
    }

    /// Monte ou descend un moment d'un cran.
    #[tool(
        name = "follow_up.move_moment",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_move_moment(
        &self,
        Parameters(args): Parameters<MoveMomentArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let mut drafts = match living_drafts(&store) {
            Ok(drafts) => drafts,
            Err(e) => return err_text(e),
        };
        let Some(index) = moment_index(&drafts, &args.key) else {
            return err_text("Ce moment n'existe pas.");
        };
        match args.direction.as_str() {
            "earlier" if index > 0 => {
                drafts.swap(index, index - 1);
                zero_first(&mut drafts);
            }
            "later" if index + 1 < drafts.len() => {
                drafts.swap(index, index + 1);
                zero_first(&mut drafts);
            }
            "earlier" | "later" => {}
            _ => return err_text("Monte ou descends le moment."),
        }
        execute_arrange(&mut store, drafts, &self.ctx(args.dry_run))
    }

    /// Règle l'écart d'un moment, en jours après le précédent.
    #[tool(
        name = "follow_up.set_gap",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn follow_up_set_gap(&self, Parameters(args): Parameters<SetGapArgs>) -> CallToolResult {
        let mut store = self.store.lock().await;
        let mut drafts = match living_drafts(&store) {
            Ok(drafts) => drafts,
            Err(e) => return err_text(e),
        };
        let Some(index) = moment_index(&drafts, &args.key) else {
            return err_text("Ce moment n'existe pas.");
        };
        drafts[index].offset_days = args.days;
        execute_arrange(&mut store, drafts, &self.ctx(args.dry_run))
    }

    /// Réécrit le nom, le sujet et le corps d'un moment. Ne change ni l'ordre ni l'écart.
    #[tool(
        name = "follow_up.rewrite_phrase",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn follow_up_rewrite_phrase(
        &self,
        Parameters(args): Parameters<RewritePhraseArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let revision = match prospect_phrases(store.connection()) {
            Ok(phrases) => phrases
                .into_iter()
                .find(|phrase| phrase.key == args.key)
                .map(|phrase| phrase.revision)
                .unwrap_or(0),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = RewriteProspectPhrases {
            phrases: vec![PhraseRewrite {
                key: args.key,
                label: args.label,
                subject: args.subject,
                body: args.body,
                revision,
            }],
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Les genres, par nom. Les mots seulement.
    #[tool(
        name = "follow_up.genres",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn follow_up_genres(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match prospect_genres(store.connection()) {
            Ok(genres) => ok_json(genres),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Crée un genre en copiant les sujets et les corps.
    #[tool(
        name = "follow_up.add_genre",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_add_genre(
        &self,
        Parameters(args): Parameters<GenreNameArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = CreateProspectGenre {
            name: args.name,
            copy_from: args.copy_from,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Retire un genre. Les fiches qui le portaient redeviennent sans genre.
    #[tool(
        name = "follow_up.drop_genre",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn follow_up_drop_genre(
        &self,
        Parameters(args): Parameters<GenreIdArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = DropProspectGenre { id: args.id };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Réécrit le sujet et le corps d'un moment, pour ce genre seulement.
    #[tool(
        name = "follow_up.rewrite_genre",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn follow_up_rewrite_genre(
        &self,
        Parameters(args): Parameters<RewriteGenreArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = RewriteGenreWords {
            genre_id: args.genre_id,
            words: vec![GenreWordDraft {
                key: args.key,
                subject: phrase_from_editor(&args.subject),
                body: phrase_from_editor(&args.body),
            }],
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Pose un genre sur le dossier, ou l'enlève si le nom est vide.
    #[tool(
        name = "follow_up.set_genre",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn follow_up_set_genre(
        &self,
        Parameters(args): Parameters<SetGenreArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(subject) => subject,
            Err(e) => return err_text(e),
        };
        let cmd = SetDossierGenre {
            subject,
            name: args.name,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Remplace le sujet et le corps de ce moment, pour ce genre seulement.
    /// Ne classe rien et n'envoie rien.
    #[tool(
        name = "follow_up.keep_words",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn follow_up_keep_words(
        &self,
        Parameters(args): Parameters<KeepWordsArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(today) => today,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(subject) => subject,
            Err(e) => return err_text(e),
        };
        let cmd = KeepGenreWords {
            subject,
            today,
            subject_line: args.subject_line.map(|text| phrase_from_editor(&text)),
            body: args.body.map(|text| phrase_from_editor(&text)),
            genre_name: args.genre_name,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.show",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn follow_up_show(&self, Parameters(args): Parameters<RefArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        match card_for(store.connection(), subject, today) {
            Ok(card) => ok_json(card),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.set_sender",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn follow_up_set_sender(
        &self,
        Parameters(args): Parameters<SenderArgs>,
    ) -> CallToolResult {
        let cmd = SetFollowUpSender {
            email: args.email,
            name: args.name,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Prépare un brouillon : renvoie le RFC 5322. N'ouvre pas le client mail (geste humain).
    #[tool(
        name = "follow_up.prepare",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_prepare(&self, Parameters(args): Parameters<LetterArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = PrepareFollowUp {
            subject,
            today,
            subject_line: args.subject_line,
            body: args.body,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Atteste que le mail est parti. Un agent dépose une action en attente.
    #[tool(
        name = "follow_up.mark_sent",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_mark_sent(
        &self,
        Parameters(args): Parameters<LetterArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = MarkFollowUpSent {
            subject,
            today,
            subject_line: args.subject_line,
            body: args.body,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.skip",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_skip(&self, Parameters(args): Parameters<RefArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = SkipFollowUpStep { subject, today };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.snooze",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_snooze(&self, Parameters(args): Parameters<SnoozeArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let until = if let Some(preset) = args.preset.as_deref() {
            let p = match preset {
                "tomorrow" => SnoozePreset::Tomorrow,
                "next_week" => SnoozePreset::NextWeek,
                "monday" => SnoozePreset::NextMonday,
                other => return err_text(format!("preset inconnu : {other}")),
            };
            snooze_date(today, p)
        } else if let Some(s) = args.until {
            match parse_date(&s) {
                Ok(d) => d,
                Err(e) => return err_text(e.to_string()),
            }
        } else {
            return err_text("précisez until ou preset (tomorrow, next_week, monday)");
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = SnoozeFollowUp {
            subject,
            until,
            today,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.schedule",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_schedule(
        &self,
        Parameters(args): Parameters<ScheduleArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let on = match parse_date(&args.on) {
            Ok(d) => d,
            Err(e) => return err_text(e.to_string()),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = SetFollowUpDate { subject, on, today };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.retract",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn follow_up_retract(&self, Parameters(args): Parameters<RefArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = RetractLastFollowUp { subject, today };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}

const NEW_MOMENT_BODY: &str = "Bonjour {{prenom}},\n\n{{sujet}}\n\nBien à vous,\n{{moi}}\n";

fn living_drafts(store: &griffe_core::store::Store) -> Result<Vec<MomentDraft>, String> {
    prospect_phrases(store.connection())
        .map(|phrases| phrases.iter().map(MomentDraft::from_phrase).collect())
        .map_err(|e| e.to_string())
}

fn moment_index(drafts: &[MomentDraft], key: &str) -> Option<usize> {
    drafts
        .iter()
        .position(|draft| draft.key.as_deref() == Some(key))
}

fn zero_first(drafts: &mut [MomentDraft]) {
    if let Some(first) = drafts.first_mut() {
        first.offset_days = 0;
    }
}

fn execute_arrange(
    store: &mut griffe_core::store::Store,
    drafts: Vec<MomentDraft>,
    ctx: &griffe_core::app::ExecutionContext,
) -> CallToolResult {
    match Executor::new(store).execute(&ArrangeProspectPhrases { moments: drafts }, ctx) {
        Ok(outcome) => ok_json(outcome_json(&outcome)),
        Err(e) => err_text(e.to_string()),
    }
}
