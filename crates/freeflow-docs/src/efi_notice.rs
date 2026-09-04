//! Notice de saisie EFI : chaque case de la liasse, le montant à recopier, et où le saisir
//! sur impots.gouv.fr (régime simplifié, sans partenaire EDI). Ce n'est pas un Cerfa.

use freeflow_core::domain::Money;

use crate::error::DocsError;
use crate::liasse::LiasseExport;
use crate::typst::{Bindings, DISCLAIMER, compile, fr_date};

/// Rend la notice EFI en PDF.
///
/// # Errors
///
/// Échec d'exécution ou de compilation `typst`.
pub fn render_efi_notice(export: &LiasseExport) -> Result<Vec<u8>, DocsError> {
    let mut b = Bindings::new();
    let title_v = b.bind("Notice de saisie — liasse fiscale (EFI)");
    let author_v = b.bind(&export.company);
    let period_v = b.bind(&format!(
        "{} — SIREN {} — exercice du {} au {}",
        export.company,
        export.siren,
        fr_date(export.period_start),
        fr_date(export.period_end)
    ));
    let intro_v = b.bind(
        "Espace professionnel impots.gouv.fr → Déclarer → Impôt sur les sociétés, régime \
         simplifié (EFI). Gratuit, sans partenaire EDI. Recopiez chaque ligne ci-dessous dans \
         la case indiquée. Une case à zéro se saisit aussi (ou se laisse vide selon le \
         formulaire). Le relevé de solde d'IS (2572) se dépose même à zéro, le 15 du quatrième \
         mois après la clôture. La case 2033-D 870 (déficits restant à reporter) est la valeur \
         fiscale de l'exercice s'il est déficitaire.",
    );
    let disclaimer_v = b.bind(DISCLAIMER);
    let mut rows = String::new();
    for entry in &export.entries {
        let form_v = b.bind(entry.form);
        let case_v = b.bind(entry.case);
        let label_v = b.bind(&entry.label);
        let amount = Money::from_cents(entry.amount_cents);
        let amount_v = b.bind(&amount.to_string());
        let hint = format!("saisir {} en case {} du {}", amount, entry.case, entry.form);
        let hint_v = b.bind(&hint);
        rows.push_str(&format!(
            "[#{form_v}], [#{case_v}], [#{label_v}], [#{amount_v}], [#{hint_v}],\n"
        ));
    }
    let source = format!(
        r#"{declarations}
#set document(title: {title_v}, author: {author_v})
#set page(margin: 1.8cm, flipped: true)
#set text(size: 9pt)

#align(center)[#text(weight: "bold", size: 13pt)[#{title_v}]]
#align(center)[#{period_v}]
#v(0.8em)
#text(size: 9pt)[#{intro_v}]
#v(0.8em)

#table(
  columns: (0.8fr, 0.6fr, 2.4fr, 1fr, 2fr),
  align: (left, left, left, right, left),
  stroke: 0.5pt,
  [*Formulaire*], [*Case*], [*Libellé*], [*Montant*], [*Consigne*],
  {rows}
)

#v(1.5em)
#line(length: 100%, stroke: 0.5pt)
#v(0.5em)
#text(size: 8pt)[#{disclaimer_v}]
"#,
        declarations = b.declarations,
    );
    compile(&source)
}
