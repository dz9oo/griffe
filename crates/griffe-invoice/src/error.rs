//! Erreurs du rendu de facture (XML CII, PDF/A-3b Factur-X).

use std::string::FromUtf8Error;

use quick_xml::Error as QuickXmlError;

#[derive(Debug, thiserror::Error)]
pub enum InvoiceError {
    #[error("échec d'écriture XML : {0}")]
    Xml(#[from] QuickXmlError),

    #[error("XML CII non-UTF8 (ne devrait jamais arriver) : {0}")]
    Utf8(#[from] FromUtf8Error),

    #[error("échec d'exécution de typst : {0}")]
    TypstIo(#[from] std::io::Error),

    #[error("typst a échoué (code {code:?}) :\n{stderr}")]
    TypstFailed { code: Option<i32>, stderr: String },
}
