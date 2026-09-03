//! Décision d'affectation du résultat : le tableau origine → affectation, l'annexe chiffrée du
//! PV d'approbation ([`crate::minutes`]). Document distinct parce qu'il circule seul (banque,
//! expert-comptable) là où le PV reste dans le registre des décisions.

use freeflow_core::company::CompanyProfile;
use freeflow_core::fiscal_year::FiscalYearRecord;

use crate::error::DocsError;
use crate::typst::{Bindings, DISCLAIMER, company_header, compile, fr_date};

/// Rend la décision d'affectation du résultat en PDF.
///
/// # Errors
///
/// Échec d'exécution ou de compilation `typst`.
pub fn render_appropriation_decision(
    profile: &CompanyProfile,
    year: &FiscalYearRecord,
) -> Result<Vec<u8>, DocsError> {
    // Le report antérieur n'est pas stocké : il se reconstitue depuis l'équation d'affectation
    // (report résultant = report antérieur + net − réserve − dividendes).
    let prior_retained =
        year.retained_earnings - year.net_result + year.legal_reserve + year.dividends;
    let distributable = year.net_result + prior_retained;
    // Lot 41 : une perte ne se « distribue » pas — le tableau d'origine parle alors du solde à
    // reporter, et l'affectation se réduit au report à nouveau.
    let is_loss = year.net_result.is_negative();

    let mut b = Bindings::new();
    let header = company_header(&mut b, profile);
    let title_v = b.bind("Affectation du résultat de l'exercice");
    let author_v = b.bind(&profile.name);
    let period_v = b.bind(&format!(
        "Exercice du {} au {}",
        fr_date(year.starts_on),
        fr_date(year.ends_on)
    ));
    let net_result_v = b.bind(&year.net_result.to_string());
    let result_label_v = b.bind(if is_loss {
        "Perte de l'exercice"
    } else {
        "Résultat net de l'exercice"
    });
    let total_label_v = b.bind(if is_loss {
        "Solde à reporter"
    } else {
        "Total distribuable"
    });
    let decision_note = if is_loss {
        let note_v = b.bind(
            "La perte de l'exercice est affectée en totalité au compte « report à nouveau » ; \
             aucune dotation ni distribution n'est décidée.",
        );
        format!("#{note_v}\n#v(0.6em)\n")
    } else {
        String::new()
    };
    let prior_retained_v = b.bind(&prior_retained.to_string());
    let distributable_v = b.bind(&distributable.to_string());
    let legal_reserve_v = b.bind(&year.legal_reserve.to_string());
    let dividends_v = b.bind(&year.dividends.to_string());
    let retained_v = b.bind(&year.retained_earnings.to_string());
    let disclaimer_v = b.bind(DISCLAIMER);

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

*Origine des sommes à affecter*

#table(
  columns: (1fr, auto),
  align: (left, right),
  stroke: 0.5pt,
  [#{result_label_v}], [#{net_result_v}],
  [Report à nouveau antérieur], [#{prior_retained_v}],
  [*#{total_label_v}*], [*#{distributable_v}*],
)

#v(1em)
*Affectation décidée*

{decision_note}
#table(
  columns: (1fr, auto),
  align: (left, right),
  stroke: 0.5pt,
  [Dotation à la réserve légale], [#{legal_reserve_v}],
  [Distribution de dividendes], [#{dividends_v}],
  [*Report à nouveau (solde après affectation)*], [*#{retained_v}*],
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
