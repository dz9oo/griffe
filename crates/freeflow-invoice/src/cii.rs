//! Génération du XML CII (Cross Industry Invoice, UN/CEFACT D16B) profil EN 16931 — le format
//! machine-lisible d'une facture électronique conforme, embarqué dans le PDF/A-3b (Factur-X,
//! [`crate::pdf`]).
//!
//! La structure (ordre des éléments, wrapping) suit le schéma XSD officiel vendorisé dans
//! `spec/xsd/` — voir `spec/reference_example.xml` pour un exemple annoté dont ce module
//! reprend directement l'agencement, réduit aux seuls champs que le domaine FreeFlow porte
//! réellement (pas de champs optionnels inventés : GTIN, contact acheteur, remises globales,
//! etc. n'existent pas encore dans notre modèle de données et ne sont donc pas émis).

use freeflow_core::billing::{VatBreakdownLine, compute_totals};
use freeflow_core::company::CompanyProfile;
use freeflow_core::domain::{Address, Client, Invoice, InvoiceLine, Money, VatRate};
use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};

use crate::error::InvoiceError;

const NS_RSM: &str = "urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100";
const NS_RAM: &str =
    "urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100";
const NS_UDT: &str = "urn:un:unece:uncefact:data:standard:UnqualifiedDataType:100";

/// Code ISO/IEC 6523 (ICD) identifiant le SIRENE français comme registre légal — c'est ce que
/// `SpecifiedLegalOrganization/ID[@schemeID]` doit porter pour qu'un SIREN soit reconnu par un
/// destinataire européen sans ambiguïté avec un autre registre national.
const SIREN_SCHEME_ID: &str = "0002";

/// # Errors
///
/// Retourne une erreur si l'écriture XML échoue (ne devrait jamais arriver en pratique : tout
/// le contenu est écrit dans un buffer en mémoire).
pub fn cii_xml(
    invoice: &Invoice,
    client: &Client,
    company: &CompanyProfile,
) -> Result<String, InvoiceError> {
    let totals = compute_totals(&invoice.lines);
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

    writer.write_event(Event::Decl(quick_xml::events::BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        None,
    )))?;

    let mut root = BytesStart::new("rsm:CrossIndustryInvoice");
    root.push_attribute(("xmlns:rsm", NS_RSM));
    root.push_attribute(("xmlns:ram", NS_RAM));
    root.push_attribute(("xmlns:udt", NS_UDT));
    writer.write_event(Event::Start(root))?;

    write_context(&mut writer)?;
    write_exchanged_document(&mut writer, invoice)?;

    writer.write_event(Event::Start(BytesStart::new(
        "rsm:SupplyChainTradeTransaction",
    )))?;
    for (index, line) in invoice.lines.iter().enumerate() {
        write_line_item(&mut writer, index + 1, line)?;
    }
    write_trade_agreement(&mut writer, client, company)?;
    writer.write_event(Event::Empty(BytesStart::new(
        "ram:ApplicableHeaderTradeDelivery",
    )))?;
    write_trade_settlement(
        &mut writer,
        invoice,
        &totals.vat_breakdown,
        totals.subtotal_ht,
        totals.total_vat,
        totals.total_ttc,
    )?;
    writer.write_event(Event::End(BytesEnd::new("rsm:SupplyChainTradeTransaction")))?;

    writer.write_event(Event::End(BytesEnd::new("rsm:CrossIndustryInvoice")))?;

    String::from_utf8(writer.into_inner()).map_err(InvoiceError::from)
}

fn el(writer: &mut Writer<Vec<u8>>, name: &str, text: &str) -> Result<(), InvoiceError> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

fn write_context(writer: &mut Writer<Vec<u8>>) -> Result<(), InvoiceError> {
    writer.write_event(Event::Start(BytesStart::new(
        "rsm:ExchangedDocumentContext",
    )))?;
    writer.write_event(Event::Start(BytesStart::new(
        "ram:GuidelineSpecifiedDocumentContextParameter",
    )))?;
    el(writer, "ram:ID", "urn:cen.eu:en16931:2017")?;
    writer.write_event(Event::End(BytesEnd::new(
        "ram:GuidelineSpecifiedDocumentContextParameter",
    )))?;
    writer.write_event(Event::End(BytesEnd::new("rsm:ExchangedDocumentContext")))?;
    Ok(())
}

