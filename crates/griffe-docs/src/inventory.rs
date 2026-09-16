//! Inventaire (L227-9 al. 3) : la liste des éléments d'actif et de passif au dernier jour de
//! l'exercice, tirée de la balance du grand livre. C'est ce qu'un greffe attend d'une TPE à
//! la place d'un inventaire de stock — pas un acte d'huissier.

use griffe_core::company::CompanyProfile;
use griffe_core::ledger::TrialBalance;

use crate::error::DocsError;
use crate::typst::{Bindings, DISCLAIMER, company_header, compile, fr_date};

/// Rend l'inventaire en PDF : balance des comptes, totaux, arrêté par le président.
///
/// # Errors
///
/// Échec d'exécution ou de compilation `typst`.
pub fn render_inventory(
    profile: &CompanyProfile,
    balance: &TrialBalance,
) -> Result<Vec<u8>, DocsError> {
    let mut b = Bindings::new();
    let header = company_header(&mut b, profile);
    let title_v = b.bind("Inventaire");
    let author_v = b.bind(&profile.name);
    let period_v = b.bind(&format!(
        "Exercice du {} au {} — arrêté au {}",
        fr_date(balance.exercise.start()),
        fr_date(balance.exercise.end()),
        fr_date(balance.exercise.end())
    ));
    let scope_v = b.bind(
        "Liste des éléments d'actif et de passif dérivée du grand livre (art. L123-12 et \
         L227-9 al. 3 du Code de commerce). Les soldes nuls sont omis. Document à joindre au \
         dépôt des comptes signés au greffe.",
    );
    let president = profile
        .president_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("le président");
    let closing_v = b.bind(&format!(
        "Arrêté le {} par {}.",
        fr_date(balance.exercise.end()),
        president
    ));
    let disclaimer_v = b.bind(DISCLAIMER);
    let mut rows = String::new();
    for row in &balance.rows {
        if row.debit.is_zero() && row.credit.is_zero() {
            continue;
        }
        let num_v = b.bind(row.account.number.as_ref());
        let label_v = b.bind(row.account.label.as_ref());
        let debit_v = b.bind(&row.debit.to_string());
        let credit_v = b.bind(&row.credit.to_string());
        rows.push_str(&format!(
            "[#{num_v}], [#{label_v}], [#{debit_v}], [#{credit_v}],\n"
        ));
    }
    let total_d_v = b.bind(&balance.total_debit.to_string());
    let total_c_v = b.bind(&balance.total_credit.to_string());
    let source = format!(
        r#"{declarations}
#set document(title: {title_v}, author: {author_v})
#set page(margin: 2.2cm)
#set text(size: 10.5pt)

{header}
#v(1.5em)
#align(center)[#text(weight: "bold", size: 13pt)[#{title_v}]]
#align(center)[#{period_v}]
#v(1em)
#text(size: 9pt)[#{scope_v}]
#v(1em)

#table(
  columns: (1.2fr, 3fr, 1.2fr, 1.2fr),
  align: (left, left, right, right),
  stroke: 0.5pt,
  [*Compte*], [*Libellé*], [*Débit*], [*Crédit*],
  {rows}
  [*Total*], [], [*#{total_d_v}*], [*#{total_c_v}*],
)

#v(1.5em)
[#{closing_v}]

#v(2em)
#line(length: 100%, stroke: 0.5pt)
#v(0.5em)
#text(size: 8pt)[#{disclaimer_v}]
"#,
        declarations = b.declarations,
    );
    compile(&source)
}
