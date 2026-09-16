//! Rendu tabulaire minimal pour la sortie humaine des commandes `list` — pas de nouvelle
//! dépendance, juste des colonnes alignées. `--json` reste le contrat machine, inchangé : ce
//! module n'est jamais utilisé quand `json` est actif.

/// Rend `rows` sous `headers` en colonnes alignées sur la largeur de leur contenu le plus long.
/// Renvoie un message dédié si `rows` est vide plutôt qu'un en-tête suivi de rien.
#[must_use]
pub fn render(headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return "(aucun résultat)".to_string();
    }

    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell.chars().count());
            }
        }
    }

    let mut out = String::new();
    push_row(&mut out, headers.iter().map(|h| (*h).to_string()), &widths);
    for row in rows {
        push_row(&mut out, row.iter().cloned(), &widths);
    }
    out.pop(); // retire le dernier saut de ligne
    out
}

fn push_row(out: &mut String, cells: impl Iterator<Item = String>, widths: &[usize]) {
    let padded: Vec<String> = cells
        .enumerate()
        .map(|(i, cell)| {
            format!(
                "{cell:<width$}",
                width = widths.get(i).copied().unwrap_or(0)
            )
        })
        .collect();
    out.push_str(padded.join("  ").trim_end());
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_columns_aligned_to_their_widest_cell() {
        let rendered = render(
            &["nom", "ville"],
            &[
                vec!["Argon Digital".to_string(), "Paris".to_string()],
                vec!["Ba".to_string(), "Lyon".to_string()],
            ],
        );
        assert_eq!(
            rendered,
            "nom            ville\nArgon Digital  Paris\nBa             Lyon"
        );
    }

    #[test]
    fn an_empty_table_says_so_rather_than_printing_a_bare_header() {
        assert_eq!(render(&["nom"], &[]), "(aucun résultat)");
    }
}
