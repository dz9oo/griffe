//! Erreurs du rendu des documents de clôture.

#[derive(Debug, thiserror::Error)]
pub enum DocsError {
    #[error("échec d'exécution de typst : {0}")]
    TypstIo(#[from] std::io::Error),

    #[error("typst a échoué (code {code:?}) :\n{stderr}")]
    TypstFailed { code: Option<i32>, stderr: String },
}
