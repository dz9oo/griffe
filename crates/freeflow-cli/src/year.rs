//! `freeflow year ...` — clôture d'exercice (lot 20) : snapshot du résultat, affectation,
//! approbation, documents de clôture (`freeflow-docs`).
//!
//! Depuis le lot 31, `year balance` et `year render … balance-sheet` exposent le grand livre
//! dérivé (`freeflow_core::ledger`) : balance des comptes et bilan 2033-A, pour un exercice clos
//! ou non — comme le FEC, c'est ce qu'on regarde *avant* de clore.
//!
//! Depuis le lot 34, `year checklist` affiche le parcours de clôture guidé
//! (`freeflow_core::closing`) : les étapes viennent du cœur, seule l'*action* suggérée pour
//! chacune (une commande à taper) est propre à cette façade — voir `cli_hint`.
//!
//! Un exercice se désigne par l'**année civile de sa clôture** (`freeflow year show 2026`) :
//! contrairement aux clients/missions, il n'a pas de nom et sa période dérive du profil — pas
//! besoin du résolveur de référence générique. Comme pour `invoice render`, l'écriture disque
//! et l'appel `typst` vivent ici, dans l'adaptateur : le cœur ne touche que `&Connection`.

use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use freeflow_core::app::{Actor, ExecutionContext, Executor};
use freeflow_core::clock::today_local;
use freeflow_core::closing::{
    ClosingChecklist, ClosingPhase, ClosingStep, ClosingStepKey, GLOSSARY, StepStatus,
    checklist_json, closing_checklist, glossary_json,
};
use freeflow_core::company::{CompanyProfile, company_profile};
use freeflow_core::domain::{FiscalYearEnd, Money, OpeningBalanceLine, format_date};
use freeflow_core::fiscal_year::{
    ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear, FiscalYearRecord,
    UpdateFiscalYearAppropriation, fiscal_year_ending_in, list_fiscal_years,
};
use freeflow_core::ledger::{
    BalanceSheet, TrialBalance, balance_json, build_ledger, ledger_ending_in,
};
use freeflow_core::opening_balance::{
    DeleteOpeningBalance, OpeningBalanceRecord, RecordOpeningBalance, UpdateOpeningBalance,
    opening_balance,
};
use freeflow_core::store::Store;
use serde_json::json;
use time::Date;

use crate::error::CliError;
use crate::output::{format_json, format_outcome, format_outcome_as};
use crate::parsers::{parse_date, parse_money, parse_opening_line};
use crate::table;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DocKind {
    /// PV des décisions de l'associé unique (approbation, affectation, quitus) — PDF.
    Minutes,
    /// Décision d'affectation du résultat (origine → affectation) — PDF.
    Appropriation,
    /// Compte de résultat simplifié, avec colonne N−1 si l'exercice précédent est clos — PDF.
    Synthesis,
    /// Cases principales 2065/2033 (dont le bilan 2033-A dérivé du grand livre) en JSON, à
    /// transmettre à l'expert-comptable.
    Liasse,
    /// Bilan simplifié (2033-A : actif brut/amortissements/net, passif) et balance des comptes,
    /// dérivés du grand livre — PDF. L'exercice n'a pas besoin d'être clos.
    BalanceSheet,
}

