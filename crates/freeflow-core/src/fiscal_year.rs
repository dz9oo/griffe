//! Clôture d'exercice (lot 20) : la table `fiscal_years` posée par la migration 0010 (lot 17)
//! trouve ici ses commandes et requêtes. La **décision d'affectation** du résultat (réserve
//! légale, dividendes, report à nouveau) est un acte juridique daté qui se cumule d'un exercice
//! sur l'autre — ce module fige un *snapshot* du résultat calculé par [`crate::accounting`] au
//! moment de la clôture, jamais recalculé après coup.
//!
//! Régime de mutabilité (doctrine du lot 15, troisième cas) : un exercice **approuvé**
//! (`approved_on` non nul) est immuable — triggers SQL de la migration 0011, comme une facture ;
//! un **projet** non approuvé reste éditable ([`UpdateFiscalYearAppropriation`]) et supprimable
//! ([`DeleteFiscalYear`]), sous révision optimiste. Un exercice suivi d'un exercice plus récent
//! n'est plus ni éditable ni supprimable même en projet : son report à nouveau a déjà été copié
//! dans la chaîne du suivant.
//!
//! **Déficits fiscaux (lot 32).** À côté du report à nouveau *comptable*, la chaîne porte le
//! stock de déficits *fiscaux* reportables en avant (art. 209 I CGI) : le bilan d'ouverture en
//! est le maillon zéro (`OpeningBalance::tax_losses`), chaque exercice clos y ajoute son déficit
//! et y prélève ce qu'il impute sur son bénéfice ([`crate::accounting::impute_prior_losses`]).
//! Ce stock n'est pas stocké : il se dérive des faits figés dans chaque snapshot
//! (`losses_imputed`, `carried_back`), si bien qu'un exercice clos avant ce lot — où rien
//! n'était imputé — garde exactement la lecture qu'il avait. L'option de **report en arrière**
//! (art. 220 quinquies) est une décision de clôture ([`CloseFiscalYear::carry_back`]) : le
//! déficit s'impute sur le bénéfice fiscal de l'exercice précédent clos ici, et la créance d'IS
//! qui en naît entre dans le résultat net (produit 699).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::accounting::{AccountingResult, CarryBackBase, carry_back, compute_result_with_losses};
use crate::app::{AppError, Command};
use crate::company::{CompanyProfile, company_profile};
use crate::domain::{FiscalYear, FiscalYearId, Money, format_date, parse_date};

/// Part du capital social que la réserve légale cumulée ne peut pas dépasser (10 %,
/// art. L232-10 du Code de commerce), en dix-millièmes.
const LEGAL_RESERVE_CAP_BPS: u32 = 1_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FiscalYearError {
    #[error("exercice introuvable : {0}")]
    NotFound(FiscalYearId),

    #[error(
        "aucun profil d'entreprise défini : `freeflow company set-profile` d'abord (le résultat \
         et la réserve légale en dépendent)"
    )]
    ProfileMissing,

    #[error("période invalide : {starts_on} n'est pas antérieur à {ends_on}")]
    InvalidPeriod { starts_on: String, ends_on: String },

    #[error("la période chevauche un exercice déjà clos ({0})")]
    Overlaps(String),

    #[error("montant d'affectation négatif : {0}")]
    NegativeAmount(&'static str),

    #[error(
        "affectation impossible : {requested} demandés pour {distributable} distribuables \
         (résultat net + report à nouveau antérieur)"
    )]
    ExceedsDistributable {
        requested: String,
        distributable: String,
    },

    #[error(
        "la réserve légale cumulée ({cumulated}) dépasserait 10 % du capital social ({cap}) — \
         plafond de l'art. L232-10 du Code de commerce"
    )]
    ReserveExceedsCap { cumulated: String, cap: String },

    #[error("l'exercice {0} est déjà approuvé : la décision d'AG fait foi, il est immuable")]
    AlreadyApproved(FiscalYearId),

    #[error(
        "l'exercice {0} est suivi d'un exercice plus récent qui a hérité de son report à \
         nouveau — corriger la chaîne se fait du plus récent au plus ancien"
    )]
    HasSuccessor(FiscalYearId),

    #[error("la date d'AG ({approved_on}) ne peut pas précéder la clôture ({ends_on})")]
    ApprovalBeforeEnd {
        approved_on: String,
        ends_on: String,
    },

    #[error(
        "le bilan d'ouverture est daté du {opens_on} : le premier exercice clos dans \
         l'application doit commencer ce jour-là, pas le {starts_on} (corrigez la période, ou le \
         bilan d'ouverture)"
    )]
    OpeningBalanceMismatch { opens_on: String, starts_on: String },

    #[error(
        "report en arrière impossible : l'exercice ne dégage aucun déficit fiscal (résultat \
         fiscal {0})"
    )]
    CarryBackWithoutDeficit(String),

    #[error(
        "report en arrière impossible : le déficit ne s'impute que sur le bénéfice de l'exercice \
         précédent (clos la veille du {0}), qui doit être clos dans l'application"
    )]
    CarryBackWithoutPriorYear(String),

    #[error(
        "report en arrière impossible : l'exercice précédent ({period}) n'offre aucun bénéfice \
         d'imputation (résultat fiscal {taxable}, distributions {distributed})"
    )]
    CarryBackNoImputableProfit {
        period: String,
        taxable: String,
        distributed: String,
    },
}

impl From<FiscalYearError> for AppError {
    fn from(e: FiscalYearError) -> Self {
        Self::Domain(e.to_string())
    }
}

/// Un exercice clos tel qu'en base : le snapshot du résultat figé à la clôture, la décision
/// d'affectation, et son état d'approbation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FiscalYearRecord {
    pub id: FiscalYearId,
    pub starts_on: Date,
    pub ends_on: Date,
    pub revenue_ht: Money,
    pub expenses: Money,
    pub director_remuneration: Money,
    /// Résultat comptable avant impôt.
    pub result_before_tax: Money,
    /// Déficits antérieurs imputés sur le bénéfice de l'exercice (ligne 360 du 2033-B).
    pub losses_imputed: Money,
    /// IS sur le résultat fiscal (`result_before_tax − losses_imputed`).
    pub corporate_tax: Money,
    /// Déficit de l'exercice reporté en arrière (ligne 356 du 2033-B), nul sans option.
    pub carried_back: Money,
    /// Créance d'IS née du report en arrière (2039-SD), produit de l'exercice.
    pub carry_back_credit: Money,
    /// `result_before_tax − corporate_tax + carry_back_credit`.
    pub net_result: Money,
    pub legal_reserve: Money,
    pub dividends: Money,
    /// Report à nouveau cumulé après cette affectation (peut être négatif).
    pub retained_earnings: Money,
    /// Déficits fiscaux reportables en avant **après** cet exercice (case 870 du 2033-D) —
    /// dérivés de la chaîne à la lecture, jamais stockés.
    pub losses_carried_forward: Money,
    /// Date de l'AG d'approbation — `None` tant que l'exercice est un projet éditable.
    pub approved_on: Option<Date>,
    pub revision: i64,
    pub created_at: OffsetDateTime,
}

