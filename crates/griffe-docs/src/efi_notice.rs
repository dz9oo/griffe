//! Notice de saisie EFI : chaque case de la liasse, le montant à recopier, et où le saisir
//! sur impots.gouv.fr (régime simplifié, sans partenaire EDI). Ce n'est pas un Cerfa.

use griffe_core::domain::Money;

use crate::error::DocsError;
use crate::liasse::LiasseExport;
use crate::typst::{Bindings, DISCLAIMER, compile, fr_date};

/// Euro entier à recopier, tel qu'il se saisit sur le formulaire.
fn euro_label(euros: i64) -> String {
    format!("{euros}\u{a0}€")
}

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
         la case indiquée, en euros entiers (art. 1657 CGI). Une case à zéro se saisit aussi \
         (ou se laisse vide selon le formulaire). Le 2033-E et le 2033-G sont néant. Le relevé \
         de solde d'IS (2572) se dépose même à zéro, le 15 du quatrième mois après la clôture. \
         La case 2033-D 870 (déficits restant à reporter) est la valeur fiscale de l'exercice \
         s'il est déficitaire.",
    );
    let disclaimer_v = b.bind(DISCLAIMER);
    let mut rows = String::new();
    for entry in &export.entries {
        let form_v = b.bind(entry.form);
        let case_v = b.bind(entry.case);
        let label_v = b.bind(&entry.label);
        let amount = euro_label(entry.amount_euros);
        let amount_v = b.bind(&amount);
        let hint = format!("saisir {amount} en case {} du {}", entry.case, entry.form);
        let hint_v = b.bind(&hint);
        rows.push_str(&format!(
            "[#{form_v}], [#{case_v}], [#{label_v}], [#{amount_v}], [#{hint_v}],\n"
        ));
    }
    for mention in &export.mentions {
        let form_v = b.bind(mention.form);
        let case_v = b.bind("—");
        let label_v = b.bind(mention.text);
        let amount_v = b.bind("—");
        let hint_v = b.bind("cocher néant");
        rows.push_str(&format!(
            "[#{form_v}], [#{case_v}], [#{label_v}], [#{amount_v}], [#{hint_v}],\n"
        ));
    }
    let mut gap_rows = String::new();
    for gap in &export.discrepancies {
        let subject_v = b.bind(gap.subject);
        let snapshot_v = b.bind(&Money::from_cents(gap.snapshot_cents).to_string());
        let book_v = b.bind(&Money::from_cents(gap.ledger_cents).to_string());
        gap_rows.push_str(&format!("[#{subject_v}], [#{snapshot_v}], [#{book_v}],\n"));
    }
    let gaps = if export.discrepancies.is_empty() {
        String::new()
    } else {
        let heading_v = b.bind("Écart entre le snapshot de clôture et le livre");
        format!(
            r#"
#v(1em)
#text(weight: "bold")[#{heading_v}]
#v(0.4em)
#table(
  columns: (2fr, 1fr, 1fr),
  align: (left, right, right),
  stroke: 0.5pt,
  [*Sujet*], [*Snapshot*], [*Livre*],
  {gap_rows}
)
"#
        )
    };
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
  [*Formulaire*], [*Case*], [*Libellé*], [*Euros*], [*Consigne*],
  {rows}
)
{gaps}
#v(1.5em)
#line(length: 100%, stroke: 0.5pt)
#v(0.5em)
#text(size: 8pt)[#{disclaimer_v}]
"#,
        declarations = b.declarations,
        gaps = gaps,
    );
    compile(&source)
}
