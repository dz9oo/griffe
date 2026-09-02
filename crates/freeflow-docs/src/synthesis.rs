//! Synthèse comptable : compte de résultat simplifié de l'exercice, avec la colonne N−1 quand
//! l'exercice précédent est clos dans FreeFlow. Le périmètre est celui de
//! [`freeflow_core::accounting`] — prestation de services sans immobilisations : pas
//! d'amortissements, de provisions ni de variation de stock, et le « bilan » se limite aux
//! capitaux propres reconstituables (capital, réserve légale, report à nouveau).

use freeflow_core::accounting::AccountingResult;
use freeflow_core::company::CompanyProfile;
use freeflow_core::fiscal_year::FiscalYearRecord;

use crate::error::DocsError;
use crate::typst::{Bindings, DISCLAIMER, company_header, compile, fr_date};

/// Rend le compte de résultat simplifié en PDF. `prior` est l'exercice précédent s'il est clos
/// dans FreeFlow — sa colonne est simplement omise sinon.
///
/// # Errors
///
/// Échec d'exécution ou de compilation `typst`.
pub fn render_synthesis(
    profile: &CompanyProfile,
    result: &AccountingResult,
    prior: Option<&FiscalYearRecord>,
) -> Result<Vec<u8>, DocsError> {
    let mut b = Bindings::new();
    let header = company_header(&mut b, profile);
    let title_v = b.bind("Compte de résultat simplifié");
    let author_v = b.bind(&profile.name);
    let period_v = b.bind(&format!(
        "Exercice du {} au {}",
        fr_date(result.period.start()),
        fr_date(result.period.end())
    ));
    let disclaimer_v = b.bind(DISCLAIMER);
    let scope_v = b.bind(&format!(
        "Périmètre simplifié : produits = factures émises HT, charges = dépenses nettes de TVA \
         déductible et rémunération du dirigeant. Sans amortissements, provisions, variation de \
         stock ni produits/charges constatés d'avance. Déficits reportables en avant après \
         l'exercice : {}{}.",
        result.losses_carried_forward(),
        if result.carried_back.is_zero() {
            String::new()
        } else {
            format!(
                " ; déficit reporté en arrière (art. 220 quinquies CGI) : {}",
                result.carried_back
            )
        }
    ));

    // Une ligne du tableau : libellé, montant N, et montant N−1 si l'exercice précédent est là.
    let rows: Vec<(String, String, Option<String>)> = [
        (
            "Produits — chiffre d'affaires HT",
            result.revenue_ht,
            prior.map(|p| p.revenue_ht),
        ),
        (
            "Charges externes (nettes de TVA déductible)",
            result.expenses,
            prior.map(|p| p.expenses),
        ),
        (
            "Rémunération du dirigeant (coût employeur)",
            result.director_remuneration,
            prior.map(|p| p.director_remuneration),
        ),
        (
            "Résultat avant impôt",
            result.result_before_tax,
            prior.map(|p| p.result_before_tax),
        ),
        (
            "Déficits antérieurs imputés (art. 209 I CGI)",
            result.losses_imputed,
            prior.map(|p| p.losses_imputed),
        ),
        (
            "Résultat fiscal",
            result.taxable_result,
            prior.map(|p| p.taxable_result()),
        ),
        (
            "Impôt sur les sociétés",
            result.corporate_tax,
            prior.map(|p| p.corporate_tax),
        ),
        (
            "Produit du report en arrière du déficit (créance d'IS)",
            result.carry_back_credit,
            prior.map(|p| p.carry_back_credit),
        ),
        (
            "Résultat net",
            result.net_result,
            prior.map(|p| p.net_result),
        ),
    ]
    .into_iter()
    .map(|(label, current, previous)| {
        (
            b.bind(label),
            b.bind(&current.to_string()),
            previous.map(|m| b.bind(&m.to_string())),
        )
    })
    .collect();

    let (columns, header_cells, body_cells) = if prior.is_some() {
        let n1_header_v = b.bind(&prior.map_or_else(String::new, |p| {
            format!("Exercice clos le {}", fr_date(p.ends_on))
        }));
        let cells: String = rows
            .iter()
            .map(|(label, current, previous)| {
                let previous = previous.as_deref().unwrap_or(current);
                format!("[#{label}], [#{current}], [#{previous}],\n")
            })
            .collect();
        (
            "(2fr, 1fr, 1fr)",
            format!("[], [*Exercice N*], [*#{n1_header_v}*],\n"),
            cells,
        )
    } else {
        let cells: String = rows
            .iter()
            .map(|(label, current, _)| format!("[#{label}], [#{current}],\n"))
            .collect();
        ("(2fr, 1fr)", String::new(), cells)
    };

    let source = format!(
        r#"{declarations}
#set document(title: {title_v}, author: {author_v})
#set page(margin: 2.2cm)
#set text(size: 10.5pt)

{header}
#v(1.5em)
#align(center)[#text(weight: "bold", size: 13pt)[#{title_v}]]
#align(center)[#{period_v}]
#v(1.5em)

#table(
  columns: {columns},
  align: (left, right, right),
  stroke: 0.5pt,
  {header_cells}{body_cells}
)

#v(1em)
#text(size: 9pt)[#{scope_v}]

#v(2em)
#line(length: 100%, stroke: 0.5pt)
#v(0.5em)
#text(size: 8pt)[#{disclaimer_v}]
"#,
        declarations = b.declarations,
    );
    compile(&source)
}