/// 380 = facture commerciale, 381 = avoir — les seuls deux types de document que FreeFlow émet.
fn document_type_code(invoice: &Invoice) -> &'static str {
    if invoice.credited_invoice_id.is_some() {
        "381"
    } else {
        "380"
    }
}

fn write_exchanged_document(
    writer: &mut Writer<Vec<u8>>,
    invoice: &Invoice,
) -> Result<(), InvoiceError> {
    writer.write_event(Event::Start(BytesStart::new("rsm:ExchangedDocument")))?;
    el(writer, "ram:ID", &invoice.number)?;
    el(writer, "ram:TypeCode", document_type_code(invoice))?;
    writer.write_event(Event::Start(BytesStart::new("ram:IssueDateTime")))?;
    write_date(writer, invoice.issued_on)?;
    writer.write_event(Event::End(BytesEnd::new("ram:IssueDateTime")))?;
    writer.write_event(Event::End(BytesEnd::new("rsm:ExchangedDocument")))?;
    Ok(())
}

/// Format `102` = `AAAAMMJJ`, le seul format de date que le profil EN 16931 autorise.
fn write_date(writer: &mut Writer<Vec<u8>>, date: time::Date) -> Result<(), InvoiceError> {
    let mut el_start = BytesStart::new("udt:DateTimeString");
    el_start.push_attribute(("format", "102"));
    writer.write_event(Event::Start(el_start))?;
    let formatted = format!(
        "{:04}{:02}{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    );
    writer.write_event(Event::Text(BytesText::new(&formatted)))?;
    writer.write_event(Event::End(BytesEnd::new("udt:DateTimeString")))?;
    Ok(())
}

fn money_decimal(amount: Money) -> String {
    let cents = amount.cents();
    let sign = if cents < 0 { "-" } else { "" };
    let magnitude = cents.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

/// EN 16931 code de catégorie de TVA (BT-151 / UNCL5305, sous-ensemble utilisé par le profil) :
/// `S` = taux standard/réduit normal, `E` = exonéré. Notre domaine (lot 5) ne distingue pas
/// encore *pourquoi* un taux est nul — autoliquidation intracommunautaire (`K`), export (`G`)
/// ou exonération simple (`E`) partagent aujourd'hui le même `VatRate::Zero` : `E` est
/// l'approximation la plus défendable en attendant que le domaine porte cette distinction.
fn vat_category_code(rate: VatRate) -> &'static str {
    if rate == VatRate::Zero { "E" } else { "S" }
}

fn vat_rate_percent(rate: VatRate) -> String {
    format!("{:.1}", f64::from(rate.basis_points()) / 100.0)
}

fn write_line_item(
    writer: &mut Writer<Vec<u8>>,
    line_id: usize,
    line: &InvoiceLine,
) -> Result<(), InvoiceError> {
    writer.write_event(Event::Start(BytesStart::new(
        "ram:IncludedSupplyChainTradeLineItem",
    )))?;

    writer.write_event(Event::Start(BytesStart::new(
        "ram:AssociatedDocumentLineDocument",
    )))?;
    el(writer, "ram:LineID", &line_id.to_string())?;
    writer.write_event(Event::End(BytesEnd::new(
        "ram:AssociatedDocumentLineDocument",
    )))?;

    writer.write_event(Event::Start(BytesStart::new("ram:SpecifiedTradeProduct")))?;
    el(writer, "ram:Name", &line.description)?;
    writer.write_event(Event::End(BytesEnd::new("ram:SpecifiedTradeProduct")))?;

    writer.write_event(Event::Start(BytesStart::new(
        "ram:SpecifiedLineTradeAgreement",
    )))?;
    writer.write_event(Event::Start(BytesStart::new(
        "ram:NetPriceProductTradePrice",
    )))?;
    el(writer, "ram:ChargeAmount", &money_decimal(line.unit_price))?;
    writer.write_event(Event::End(BytesEnd::new("ram:NetPriceProductTradePrice")))?;
    writer.write_event(Event::End(BytesEnd::new("ram:SpecifiedLineTradeAgreement")))?;

    writer.write_event(Event::Start(BytesStart::new(
        "ram:SpecifiedLineTradeDelivery",
    )))?;
    let mut quantity_el = BytesStart::new("ram:BilledQuantity");
    quantity_el.push_attribute(("unitCode", "C62"));
    writer.write_event(Event::Start(quantity_el))?;
    writer.write_event(Event::Text(BytesText::new(&format!(
        "{:.2}",
        line.quantity
    ))))?;
    writer.write_event(Event::End(BytesEnd::new("ram:BilledQuantity")))?;
    writer.write_event(Event::End(BytesEnd::new("ram:SpecifiedLineTradeDelivery")))?;

    writer.write_event(Event::Start(BytesStart::new(
        "ram:SpecifiedLineTradeSettlement",
    )))?;
    writer.write_event(Event::Start(BytesStart::new("ram:ApplicableTradeTax")))?;
    el(writer, "ram:TypeCode", "VAT")?;
    el(writer, "ram:CategoryCode", vat_category_code(line.vat_rate))?;
    el(
        writer,
        "ram:RateApplicablePercent",
        &vat_rate_percent(line.vat_rate),
    )?;
    writer.write_event(Event::End(BytesEnd::new("ram:ApplicableTradeTax")))?;
    writer.write_event(Event::Start(BytesStart::new(
        "ram:SpecifiedTradeSettlementLineMonetarySummation",
    )))?;
    let line_total = line.unit_price.multiply_by_quantity(line.quantity);
    el(writer, "ram:LineTotalAmount", &money_decimal(line_total))?;
    writer.write_event(Event::End(BytesEnd::new(
        "ram:SpecifiedTradeSettlementLineMonetarySummation",
    )))?;
    writer.write_event(Event::End(BytesEnd::new(
        "ram:SpecifiedLineTradeSettlement",
    )))?;

    writer.write_event(Event::End(BytesEnd::new(
        "ram:IncludedSupplyChainTradeLineItem",
    )))?;
    Ok(())
}

fn write_party(
    writer: &mut Writer<Vec<u8>>,
    name: &str,
    siren: Option<&str>,
    vat_number: Option<&str>,
    address: Option<&Address>,
) -> Result<(), InvoiceError> {
    el(writer, "ram:Name", name)?;
    if let Some(siren) = siren {
        writer.write_event(Event::Start(BytesStart::new(
            "ram:SpecifiedLegalOrganization",
        )))?;
        let mut id_el = BytesStart::new("ram:ID");
        id_el.push_attribute(("schemeID", SIREN_SCHEME_ID));
        writer.write_event(Event::Start(id_el))?;
        writer.write_event(Event::Text(BytesText::new(siren)))?;
        writer.write_event(Event::End(BytesEnd::new("ram:ID")))?;
        writer.write_event(Event::End(BytesEnd::new("ram:SpecifiedLegalOrganization")))?;
    }
    if let Some(address) = address {
        writer.write_event(Event::Start(BytesStart::new("ram:PostalTradeAddress")))?;
        el(writer, "ram:PostcodeCode", &address.postal_code)?;
        el(writer, "ram:LineOne", &address.street)?;
        el(writer, "ram:CityName", &address.city)?;
        el(writer, "ram:CountryID", &address.country)?;
        writer.write_event(Event::End(BytesEnd::new("ram:PostalTradeAddress")))?;
    }
    if let Some(vat_number) = vat_number {
        writer.write_event(Event::Start(BytesStart::new(
            "ram:SpecifiedTaxRegistration",
        )))?;
        let mut id_el = BytesStart::new("ram:ID");
        id_el.push_attribute(("schemeID", "VA"));
        writer.write_event(Event::Start(id_el))?;
        writer.write_event(Event::Text(BytesText::new(vat_number)))?;
        writer.write_event(Event::End(BytesEnd::new("ram:ID")))?;
        writer.write_event(Event::End(BytesEnd::new("ram:SpecifiedTaxRegistration")))?;
    }
    Ok(())
}

fn write_trade_agreement(
    writer: &mut Writer<Vec<u8>>,
    client: &Client,
    company: &CompanyProfile,
) -> Result<(), InvoiceError> {
    writer.write_event(Event::Start(BytesStart::new(
        "ram:ApplicableHeaderTradeAgreement",
    )))?;

    writer.write_event(Event::Start(BytesStart::new("ram:SellerTradeParty")))?;
    write_party(
        writer,
        &company.name,
        Some(&company.siren.to_string()),
        company
            .vat_number
            .as_ref()
            .map(std::string::ToString::to_string)
            .as_deref(),
        Some(&company.address),
    )?;
    writer.write_event(Event::End(BytesEnd::new("ram:SellerTradeParty")))?;

    writer.write_event(Event::Start(BytesStart::new("ram:BuyerTradeParty")))?;
    write_party(
        writer,
        &client.name,
        client.siren.map(|s| s.to_string()).as_deref(),
        client
            .vat_number
            .as_ref()
            .map(std::string::ToString::to_string)
            .as_deref(),
        client.address.as_ref(),
    )?;
    writer.write_event(Event::End(BytesEnd::new("ram:BuyerTradeParty")))?;

    writer.write_event(Event::End(BytesEnd::new(
        "ram:ApplicableHeaderTradeAgreement",
    )))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)] // fonction interne, un paramètre par élément XML à écrire.
