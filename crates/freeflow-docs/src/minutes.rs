//! Procès-verbal des décisions de l'associé unique : approbation des comptes de l'exercice,
//! affectation du résultat, quitus au président. Une SASU/EURL d'indépendant n'a pas
//! d'assemblée plurale — l'« AG d'approbation » du calendrier fiscal y prend la forme de ce PV,
//! consigné au registre des décisions de l'associé unique (art. L227-9 C. com.).
//!
//! Lot 41 : le PV est **nominatif** (associé unique et président tirés du profil, à défaut un
//! blanc à compléter), porte les mentions qu'un greffe ou un vérificateur attend — charges non
//! déductibles (art. 223 quater CGI), conventions réglementées (art. L227-10), dispense de
//! rapport de gestion (art. L232-1 IV) — et sait dire une perte sans parler de « distribuable ».

use freeflow_core::company::CompanyProfile;
use freeflow_core::domain::Money;
use freeflow_core::fiscal_year::FiscalYearRecord;
use time::Date;

use crate::error::DocsError;
use crate::typst::{Bindings, DISCLAIMER, company_header, compile, fr_date};

/// Le nom à imprimer, ou un blanc à compléter à la main quand le profil ne le porte pas.
fn name_or_blank(name: Option<&str>) -> String {
    name.map(str::trim)
        .filter(|n| !n.is_empty())
        .map_or_else(|| "________________________".to_string(), str::to_string)
}

/// L'associé unique est-il aussi le président (même personne, même nom) ? Décide de la mention
/// de l'art. L227-9 al. 3 : le dépôt des comptes signés au greffe vaut alors approbation.
fn sole_shareholder_is_president(profile: &CompanyProfile) -> bool {
    match (&profile.sole_shareholder_name, &profile.president_name) {
        (Some(a), Some(b)) => {
            let a = a.trim();
            !a.is_empty() && a.eq_ignore_ascii_case(b.trim())
        }
        _ => false,
    }
}