#[derive(Debug, Subcommand)]
pub enum YearCommand {
    /// Clôt un exercice : fige le résultat calculé (CA, charges, IS) et enregistre
    /// l'affectation (réserve légale, dividendes) en projet, à approuver ensuite. Nécessite
    /// confirmation humaine quand `--actor agent:...`.
    Close {
        /// Année civile de la clôture — la période dérive de la date de clôture du profil
        /// (année civile à défaut). Sinon, précisez `--starts-on`/`--ends-on`.
        #[arg(long, conflicts_with_all = ["starts_on", "ends_on"])]
        period: Option<i32>,
        #[arg(long, value_parser = parse_date, requires = "ends_on")]
        starts_on: Option<Date>,
        #[arg(long, value_parser = parse_date, requires = "starts_on")]
        ends_on: Option<Date>,
        /// Dotation à la réserve légale (euros, ex. `500` ou `500.00`).
        #[arg(long, value_parser = parse_money, default_value = "0")]
        legal_reserve: Money,
        /// Dividendes distribués (euros).
        #[arg(long, value_parser = parse_money, default_value = "0")]
        dividends: Money,
        /// Opte pour le report en arrière du déficit de l'exercice (art. 220 quinquies CGI) sur
        /// le bénéfice de l'exercice précédent clos ici : la créance d'IS entre dans le résultat
        /// net. Refusé sans déficit, sans exercice précédent ou sans bénéfice d'imputation.
        #[arg(long)]
        carry_back: bool,
        /// Charges non déductibles fiscalement (art. 39-4 CGI : amendes, dépenses somptuaires…),
        /// à réintégrer — reportées sur le PV (art. 223 quater) et la 2033-B. Euros, défaut 0.
        #[arg(long, value_parser = parse_money, default_value = "0")]
        non_deductible: Money,
        /// Date du jour (défaut : aujourd'hui, heure locale) — la clôture est refusée tant que
        /// l'exercice n'est pas écoulé.
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Liste les exercices clos, du plus ancien au plus récent.
    List,
    /// Détail d'un exercice clos (snapshot, affectation, approbation).
    Show {
        /// Année civile de la clôture (ex. `2026`).
        period: i32,
    },
    /// Révise l'affectation d'un exercice encore en projet (le snapshot reste figé).
    Amend {
        period: i32,
        #[arg(long, value_parser = parse_money)]
        legal_reserve: Option<Money>,
        #[arg(long, value_parser = parse_money)]
        dividends: Option<Money>,
    },
    /// Approuve un exercice (date de l'AG) — il devient immuable. Une sauvegarde du coffre est
    /// écrite juste avant (`backups/pre-approve-<période>-<horodatage>.db`) : c'est le seul
    /// filet, une approbation ne se défait pas. Nécessite confirmation humaine quand
    /// `--actor agent:...`.
    Approve {
        period: i32,
        /// Date de la décision d'approbation — ni avant la clôture, ni dans le futur.
        #[arg(long, value_parser = parse_date)]
        approved_on: Date,
        /// Date du jour (défaut : aujourd'hui, heure locale).
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Supprime un exercice encore en projet (clos par erreur).
    Rm { period: i32 },
    /// Parcours de clôture guidé de l'exercice clos dans PERIOD : où en est la clôture, ce qui
    /// la bloque, ce qui mérite attention, et ce qui reste à faire après (approbation,
    /// documents, solde d'IS, liasse, dépôt au greffe). Lecture seule.
    Checklist {
        /// Année civile de la clôture (ex. `2026`) — même désignation que `show`.
        period: i32,
        /// Date du jour (défaut : aujourd'hui) — décide si l'exercice est écoulé et des
        /// échéances dépassées.
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Lexique de la clôture : les mots du parcours et des documents (bilan, à-nouveaux,
    /// report à nouveau, réserve légale, liasse, FEC…) expliqués sans jargon.
    Glossary,
    /// Balance des comptes et bilan (2033-A) dérivés du grand livre de l'exercice clos dans
    /// PERIOD — clos ou non : à-nouveaux, ventes, achats, banque, opérations de clôture.
    Balance {
        /// Année civile de la clôture (ex. `2026`) — même désignation que `show` et `fec export`.
        period: i32,
    },
    /// Rend un document de clôture (PDF, ou JSON pour la liasse) dans `--out`.
    Render {
        period: i32,
        #[arg(value_enum)]
        doc: DocKind,
        #[arg(long)]
        out: PathBuf,
        /// Date du jour, requise pour un PV en projet (non approuvé), ignorée sinon.
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Bilan d'ouverture : reprise du dernier bilan tenu avant FreeFlow (expert-comptable),
    /// point de départ du report à nouveau, de la réserve légale et des à-nouveaux du FEC.
    #[command(subcommand)]
    Opening(OpeningCommand),
}

/// La reprise se saisit **avant** toute clôture dans l'application : dès qu'un exercice est clos
/// ici, son snapshot a hérité de ce bilan et le figer devient la règle. Une ligne s'écrit
/// `compte:libellé:D|C:montant` (ex. `101000:Capital social:C:1000.00`) ; seuls les comptes de
/// bilan (classes 1 à 5) sont admis, et le total des débits doit égaler celui des crédits.
#[derive(Debug, Subcommand)]
pub enum OpeningCommand {
    /// Affiche le bilan d'ouverture : lignes, totaux, capitaux propres repris.
    Show,
    /// Enregistre le bilan d'ouverture, ou le remplace en entier s'il existe déjà. Nécessite
    /// confirmation humaine quand `--actor agent:...`.
    Set {
        /// Premier jour de l'exercice qui s'ouvre sur ce bilan (lendemain de la clôture reprise).
        #[arg(long, value_parser = parse_date)]
        opens_on: Date,
        /// Provenance, libre (ex. « bilan au 30/09/2025, cabinet X »).
        #[arg(long)]
        source: Option<String>,
        /// Une ligne `compte:libellé:D|C:montant`, répétable.
        #[arg(long = "line", value_name = "SPEC", value_parser = parse_opening_line, required_unless_present = "lines_file")]
        lines: Vec<OpeningBalanceLine>,
        /// Fichier texte : une ligne par ligne de bilan, même syntaxe ; lignes vides et `#`
        /// ignorées. Se cumule avec `--line`.
        #[arg(long, value_name = "FICHIER")]
        lines_file: Option<PathBuf>,
        /// Déficits fiscaux antérieurs encore reportables (euros, case 870 du dernier 2033-D),
        /// hors bilan : imputés sur les bénéfices des exercices clos ici.
        #[arg(long, value_parser = parse_money, default_value = "0")]
        tax_losses: Money,
        /// IS de l'exercice précédent (euros, relevé 2572) : base des acomptes d'IS du premier
        /// exercice suivi ici — sans lui, le calendrier dit « base inconnue ».
        #[arg(long, value_parser = parse_money)]
        prior_is: Option<Money>,
        /// TVA due au titre de l'exercice précédent (euros, CA12) : base des acomptes 3514 du
        /// réel simplifié.
        #[arg(long, value_parser = parse_money)]
        prior_vat: Option<Money>,
    },
    /// Reprend le bilan d'ouverture depuis un fichier du cabinet : une **balance générale** en
    /// CSV (compte, libellé, débit, crédit — ou soldes), ou le **FEC** de l'exercice précédent
    /// (18 colonnes, `|` ou tabulation — l'export de Tiime : Comptabilité → Exports → FEC ;
    /// d'Indy : Documents → Export FEC). Les comptes de bilan sont repris tels quels, les
    /// comptes 6/7 agrégés en résultat (120/129) sauf si la balance est déjà après affectation ;
    /// le reste est écarté et dit. `--dry-run` montre l'aperçu (lignes retenues, écartées,
    /// résultat dérivé, avertissements) sans rien écrire. Nécessite confirmation humaine quand
    /// `--actor agent:...`.
    Import {
        #[arg(value_name = "FICHIER")]
        file: PathBuf,
        /// Premier jour de l'exercice qui s'ouvre sur ce bilan (lendemain de la clôture reprise).
        #[arg(long, value_parser = parse_date)]
        opens_on: Date,
        /// Force le format (`balance`, `fec`) si la détection se trompe.
        #[arg(long, value_parser = clap::value_parser!(freeflow_core::opening_balance::import::OpeningImportFormat))]
        format: Option<freeflow_core::opening_balance::import::OpeningImportFormat>,
        /// Provenance, libre (défaut : le nom du fichier).
        #[arg(long)]
        source: Option<String>,
        #[arg(long, value_parser = parse_money, default_value = "0")]
        tax_losses: Money,
        #[arg(long, value_parser = parse_money)]
        prior_is: Option<Money>,
        #[arg(long, value_parser = parse_money)]
        prior_vat: Option<Money>,
        /// Durée d'usage en mois appliquée à chaque immobilisation reprise (couple 2xx/28x).
        /// Sans elle, les candidats sont seulement signalés : `freeflow asset add` ensuite.
        #[arg(long)]
        duration: Option<u32>,
    },
    /// Reprend le bilan d'ouverture depuis les **cases du 2033-A** du cabinet (le PDF n'a pas
    /// de numéros de compte). `--box 084=9540.00` répétable ; la case 169 (dont CCA d'associés)
    /// est extraite de la 172, jamais ajoutée en plus. `--dry-run` montre l'aperçu.
    From2033a {
        #[arg(long, value_parser = parse_date)]
        opens_on: Date,
        /// Case 2033-A `NNN=montant` (euros), répétable — ex. `--box 084=9540.00`.
        #[arg(long = "box", value_name = "CASE=MONTANT", value_parser = parse_cerfa_box, required = true)]
        boxes: Vec<(String, Money)>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long, value_parser = parse_money, default_value = "0")]
        tax_losses: Money,
        #[arg(long, value_parser = parse_money)]
        prior_is: Option<Money>,
        #[arg(long, value_parser = parse_money)]
        prior_vat: Option<Money>,
    },
    /// Supprime le bilan d'ouverture (tant qu'aucun exercice n'est clos).
    Rm,
}

fn parse_cerfa_box(s: &str) -> Result<(String, Money), String> {
    let (case, amount) = s
        .split_once('=')
        .ok_or_else(|| format!("attendu CASE=MONTANT (ex. 084=9540.00), reçu « {s} »"))?;
    let amount = Money::parse_decimal(amount.trim()).map_err(|e| e.to_string())?;
    Ok((case.trim().to_string(), amount))
}

/// Enregistrer ou remplacer : la CLI offre la sémantique « set », la commande envoyée au cœur
/// reste l'une des deux commandes en état complet.
#[allow(clippy::too_many_arguments)]
fn record_or_replace(
    store: &mut Store,
    ctx: &ExecutionContext,
    opens_on: Date,
    source: Option<String>,
    lines: Vec<OpeningBalanceLine>,
    tax_losses: Money,
    prior_corporate_tax: Option<Money>,
    prior_vat_due: Option<Money>,
) -> Result<freeflow_core::app::Outcome<i64>, CliError> {
    Ok(match opening_balance(store.connection())? {
        Some(existing) => Executor::new(store).execute(
            &UpdateOpeningBalance {
                revision: existing.revision,
                opens_on,
                source,
                lines,
                tax_losses,
                prior_corporate_tax,
                prior_vat_due,
            },
            ctx,
        )?,
        None => Executor::new(store).execute(
            &RecordOpeningBalance {
                opens_on,
                source,
                lines,
                tax_losses,
                prior_corporate_tax,
                prior_vat_due,
            },
            ctx,
        )?,
    })
}

/// L'aperçu d'un import de bilan : lignes retenues, écartées, résultat dérivé, avertissements.
fn import_preview_human(preview: &freeflow_core::opening_balance::import::ImportPreview) -> String {
    let rows: Vec<Vec<String>> = preview
        .lines
        .iter()
        .map(|l| {
            let (debit, credit) = match l.side {
                freeflow_core::domain::Side::Debit => (l.amount.to_string(), String::new()),
                freeflow_core::domain::Side::Credit => (String::new(), l.amount.to_string()),
            };
            vec![l.account.to_string(), l.label.clone(), debit, credit]
        })
        .collect();
    let balance = preview.to_opening_balance(None);
    let mut out = format!(
        "(dry-run) bilan d'ouverture au {} lu depuis {} — {} compte(s) retenu(s)\n{}\nTotal \
         débit : {} — total crédit : {}{}",
        format_date(preview.opens_on),
        match preview.format {
            freeflow_core::opening_balance::import::OpeningImportFormat::Balance => {
                "une balance générale"
            }
            freeflow_core::opening_balance::import::OpeningImportFormat::Fec => "un FEC",
            freeflow_core::opening_balance::import::OpeningImportFormat::Cerfa2033A => {
                "un 2033-A"
            }
        },
        preview.lines.len(),
        table::render(&["Compte", "Libellé", "Débit", "Crédit"], &rows),
        balance.total_debit(),
        balance.total_credit(),
        preview.derived_result.map_or(String::new(), |r| format!(
            "\nRésultat dérivé des comptes 6/7 : {r} (posé en {})",
            if r.is_negative() { "129" } else { "120" }
        )),
    );
    if !preview.dropped.is_empty() {
        out.push_str(&format!(
            "\nÉcarté : {}",
            preview
                .dropped
                .iter()
                .map(|(what, why)| format!("{what} ({why})"))
                .collect::<Vec<_>>()
                .join(" ; ")
        ));
    }
    for w in &preview.warnings {
        out.push_str(&format!("\n⚠ {w}"));
    }
    out
}

fn opening_json(r: &OpeningBalanceRecord) -> serde_json::Value {
    let equity = r.equity();
    json!({
        "opens_on": format_date(r.balance.opens_on),
        "source": r.balance.source,
        "lines": r.balance.lines.iter().map(|l| json!({
            "account": l.account.as_str(),
            "label": l.label,
            "side": l.side.as_str(),
            "amount_cents": l.amount.cents(),
        })).collect::<Vec<_>>(),
        "total_debit_cents": r.balance.total_debit().cents(),
        "total_credit_cents": r.balance.total_credit().cents(),
        "equity": {
            "share_capital_cents": equity.share_capital.cents(),
            "legal_reserve_cents": equity.legal_reserve.cents(),
            "retained_earnings_cents": equity.retained_earnings.cents(),
        },
        "tax_losses_cents": r.balance.tax_losses.cents(),
        "prior_corporate_tax_cents": r.prior_corporate_tax.map(Money::cents),
        "prior_vat_due_cents": r.prior_vat_due.map(Money::cents),
        "revision": r.revision,
    })
}

fn opening_human(r: &OpeningBalanceRecord) -> String {
    let equity = r.equity();
    let rows: Vec<Vec<String>> = r
        .balance
        .lines
        .iter()
        .map(|l| {
            let (debit, credit) = match l.side {
                freeflow_core::domain::Side::Debit => (l.amount.to_string(), String::new()),
                freeflow_core::domain::Side::Credit => (String::new(), l.amount.to_string()),
            };
            vec![l.account.to_string(), l.label.clone(), debit, credit]
        })
        .collect();
    format!(
        "Bilan d'ouverture au {}{}\n{}\nTotal débit : {} — total crédit : {}\n\
         Capitaux propres repris : capital {}, réserve légale {}, report à nouveau {}\n\
         Déficits fiscaux reportables repris : {}\nIS de l'exercice précédent : {} — TVA due de \
         l'exercice précédent : {}\n(révision {})",
        format_date(r.balance.opens_on),
        r.balance
            .source
            .as_deref()
            .map_or_else(String::new, |s| format!(" — {s}")),
        table::render(&["Compte", "Libellé", "Débit", "Crédit"], &rows),
        r.balance.total_debit(),
        r.balance.total_credit(),
        equity.share_capital,
        equity.legal_reserve,
        equity.retained_earnings,
        r.balance.tax_losses,
        r.prior_corporate_tax
            .map_or_else(|| "inconnu".to_string(), |m| m.to_string()),
        r.prior_vat_due
            .map_or_else(|| "inconnue".to_string(), |m| m.to_string()),
        r.revision,
    )
}

/// Les lignes de `--lines-file` : une par ligne, vides et commentaires `#` ignorés.
fn read_lines_file(path: &Path) -> Result<Vec<OpeningBalanceLine>, CliError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        CliError::Unexpected(format!("lecture de {} impossible : {e}", path.display()))
    })?;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| parse_opening_line(l).map_err(CliError::Domain))
        .collect()
}

