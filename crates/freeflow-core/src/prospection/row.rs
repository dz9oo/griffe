//! Correspondance ligne SQL <-> types du domaine, pour ce module uniquement — aucune requête
//! SQL sur ces tables ne doit exister ailleurs dans l'application.

use rusqlite::{Connection, OptionalExtension, Row, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::AppError;
use crate::domain::{
    self, Interaction, LossReason, Money, Opportunity, OpportunityId, OpportunityStage, Probability,
};

/// Convertit une erreur de (dé)sérialisation survenant *pendant* le mapping d'une ligne en
/// `rusqlite::Error` : les closures de `query_map`/`query_row` doivent renvoyer un
/// `rusqlite::Result`, pas un `AppError`.
fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

pub(super) fn insert_opportunity(conn: &Connection, opp: &Opportunity) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO opportunities
            (id, client_id, name, stage, amount_cents, probability_percent, next_action_at, source, loss_reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            opp.id.to_string(),
            opp.client_id.to_string(),
            opp.name,
            opp.stage.as_str(),
            opp.amount.cents(),
            i64::from(opp.probability.percent()),
            opp.next_action_at.map(domain::format_date),
            opp.source,
            opp.loss_reason.as_ref().map(serde_json::to_string).transpose()?,
            opp.created_at.format(&Rfc3339)?,
        ],
    )?;
    Ok(())
}

pub(super) fn opportunity_by_id(
    conn: &Connection,
    id: OpportunityId,
) -> Result<Option<Opportunity>, AppError> {
    conn.query_row(
        "SELECT * FROM opportunities WHERE id = ?1",
        [id.to_string()],
        row_to_opportunity,
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn update_stage(
    conn: &Connection,
    id: OpportunityId,
    stage: OpportunityStage,
    next_action_at: Option<time::Date>,
    loss_reason: Option<&LossReason>,
) -> Result<(), AppError> {
    let loss_reason_json = loss_reason.map(serde_json::to_string).transpose()?;
    conn.execute(
        "UPDATE opportunities SET stage = ?1, next_action_at = ?2, loss_reason = ?3 WHERE id = ?4",
        params![
            stage.as_str(),
            next_action_at.map(domain::format_date),
            loss_reason_json,
            id.to_string()
        ],
    )?;
    Ok(())
}

pub(super) fn row_to_opportunity(row: &Row) -> rusqlite::Result<Opportunity> {
    let stage_str: String = row.get("stage")?;
    let stage = stage_str.parse::<OpportunityStage>().map_err(conv_err)?;

    let probability_percent: i64 = row.get("probability_percent")?;
    let probability =
        Probability::new(u8::try_from(probability_percent).map_err(conv_err)?).map_err(conv_err)?;

    let next_action_at: Option<String> = row.get("next_action_at")?;
    let next_action_at = next_action_at
        .map(|s| domain::parse_date(&s))
        .transpose()
        .map_err(conv_err)?;

    let loss_reason: Option<String> = row.get("loss_reason")?;
    let loss_reason = loss_reason
        .map(|s| serde_json::from_str::<LossReason>(&s))
        .transpose()
        .map_err(conv_err)?;

    let created_at: String = row.get("created_at")?;
    let created_at = OffsetDateTime::parse(&created_at, &Rfc3339).map_err(conv_err)?;

    let id: String = row.get("id")?;
    let client_id: String = row.get("client_id")?;

    Ok(Opportunity {
        id: id.parse().map_err(conv_err)?,
        client_id: client_id.parse().map_err(conv_err)?,
        name: row.get("name")?,
        stage,
        amount: Money::from_cents(row.get("amount_cents")?),
        probability,
        next_action_at,
        source: row.get("source")?,
        loss_reason,
        created_at,
    })
}

pub(super) fn insert_interaction(
    conn: &Connection,
    interaction: &Interaction,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO interactions (id, opportunity_id, kind, note, occurred_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            interaction.id.to_string(),
            interaction.opportunity_id.to_string(),
            interaction.kind.as_str(),
            interaction.note,
            interaction.occurred_at.format(&Rfc3339)?,
        ],
    )?;
    Ok(())
}
