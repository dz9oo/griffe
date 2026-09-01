//! Procès-verbal des décisions de l'associé unique : approbation des comptes de l'exercice,
//! affectation du résultat, quitus au président. Une SASU/EURL d'indépendant n'a pas
//! d'assemblée plurale — l'« AG d'approbation » du calendrier fiscal y prend la forme de ce PV.

use freeflow_core::company::CompanyProfile;
use freeflow_core::fiscal_year::FiscalYearRecord;
use time::Date;

use crate::error::DocsError;
use crate::typst::{Bindings, DISCLAIMER, company_header, compile, fr_date};

/// Rend le PV d'approbation des comptes en PDF. La date de l'acte est `approved_on` si
/// l'exercice est approuvé, sinon `today` (projet de PV à faire signer).
///
/// # Errors
///
/// Échec d'exécution ou de compilation `typst`.
pub fn render_approval_minutes(
    profile: &CompanyProfile,
    year: &FiscalYearRecord,
    today: Date,
) -> Result<Vec<u8>, DocsError> {
    let mut b = Bindings::new();
    let header = company_header(&mut b, profile);
    let decided_on = year.approved_on.unwrap_or(today);
    let decided_on_v = b.bind(&fr_date(decided_on));
    let ends_on_v = b.bind(&fr_date(year.ends_on));
    let starts_on_v = b.bind(&fr_date(year.starts_on));
    let net_result_v = b.bind(&year.net_result.to_string());
    let legal_reserve_v = b.bind(&year.legal_reserve.to_string());
    let dividends_v = b.bind(&year.dividends.to_string());
    let retained_v = b.bind(&year.retained_earnings.to_string());
    let disclaimer_v = b.bind(DISCLAIMER);
    let title_v = b.bind("Procès-verbal des décisions de l'associé unique");
    let author_v = b.bind(&profile.name);
    let draft_note = if year.approved_on.is_none() {
        let draft_v = b.bind("PROJET — non approuvé à ce jour");
        format!("#align(center)[#text(weight: \"bold\", fill: rgb(\"#8a5a00\"))[#{draft_v}]]\n")
    } else {
        String::new()
    };

    let source = format!(
        r#"{declarations}
#set document(title: {title_v}, author: {author_v})
#set page(margin: 2.2cm)
#set text(size: 10.5pt)

{header}
#v(1.5em)
#align(center)[#text(weight: "bold", size: 13pt)[#{title_v}]]
{draft_note}
#v(1em)

Le #{decided_on_v}, l'associé unique, exerçant les pouvoirs dévolus à la collectivité des
associés, a pris les décisions suivantes relatives à l'exercice social ouvert le #{starts_on_v}
et clos le #{ends_on_v}.

#v(0.8em)
*Première décision — Approbation des comptes.* L'associé unique, après avoir pris connaissance
des comptes de l'exercice, les approuve tels qu'ils lui ont été présentés. Ces comptes font
apparaître un résultat net de #{net_result_v}.

#v(0.8em)
*Deuxième décision — Affectation du résultat.* L'associé unique décide d'affecter le résultat de
l'exercice comme suit :

#table(
  columns: (1fr, auto),
  align: (left, right),
  stroke: 0.5pt,
  [Dotation à la réserve légale], [#{legal_reserve_v}],
  [Distribution de dividendes], [#{dividends_v}],
  [Report à nouveau (solde après affectation)], [#{retained_v}],
)

#v(0.8em)
*Troisième décision — Quitus.* L'associé unique donne quitus entier et sans réserve au président
pour l'exécution de son mandat au titre de l'exercice écoulé.

#v(2.5em)
Fait le #{decided_on_v}.

#v(2em)
L'associé unique

#v(2em)
#line(length: 100%, stroke: 0.5pt)
#v(0.5em)
#text(size: 8pt)[#{disclaimer_v}]
"#,
        declarations = b.declarations,
    );
    compile(&source)
}