fn write_trade_settlement(
    writer: &mut Writer<Vec<u8>>,
    invoice: &Invoice,
    vat_breakdown: &[VatBreakdownLine],
    subtotal_ht: Money,
    total_vat: Money,
    total_ttc: Money,
) -> Result<(), InvoiceError> {
    writer.write_event(Event::Start(BytesStart::new(
        "ram:ApplicableHeaderTradeSettlement",
    )))?;
    el(writer, "ram:InvoiceCurrencyCode", "EUR")?;

    // Ordre imposé par le XSD (`ApplicableHeaderTradeSettlementType`) : la TVA d'en-tête
    // précède les conditions de paiement, qui précèdent elles-mêmes le récapitulatif monétaire.
    for line in vat_breakdown {
        writer.write_event(Event::Start(BytesStart::new("ram:ApplicableTradeTax")))?;
        el(
            writer,
            "ram:CalculatedAmount",
            &money_decimal(line.vat_amount),
        )?;
        el(writer, "ram:TypeCode", "VAT")?;
        el(
            writer,
            "ram:BasisAmount",
            &money_decimal(line.taxable_amount),
        )?;
        el(writer, "ram:CategoryCode", vat_category_code(line.rate))?;
        el(
            writer,
            "ram:RateApplicablePercent",
            &vat_rate_percent(line.rate),
        )?;
        writer.write_event(Event::End(BytesEnd::new("ram:ApplicableTradeTax")))?;
    }

    writer.write_event(Event::Start(BytesStart::new(
        "ram:SpecifiedTradePaymentTerms",
    )))?;
    writer.write_event(Event::Start(BytesStart::new("ram:DueDateDateTime")))?;
    write_date(writer, invoice.due_on)?;
    writer.write_event(Event::End(BytesEnd::new("ram:DueDateDateTime")))?;
    writer.write_event(Event::End(BytesEnd::new("ram:SpecifiedTradePaymentTerms")))?;

    writer.write_event(Event::Start(BytesStart::new(
        "ram:SpecifiedTradeSettlementHeaderMonetarySummation",
    )))?;
    el(writer, "ram:LineTotalAmount", &money_decimal(subtotal_ht))?;
    el(
        writer,
        "ram:TaxBasisTotalAmount",
        &money_decimal(subtotal_ht),
    )?;
    let mut tax_total_el = BytesStart::new("ram:TaxTotalAmount");
    tax_total_el.push_attribute(("currencyID", "EUR"));
    writer.write_event(Event::Start(tax_total_el))?;
    writer.write_event(Event::Text(BytesText::new(&money_decimal(total_vat))))?;
    writer.write_event(Event::End(BytesEnd::new("ram:TaxTotalAmount")))?;
    el(writer, "ram:GrandTotalAmount", &money_decimal(total_ttc))?;
    el(writer, "ram:DuePayableAmount", &money_decimal(total_ttc))?;
    writer.write_event(Event::End(BytesEnd::new(
        "ram:SpecifiedTradeSettlementHeaderMonetarySummation",
    )))?;

    writer.write_event(Event::End(BytesEnd::new(
        "ram:ApplicableHeaderTradeSettlement",
    )))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use freeflow_core::domain::{ClientId, InvoiceId, InvoiceStatus, Siren};
    use time::{Date, Month};

    use super::*;

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
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

    fn client(name: &str) -> Client {
        Client {
            id: ClientId::new(),
            name: name.to_string(),
            siren: None,
            vat_number: None,
            address: None,
            created_at: time::OffsetDateTime::now_utc(),
            revision: 1,
            archived_at: None,
        }
    }

    fn invoice(lines: Vec<InvoiceLine>, credited_invoice_id: Option<InvoiceId>) -> Invoice {
        Invoice {
            id: InvoiceId::new(),
            number: "FA-2026-0001".to_string(),
            client_id: ClientId::new(),
            mission_id: None,
            lines,
            status: InvoiceStatus::Issued,
            issued_on: date(2026, Month::September, 30),
            due_on: date(2026, Month::October, 30),
            previous_hash: None,
            hash: "deadbeef".to_string(),
            credited_invoice_id,
        }
    }

    fn two_rate_lines() -> Vec<InvoiceLine> {
        vec![
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
        ]
    }

    #[test]
    fn generated_xml_is_well_formed_and_declares_the_en16931_guideline() {
        let xml = cii_xml(
            &invoice(two_rate_lines(), None),
            &client("Kappa Software"),
            &company(),
        )
        .unwrap();
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("urn:cen.eu:en16931:2017"));

        // Bien formé : un lecteur XML doit pouvoir le parcourir sans erreur.
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Eof) => break,
                Err(e) => panic!("XML mal formé : {e}"),
                _ => {}
            }
            buf.clear();
        }
    }

    #[test]
    fn a_credit_note_uses_type_code_381_and_a_regular_invoice_uses_380() {
        let regular = cii_xml(
            &invoice(two_rate_lines(), None),
            &client("Kappa"),
            &company(),
        )
        .unwrap();
        assert!(regular.contains("<ram:TypeCode>380</ram:TypeCode>"));

        let credit_note = cii_xml(
            &invoice(two_rate_lines(), Some(InvoiceId::new())),
            &client("Kappa"),
            &company(),
        )
        .unwrap();
        assert!(credit_note.contains("<ram:TypeCode>381</ram:TypeCode>"));
    }

    #[test]
    fn header_totals_match_compute_totals_exactly() {
        let inv = invoice(two_rate_lines(), None);
        let totals = compute_totals(&inv.lines);
        let xml = cii_xml(&inv, &client("Kappa"), &company()).unwrap();

        assert!(xml.contains(&format!(
            "<ram:TaxBasisTotalAmount>{}</ram:TaxBasisTotalAmount>",
            money_decimal(totals.subtotal_ht)
        )));
        assert!(xml.contains(&format!(
            "<ram:GrandTotalAmount>{}</ram:GrandTotalAmount>",
            money_decimal(totals.total_ttc)
        )));
        // Une ligne ApplicableTradeTax d'en-tête par taux réellement présent sur la facture —
        // `BasisAmount` n'existe que dans ces lignes-là, pas dans la TVA au niveau ligne.
        assert_eq!(
            xml.matches("<ram:BasisAmount>").count(),
            totals.vat_breakdown.len(),
        );
    }

    #[test]
    fn a_client_name_containing_markup_characters_does_not_corrupt_the_xml() {
        let xml = cii_xml(
            &invoice(two_rate_lines(), None),
            &client("Kappa <Software> & \"Co\""),
            &company(),
        )
        .unwrap();

        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Eof) => break,
                Err(e) => {
                    panic!("un nom de client avec des caractères spéciaux a cassé le XML : {e}")
                }
                _ => {}
            }
            buf.clear();
        }
        assert!(
            xml.contains("Kappa &lt;Software&gt; &amp; &quot;Co&quot;"),
            "actual xml:\n{xml}"
        );
    }
}
