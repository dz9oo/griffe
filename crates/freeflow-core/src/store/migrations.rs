//! Schéma versionné, appliqué automatiquement à l'ouverture d'un coffre.

use rusqlite_migration::{M, Migrations};

#[must_use]
pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("migrations/0001_init_up.sql"))
            .down(include_str!("migrations/0001_init_down.sql")),
        M::up(include_str!("migrations/0002_app_layer_up.sql"))
            .down(include_str!("migrations/0002_app_layer_down.sql")),
        M::up(include_str!("migrations/0003_prospection_up.sql"))
            .down(include_str!("migrations/0003_prospection_down.sql")),
        M::up(include_str!("migrations/0004_billing_up.sql"))
            .down(include_str!("migrations/0004_billing_down.sql")),
        M::up(include_str!("migrations/0005_quotes_up.sql"))
            .down(include_str!("migrations/0005_quotes_down.sql")),
        M::up(include_str!("migrations/0006_company_profile_up.sql"))
            .down(include_str!("migrations/0006_company_profile_down.sql")),
        M::up(include_str!("migrations/0007_expenses_up.sql"))
            .down(include_str!("migrations/0007_expenses_down.sql")),
        M::up(include_str!("migrations/0008_mutability_up.sql"))
            .down(include_str!("migrations/0008_mutability_down.sql")),
        M::up(include_str!(
            "migrations/0009_prospection_mutability_up.sql"
        ))
        .down(include_str!(
            "migrations/0009_prospection_mutability_down.sql"
        )),
        M::up(include_str!("migrations/0010_fiscal_year_up.sql"))
            .down(include_str!("migrations/0010_fiscal_year_down.sql")),
        M::up(include_str!(
            "migrations/0011_fiscal_year_immutability_up.sql"
        ))
        .down(include_str!(
            "migrations/0011_fiscal_year_immutability_down.sql"
        )),
        M::up(include_str!("migrations/0012_mission_lineage_up.sql"))
            .down(include_str!("migrations/0012_mission_lineage_down.sql")),
        M::up(include_str!("migrations/0013_payment_corrections_up.sql"))
            .down(include_str!("migrations/0013_payment_corrections_down.sql")),
        M::up(include_str!("migrations/0014_opening_balance_up.sql"))
            .down(include_str!("migrations/0014_opening_balance_down.sql")),
        M::up(include_str!("migrations/0015_tax_losses_up.sql"))
            .down(include_str!("migrations/0015_tax_losses_down.sql")),
    ])
}