impl FiscalYearRecord {
    #[must_use]
    pub const fn period(&self) -> FiscalYear {
        FiscalYear::new(self.starts_on, self.ends_on)
    }

    #[must_use]
    pub const fn is_approved(&self) -> bool {
        self.approved_on.is_some()
    }

    /// Résultat fiscal : le résultat comptable avant IS diminué des déficits antérieurs imputés
    /// (négatif = déficit fiscal de l'exercice).
    #[must_use]
    pub fn taxable_result(&self) -> Money {
        self.result_before_tax - self.losses_imputed
    }

    /// Déficit fiscal de l'exercice (positif), ou zéro sur un bénéfice.
    #[must_use]
    pub fn deficit(&self) -> Money {
        (-self.taxable_result()).max(Money::ZERO)
    }

    /// Déficits antérieurs reportables à l'ouverture de l'exercice (case 982 du 2033-D) — la
    /// chaîne lue à l'envers depuis le stock après l'exercice.
    #[must_use]
    pub fn losses_available_before(&self) -> Money {
        self.losses_carried_forward + self.losses_imputed - (self.deficit() - self.carried_back)
    }

    /// Le snapshot figé relu comme un [`AccountingResult`] — pour les documents, qui reflètent
    /// la photo prise à la clôture, pas un recalcul vivant.
    #[must_use]
    pub fn accounting_result(&self) -> AccountingResult {
        AccountingResult {
            period: self.period(),
            revenue_ht: self.revenue_ht,
            expenses: self.expenses,
            director_remuneration: self.director_remuneration,
            result_before_tax: self.result_before_tax,
            prior_losses_available: self.losses_available_before(),
            losses_imputed: self.losses_imputed,
            taxable_result: self.taxable_result(),
            corporate_tax: self.corporate_tax,
            carried_back: self.carried_back,
            carry_back_credit: self.carry_back_credit,
            net_result: self.net_result,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Validation d'affectation, partagée par la clôture et la révision d'un projet
// ---------------------------------------------------------------------------------------------

/// Vérifie une affectation (réserve légale, dividendes) contre le distribuable et le plafond de
/// réserve légale, et renvoie le report à nouveau résultant.
fn validate_appropriation(
    profile: &CompanyProfile,
    net_result: Money,
    prior_retained: Money,
    prior_reserve: Money,
    legal_reserve: Money,
    dividends: Money,
) -> Result<Money, AppError> {
    if legal_reserve.cents() < 0 {
        return Err(FiscalYearError::NegativeAmount("réserve légale").into());
    }
    if dividends.cents() < 0 {
        return Err(FiscalYearError::NegativeAmount("dividendes").into());
    }
    let distributable = net_result + prior_retained;
    let requested = legal_reserve + dividends;
    if requested > distributable.max(Money::ZERO) {
        return Err(FiscalYearError::ExceedsDistributable {
            requested: requested.to_string(),
            distributable: distributable.max(Money::ZERO).to_string(),
        }
        .into());
    }
    if let Some(capital) = profile.share_capital {
        let cap = capital.apply_rate_bps(LEGAL_RESERVE_CAP_BPS);
        let cumulated = prior_reserve + legal_reserve;
        if cumulated > cap {
            return Err(FiscalYearError::ReserveExceedsCap {
                cumulated: cumulated.to_string(),
                cap: cap.to_string(),
            }
            .into());
        }
    }
    Ok(prior_retained + net_result - legal_reserve - dividends)
}

/// Ce dont hérite l'exercice qui commence à une date donnée : la chaîne des exercices clos
/// avant lui, bilan d'ouverture compris.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PriorChain {
    /// Report à nouveau comptable.
    retained: Money,
    /// Réserve légale cumulée (dotations en base + réserve reprise).
    reserve: Money,
    /// Déficits fiscaux encore reportables en avant.
    tax_losses: Money,
}

/// Contribution des exercices clos **avant** `before` au stock de déficits reportables : chaque
/// déficit, moins ce qui en a été reporté en arrière, moins ce que les exercices bénéficiaires
/// ont imputé. Un exercice clos avant le lot 32 a ses deux colonnes à zéro : son déficit reste
/// intégralement reportable, comme il l'était.
fn losses_from_closed_years(conn: &Connection, before: Date) -> Result<Money, AppError> {
    let cents: i64 = conn.query_row(
        "SELECT coalesce(sum(max(0, -result_before_tax_cents) - carried_back_cents \
                             - losses_imputed_cents), 0)
         FROM fiscal_years WHERE ends_on < ?1",
        [format_date(before)],
        |row| row.get(0),
    )?;
    Ok(Money::from_cents(cents))
}

/// Report à nouveau, réserve légale et déficits reportables cumulés des exercices clos **avant**
/// `starts_on` — la chaîne dont hérite l'exercice qui commence à cette date.
///
/// Le bilan d'ouverture (lot 30) en est le maillon zéro : pour le **premier** exercice clos ici,
/// le report à nouveau est celui des capitaux propres repris (et il doit ouvrir ce jour-là —
/// une reprise datée d'un autre jour est une incohérence refusée, jamais ignorée en silence) ;
/// pour les suivants, le report vient du snapshot du précédent, qui l'a déjà intégré. La réserve
/// légale reprise et les déficits repris, eux, s'ajoutent toujours : aucun snapshot ne les
/// porte.
fn prior_chain(conn: &Connection, starts_on: Date) -> Result<PriorChain, AppError> {
    let retained: Option<i64> = conn
        .query_row(
            "SELECT retained_earnings_cents FROM fiscal_years WHERE ends_on < ?1
             ORDER BY ends_on DESC LIMIT 1",
            [format_date(starts_on)],
            |row| row.get(0),
        )
        .optional()?;
    let reserve: i64 = conn.query_row(
        "SELECT coalesce(sum(legal_reserve_cents), 0) FROM fiscal_years WHERE ends_on < ?1",
        [format_date(starts_on)],
        |row| row.get(0),
    )?;
    let opening = crate::opening_balance::opening_balance(conn)?
        .map(|r| (r.balance.opens_on, r.equity(), r.balance.tax_losses));
    let (retained, opening_reserve, opening_losses) = match (retained, opening) {
        (Some(from_chain), Some((_, equity, losses))) => {
            (Money::from_cents(from_chain), equity.legal_reserve, losses)
        }
        (Some(from_chain), None) => (Money::from_cents(from_chain), Money::ZERO, Money::ZERO),
        (None, Some((opens_on, equity, losses))) => {
            if opens_on != starts_on {
                return Err(FiscalYearError::OpeningBalanceMismatch {
                    opens_on: format_date(opens_on),
                    starts_on: format_date(starts_on),
                }
                .into());
            }
            (equity.retained_earnings, equity.legal_reserve, losses)
        }
        (None, None) => (Money::ZERO, Money::ZERO, Money::ZERO),
    };
    Ok(PriorChain {
        retained,
        reserve: Money::from_cents(reserve) + opening_reserve,
        tax_losses: losses_from_closed_years(conn, starts_on)? + opening_losses,
    })
}

/// Report à nouveau hérité par un exercice commençant à `starts_on` : celui du dernier exercice
/// clos avant cette date, ou zéro s'il n'y en a aucun.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn latest_retained_earnings(conn: &Connection, starts_on: Date) -> Result<Money, AppError> {
    prior_chain(conn, starts_on).map(|chain| chain.retained)
}

/// Déficits fiscaux reportables en avant à l'ouverture d'un exercice commençant à `starts_on`
/// — la lecture *tolérante* de la chaîne, pour un calcul indicatif (calendrier, grand livre
/// d'un exercice non clos) : un bilan d'ouverture daté d'un autre jour et sans exercice clos
/// entre les deux ne compte pas, là où [`CloseFiscalYear`] le refuse.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn tax_losses_available(conn: &Connection, starts_on: Date) -> Result<Money, AppError> {
    let from_years = losses_from_closed_years(conn, starts_on)?;
    let has_prior_year: bool = conn.query_row(
        "SELECT count(*) > 0 FROM fiscal_years WHERE ends_on < ?1",
        [format_date(starts_on)],
        |row| row.get(0),
    )?;
    let opening = crate::opening_balance::opening_balance(conn)?
        .filter(|r| has_prior_year || r.balance.opens_on == starts_on)
        .map_or(Money::ZERO, |r| r.balance.tax_losses);
    Ok(from_years + opening)
}