fn run_opening(
    cmd: OpeningCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        OpeningCommand::Show => match opening_balance(store.connection())? {
            Some(record) if json => format_json(&opening_json(&record)),
            Some(record) => opening_human(&record),
            None if json => format_json(&serde_json::Value::Null),
            None => "aucun bilan d'ouverture — `freeflow year opening set`".to_string(),
        },
        OpeningCommand::Set {
            opens_on,
            source,
            mut lines,
            lines_file,
            tax_losses,
            prior_is,
            prior_vat,
        } => {
            if let Some(path) = lines_file {
                lines.extend(read_lines_file(&path)?);
            }
            let outcome = record_or_replace(
                store, ctx, opens_on, source, lines, tax_losses, prior_is, prior_vat,
            )?;
            format_outcome_as(&outcome, json, |revision| {
                format!("bilan d'ouverture enregistré (révision {revision})")
            })
        }
        OpeningCommand::Import {
            file,
            opens_on,
            format,
            source,
            tax_losses,
            prior_is,
            prior_vat,
            duration,
        } => {
            let bytes = std::fs::read(&file).map_err(|e| {
                CliError::Unexpected(format!("lecture de {} impossible : {e}", file.display()))
            })?;
            let preview = freeflow_core::opening_balance::import::import_opening_balance(
                &bytes, opens_on, format,
            )
            .map_err(|e| CliError::Domain(e.to_string()))?;
            if ctx.dry_run {
                return Ok(if json {
                    format_json(&freeflow_core::opening_balance::import::preview_json(
                        &preview,
                    ))
                } else {
                    import_preview_human(&preview)
                });
            }
            let source = source.or_else(|| {
                file.file_name()
                    .map(|n| format!("import de {}", n.to_string_lossy()))
            });
            let outcome = record_or_replace(
                store,
                ctx,
                opens_on,
                source,
                preview.lines.clone(),
                tax_losses,
                prior_is,
                prior_vat,
            )?;
            let rendered = format_outcome_as(&outcome, json, |revision| {
                format!(
                    "bilan d'ouverture repris depuis {} : {} compte(s){} (révision {revision})",
                    file.display(),
                    preview.lines.len(),
                    preview
                        .derived_result
                        .map_or(String::new(), |r| format!(", résultat dérivé {r}"))
                )
            });
            let mut extra = String::new();
            if let Some(months) = duration
                && matches!(outcome, freeflow_core::app::Outcome::Applied(_))
            {
                let candidates = freeflow_core::domain::fixed_asset_candidates(&preview.lines);
                let mut declared = 0usize;
                for candidate in &candidates {
                    let cmd = freeflow_core::fixed_assets::AddFixedAsset::from_candidate(
                        candidate, months, opens_on,
                    );
                    Executor::new(store).execute(&cmd, ctx)?;
                    declared += 1;
                }
                if declared > 0 {
                    extra = format!("\n{declared} immobilisation(s) déclarée(s) sur {months} mois");
                }
            }
            if json || (preview.warnings.is_empty() && extra.is_empty()) {
                format!("{rendered}{extra}")
            } else {
                format!("{rendered}{extra}\n⚠ {}", preview.warnings.join("\n⚠ "))
            }
        }
        OpeningCommand::From2033a {
            opens_on,
            boxes,
            source,
            tax_losses,
            prior_is,
            prior_vat,
        } => {
            let mapped: freeflow_core::opening_balance::import::CerfaBoxes =
                boxes.into_iter().collect();
            let preview = freeflow_core::opening_balance::import::from_2033a(opens_on, &mapped)
                .map_err(|e| CliError::Domain(e.to_string()))?;
            if ctx.dry_run {
                return Ok(if json {
                    format_json(&freeflow_core::opening_balance::import::preview_json(
                        &preview,
                    ))
                } else {
                    import_preview_human(&preview)
                });
            }
            let source = source.or_else(|| Some("2033-A du cabinet".to_string()));
            let outcome = record_or_replace(
                store,
                ctx,
                opens_on,
                source,
                preview.lines.clone(),
                tax_losses,
                prior_is,
                prior_vat,
            )?;
            format_outcome_as(&outcome, json, |revision| {
                format!(
                    "bilan d'ouverture repris du 2033-A : {} compte(s) (révision {revision})",
                    preview.lines.len()
                )
            })
        }
        OpeningCommand::Rm => {
            let existing = opening_balance(store.connection())?.ok_or_else(|| {
                CliError::Domain("aucun bilan d'ouverture enregistré".to_string())
            })?;
            let outcome = Executor::new(store).execute(
                &DeleteOpeningBalance {
                    revision: existing.revision,
                },
                ctx,
            )?;
            format_outcome_as(&outcome, json, |()| {
                "bilan d'ouverture supprimé".to_string()
            })
        }
    };
    Ok(output)
}

