//! Rendu PDF/A-3b : un document Typst généré en mémoire, compilé via le binaire `typst`
//! (`--pdf-standard a-3b`), avec le XML CII ([`crate::cii`]) embarqué comme pièce jointe
//! (`#pdf.attach`, profil Factur-X). Voir le spike de lot 10 : Typst 0.15+ produit nativement
//! du PDF/A-3b conforme avec fichier associé correctement enregistré (`/AF` du catalogue) —
//! aucun repli `printpdf`/`lopdf` n'est nécessaire.
//!
//! Toute donnée dynamique (nom client, description de ligne…) est injectée comme littéral de
//! chaîne Typst (`#let x = "...";`) puis référencée dans le corps via `#x` : Typst affiche le
//! contenu d'une variable `str` tel quel, sans le réinterpréter comme balisage — c'est ce qui
//! protège contre l'injection de balisage Typst depuis une donnée métier, pas un filtrage
//! ad hoc de caractères. Les tableaux (lignes de facture, ventilation TVA) sont construits en
//! générant une variable Typst nommée par ligne côté Rust plutôt qu'en bouclant côté Typst : ça
//! évite de parier sur une syntaxe de boucle/fermeture Typst qu'on ne peut pas vérifier ici
//! aussi facilement que le squelette déjà validé par le spike.

use griffe_core::company::CompanyProfile;
use griffe_core::domain::{Address, Client, Invoice, InvoiceLine, VatRate, format_date};

use crate::cii::cii_xml;
use crate::error::InvoiceError;

fn typst_command() -> std::process::Command {
    if let Some(explicit) = std::env::var_os("GRIFFE_TYPST") {
        return std::process::Command::new(explicit);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for name in ["typst", "typst-x86_64-unknown-linux-gnu"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return std::process::Command::new(candidate);
            }
        }
    }
    std::process::Command::new("typst")
}

fn typst_string(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

fn vat_rate_label(rate: VatRate) -> &'static str {
    match rate {
        VatRate::Standard => "20,0 %",
        VatRate::Intermediate => "10,0 %",
        VatRate::Reduced => "5,5 %",
        VatRate::SuperReduced => "2,1 %",
        VatRate::Zero => "0,0 %",
    }
}

/// Deux lignes plutôt qu'une chaîne unique contenant un saut de ligne : une valeur `str`
/// insérée via `#var` est affichée comme du texte contigu — un `\n` dans la donnée ne produit
/// pas de saut visuel. Le saut de ligne est un `\` de balisage Typst placé *entre* deux
/// références `#var`, jamais *dans* une valeur de chaîne.
fn address_lines(address: &Address) -> [String; 2] {
    [
        address.street.clone(),
        format!(
            "{} {}, {}",
            address.postal_code, address.city, address.country
        ),
    ]
}

fn legal_mentions_lines(company: &CompanyProfile, payment_terms_days: Option<u32>) -> Vec<String> {
    let mut lines = vec![format!(
        "{}{} — SIREN {}",
        company.legal_form,
        company
            .share_capital
            .map_or_else(String::new, |c| format!(" au capital de {c}")),
        company.siren
    )];
    if let Some(vat) = &company.vat_number {
        lines.push(format!("TVA intracommunautaire : {vat}"));
    }
    if let Some(city) = &company.rcs_city {
        lines.push(format!("RCS {city} {}", company.siren));
    }
    if let Some(days) = payment_terms_days {
        lines.push(format!("Délai de paiement : {days} jours"));
    }
    lines.push(
        "En cas de retard de paiement, une pénalité au taux de 3 fois le taux d'intérêt légal \
         sera appliquée, ainsi qu'une indemnité forfaitaire pour frais de recouvrement de 40 €, \
         conformément aux articles L441-10 et D441-5 du Code de commerce."
            .to_string(),
    );
    lines
}

/// Accumule des déclarations `#let nom = "valeur";` et retourne leur nom, pour être référencées
/// ensuite dans le corps du document via `#nom` sans jamais réinterpoler du texte brut dans du
/// balisage.
struct Bindings {
    declarations: String,
    counter: usize,
}

impl Bindings {
    fn new() -> Self {
        Self {
            declarations: String::new(),
            counter: 0,
        }
    }

    fn bind(&mut self, value: &str) -> String {
        let name = format!("v{}", self.counter);
        self.counter += 1;
        self.declarations
            .push_str(&format!("#let {name} = {};\n", typst_string(value)));
        name
    }
}