/// L'exercice clos la veille de `starts_on`, s'il l'est dans l'application — le seul sur
/// lequel un déficit se reporte en arrière (art. 220 quinquies I CGI : « bénéfice de l'exercice
/// précédent »).
fn previous_year(conn: &Connection, starts_on: Date) -> Result<Option<FiscalYearRecord>, AppError> {
    let Some(ends_on) = starts_on.previous_day() else {
        return Ok(None);
    };
    conn.query_row(
        &format!("{RECORD_SELECT} WHERE fy.ends_on = ?1"),
        [format_date(ends_on)],
        row_to_record,
    )
    .optional()
    .map_err(AppError::from)
}

// ---------------------------------------------------------------------------------------------
// Commandes
// ---------------------------------------------------------------------------------------------

/// Clôt un exercice : recalcule le résultat via [`crate::accounting::compute_result_with_losses`]
/// (déficits antérieurs de la chaîne imputés sur le bénéfice), le fige en snapshot, et
/// enregistre la décision d'affectation en **projet** (non approuvé). Le report à nouveau
/// résultant est `report antérieur + résultat net − réserve légale − dividendes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseFiscalYear {
    pub starts_on: Date,
    pub ends_on: Date,
    pub legal_reserve: Money,
    pub dividends: Money,
    /// Option de report en arrière du déficit de l'exercice (art. 220 quinquies CGI) sur le
    /// bénéfice de l'exercice précédent clos ici : la créance d'IS qui en naît entre dans le
    /// résultat net. Refusée sans déficit, sans exercice précédent, ou sans bénéfice
    /// d'imputation. Une décision de clôture : pour la changer, supprimer le projet et clore à
    /// nouveau. `false` par défaut (entrées d'audit antérieures comprises).
    #[serde(default)]
    pub carry_back: bool,
}

impl Command for CloseFiscalYear {
    type Output = FiscalYearId;
    const NAME: &'static str = "fiscal.close_year";

    /// Acte à portée juridique et financière : un agent MCP le propose, un humain le confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.starts_on >= self.ends_on {
            return Err(FiscalYearError::InvalidPeriod {
                starts_on: format_date(self.starts_on),
                ends_on: format_date(self.ends_on),
            }
            .into());
        }
        let profile = company_profile(conn)?.ok_or(FiscalYearError::ProfileMissing)?;

        let overlapping: Option<String> = conn
            .query_row(
                "SELECT starts_on || ' → ' || ends_on FROM fiscal_years
                 WHERE starts_on <= ?1 AND ends_on >= ?2 LIMIT 1",
                [format_date(self.ends_on), format_date(self.starts_on)],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(period) = overlapping {
            return Err(FiscalYearError::Overlaps(period).into());
        }

        let chain = prior_chain(conn, self.starts_on)?;
        let mut result = compute_result_with_losses(
            conn,
            FiscalYear::new(self.starts_on, self.ends_on),
            &profile,
            chain.tax_losses,
        )?;
        if self.carry_back {
            result = result.with_carry_back(self.carry_back_of(conn, &result)?);
        }
        let retained_earnings = validate_appropriation(
            &profile,
            result.net_result,
            chain.retained,
            chain.reserve,
            self.legal_reserve,
            self.dividends,
        )?;

        let id = FiscalYearId::new();
        conn.execute(
            "INSERT INTO fiscal_years
                (id, starts_on, ends_on, revenue_ht_cents, expenses_cents,
                 director_remuneration_cents, result_before_tax_cents, corporate_tax_cents,
                 net_result_cents, legal_reserve_cents, dividends_cents, retained_earnings_cents,
                 approved_on, revision, created_at, losses_imputed_cents, carried_back_cents,
                 carry_back_credit_cents)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, 1, ?13, ?14, ?15,
                     ?16)",
            params![
                id.to_string(),
                format_date(self.starts_on),
                format_date(self.ends_on),
                result.revenue_ht.cents(),
                result.expenses.cents(),
                result.director_remuneration.cents(),
                result.result_before_tax.cents(),
                result.corporate_tax.cents(),
                result.net_result.cents(),
                self.legal_reserve.cents(),
                self.dividends.cents(),
                retained_earnings.cents(),
                OffsetDateTime::now_utc().format(&Rfc3339)?,
                result.losses_imputed.cents(),
                result.carried_back.cents(),
                result.carry_back_credit.cents(),
            ],
        )?;
        Ok(id)
    }
}