fn year_json(r: &FiscalYearRecord) -> serde_json::Value {
    json!({
        "id": r.id.to_string(),
        "starts_on": format_date(r.starts_on),
        "ends_on": format_date(r.ends_on),
        "revenue_ht_cents": r.revenue_ht.cents(),
        "expenses_cents": r.expenses.cents(),
        "director_remuneration_cents": r.director_remuneration.cents(),
        "depreciation_cents": r.depreciation.cents(),
        "result_before_tax_cents": r.result_before_tax.cents(),
        "losses_imputed_cents": r.losses_imputed.cents(),
        "taxable_result_cents": r.taxable_result().cents(),
        "corporate_tax_cents": r.corporate_tax.cents(),
        "carried_back_cents": r.carried_back.cents(),
        "carry_back_credit_cents": r.carry_back_credit.cents(),
        "net_result_cents": r.net_result.cents(),
        "legal_reserve_cents": r.legal_reserve.cents(),
        "dividends_cents": r.dividends.cents(),
        "retained_earnings_cents": r.retained_earnings.cents(),
        "losses_carried_forward_cents": r.losses_carried_forward.cents(),
        "approved_on": r.approved_on.map(format_date),
        "revision": r.revision,
    })
}

fn require_year(store: &Store, period: i32) -> Result<FiscalYearRecord, CliError> {
    fiscal_year_ending_in(store.connection(), period)?.ok_or_else(|| {
        CliError::Domain(format!(
            "aucun exercice clos ne se termine en {period} — voir `freeflow year list`"
        ))
    })
}