/// Rend le PV d'approbation des comptes en PDF. La date de l'acte est `approved_on` si
/// l'exercice est approuvé, sinon `today` (projet de PV à faire signer).
///
/// # Errors
///
/// Échec d'exécution ou de compilation `typst`.
#[allow(clippy::too_many_lines)]
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
    let shareholder_v = b.bind(&name_or_blank(profile.sole_shareholder_name.as_deref()));
    let shareholder_address_v = b.bind(&name_or_blank(profile.sole_shareholder_address.as_deref()));
    let president_v = b.bind(&name_or_blank(profile.president_name.as_deref()));
    let is_loss = year.net_result.is_negative();
    let net_abs_v = b.bind(&Money::from_cents(year.net_result.cents().abs()).to_string());
    let legal_reserve_v = b.bind(&year.legal_reserve.to_string());
    let dividends_v = b.bind(&year.dividends.to_string());
    let retained_v = b.bind(&year.retained_earnings.to_string());
    let corporate_tax_v = b.bind(&year.corporate_tax.to_string());
    let non_deductible_v = b.bind(&year.non_deductible_expenses.to_string());
    let disclaimer_v = b.bind(DISCLAIMER);
    let title_v = b.bind("Procès-verbal des décisions de l'associé unique");
    let author_v = b.bind(&profile.name);
    let draft_note = if year.approved_on.is_none() {
        let draft_v = b.bind("PROJET — non approuvé à ce jour");
        format!("#align(center)[#text(weight: \"bold\", fill: rgb(\"#8a5a00\"))[#{draft_v}]]\n")
    } else {
        String::new()
    };

    let result_sentence = if is_loss {
        format!("Ces comptes font apparaître une perte de #{net_abs_v}.")
    } else {
        format!("Ces comptes font apparaître un bénéfice net de #{net_abs_v}.")
    };
    let tax_sentence = if year.non_deductible_expenses > Money::ZERO {
        format!(
            "Conformément à l'article 223 quater du Code général des impôts, l'associé unique \
             prend acte que les comptes de l'exercice comprennent #{non_deductible_v} de \
             dépenses et charges visées à l'article 39-4 du même code, non déductibles du \
             résultat fiscal, et de l'impôt correspondant, compris dans les #{corporate_tax_v} \
             d'impôt sur les sociétés de l'exercice."
        )
    } else {
        "Conformément à l'article 223 quater du Code général des impôts, l'associé unique \
         prend acte que les comptes de l'exercice ne comprennent aucune dépense ni charge \
         visée à l'article 39-4 du même code."
            .to_string()
    };
    let appropriation = if is_loss {
        "*Deuxième décision — Affectation du résultat.* L'associé unique décide d'affecter la \
         perte de l'exercice en totalité au compte « report à nouveau », dont le solde après \
         affectation s'établit à #{retained_v}. Aucune distribution n'est décidée."
            .replace("#{retained_v}", &format!("#{retained_v}"))
    } else {
        format!(
            "*Deuxième décision — Affectation du résultat.* L'associé unique décide d'affecter \
             le bénéfice de l'exercice comme suit :\n\n#table(\n  columns: (1fr, auto),\n  \
             align: (left, right),\n  stroke: 0.5pt,\n  [Dotation à la réserve légale], \
             [#{legal_reserve_v}],\n  [Distribution de dividendes], [#{dividends_v}],\n  \
             [Report à nouveau (solde après affectation)], [#{retained_v}],\n)"
        )
    };
    let simple_way = if sole_shareholder_is_president(profile) {
        "\n#v(0.8em)\n_L'associé unique exerçant lui-même la présidence, le dépôt au greffe, \
         dans les six mois de la clôture, de l'inventaire et des comptes annuels dûment \
         signés vaut approbation des comptes (art. L227-9 al. 3 C. com.) ; le présent \
         procès-verbal en tient lieu de trace au registre des décisions._\n"
    } else {
        ""
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

Le #{decided_on_v}, #{shareholder_v}, demeurant #{shareholder_address_v}, associé unique de
la société, exerçant les pouvoirs dévolus à la collectivité des associés, a pris les décisions
suivantes relatives à l'exercice social ouvert le #{starts_on_v} et clos le #{ends_on_v},
après avoir pris connaissance des comptes annuels arrêtés par le président, #{president_v}.
La société remplissant les conditions de l'article L232-1 IV du Code de commerce, aucun
rapport de gestion n'est établi.

#v(0.8em)
*Première décision — Approbation des comptes.* L'associé unique approuve les comptes de
l'exercice tels qu'ils lui ont été présentés, ainsi que les opérations traduites dans ces
comptes. {result_sentence} {tax_sentence}

#v(0.8em)
{appropriation}

#v(0.8em)
*Troisième décision — Conventions réglementées.* L'associé unique prend acte qu'aucune
convention visée à l'article L227-10 du Code de commerce n'est intervenue au cours de
l'exercice, hormis, le cas échéant, les opérations courantes conclues à des conditions
normales ; les conventions conclues avec l'associé unique sont mentionnées au registre des
décisions.

#v(0.8em)
*Quatrième décision — Quitus.* L'associé unique donne quitus entier et sans réserve au
président pour l'exécution de son mandat au titre de l'exercice écoulé.
{simple_way}
#v(2em)
De tout ce que dessus, il a été dressé le présent procès-verbal, signé par l'associé unique et
consigné au registre des décisions de l'associé unique (art. L227-9 C. com.).

#v(1.5em)
Fait le #{decided_on_v}.

#v(2em)
#{shareholder_v}, associé unique

#v(2em)
#line(length: 100%, stroke: 0.5pt)
#v(0.5em)
#text(size: 8pt)[#{disclaimer_v}]
"#,
        declarations = b.declarations,
    );
    compile(&source)
}