impl CloseFiscalYear {
    /// Le report en arrière du déficit de `result` sur l'exercice précédent : ses trois refus
    /// (pas de déficit, pas d'exercice précédent clos la veille, pas de bénéfice d'imputation)
    /// sont des erreurs explicites, jamais un report silencieusement nul.
    fn carry_back_of(
        &self,
        conn: &Connection,
        result: &AccountingResult,
    ) -> Result<crate::accounting::CarryBack, AppError> {
        let deficit = result.deficit();
        if deficit.is_zero() {
            return Err(FiscalYearError::CarryBackWithoutDeficit(
                result.taxable_result.to_string(),
            )
            .into());
        }
        let previous = previous_year(conn, self.starts_on)?.ok_or_else(|| {
            FiscalYearError::CarryBackWithoutPriorYear(format_date(self.starts_on))
        })?;
        let base = CarryBackBase {
            taxable_profit: previous.taxable_result(),
            distributed: previous.dividends,
        };
        let carried = carry_back(deficit, base);
        if carried.imputed.is_zero() {
            return Err(FiscalYearError::CarryBackNoImputableProfit {
                period: format!(
                    "{} → {}",
                    format_date(previous.starts_on),
                    format_date(previous.ends_on)
                ),
                taxable: base.taxable_profit.to_string(),
                distributed: base.distributed.to_string(),
            }
            .into());
        }
        Ok(carried)
    }
}

/// Révise l'affectation d'un exercice encore en **projet** — le snapshot du résultat, lui, reste
/// figé (c'est la photo prise à la clôture, pas une valeur vivante). Refusée si l'exercice est
/// approuvé ou si un exercice plus récent a déjà hérité de son report à nouveau.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFiscalYearAppropriation {
    pub id: FiscalYearId,
    /// Révision lue avant modification — écriture concurrente ⇒ [`AppError::Conflict`].
    pub revision: i64,
    pub legal_reserve: Money,
    pub dividends: Money,
}

impl Command for UpdateFiscalYearAppropriation {
    /// La révision résultante, comme les autres `Update*` du dépôt.
    type Output = i64;
    const NAME: &'static str = "fiscal.update_year_appropriation";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let record = fiscal_year_by_id(conn, self.id)?.ok_or(FiscalYearError::NotFound(self.id))?;
        require_editable(conn, &record)?;
        let profile = company_profile(conn)?.ok_or(FiscalYearError::ProfileMissing)?;
        let chain = prior_chain(conn, record.starts_on)?;
        let retained_earnings = validate_appropriation(
            &profile,
            record.net_result,
            chain.retained,
            chain.reserve,
            self.legal_reserve,
            self.dividends,
        )?;

        let new_revision = require_fiscal_year_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE fiscal_years SET legal_reserve_cents = ?1, dividends_cents = ?2,
                retained_earnings_cents = ?3, revision = ?4
             WHERE id = ?5 AND revision = ?6",
            params![
                self.legal_reserve.cents(),
                self.dividends.cents(),
                retained_earnings.cents(),
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

/// Approuve un exercice : y appose la date de l'AG, après quoi la ligne devient immuable
/// (triggers de la migration 0011).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveFiscalYear {
    pub id: FiscalYearId,
    pub revision: i64,
    /// Date de l'AG d'approbation — au plus tôt le jour de la clôture.
    pub approved_on: Date,
}

impl Command for ApproveFiscalYear {
    type Output = i64;
    const NAME: &'static str = "fiscal.approve_year";

    /// Le geste qui scelle un acte juridique : jamais appliqué par un agent sans humain.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let record = fiscal_year_by_id(conn, self.id)?.ok_or(FiscalYearError::NotFound(self.id))?;
        if record.is_approved() {
            return Err(FiscalYearError::AlreadyApproved(self.id).into());
        }
        if self.approved_on < record.ends_on {
            return Err(FiscalYearError::ApprovalBeforeEnd {
                approved_on: format_date(self.approved_on),
                ends_on: format_date(record.ends_on),
            }
            .into());
        }
        let new_revision = require_fiscal_year_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE fiscal_years SET approved_on = ?1, revision = ?2
             WHERE id = ?3 AND revision = ?4",
            params![
                format_date(self.approved_on),
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

/// Supprime un exercice encore en **projet** (clos par erreur, par exemple). Un exercice
/// approuvé est immuable ; un projet suivi d'un exercice plus récent ne se supprime pas non
/// plus, sa chaîne de report ayant déjà été copiée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFiscalYear {
    pub id: FiscalYearId,
    pub revision: i64,
}

impl Command for DeleteFiscalYear {
    type Output = ();
    const NAME: &'static str = "fiscal.delete_year";