fn require_profile(store: &Store) -> Result<CompanyProfile, CliError> {
    company_profile(store.connection())?.ok_or_else(|| {
        CliError::Domain("aucun profil d'entreprise défini : freeflow company set-profile".into())
    })
}

/// La période à clore : explicite (`--starts-on`/`--ends-on`), ou dérivée du profil pour
/// `--period` (l'exercice dont la clôture récurrente tombe dans cette année civile).
fn resolve_close_period(
    store: &Store,
    period: Option<i32>,
    starts_on: Option<Date>,
    ends_on: Option<Date>,
) -> Result<(Date, Date), CliError> {
    match (period, starts_on, ends_on) {
        (Some(year), None, None) => {
            let fiscal_year_end = require_profile(store)?
                .fiscal_year_end
                .unwrap_or(FiscalYearEnd::CALENDAR);
            let end = fiscal_year_end.end_in_year(year);
            let fy = fiscal_year_end.containing(end);
            Ok((fy.start(), fy.end()))
        }
        (None, Some(start), Some(end)) => Ok((start, end)),
        _ => Err(CliError::Domain(
            "précisez soit --period, soit --starts-on et --ends-on".into(),
        )),
    }
}

/// La sauvegarde écrite avant une approbation : `backups/pre-approve-<période>-<horodatage>.db`
/// à côté du coffre, chiffrée comme lui. Partagée par `year approve` et `freeflow confirm`.
///
/// # Errors
///
/// L'échec de la sauvegarde — l'appelant abandonne alors l'approbation.
pub(crate) fn pre_approve_backup(store: &Store, period: i32) -> Result<PathBuf, CliError> {
    let stamp = time::OffsetDateTime::now_utc()
        .format(&time::macros::format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .map_err(|e| CliError::Unexpected(e.to_string()))?;
    // Un nom neuf : `backup_to` refuse d'écraser, et deux approbations dans la même seconde
    // (une refusée par le cœur puis une acceptée, par exemple) partagent l'horodatage.
    let backups = store.db_path().with_file_name("backups");
    let dest = (0u32..)
        .map(|n| {
            let suffix = if n == 0 {
                String::new()
            } else {
                format!("-{n}")
            };
            backups.join(format!("pre-approve-{period}-{stamp}{suffix}.db"))
        })
        .find(|p| !p.exists() && !p.with_extension("db.kdf").exists())
        .expect("un suffixe libre finit toujours par exister");
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CliError::Unexpected(format!(
                "sauvegarde préalable impossible ({}) : {e} — approbation abandonnée",
                parent.display()
            ))
        })?;
    }
    store.backup_to(&dest).map_err(|e| {
        CliError::Unexpected(format!(
            "sauvegarde préalable impossible ({}) : {e} — approbation abandonnée",
            dest.display()
        ))
    })?;
    Ok(dest)
}

