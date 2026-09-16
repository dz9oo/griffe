//! Bilan de clôture (lot 31) : l'actif et le passif dans la présentation du tableau 2033-A-SD
//! (brut, amortissements, net ; capitaux propres, provisions, dettes), suivi de la balance des
//! comptes dont il est tiré. Les deux viennent du grand livre dérivé de
//! [`griffe_core::ledger`] — ce document existe aussi pour un exercice pas encore clos, comme
//! le FEC : c'est ce qu'on regarde *avant* de clore.

use griffe_core::company::CompanyProfile;
use griffe_core::domain::Money;
use griffe_core::ledger::{BalanceSheet, LiabilitySection, TrialBalance};

use crate::error::DocsError;
use crate::typst::{Bindings, DISCLAIMER, company_header, compile, fr_date};

/// Une cellule de montant, vide pour zéro (les formulaires laissent les cases vides).
fn amount_cell(b: &mut Bindings, amount: Money) -> String {
    if amount.is_zero() {
        "[]".to_string()
    } else {
        format!("[#{}]", b.bind(&amount.to_string()))
    }
}

/// Rend le bilan (2033-A) et la balance des comptes en PDF.
///
/// # Errors
///
/// Échec d'exécution ou de compilation `typst`.
pub fn render_balance_sheet(
    profile: &CompanyProfile,
    sheet: &BalanceSheet,
    balance: &TrialBalance,
) -> Result<Vec<u8>, DocsError> {
    let mut b = Bindings::new();
    let header = company_header(&mut b, profile);
    let title_v = b.bind("Bilan simplifié");
    let author_v = b.bind(&profile.name);
    let period_v = b.bind(&format!(
        "Exercice du {} au {} — bilan au {}",
        fr_date(sheet.exercise.start()),
        fr_date(sheet.exercise.end()),
        fr_date(sheet.exercise.end())
    ));
    let disclaimer_v = b.bind(DISCLAIMER);
    let scope_v = b.bind(
        "Bilan dérivé des faits enregistrés (bilan d'ouverture, factures et avoirs, \
         encaissements, dépenses) et des opérations de clôture estimées (rémunération du \
         dirigeant réputée due, impôt sur les sociétés). Présentation du tableau 2033-A-SD. \
         Sans amortissement de l'exercice, provision ni régularisation ; la TVA n'est pas \
         liquidée ; les dépenses sont réputées payées à leur date. À faire relire par \
         l'expert-comptable avant tout dépôt.",
    );
    let balance_title_v = b.bind("Balance des comptes");

    // --- Actif : rubrique, brut, amortissements, net ; sous-totaux I et II, total général. ---
    let mut asset_rows = String::new();
    let mut push_asset_section = |b: &mut Bindings, fixed: bool, title: &str, total: Money| {
        let title_v = b.bind(title);
        asset_rows.push_str(&format!("table.cell(colspan: 4)[*#{title_v}*],\n"));
        for a in sheet
            .assets
            .iter()
            .filter(|a| a.rubric.is_fixed_asset() == fixed)
        {
            let label_v = b.bind(&format!("{} ({})", a.label, a.case_gross));
            asset_rows.push_str(&format!(
                "[#{label_v}], {}, {}, {},\n",
                amount_cell(b, a.gross),
                amount_cell(b, a.depreciation),
                amount_cell(b, a.net)
            ));
        }
        let total_label_v = b.bind(if fixed { "Total I" } else { "Total II" });
        let total_v = b.bind(&total.to_string());
        asset_rows.push_str(&format!("[*#{total_label_v}*], [], [], [*#{total_v}*],\n"));
    };
    push_asset_section(&mut b, true, "Actif immobilisé", sheet.fixed_assets_net);
    push_asset_section(&mut b, false, "Actif circulant", sheet.current_assets_net);
    let total_gross_v = b.bind(&sheet.total_assets_gross.to_string());
    let total_dep_v = b.bind(&sheet.total_depreciation.to_string());
    let total_net_v = b.bind(&sheet.total_assets_net.to_string());
    asset_rows.push_str(&format!(
        "[*Total général (110 / 112)*], [*#{total_gross_v}*], [*#{total_dep_v}*], \
         [*#{total_net_v}*],\n"
    ));

    // --- Passif : rubrique, montant ; totaux I, II, III, général. ---
    let mut liability_rows = String::new();
    let sections = [
        (
            LiabilitySection::Equity,
            "Capitaux propres",
            "Total I (142)",
            sheet.total_equity,
        ),
        (
            LiabilitySection::Provisions,
            "Provisions pour risques et charges",
            "Total II (154)",
            sheet.total_provisions,
        ),
        (
            LiabilitySection::Debts,
            "Dettes",
            "Total III",
            sheet.total_debts,
        ),
    ];
    for (section, title, total_label, total) in sections {
        let title_v = b.bind(title);
        liability_rows.push_str(&format!("table.cell(colspan: 2)[*#{title_v}*],\n"));
        for l in sheet
            .liabilities
            .iter()
            .filter(|l| l.rubric.section() == section)
        {
            let label_v = b.bind(&format!("{} ({})", l.label, l.case));
            liability_rows.push_str(&format!(
                "[#{label_v}], {},\n",
                amount_cell(&mut b, l.amount)
            ));
        }
        let total_label_v = b.bind(total_label);
        let total_v = b.bind(&total.to_string());
        liability_rows.push_str(&format!("[*#{total_label_v}*], [*#{total_v}*],\n"));
    }
    if !sheet.shareholder_current_accounts.is_zero() {
        let label_v = b.bind("dont comptes courants d'associés (169)");
        let amount_v = b.bind(&sheet.shareholder_current_accounts.to_string());
        liability_rows.push_str(&format!("[#emph({label_v})], [#emph({amount_v})],\n"));
    }
    let total_liabilities_v = b.bind(&sheet.total_liabilities.to_string());
    liability_rows.push_str(&format!(
        "[*Total général (180)*], [*#{total_liabilities_v}*],\n"
    ));

    // --- Balance des comptes. ---
    let mut balance_rows = String::new();
    for row in &balance.rows {
        let number_v = b.bind(&row.account.number);
        let label_v = b.bind(&row.account.label);
        let (debit_balance, credit_balance) = if row.balance.is_negative() {
            (Money::ZERO, -row.balance)
        } else {
            (row.balance, Money::ZERO)
        };
        balance_rows.push_str(&format!(
            "[#{number_v}], [#{label_v}], {}, {}, {}, {},\n",
            amount_cell(&mut b, row.debit),
            amount_cell(&mut b, row.credit),
            amount_cell(&mut b, debit_balance),
            amount_cell(&mut b, credit_balance)
        ));
    }
    let total_debit_v = b.bind(&balance.total_debit.to_string());
    let total_credit_v = b.bind(&balance.total_credit.to_string());
    balance_rows.push_str(&format!(
        "[], [*Totaux*], [*#{total_debit_v}*], [*#{total_credit_v}*], [], [],\n"
    ));

    let source = format!(
        r#"{declarations}
#set document(title: {title_v}, author: {author_v})
#set page(margin: 2cm)
#set text(size: 9.5pt)

{header}
#v(1.2em)
#align(center)[#text(weight: "bold", size: 13pt)[#{title_v}]]
#align(center)[#{period_v}]
#v(1em)

*Actif*
#table(
  columns: (2.4fr, 1fr, 1fr, 1fr),
  align: (left, right, right, right),
  stroke: 0.5pt,
  [], [*Brut*], [*Amortissements, dépréciations*], [*Net*],
  {asset_rows}
)

#v(0.8em)
*Passif*
#table(
  columns: (2.4fr, 1fr),
  align: (left, right),
  stroke: 0.5pt,
  [], [*Net*],
  {liability_rows}
)

#v(0.8em)
#text(size: 8.5pt)[#{scope_v}]

#pagebreak()
#align(center)[#text(weight: "bold", size: 12pt)[#{balance_title_v}]]
#align(center)[#{period_v}]
#v(0.8em)
#table(
  columns: (auto, 2fr, 1fr, 1fr, 1fr, 1fr),
  align: (left, left, right, right, right, right),
  stroke: 0.5pt,
  [*Compte*], [*Libellé*], [*Débit*], [*Crédit*], [*Solde débiteur*], [*Solde créditeur*],
  {balance_rows}
)

#v(2em)
#line(length: 100%, stroke: 0.5pt)
#v(0.5em)
#text(size: 8pt)[#{disclaimer_v}]
"#,
        declarations = b.declarations,
    );
    compile(&source)
}