fn typst_source(
    invoice: &Invoice,
    client: &Client,
    company: &CompanyProfile,
    payment_terms_days: Option<u32>,
) -> String {
    let title = if invoice.credited_invoice_id.is_some() {
        "AVOIR"
    } else {
        "FACTURE"
    };
    let totals = griffe_core::billing::compute_totals(&invoice.lines);
    let [client_addr_line1, client_addr_line2] = client
        .address
        .as_ref()
        .map_or_else(|| [String::new(), String::new()], address_lines);
    let [seller_addr_line1, seller_addr_line2] = address_lines(&company.address);

    let mut b = Bindings::new();
    let title_v = b.bind(title);
    let number_v = b.bind(&invoice.number);
    let issued_v = b.bind(&format_date(invoice.issued_on));
    let due_v = b.bind(&format_date(invoice.due_on));
    let seller_name_v = b.bind(&company.name);
    let seller_addr1_v = b.bind(&seller_addr_line1);
    let seller_addr2_v = b.bind(&seller_addr_line2);
    let client_name_v = b.bind(&client.name);
    let client_addr1_v = b.bind(&client_addr_line1);
    let client_addr2_v = b.bind(&client_addr_line2);
    let subtotal_v = b.bind(&totals.subtotal_ht.to_string());
    let total_ttc_v = b.bind(&totals.total_ttc.to_string());

    let line_cells: Vec<[String; 5]> = invoice
        .lines
        .iter()
        .map(|line: &InvoiceLine| {
            let total = line.unit_price.multiply_by_quantity(line.quantity);
            [
                b.bind(&line.description),
                b.bind(&format!("{:.2}", line.quantity)),
                b.bind(&line.unit_price.to_string()),
                b.bind(vat_rate_label(line.vat_rate)),
                b.bind(&total.to_string()),
            ]
        })
        .collect();

    let vat_cells: Vec<[String; 3]> = totals
        .vat_breakdown
        .iter()
        .map(|line| {
            [
                b.bind(vat_rate_label(line.rate)),
                b.bind(&line.taxable_amount.to_string()),
                b.bind(&line.vat_amount.to_string()),
            ]
        })
        .collect();

    let mention_vars: Vec<String> = legal_mentions_lines(company, payment_terms_days)
        .iter()
        .map(|line| b.bind(line))
        .collect();

    let line_table_cells: String = line_cells
        .iter()
        .map(|[d, q, p, v, t]| format!("[#{d}], [#{q}], [#{p}], [#{v}], [#{t}],\n"))
        .collect();
    let vat_table_rows: String = vat_cells
        .iter()
        .map(|[rate, basis, amount]| format!("[TVA #{rate} sur #{basis}], [#{amount}],\n"))
        .collect();
    let mentions_body: String = mention_vars.iter().map(|v| format!("#{v}\\\n")).collect();

    format!(
        r#"{declarations}
#set document(title: {title_v}, author: {seller_name_v})
#set page(margin: 2.2cm)
#set text(size: 10pt)

#pdf.attach(
  "invoice.xml",
  relationship: "alternative",
  mime-type: "text/xml",
  description: "Facture électronique CII (EN 16931)",
)

#grid(
  columns: (1fr, 1fr),
  [
    #text(weight: "bold", size: 14pt)[#{seller_name_v}] \
    #{seller_addr1_v} \
    #{seller_addr2_v}
  ],
  align(right)[
    #text(weight: "bold", size: 18pt)[#{title_v} #{number_v}] \
    Émise le #{issued_v} \
    Échéance le #{due_v}
  ],
)

#v(1em)
*Facturé à :* \
#{client_name_v} \
#{client_addr1_v} \
#{client_addr2_v}

#v(1.5em)

#table(
  columns: (2fr, 0.7fr, 1fr, 0.8fr, 1fr),
  align: (left, right, right, right, right),
  [*Description*], [*Qté*], [*PU HT*], [*TVA*], [*Total HT*],
  {line_table_cells}
)

#v(1em)
#align(right)[
  #table(
    columns: (auto, auto),
    align: (left, right),
    [Total HT], [#{subtotal_v}],
    {vat_table_rows}
    [*Total TTC*], [*#{total_ttc_v}*],
  )
]

#v(2em)
#line(length: 100%, stroke: 0.5pt)
#v(0.5em)
#text(size: 8pt)[
  {mentions_body}
]
"#,
        declarations = b.declarations,
    )
}

/// Compile la facture en PDF/A-3b avec le XML CII embarqué en pièce jointe (Factur-X).
///
/// `payment_terms_days` n'est porté que par `EmitInvoice` (lot 5), pas par `Invoice` — passez
/// `None` si vous ne l'avez pas sous la main : la mention de délai de paiement sera simplement
/// omise, le reste des mentions légales obligatoires reste présent.
///
/// # Errors
///
/// Retourne une erreur si le XML CII ne peut pas être sérialisé, si le binaire `typst` est
/// introuvable, ou si la compilation échoue.
pub fn render_pdf(
    invoice: &Invoice,
    client: &Client,
    company: &CompanyProfile,
    payment_terms_days: Option<u32>,
) -> Result<Vec<u8>, InvoiceError> {
    let xml = cii_xml(invoice, client, company)?;
    let source = typst_source(invoice, client, company, payment_terms_days);

    let workdir = tempfile::tempdir()?;
    std::fs::write(workdir.path().join("invoice.xml"), &xml)?;
    std::fs::write(workdir.path().join("invoice.typ"), &source)?;
    let output_path = workdir.path().join("invoice.pdf");

    let output = typst_command()
        .arg("compile")
        .arg("--pdf-standard")
        .arg("a-3b")
        .arg("invoice.typ")
        .arg(&output_path)
        .current_dir(workdir.path())
        .output()?;

    if !output.status.success() {
        return Err(InvoiceError::TypstFailed {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(std::fs::read(&output_path)?)
}

#[cfg(test)]
mod tests {
    use super::typst_command;

    #[test]
    fn typst_command_falls_back_to_bare_typst_name() {
        let prog = typst_command().get_program().to_string_lossy().into_owned();
        assert!(
            prog == "typst"
                || prog.ends_with("/typst")
                || prog.ends_with("typst-x86_64-unknown-linux-gnu"),
            "programme typst inattendu : {prog}"
        );
    }
}