/// La sauvegarde préalable d'une approbation confirmée après coup (`freeflow confirm`) : la
/// période se relit dans l'action en attente.
pub(crate) fn pre_approve_backup_for_action(
    store: &Store,
    command_json: &str,
) -> Result<PathBuf, CliError> {
    let cmd: ApproveFiscalYear = serde_json::from_str(command_json)
        .map_err(|e| CliError::Unexpected(format!("action en attente illisible : {e}")))?;
    let period = freeflow_core::fiscal_year::fiscal_year_by_id(store.connection(), cmd.id)?
        .map_or(0, |r| r.ends_on.year());
    pre_approve_backup(store, period)
}

/// Ajoute au texte rendu l'emplacement de la sauvegarde préalable (jamais au JSON, dont le
/// contrat ne change pas).
pub(crate) fn with_backup_note(rendered: String, backup: &Path, json: bool) -> String {
    if json {
        rendered
    } else {
        format!("{rendered}\nSauvegarde préalable : {}", backup.display())
    }
}

fn write_document(out: &Path, bytes: &[u8]) -> Result<String, CliError> {
    std::fs::write(out, bytes).map_err(|e| {
        CliError::Unexpected(format!("écriture de {} impossible : {e}", out.display()))
    })?;
    Ok(format!(
        "✓ {} écrit ({} octets)",
        out.display(),
        bytes.len()
    ))
}

/// La commande qui fait avancer une étape du parcours, quand il y en a une — propre à cette
/// façade (la fenêtre et le serveur MCP ont les leurs), branchée sur la clé stable de l'étape.
fn cli_hint(checklist: &ClosingChecklist, step: &ClosingStep) -> Option<String> {
    let period = checklist.period;
    let reserve = checklist
        .minimum_legal_reserve
        .filter(|m| !m.is_zero())
        .map_or(String::new(), |m| {
            format!(" --legal-reserve {}", m.to_decimal_string())
        });
    let hint = match step.key {
        ClosingStepKey::Profile => "freeflow company set-profile …".to_string(),
        ClosingStepKey::PeriodEnded => return None,
        ClosingStepKey::OpeningBalance => format!(
            "freeflow year opening set --opens-on {} --line \"compte:libellé:D|C:montant\" …",
            format_date(checklist.exercise.start())
        ),
        ClosingStepKey::PreviousYear => format!(
            "freeflow year approve {} --approved-on AAAA-MM-JJ (ou year amend / year rm)",
            period - 1
        ),
        ClosingStepKey::Invoices => {
            "freeflow invoice aged-balance ; freeflow payment record …".to_string()
        }
        ClosingStepKey::Expenses => {
            "freeflow expense edit <RÉF> --receipt <fichier> ; asset add (matériel ≥ 500 € HT)"
                .to_string()
        }
        ClosingStepKey::Bank => {
            "freeflow bank list --unmatched ; bank reconcile / expense reconcile".to_string()
        }
        ClosingStepKey::Result | ClosingStepKey::BalanceSheet => {
            format!("freeflow year balance {period}")
        }
        ClosingStepKey::Close => format!(
            "freeflow year close --period {period}{reserve}{}",
            if checklist.carry_back_available.is_some() {
                " [--carry-back]"
            } else {
                ""
            }
        ),
        ClosingStepKey::Appropriation => format!(
            "freeflow year amend {period}{}",
            if reserve.is_empty() {
                " --legal-reserve …".to_string()
            } else {
                reserve
            }
        ),
        ClosingStepKey::Approve => {
            format!("freeflow year approve {period} --approved-on AAAA-MM-JJ")
        }
        ClosingStepKey::Documents => format!(
            "freeflow year render {period} <minutes|appropriation|synthesis|balance-sheet|liasse> \
             --out … ; freeflow fec export {period} --out …"
        ),
        ClosingStepKey::Liasse => format!("freeflow year render {period} liasse --out …"),
        ClosingStepKey::Vat => "freeflow fiscal calendar (CA3/CA12 à venir) ; company \
                                set-profile --vat-regime … si le régime manque"
            .to_string(),
        ClosingStepKey::Das2 => match step.status {
            StepStatus::Warning => "freeflow expense edit <RÉF> --supplier <nom>".to_string(),
            _ => return None,
        },
        ClosingStepKey::CorporateTax | ClosingStepKey::Filing => return None,
    };
    Some(hint)
}

const fn status_glyph(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Done => "✓",
        StepStatus::Todo => "→",
        StepStatus::Warning => "!",
        StepStatus::Blocked => "✗",
        StepStatus::Info => "·",
        StepStatus::Later => "○",
    }
}

/// Le parcours en texte : une section par phase, une ligne par étape (glyphe de statut, titre,
/// échéance), son détail en retrait, et la commande suggérée quand l'étape appelle un geste.
fn checklist_human(checklist: &ClosingChecklist) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "Parcours de clôture — exercice du {} au {} (clos en {}), vu le {} : {}\n",
        format_date(checklist.exercise.start()),
        format_date(checklist.exercise.end()),
        checklist.period,
        format_date(checklist.today),
        checklist.stage.label().to_uppercase(),
    );
    for phase in ClosingPhase::ALL {
        let _ = writeln!(out, "\n{}", phase.label());
        for step in checklist.steps_in(phase) {
            let due = step
                .due_on
                .map_or(String::new(), |d| format!(" (échéance {})", format_date(d)));
            let _ = writeln!(out, "  {} {}{due}", status_glyph(step.status), step.title);
            let _ = writeln!(out, "      {}", step.detail);
            if matches!(
                step.status,
                StepStatus::Todo | StepStatus::Warning | StepStatus::Blocked
            ) && let Some(hint) = cli_hint(checklist, step)
            {
                let _ = writeln!(out, "      ↳ {hint}");
            }
        }
    }
    out.push_str("\nLes mots de ce parcours sont expliqués par `freeflow year glossary`.");
    out.trim_end().to_string()
}