    /// Destructeur réel, cohérent avec les autres `Delete*` : un agent propose, un humain
    /// confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let record = fiscal_year_by_id(conn, self.id)?.ok_or(FiscalYearError::NotFound(self.id))?;
        require_editable(conn, &record)?;
        require_fiscal_year_revision(conn, self.id, self.revision)?;
        conn.execute(
            "DELETE FROM fiscal_years WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
    }
}

/// Un exercice n'est éditable/supprimable que non approuvé **et** sans successeur — voir le
/// commentaire de module.
fn require_editable(conn: &Connection, record: &FiscalYearRecord) -> Result<(), AppError> {
    if record.is_approved() {
        return Err(FiscalYearError::AlreadyApproved(record.id).into());
    }
    let successors: i64 = conn.query_row(
        "SELECT count(*) FROM fiscal_years WHERE ends_on > ?1",
        [format_date(record.ends_on)],
        |row| row.get(0),
    )?;
    if successors > 0 {
        return Err(FiscalYearError::HasSuccessor(record.id).into());
    }
    Ok(())
}

fn require_fiscal_year_revision(
    conn: &Connection,
    id: FiscalYearId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "fiscal_years", &id_str)?
        .ok_or(FiscalYearError::NotFound(id))?;
    crate::app::revision::require_revision(current, expected, "exercice", &id_str)
}

// ---------------------------------------------------------------------------------------------
// Requêtes
// ---------------------------------------------------------------------------------------------

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

/// La projection d'un exercice : ses colonnes, plus le stock de déficits reportables **après**
/// lui, dérivé de la chaîne (déficits de tous les exercices clos jusqu'à lui, moins ce qui en a
/// été reporté en arrière ou imputé, plus les déficits repris au bilan d'ouverture — qui ouvre
/// nécessairement la chaîne dès qu'un exercice existe, voir [`prior_chain`]).
const RECORD_SELECT: &str = "SELECT fy.*,
    (SELECT coalesce(sum(max(0, -p.result_before_tax_cents) - p.carried_back_cents \
                         - p.losses_imputed_cents), 0)
       FROM fiscal_years p WHERE p.ends_on <= fy.ends_on)
    + coalesce((SELECT tax_losses_cents FROM opening_balance WHERE id = 1), 0)
    AS losses_carried_forward_cents
 FROM fiscal_years fy";

fn row_to_record(row: &Row) -> rusqlite::Result<FiscalYearRecord> {
    let id: String = row.get("id")?;
    let starts_on: String = row.get("starts_on")?;
    let ends_on: String = row.get("ends_on")?;
    let approved_on: Option<String> = row.get("approved_on")?;
    let created_at: String = row.get("created_at")?;
    Ok(FiscalYearRecord {
        id: id.parse().map_err(conv_err)?,
        starts_on: parse_date(&starts_on).map_err(conv_err)?,
        ends_on: parse_date(&ends_on).map_err(conv_err)?,
        revenue_ht: Money::from_cents(row.get("revenue_ht_cents")?),
        expenses: Money::from_cents(row.get("expenses_cents")?),
        director_remuneration: Money::from_cents(row.get("director_remuneration_cents")?),
        result_before_tax: Money::from_cents(row.get("result_before_tax_cents")?),
        losses_imputed: Money::from_cents(row.get("losses_imputed_cents")?),
        corporate_tax: Money::from_cents(row.get("corporate_tax_cents")?),
        carried_back: Money::from_cents(row.get("carried_back_cents")?),
        carry_back_credit: Money::from_cents(row.get("carry_back_credit_cents")?),
        net_result: Money::from_cents(row.get("net_result_cents")?),
        legal_reserve: Money::from_cents(row.get("legal_reserve_cents")?),
        dividends: Money::from_cents(row.get("dividends_cents")?),
        retained_earnings: Money::from_cents(row.get("retained_earnings_cents")?),
        losses_carried_forward: Money::from_cents(row.get("losses_carried_forward_cents")?),
        approved_on: approved_on
            .map(|s| parse_date(&s))
            .transpose()
            .map_err(conv_err)?,
        revision: row.get("revision")?,
        created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(conv_err)?,
    })
}

/// # Errors
pub fn fiscal_year_by_id(
    conn: &Connection,
    id: FiscalYearId,
) -> Result<Option<FiscalYearRecord>, AppError> {
    conn.query_row(
        &format!("{RECORD_SELECT} WHERE fy.id = ?1"),
        [id.to_string()],
        row_to_record,
    )
    .optional()
    .map_err(AppError::from)
}

/// # Errors
pub fn fiscal_year_by_period(
    conn: &Connection,
    period: FiscalYear,
) -> Result<Option<FiscalYearRecord>, AppError> {
    conn.query_row(
        &format!("{RECORD_SELECT} WHERE fy.starts_on = ?1 AND fy.ends_on = ?2"),
        [format_date(period.start()), format_date(period.end())],
        row_to_record,
    )
    .optional()
    .map_err(AppError::from)
}

/// L'exercice clos dont la clôture tombe dans l'année civile `year` — la façon dont CLI, MCP et
/// GUI désignent un exercice (`--period 2025`), unique par construction (deux exercices ne se
/// chevauchent pas et durent ≥ 1 jour... un seul peut se terminer dans une année donnée pour
/// une clôture annuelle récurrente ; en cas d'exercices multiples la requête prend le dernier).
///
/// # Errors
pub fn fiscal_year_ending_in(
    conn: &Connection,
    year: i32,
) -> Result<Option<FiscalYearRecord>, AppError> {
    conn.query_row(
        &format!(
            "{RECORD_SELECT} WHERE fy.ends_on >= ?1 AND fy.ends_on <= ?2
             ORDER BY fy.ends_on DESC LIMIT 1"
        ),
        [format!("{year:04}-01-01"), format!("{year:04}-12-31")],
        row_to_record,
    )
    .optional()
    .map_err(AppError::from)
}

/// Tous les exercices clos, du plus ancien au plus récent.
///
/// # Errors
pub fn list_fiscal_years(conn: &Connection) -> Result<Vec<FiscalYearRecord>, AppError> {
    let mut stmt = conn.prepare(&format!("{RECORD_SELECT} ORDER BY fy.starts_on ASC"))?;
    let rows = stmt.query_map([], row_to_record)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::EmitInvoice;
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ClientId, ExpenseCategory, FiscalYearEnd, InvoiceLine, Siren, VatRate, VatRegime,
    };
    use crate::expenses::RecordExpense;
    use crate::store::{Passphrase, Store};
    use time::Month as TimeMonth;

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-fiscal-year-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn date(y: i32, m: TimeMonth, d: u8) -> Date {
        Date::from_calendar_date(y, m, d).unwrap()
    }

    fn set_profile(store: &mut Store, capital_cents: Option<i64>) {
        let cmd = SetCompanyProfile {
            name: "Argon Digital".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("552100554").unwrap(),
            vat_number: None,
            address: Address {
                street: "12 rue de la Paix".to_string(),
                postal_code: "75002".to_string(),
                city: "Paris".to_string(),
                country: "FR".to_string(),
            },
            share_capital: capital_cents.map(Money::from_cents),
            rcs_city: Some("Paris".to_string()),
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: Some(VatRegime::RealNormalMonthly),
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
        };
        Executor::new(store).execute(&cmd, &human()).unwrap();
    }

    /// Une facture de 6 175 € HT en septembre `year` et une dépense nette de 800 € en octobre :
    /// résultat avant IS 5 375 €, IS 806,25 €, net 4 568,75 € (mêmes chiffres que le test
    /// d'intégration de `accounting.rs`).
    fn seed_activity(store: &mut Store, year: i32) {
        let client_id = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) \
                 VALUES (?1, 'Argon Digital', '2026-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        Executor::new(store)
            .execute(
                &EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity: 9.5,
                        unit_price: Money::from_cents(65_000),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on: date(year, TimeMonth::September, 30),
                    payment_terms_days: 30,
                },
                &human(),
            )
            .unwrap();
        Executor::new(store)
            .execute(
                &RecordExpense {
                    label: "Matériel".to_string(),
                    category: ExpenseCategory::Equipment,
                    amount: Money::from_cents(100_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(20_000),
                    incurred_on: date(year, TimeMonth::October, 5),
                    receipt_hash: None,
                    receipt_filename: None,
                    bank_transaction_id: None,
                },
                &human(),
            )
            .unwrap();
    }

    fn close_2026(store: &mut Store, legal_reserve: i64, dividends: i64) -> FiscalYearId {
        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::from_cents(legal_reserve),
            dividends: Money::from_cents(dividends),
            carry_back: false,
        };
        let Outcome::Applied(id) = Executor::new(store).execute(&cmd, &human()).unwrap() else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn closing_a_year_freezes_the_computed_result_and_chains_retained_earnings() {
        let mut store = test_store("close");
        set_profile(&mut store, Some(100_000));
        seed_activity(&mut store, 2026);

        // Réserve légale 50 €, dividendes 1 000 €.
        let id = close_2026(&mut store, 5_000, 100_000);

        let record = fiscal_year_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(record.revenue_ht, Money::from_cents(617_500));
        assert_eq!(record.expenses, Money::from_cents(80_000));
        assert_eq!(record.result_before_tax, Money::from_cents(537_500));
        assert_eq!(record.corporate_tax, Money::from_cents(80_625));
        assert_eq!(record.net_result, Money::from_cents(456_875));
        // Report à nouveau : 4 568,75 − 50 − 1 000 = 3 518,75 €.
        assert_eq!(record.retained_earnings, Money::from_cents(351_875));
        assert_eq!(record.approved_on, None);
        assert_eq!(record.revision, 1);

        // L'exercice suivant hérite du report.
        assert_eq!(
            latest_retained_earnings(store.connection(), date(2027, TimeMonth::January, 1))
                .unwrap(),
            Money::from_cents(351_875)
        );
        // Et la liste le contient.
        assert_eq!(list_fiscal_years(store.connection()).unwrap().len(), 1);
        assert_eq!(
            fiscal_year_ending_in(store.connection(), 2026)
                .unwrap()
                .unwrap()
                .id,
            id
        );
    }

