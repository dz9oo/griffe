//! Les erreurs du cœur en langage courant (lot 39). `AppError::Domain` porte un préfixe
//! technique (« règle métier violée : ») utile en CLI, pas dans un bandeau ; et quelques refus
//! typés appellent un geste précis, nommé à la suite du message — la fenêtre n'a pas
//! d'identifiant d'erreur à matcher (les erreurs typées du cœur sont aplaties en texte à la
//! frontière `AppError`), elle reconnaît les refus par leur phrase.

use freeflow_core::app::AppError;

/// Le geste qui répond à un refus, quand la fenêtre le connaît : `(libellé, écran)`.
#[must_use]
pub fn action(e: &AppError) -> Option<(&'static str, &'static str)> {
    let text = match e {
        AppError::Domain(t) => t.as_str(),
        AppError::Conflict { .. } => return Some(("rechargez la fiche", "")),
        _ => return None,
    };
    if text.contains("exercice déjà clôturé") || text.contains("est déjà approuvé") {
        return Some(("voir les exercices clos", "/societe/cloture"));
    }
    if text.contains("bilan d'ouverture") && text.contains("figé") {
        return Some(("voir les exercices clos", "/societe/cloture"));
    }
    if text.contains("aucun profil d'entreprise") {
        return Some((
            "renseigner le profil dans l'écran société",
            "/societe/identite",
        ));
    }
    if text.contains("rapproch") {
        return Some(("voir les dépenses et le relevé", "/societe/releve"));
    }
    if text.contains("email avec lequel vous écrivez") {
        return Some(("indiquer l'expéditeur", "/view/relances"));
    }
    None
}

/// La phrase à afficher, sans préfixe technique, suivie du geste qui y répond s'il y en a un.
#[must_use]
pub fn message(e: &AppError) -> String {
    let base = match e {
        AppError::Domain(text) => text.clone(),
        AppError::Conflict { .. } => {
            format!("{e} — quelqu'un d'autre (ou un autre écran) a modifié cet élément.")
        }
        other => other.to_string(),
    };
    match action(e) {
        Some((label, _)) if !base.contains(label) => format!("{base} → {label}"),
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_error_loses_its_technical_prefix_and_names_the_next_gesture() {
        let e = AppError::Domain("aucun profil d'entreprise défini : renseignez-le d'abord".into());
        let text = message(&e);
        assert!(!text.starts_with("règle métier"), "{text}");
        assert!(
            text.ends_with("→ renseigner le profil dans l'écran société"),
            "{text}"
        );
        assert_eq!(action(&AppError::Domain("autre chose".into())), None);
    }
}