/// Le lexique en texte : un terme par paragraphe, sa définition en retrait.
fn glossary_human() -> String {
    use std::fmt::Write as _;
    let mut out = String::from("Lexique de la clôture\n");
    for entry in GLOSSARY {
        let _ = write!(out, "\n{}\n    {}\n", entry.term, entry.meaning);
    }
    out.trim_end().to_string()
}

/// Balance des comptes et bilan en texte : les deux tableaux du 2033-A, puis la balance.
fn balance_human(balance: &TrialBalance, sheet: &BalanceSheet) -> String {
    let money = |m: Money| {
        if m.is_zero() {
            String::new()
        } else {
            m.to_string()
        }
    };
    let asset_rows: Vec<Vec<String>> = sheet
        .assets
        .iter()
        .map(|a| {
            vec![
                a.case_gross.to_string(),
                a.label.to_string(),
                money(a.gross),
                money(a.depreciation),
                money(a.net),
            ]
        })
        .chain([vec![
            "110/112".to_string(),
            "Total général".to_string(),
            sheet.total_assets_gross.to_string(),
            money(sheet.total_depreciation),
            sheet.total_assets_net.to_string(),
        ]])
        .collect();
    let liability_rows: Vec<Vec<String>> = sheet
        .liabilities
        .iter()
        .map(|l| vec![l.case.to_string(), l.label.to_string(), money(l.amount)])
        .chain([
            vec![
                "142".to_string(),
                "Total I — capitaux propres".to_string(),
                sheet.total_equity.to_string(),
            ],
            vec![
                "180".to_string(),
                "Total général".to_string(),
                sheet.total_liabilities.to_string(),
            ],
        ])
        .collect();
    let balance_rows: Vec<Vec<String>> = balance
        .rows
        .iter()
        .map(|r| {
            let (debit, credit) = if r.balance.is_negative() {
                (String::new(), (-r.balance).to_string())
            } else {
                (r.balance.to_string(), String::new())
            };
            vec![
                r.account.number.to_string(),
                r.account.label.to_string(),
                money(r.debit),
                money(r.credit),
                debit,
                credit,
            ]
        })
        .collect();
    format!(
        "Bilan au {} (exercice du {} au {}) — présentation 2033-A\n\nActif\n{}\n\nPassif\n{}\n\n\
         Balance des comptes\n{}\nTotal débit : {} — total crédit : {}{}",
        format_date(sheet.exercise.end()),
        format_date(sheet.exercise.start()),
        format_date(sheet.exercise.end()),
        table::render(&["Case", "Rubrique", "Brut", "Amort.", "Net"], &asset_rows),
        table::render(&["Case", "Rubrique", "Montant"], &liability_rows),
        table::render(
            &["Compte", "Libellé", "Débit", "Crédit", "Solde D", "Solde C"],
            &balance_rows,
        ),
        balance.total_debit,
        balance.total_credit,
        if sheet.is_balanced() {
            String::new()
        } else {
            "\n⚠ bilan déséquilibré".to_string()
        },
    )
}

fn render(
    store: &Store,
    period: i32,
    doc: DocKind,
    out: &Path,
    today: Option<Date>,
) -> Result<String, CliError> {
    if matches!(doc, DocKind::BalanceSheet) {
        // Dérivé du grand livre : pas besoin d'un exercice clos, comme le FEC.
        let (profile, ledger) = ledger_ending_in(store.connection(), period)?;
        let bytes = freeflow_docs::render_balance_sheet(
            &profile,
            &ledger.balance_sheet(),
            &ledger.trial_balance(),
        )
        .map_err(|e| CliError::Unexpected(e.to_string()))?;
        return write_document(out, &bytes);
    }
    let record = require_year(store, period)?;
    let profile = require_profile(store)?;
    let bytes = match doc {
        DocKind::BalanceSheet => unreachable!("traité ci-dessus"),
        DocKind::Minutes => {
            let today = record.approved_on.or(today).ok_or_else(|| {
                CliError::Domain(
                    "cet exercice n'est pas approuvé : précisez --today pour dater le projet \
                     de PV"
                        .into(),
                )
            })?;
            freeflow_docs::render_approval_minutes(&profile, &record, today)
                .map_err(|e| CliError::Unexpected(e.to_string()))?
        }
        DocKind::Appropriation => freeflow_docs::render_appropriation_decision(&profile, &record)
            .map_err(|e| CliError::Unexpected(e.to_string()))?,
        DocKind::Synthesis => {
            // Le document reflète le snapshot figé à la clôture, pas un recalcul vivant.
            let result = record.accounting_result();
            let years = list_fiscal_years(store.connection())?;
            let prior = years
                .iter()
                .filter(|y| y.ends_on < record.starts_on)
                .max_by_key(|y| y.ends_on);
            freeflow_docs::render_synthesis(&profile, &result, prior)
                .map_err(|e| CliError::Unexpected(e.to_string()))?
        }
        DocKind::Liasse => {
            // La liasse est un JSON : une extension `.pdf` demandée par erreur produirait un
            // fichier que rien n'ouvre (lot 36).
            if out
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
            {
                return Err(CliError::Domain(
                    "la liasse est un fichier JSON, pas un PDF : donnez une extension `.json` \
                     (les PDF sont minutes, appropriation, synthesis, balance-sheet)"
                        .into(),
                ));
            }
            let ledger = build_ledger(store.connection(), record.period())?;
            let export = freeflow_docs::liasse_export(&profile, &record, Some(&ledger));
            let mut bytes = serde_json::to_vec_pretty(&export)
                .map_err(|e| CliError::Unexpected(e.to_string()))?;
            bytes.push(b'\n');
            bytes
        }
    };
    write_document(out, &bytes)
}

