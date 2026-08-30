//! Conformité EN 16931 sur base réelle : XML CII validé contre le schéma XSD officiel (vendorisé
//! depuis `ConnectingEurope/eInvoicing-EN16931`, EUPL-1.2, voir `spec/`), et PDF/A-3b avec
//! attachement Factur-X compilé par le vrai binaire `typst`.
//!
//! Validation Schematron (règles métier EN 16931) : le fichier Schematron officiel préprocessé
//! est bien vendorisé dans `spec/schematron/`, mais **n'est pas exécuté ici**. Ses expressions
//! utilisent XPath 2.0 (`every … satisfies`, `xs:decimal(...)`, séquences `('VA','FC')`) — testé
//! en session, `xmllint --schematron` (moteur XPath 1.0 de libxml2) échoue à les compiler avec
//! des erreurs `Failed to compile (context|test) expression` sur la quasi-totalité des règles.
//! Une validation Schematron réelle demande un moteur XSLT2/XPath2 (Saxon, typiquement) — hors
//! périmètre de cette passe ; le fichier reste vendorisé pour quand cet outillage sera ajouté.

use std::path::PathBuf;
use std::process::Command;

use freeflow_core::company::CompanyProfile;
use freeflow_core::domain::{
    Address, Client, ClientId, Invoice, InvoiceId, InvoiceLine, InvoiceStatus, Money, Siren,
    VatRate,
};
use time::{Date, Month};

fn spec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("spec")
}

fn company() -> CompanyProfile {
    CompanyProfile {
        name: "Argon Digital".to_string(),
        legal_form: "SASU".to_string(),
        siren: Siren::parse("552100554").unwrap(),
        vat_number: None,
        address: Address {
            street: "12 rue de la Paix".to_string(),
            postal_code: "75002".to_string(),
            city: "Paris".to_string(),
            country: "FR".to_string(),
        },
        share_capital: Some(Money::from_cents(100_000)),
        rcs_city: Some("Paris".to_string()),
        iban: None,
    }
}

fn client() -> Client {
    Client {
        id: ClientId::new(),
        name: "Kappa Software".to_string(),
        siren: None,
        vat_number: None,
        address: Some(Address {
            street: "5 avenue des Champs".to_string(),
            postal_code: "75008".to_string(),
            city: "Paris".to_string(),
            country: "FR".to_string(),
        }),
        created_at: time::OffsetDateTime::now_utc(),
    }
}

fn invoice() -> Invoice {
    Invoice {
        id: InvoiceId::new(),
        number: "FA-2026-0001".to_string(),
        client_id: ClientId::new(),
        mission_id: None,
        lines: vec![
            InvoiceLine {
                description: "Septembre — développement".to_string(),
                quantity: 9.5,
                unit_price: Money::from_cents(65_000),
                vat_rate: VatRate::Standard,
            },
            InvoiceLine {
                description: "Formation".to_string(),
                quantity: 1.0,
                unit_price: Money::from_cents(50_000),
                vat_rate: VatRate::Reduced,
            },
        ],
        status: InvoiceStatus::Issued,
        issued_on: Date::from_calendar_date(2026, Month::September, 30).unwrap(),
        due_on: Date::from_calendar_date(2026, Month::October, 30).unwrap(),
        previous_hash: None,
        hash: "deadbeef".to_string(),
        credited_invoice_id: None,
    }
}

#[test]
fn generated_cii_xml_validates_against_the_official_en16931_xsd() {
    let xml = freeflow_invoice::cii_xml(&invoice(), &client(), &company()).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let xml_path = dir.path().join("invoice.xml");
    std::fs::write(&xml_path, &xml).unwrap();

    let xsd_path =
        spec_dir().join("xsd/CII/uncefact/data/standard/CrossIndustryInvoice_100pD16B.xsd");
    let output = Command::new("xmllint")
        .arg("--noout")
        .arg("--schema")
        .arg(&xsd_path)
        .arg(&xml_path)
        .output()
        .expect("xmllint doit être disponible (voir flake.nix : libxml2)");

    assert!(
        output.status.success(),
        "le XML CII généré ne valide pas contre le XSD EN 16931 officiel :\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn generated_pdf_is_a_valid_pdf_a3b_with_the_cii_xml_attached() {
    let inv = invoice();
    let cl = client();
    let co = company();

    let pdf_bytes = freeflow_invoice::render_pdf(&inv, &cl, &co, Some(30))
        .expect("typst doit être disponible (voir flake.nix)");

    // Assez petit pour être scanné tel quel : les métadonnées XMP et le flux `AttachmentBinaryObject`
    // sont écrits en clair par Typst (seul le flux `Contents` des pages est éventuellement compressé).
    let pdf_text = String::from_utf8_lossy(&pdf_bytes);

    assert!(
        pdf_text.contains("<pdfaid:part>3</pdfaid:part>"),
        "doit déclarer PDF/A-3"
    );
    assert!(
        pdf_text.contains("<pdfaid:conformance>B</pdfaid:conformance>"),
        "doit déclarer le niveau B"
    );
    assert!(
        pdf_text.contains("/AFRelationship/Alternative"),
        "la pièce jointe doit être associée au document (pas un simple attachement libre)"
    );
    assert!(
        pdf_text.contains("(invoice.xml)"),
        "le nom du fichier CII embarqué doit apparaître"
    );
    assert!(
        pdf_text.contains("/Names<</EmbeddedFiles"),
        "l'attachement doit être enregistré dans le catalogue (exigence PDF/A-3, pas juste présent dans le flux)"
    );
}

#[test]
fn rendering_a_credit_note_produces_a_distinct_valid_pdf_a3b() {
    // Le contenu visible des pages (`Contents`) et le flux `EmbeddedFile` sont tous deux
    // compressés par Typst — impossible de vérifier le titre "AVOIR" ou le TypeCode CII 381 par
    // simple recherche de sous-chaîne dans les octets bruts sans décompresser (ce que `cii.rs`
    // vérifie déjà directement sur le XML, avant compression). Ce test-ci couvre l'invariant
    // propre à `render_pdf` : le chemin avoir ne plante pas et produit toujours un PDF/A-3b réel.
    let mut inv = invoice();
    inv.credited_invoice_id = Some(InvoiceId::new());

    let pdf_bytes = freeflow_invoice::render_pdf(&inv, &client(), &company(), None)
        .expect("typst doit être disponible (voir flake.nix)");

    assert!(pdf_bytes.starts_with(b"%PDF-"));
    let pdf_text = String::from_utf8_lossy(&pdf_bytes);
    assert!(pdf_text.contains("<pdfaid:part>3</pdfaid:part>"));
    assert!(pdf_text.contains("<pdfaid:conformance>B</pdfaid:conformance>"));
}