    #[test]
    fn closing_requires_a_company_profile() {
        let mut store = test_store("no-profile");
        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: false,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("profil d'entreprise")));
    }

    #[test]
    fn closing_an_overlapping_period_is_refused() {
        let mut store = test_store("overlap");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        close_2026(&mut store, 0, 0);

        let overlapping = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::July, 1),
            ends_on: date(2027, TimeMonth::June, 30),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: false,
        };
        let err = Executor::new(&mut store)
            .execute(&overlapping, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("chevauche")));
    }

    #[test]
    fn dividends_beyond_the_distributable_are_refused() {
        let mut store = test_store("distributable");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);

        // Net 4 568,75 € : demander 5 000 € de dividendes dépasse le distribuable.
        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::from_cents(500_000),
            carry_back: false,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("distribuables")));
    }

    #[test]
    fn legal_reserve_beyond_ten_percent_of_capital_is_refused() {
        let mut store = test_store("reserve-cap");
        // Capital 1 000 € → plafond de réserve légale 100 €.
        set_profile(&mut store, Some(100_000));
        seed_activity(&mut store, 2026);

        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::from_cents(20_000),
            dividends: Money::ZERO,
            carry_back: false,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("réserve légale cumulée")));
    }

    #[test]
    fn a_draft_can_be_amended_and_the_retained_earnings_follow() {
        let mut store = test_store("amend");
        set_profile(&mut store, Some(100_000));
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        let Outcome::Applied(new_revision) = Executor::new(&mut store)
            .execute(
                &UpdateFiscalYearAppropriation {
                    id,
                    revision: 1,
                    legal_reserve: Money::from_cents(10_000),
                    dividends: Money::from_cents(200_000),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(new_revision, 2);

        let record = fiscal_year_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(record.legal_reserve, Money::from_cents(10_000));
        assert_eq!(record.dividends, Money::from_cents(200_000));
        // 4 568,75 − 100 − 2 000 = 2 468,75 €.
        assert_eq!(record.retained_earnings, Money::from_cents(246_875));
        // Le snapshot, lui, n'a pas bougé.
        assert_eq!(record.net_result, Money::from_cents(456_875));
    }

    #[test]
    fn amending_with_a_stale_revision_is_a_conflict() {
        let mut store = test_store("amend-stale");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        let err = Executor::new(&mut store)
            .execute(
                &UpdateFiscalYearAppropriation {
                    id,
                    revision: 99,
                    legal_reserve: Money::ZERO,
                    dividends: Money::ZERO,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::Conflict {
                entity: "exercice",
                ..
            }
        ));
    }

    #[test]
    fn an_approved_year_is_immutable_even_by_direct_sql() {
        let mut store = test_store("approved-immutable");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        Executor::new(&mut store)
            .execute(
                &ApproveFiscalYear {
                    id,
                    revision: 1,
                    approved_on: date(2027, TimeMonth::May, 15),
                },
                &human(),
            )
            .unwrap();
        let record = fiscal_year_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(record.approved_on, Some(date(2027, TimeMonth::May, 15)));
        assert_eq!(record.revision, 2);

        // La commande d'édition refuse...
        let err = Executor::new(&mut store)
            .execute(
                &UpdateFiscalYearAppropriation {
                    id,
                    revision: 2,
                    legal_reserve: Money::ZERO,
                    dividends: Money::ZERO,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà approuvé")));

        // ... la suppression aussi...
        let err = Executor::new(&mut store)
            .execute(&DeleteFiscalYear { id, revision: 2 }, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà approuvé")));

        // ... et une altération SQL directe échoue sur le trigger — vrai régime de
        // contre-écriture, pas une convention de commande.
        let err = store
            .connection()
            .execute(
                "UPDATE fiscal_years SET dividends_cents = 0 WHERE id = ?1",
                [id.to_string()],
            )
            .unwrap_err();
        assert!(err.to_string().contains("immuable"));
        let err = store
            .connection()
            .execute("DELETE FROM fiscal_years WHERE id = ?1", [id.to_string()])
            .unwrap_err();
        assert!(err.to_string().contains("immuable"));
    }

    #[test]
    fn approving_before_the_period_ends_is_refused() {
        let mut store = test_store("approve-early");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        let err = Executor::new(&mut store)
            .execute(
                &ApproveFiscalYear {
                    id,
                    revision: 1,
                    approved_on: date(2026, TimeMonth::June, 1),
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("précéder la clôture")));
    }

    #[test]
    fn a_year_with_a_successor_is_neither_editable_nor_deletable() {
        let mut store = test_store("successor");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id_2026 = close_2026(&mut store, 0, 0);

        let cmd = CloseFiscalYear {
            starts_on: date(2027, TimeMonth::January, 1),
            ends_on: date(2027, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: false,
        };
        Executor::new(&mut store).execute(&cmd, &human()).unwrap();

        let err = Executor::new(&mut store)
            .execute(
                &UpdateFiscalYearAppropriation {
                    id: id_2026,
                    revision: 1,
                    legal_reserve: Money::ZERO,
                    dividends: Money::ZERO,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("hérité")));

        let err = Executor::new(&mut store)
            .execute(
                &DeleteFiscalYear {
                    id: id_2026,
                    revision: 1,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("hérité")));
    }

    #[test]
    fn deleting_a_draft_removes_it() {
        let mut store = test_store("delete-draft");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        Executor::new(&mut store)
            .execute(&DeleteFiscalYear { id, revision: 1 }, &human())
            .unwrap();
        assert_eq!(fiscal_year_by_id(store.connection(), id).unwrap(), None);
    }

    #[test]
    fn closing_as_an_agent_requires_human_confirmation() {
        let mut store = test_store("agent-confirm");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);

        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: false,
        };
        let agent = ExecutionContext::new(
            Actor::Agent {
                session: "test".to_string(),
            },
            false,
        );
        let outcome = Executor::new(&mut store).execute(&cmd, &agent).unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert!(
            list_fiscal_years(store.connection()).unwrap().is_empty(),
            "rien n'est écrit tant qu'un humain n'a pas confirmé"
        );
    }

    fn record_opening(store: &mut Store, opens_on: Date, specs: &[&str]) {
        let cmd = crate::opening_balance::RecordOpeningBalance {
            opens_on,
            source: Some("bilan repris".to_string()),
            lines: specs.iter().map(|s| s.parse().unwrap()).collect(),
            tax_losses: Money::ZERO,
        };
        Executor::new(store).execute(&cmd, &human()).unwrap();
    }

    #[test]
    fn the_first_closed_year_inherits_the_opening_balance_equity() {
        let mut store = test_store("opening-chain");
        // Capital 1 000 € → plafond de réserve légale 100 €, dont 60 € déjà constitués.
        set_profile(&mut store, Some(100_000));
        record_opening(
            &mut store,
            date(2026, TimeMonth::January, 1),
            &[
                "101000:Capital social:C:1000.00",
                "106100:Réserve légale:C:60.00",
                "110000:Report à nouveau:C:250.00",
                "512000:Banque:D:1310.00",
            ],
        );
        seed_activity(&mut store, 2026);

        // 50 € de réserve dépasseraient le plafond une fois les 60 € repris comptés.
        let too_much = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::from_cents(5_000),
            dividends: Money::ZERO,
            carry_back: false,
        };
        let err = Executor::new(&mut store)
            .execute(&too_much, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("réserve légale cumulée")));

        // 40 € passent ; report = 250 (repris) + 4 568,75 − 40 = 4 778,75 €.
        let id = close_2026(&mut store, 4_000, 0);
        let record = fiscal_year_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(record.retained_earnings, Money::from_cents(477_875));

        // Le bilan d'ouverture est désormais figé par ce snapshot.
        let delete = crate::opening_balance::DeleteOpeningBalance { revision: 1 };
        let err = Executor::new(&mut store)
            .execute(&delete, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà clos")));

        // Et l'exercice suivant enchaîne sur le snapshot, en comptant toujours la réserve reprise
        // (60 + 40 = 100 € : plus un centime de dotation possible).
        seed_activity(&mut store, 2027);
        let next = CloseFiscalYear {
            starts_on: date(2027, TimeMonth::January, 1),
            ends_on: date(2027, TimeMonth::December, 31),
            legal_reserve: Money::from_cents(1),
            dividends: Money::ZERO,
            carry_back: false,
        };
        let err = Executor::new(&mut store)
            .execute(&next, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("réserve légale cumulée")));
    }

    #[test]
    fn a_first_year_not_starting_on_the_opening_date_is_refused() {
        let mut store = test_store("opening-mismatch");
        set_profile(&mut store, None);
        record_opening(
            &mut store,
            date(2025, TimeMonth::October, 1),
            &["101000:Capital:C:10.00", "512000:Banque:D:10.00"],
        );
        seed_activity(&mut store, 2026);
        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: false,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("2025-10-01")));
    }

    #[test]
    fn a_loss_year_carries_a_negative_retained_earnings_and_allows_no_appropriation() {
        let mut store = test_store("loss");
        set_profile(&mut store, Some(100_000));
        record_opening(
            &mut store,
            date(2026, TimeMonth::January, 1),
            &[
                "101000:Capital:C:1000.00",
                "110000:Report à nouveau:C:300.00",
                "512000:Banque:D:1300.00",
            ],
        );
        // Une seule dépense, aucune facture : perte de 800 € nets.
        Executor::new(&mut store)
            .execute(
                &RecordExpense {
                    label: "Honoraires".to_string(),
                    category: ExpenseCategory::Professional,
                    amount: Money::from_cents(96_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(16_000),
                    incurred_on: date(2026, TimeMonth::March, 5),
                    receipt_hash: None,
                    receipt_filename: None,
                    bank_transaction_id: None,
                },
                &human(),
            )
            .unwrap();

        let with_dividends = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::from_cents(100),
            carry_back: false,
        };
        let err = Executor::new(&mut store)
            .execute(&with_dividends, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("distribuables")));

        let id = close_2026(&mut store, 0, 0);
        let record = fiscal_year_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(record.revenue_ht, Money::ZERO);
        assert_eq!(record.corporate_tax, Money::ZERO);
        assert_eq!(record.net_result, Money::from_cents(-80_000));
        // 300 € repris − 800 € de perte : report à nouveau débiteur de 500 €.
        assert_eq!(record.retained_earnings, Money::from_cents(-50_000));
        // Et fiscalement, 800 € de déficit reportable en avant (lot 32).
        assert_eq!(record.taxable_result(), Money::from_cents(-80_000));
        assert_eq!(record.deficit(), Money::from_cents(80_000));
        assert_eq!(record.losses_imputed, Money::ZERO);
        assert_eq!(record.losses_carried_forward, Money::from_cents(80_000));
        assert_eq!(record.losses_available_before(), Money::ZERO);
    }

    // --- Déficits fiscaux : report en avant et report en arrière (lot 32). ---

    /// Une seule dépense nette de 800 € en mars `year`, aucune facture : déficit de 800 €.
    fn seed_loss(store: &mut Store, year: i32) {
        Executor::new(store)
            .execute(
                &RecordExpense {
                    label: "Honoraires".to_string(),
                    category: ExpenseCategory::Professional,
                    amount: Money::from_cents(96_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(16_000),
                    incurred_on: date(year, TimeMonth::March, 5),
                    receipt_hash: None,
                    receipt_filename: None,
                    bank_transaction_id: None,
                },
                &human(),
            )
            .unwrap();
    }

    fn close_year(store: &mut Store, year: i32, carry_back: bool) -> FiscalYearRecord {
        let cmd = CloseFiscalYear {
            starts_on: date(year, TimeMonth::January, 1),
            ends_on: date(year, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back,
        };
        let Outcome::Applied(id) = Executor::new(store).execute(&cmd, &human()).unwrap() else {
            panic!("expected Applied")
        };
        fiscal_year_by_id(store.connection(), id).unwrap().unwrap()
    }

    #[test]
    fn a_prior_deficit_is_imputed_on_the_next_profit_and_lowers_the_is() {
        let mut store = test_store("carry-forward");
        set_profile(&mut store, Some(100_000));
        // 2026 : déficit de 800 €, aucun IS, stock reportable de 800 €.
        seed_loss(&mut store, 2026);
        let loss_year = close_year(&mut store, 2026, false);
        assert_eq!(loss_year.losses_carried_forward, Money::from_cents(80_000));
        assert_eq!(
            tax_losses_available(store.connection(), date(2027, TimeMonth::January, 1)).unwrap(),
            Money::from_cents(80_000)
        );

        // 2027 : bénéfice comptable de 5 375 € → résultat fiscal 4 575 €, IS 15 % = 686,25 €
        // (au lieu de 806,25 € sans imputation), net = 5 375 − 686,25 = 4 688,75 €.
        seed_activity(&mut store, 2027);
        let profit_year = close_year(&mut store, 2027, false);
        assert_eq!(profit_year.result_before_tax, Money::from_cents(537_500));
        assert_eq!(profit_year.losses_imputed, Money::from_cents(80_000));
        assert_eq!(profit_year.taxable_result(), Money::from_cents(457_500));
        assert_eq!(profit_year.corporate_tax, Money::from_cents(68_625));
        assert_eq!(profit_year.net_result, Money::from_cents(468_875));
        assert_eq!(
            profit_year.losses_available_before(),
            Money::from_cents(80_000)
        );
        assert_eq!(profit_year.losses_carried_forward, Money::ZERO);
        // Report à nouveau comptable : −800 + 4 688,75 = 3 888,75 €.
        assert_eq!(profit_year.retained_earnings, Money::from_cents(388_875));
        // Le stock relu sur l'exercice déficitaire n'a pas bougé : c'est un stock *après* lui.
        let loss_year = fiscal_year_by_id(store.connection(), loss_year.id)
            .unwrap()
            .unwrap();
        assert_eq!(loss_year.losses_carried_forward, Money::from_cents(80_000));
        assert_eq!(
            tax_losses_available(store.connection(), date(2028, TimeMonth::January, 1)).unwrap(),
            Money::ZERO
        );
    }

    #[test]
    fn the_opening_balance_carries_prior_tax_losses_into_the_first_close() {
        let mut store = test_store("opening-losses");
        set_profile(&mut store, Some(100_000));
        let cmd = crate::opening_balance::RecordOpeningBalance {
            opens_on: date(2026, TimeMonth::January, 1),
            source: None,
            lines: ["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"]
                .iter()
                .map(|s| s.parse().unwrap())
                .collect(),
            // 3 000 € de déficits antérieurs (case 870 du dernier 2033-D).
            tax_losses: Money::from_cents(300_000),
        };
        Executor::new(&mut store).execute(&cmd, &human()).unwrap();
        // Lecture tolérante : le stock repris compte dès l'exercice qui ouvre ce jour-là, pas
        // pour un exercice commençant un autre jour sans exercice clos entre les deux.
        assert_eq!(
            tax_losses_available(store.connection(), date(2026, TimeMonth::January, 1)).unwrap(),
            Money::from_cents(300_000)
        );
        assert_eq!(
            tax_losses_available(store.connection(), date(2026, TimeMonth::July, 1)).unwrap(),
            Money::ZERO
        );

        // Bénéfice 5 375 € : 3 000 € imputés, IS sur 2 375 € = 356,25 €.
        seed_activity(&mut store, 2026);
        let year = close_year(&mut store, 2026, false);
        assert_eq!(year.losses_imputed, Money::from_cents(300_000));
        assert_eq!(year.taxable_result(), Money::from_cents(237_500));
        assert_eq!(year.corporate_tax, Money::from_cents(35_625));
        assert_eq!(year.losses_carried_forward, Money::ZERO);
        assert_eq!(year.losses_available_before(), Money::from_cents(300_000));
        assert_eq!(
            year.accounting_result().prior_losses_available,
            Money::from_cents(300_000)
        );
    }

    #[test]
    fn carrying_a_deficit_back_creates_an_is_credit_in_the_net_result() {
        let mut store = test_store("carry-back");
        set_profile(&mut store, Some(100_000));
        // 2026 : bénéfice fiscal 5 375 €, IS 806,25 €, 1 000 € distribués.
        seed_activity(&mut store, 2026);
        let profit_year = close_2026(&mut store, 0, 100_000);
        let profit_year = fiscal_year_by_id(store.connection(), profit_year)
            .unwrap()
            .unwrap();
        assert_eq!(profit_year.retained_earnings, Money::from_cents(356_875));

        // 2027 : déficit de 800 €, reporté en arrière sur les 4 375 € non distribués de 2026,
        // entièrement au taux réduit : créance de 15 % × 800 = 120 €.
        seed_loss(&mut store, 2027);
        let loss_year = close_year(&mut store, 2027, true);
        assert_eq!(loss_year.taxable_result(), Money::from_cents(-80_000));
        assert_eq!(loss_year.carried_back, Money::from_cents(80_000));
        assert_eq!(loss_year.carry_back_credit, Money::from_cents(12_000));
        assert_eq!(loss_year.corporate_tax, Money::ZERO);
        // Net comptable = −800 + 120 = −680 € ; report = 3 568,75 − 680 = 2 888,75 €.
        assert_eq!(loss_year.net_result, Money::from_cents(-68_000));
        assert_eq!(loss_year.retained_earnings, Money::from_cents(288_875));
        // Le déficit reporté en arrière ne l'est plus en avant.
        assert_eq!(loss_year.losses_carried_forward, Money::ZERO);
        assert_eq!(
            tax_losses_available(store.connection(), date(2028, TimeMonth::January, 1)).unwrap(),
            Money::ZERO
        );
        let result = loss_year.accounting_result();
        assert_eq!(result.carry_back_credit, Money::from_cents(12_000));
        assert_eq!(result.losses_carried_forward(), Money::ZERO);
    }

    #[test]
    fn carry_back_is_refused_without_a_deficit_a_previous_year_or_an_imputable_profit() {
        let mut store = test_store("carry-back-refused");
        set_profile(&mut store, Some(100_000));

        // Pas d'exercice précédent clos ici.
        seed_loss(&mut store, 2026);
        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: true,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(
            matches!(&err, AppError::Domain(msg) if msg.contains("exercice précédent")),
            "{err}"
        );
        assert!(list_fiscal_years(store.connection()).unwrap().is_empty());
        close_year(&mut store, 2026, false);

        // 2027 bénéficiaire : pas de déficit à reporter.
        seed_activity(&mut store, 2027);
        let cmd = CloseFiscalYear {
            starts_on: date(2027, TimeMonth::January, 1),
            ends_on: date(2027, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: true,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(
            matches!(&err, AppError::Domain(msg) if msg.contains("aucun déficit")),
            "{err}"
        );
        let cmd = CloseFiscalYear {
            carry_back: false,
            ..cmd
        };
        Executor::new(&mut store).execute(&cmd, &human()).unwrap();

        // 2028 et 2029 déficitaires : le déficit 2029 n'a, la veille, qu'un exercice lui-même
        // déficitaire — aucun bénéfice d'imputation (le report en arrière ne remonte jamais
        // au-delà de l'exercice précédent).
        seed_loss(&mut store, 2028);
        close_year(&mut store, 2028, false);
        seed_loss(&mut store, 2029);
        let cmd = CloseFiscalYear {
            starts_on: date(2029, TimeMonth::January, 1),
            ends_on: date(2029, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: true,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(
            matches!(&err, AppError::Domain(msg) if msg.contains("bénéfice d'imputation")),
            "{err}"
        );
    }
}