pub fn run(
    cmd: YearCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        YearCommand::Close {
            period,
            starts_on,
            ends_on,
            legal_reserve,
            dividends,
            carry_back,
            non_deductible,
            today,
        } => {
            let (starts_on, ends_on) = resolve_close_period(store, period, starts_on, ends_on)?;
            let command = CloseFiscalYear {
                starts_on,
                ends_on,
                legal_reserve,
                dividends,
                carry_back,
                today: Some(today.unwrap_or_else(today_local)),
                non_deductible_expenses: non_deductible,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome_as(&outcome, json, |id| {
                format!(
                    "exercice clos en projet ({id}) — `freeflow year show {}`",
                    ends_on.year()
                )
            })
        }
        YearCommand::List => {
            let years = list_fiscal_years(store.connection())?;
            if json {
                format_json(&years.iter().map(year_json).collect::<Vec<_>>())
            } else {
                let rows: Vec<Vec<String>> = years
                    .iter()
                    .map(|y| {
                        vec![
                            format_date(y.ends_on),
                            y.net_result.to_string(),
                            y.legal_reserve.to_string(),
                            y.dividends.to_string(),
                            y.retained_earnings.to_string(),
                            y.approved_on.map_or_else(
                                || "projet".to_string(),
                                |d| format!("approuvé le {}", format_date(d)),
                            ),
                        ]
                    })
                    .collect();
                table::render(
                    &[
                        "Clôture",
                        "Résultat net",
                        "Réserve",
                        "Dividendes",
                        "Report",
                        "Statut",
                    ],
                    &rows,
                )
            }
        }
        YearCommand::Show { period } => {
            let record = require_year(store, period)?;
            if json {
                format_json(&year_json(&record))
            } else {
                let status = record.approved_on.map_or_else(
                    || "projet (non approuvé)".to_string(),
                    |d| format!("approuvé le {}", format_date(d)),
                );
                format!(
                    "Exercice du {} au {} — {status}\n\
                     CA HT : {}\nCharges externes : {}\nRémunération dirigeant : {}\n\
                     Dotations aux amortissements : {}\n\
                     Résultat avant IS : {}\nDéficits antérieurs imputés : {}\n\
                     Résultat fiscal : {}\nIS : {}\nDéficit reporté en arrière : {}\n\
                     Créance de report en arrière : {}\nRésultat net : {}\n\
                     Réserve légale : {}\nDividendes : {}\nReport à nouveau : {}\n\
                     Déficits reportables en avant : {}\n(id {}, révision {})",
                    format_date(record.starts_on),
                    format_date(record.ends_on),
                    record.revenue_ht,
                    record.expenses,
                    record.director_remuneration,
                    record.depreciation,
                    record.result_before_tax,
                    record.losses_imputed,
                    record.taxable_result(),
                    record.corporate_tax,
                    record.carried_back,
                    record.carry_back_credit,
                    record.net_result,
                    record.legal_reserve,
                    record.dividends,
                    record.retained_earnings,
                    record.losses_carried_forward,
                    record.id,
                    record.revision,
                )
            }
        }
        YearCommand::Amend {
            period,
            legal_reserve,
            dividends,
        } => {
            // Lire-modifier-écrire dans la même invocation, comme `client edit` (lot 15) : la
            // commande envoyée au cœur porte toujours l'affectation complète.
            let record = require_year(store, period)?;
            let command = UpdateFiscalYearAppropriation {
                id: record.id,
                revision: record.revision,
                legal_reserve: legal_reserve.unwrap_or(record.legal_reserve),
                dividends: dividends.unwrap_or(record.dividends),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        YearCommand::Approve {
            period,
            approved_on,
            today,
        } => {
            let record = require_year(store, period)?;
            let command = ApproveFiscalYear {
                id: record.id,
                revision: record.revision,
                approved_on,
                today: Some(today.unwrap_or_else(today_local)),
            };
            // Sauvegarde préalable obligatoire (lot 36) : l'approbation rend l'exercice
            // immuable, une sauvegarde est le seul retour en arrière possible. Son échec
            // abandonne l'approbation ; en dry-run ou pour un agent (action en attente), rien
            // n'est encore approuvé — la sauvegarde se fait alors à la confirmation.
            let backup = if ctx.dry_run || !matches!(ctx.actor, Actor::Human) {
                None
            } else {
                Some(pre_approve_backup(store, period)?)
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            let rendered = format_outcome(&outcome, json);
            match backup {
                Some(b) => with_backup_note(rendered, &b, json),
                None => rendered,
            }
        }
        YearCommand::Opening(cmd) => return run_opening(cmd, store, ctx, json),
        YearCommand::Balance { period } => {
            let (_, ledger) = ledger_ending_in(store.connection(), period)?;
            let (balance, sheet) = (ledger.trial_balance(), ledger.balance_sheet());
            if json {
                format_json(&balance_json(&balance, &sheet))
            } else {
                balance_human(&balance, &sheet)
            }
        }
        YearCommand::Checklist { period, today } => {
            let today = today.unwrap_or_else(today_local);
            let checklist = closing_checklist(store.connection(), period, today)?;
            if json {
                format_json(&checklist_json(&checklist))
            } else {
                checklist_human(&checklist)
            }
        }
        YearCommand::Glossary => {
            if json {
                format_json(&glossary_json())
            } else {
                glossary_human()
            }
        }
        YearCommand::Rm { period } => {
            let record = require_year(store, period)?;
            let command = DeleteFiscalYear {
                id: record.id,
                revision: record.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome_as(&outcome, json, |()| {
                "projet de clôture supprimé".to_string()
            })
        }
        YearCommand::Render {
            period,
            doc,
            out,
            today,
        } => render(store, period, doc, &out, today)?,
    };
    Ok(output)
}
