//! Feuille d'une lettre : deux couleurs livrées, et une lue dans un fichier.
//! Le parseur ne touche pas le disque. La lecture du fichier n'est pas une
//! `Command` : les trois adaptateurs et l'envoi partagent la même résolution,
//! et ces couleurs ne sont pas une donnée du coffre.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Feuille peinte sur la carte. Les couleurs sont `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetterPalette {
    pub margin: String,
    pub paper: String,
    pub ink: String,
    pub link: String,
}

impl LetterPalette {
    #[must_use]
    pub fn clair() -> Self {
        Self {
            margin: "#f6f0e4".into(),
            paper: "#f6f0e4".into(),
            ink: "#2c2418".into(),
            link: "#8a3e24".into(),
        }
    }

    #[must_use]
    pub fn sauge() -> Self {
        Self {
            margin: "#e5eee6".into(),
            paper: "#e5eee6".into(),
            ink: "#1c2c24".into(),
            link: "#1e6b45".into(),
        }
    }
}

/// Mot enregistré dans le coffre. `vert`, `encre` et `sceau` se lisent comme Clair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LetterTheme {
    #[default]
    Clair,
    Sauge,
    /// Couleurs du fichier de configuration, si elles sont lisibles.
    Fichier,
}

impl LetterTheme {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clair => "clair",
            Self::Sauge => "sauge",
            Self::Fichier => "fichier",
        }
    }

    /// Mot accepté à l'enregistrement. Les anciens mots deviennent Clair.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "clair" | "vert" | "encre" | "sceau" => Some(Self::Clair),
            "sauge" => Some(Self::Sauge),
            "fichier" => Some(Self::Fichier),
            _ => None,
        }
    }

    /// Mot déjà dans le coffre. Inconnu : Clair.
    #[must_use]
    pub fn read(value: &str) -> Self {
        Self::parse(value).unwrap_or(Self::Clair)
    }

    /// Couleurs à peindre. `fichier` sans feuille lisible retombe sur Clair.
    /// Clair et Sauge ne lisent pas le fichier.
    #[must_use]
    pub fn palette(self) -> LetterPalette {
        let custom = if self == Self::Fichier {
            read_custom_letter()
        } else {
            None
        };
        self.resolved(custom)
    }

    fn resolved(self, custom: Option<CustomLetter>) -> LetterPalette {
        match self {
            Self::Clair => LetterPalette::clair(),
            Self::Sauge => LetterPalette::sauge(),
            Self::Fichier => custom.map_or_else(LetterPalette::clair, |letter| letter.palette),
        }
    }
}

/// Feuille nommée par le fichier de configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomLetter {
    pub name: String,
    pub palette: LetterPalette,
}

/// Un mot du choix, tel qu'il s'affiche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetterChoice {
    pub id: &'static str,
    pub label: String,
}

/// Clair, Sauge, et le nom du fichier quand il est valide.
#[must_use]
pub fn letter_choices() -> Vec<LetterChoice> {
    let mut choices = vec![
        LetterChoice {
            id: "clair",
            label: "Clair".into(),
        },
        LetterChoice {
            id: "sauge",
            label: "Sauge".into(),
        },
    ];
    if let Some(custom) = read_custom_letter() {
        choices.push(LetterChoice {
            id: "fichier",
            label: custom.name,
        });
    }
    choices
}

/// Mot coché. Un fichier absent ne laisse pas `fichier` coché.
#[must_use]
pub fn selected_sheet(theme: LetterTheme) -> &'static str {
    match theme {
        LetterTheme::Sauge => "sauge",
        LetterTheme::Fichier if read_custom_letter().is_some() => "fichier",
        LetterTheme::Clair | LetterTheme::Fichier => "clair",
    }
}

/// Fichier illisible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetterFileError {
    /// Clé absente, ligne illisible, ou couleur qui n'est pas `#rrggbb`.
    Shape,
    /// L'encre ou le lien passe sous 4,5 sur le papier.
    Contrast,
}

/// Lit le texte d'une feuille. N'ouvre pas de fichier.
///
/// # Errors
///
/// La forme est incomplète, ou le contraste est insuffisant.
pub fn parse_letter_file(text: &str) -> Result<CustomLetter, LetterFileError> {
    let mut name = None;
    let mut margin = None;
    let mut paper = None;
    let mut ink = None;
    let mut lien = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(LetterFileError::Shape);
        };
        let key = key.trim();
        let value = value.trim();
        let slot = match key {
            "nom" => &mut name,
            "marge" => &mut margin,
            "papier" => &mut paper,
            "encre" => &mut ink,
            "lien" => &mut lien,
            _ => return Err(LetterFileError::Shape),
        };
        if slot.is_some() {
            return Err(LetterFileError::Shape);
        }
        let parsed = if key == "nom" {
            checked_name(value).ok_or(LetterFileError::Shape)?
        } else {
            normalize_hex(value).ok_or(LetterFileError::Shape)?
        };
        *slot = Some(parsed);
    }
    let (Some(name), Some(margin), Some(paper), Some(ink), Some(lien)) =
        (name, margin, paper, ink, lien)
    else {
        return Err(LetterFileError::Shape);
    };
    let paper_rgb = channel(&paper).ok_or(LetterFileError::Shape)?;
    let ink_rgb = channel(&ink).ok_or(LetterFileError::Shape)?;
    let lien_rgb = channel(&lien).ok_or(LetterFileError::Shape)?;
    if contrast(ink_rgb, paper_rgb) < MIN_CONTRAST || contrast(lien_rgb, paper_rgb) < MIN_CONTRAST {
        return Err(LetterFileError::Contrast);
    }
    Ok(CustomLetter {
        name,
        palette: LetterPalette {
            margin,
            paper,
            ink,
            link: lien,
        },
    })
}

/// Feuille du fichier, si elle existe et passe le contraste.
///
/// `GRIFFE_LETTRE` force le chemin. Vide ou `-`, rien n'est lu.
/// Sinon : `$XDG_CONFIG_HOME/griffe/lettre.toml`, ou `~/.config/griffe/lettre.toml`.
#[must_use]
pub fn read_custom_letter() -> Option<CustomLetter> {
    let path = letter_path()?;
    let text = fs::read_to_string(path).ok()?;
    parse_letter_file(&text).ok()
}

fn letter_path() -> Option<PathBuf> {
    match env::var("GRIFFE_LETTRE") {
        Ok(value) => {
            let value = value.trim();
            if value.is_empty() || value == "-" {
                None
            } else {
                Some(PathBuf::from(value))
            }
        }
        Err(_) => config_home().map(|dir| dir.join("griffe").join("lettre.toml")),
    }
}

fn config_home() -> Option<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME")
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    let home = env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config"))
}

fn checked_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_string())
}

fn normalize_hex(raw: &str) -> Option<String> {
    let (red, green, blue) = channel(raw)?;
    Some(format!("#{red:02x}{green:02x}{blue:02x}"))
}

fn channel(raw: &str) -> Option<(u8, u8, u8)> {
    let hex = raw.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    let byte = |shift| u8::try_from((value >> shift) & 0xff_u32).unwrap_or(0);
    Some((byte(16), byte(8), byte(0)))
}

const MIN_CONTRAST: f64 = 4.5;

fn contrast(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> f64 {
    let left = luminance(fg);
    let right = luminance(bg);
    let (hi, lo) = if left >= right {
        (left, right)
    } else {
        (right, left)
    };
    (hi + 0.05) / (lo + 0.05)
}

fn luminance((red, green, blue): (u8, u8, u8)) -> f64 {
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

fn linear(channel: u8) -> f64 {
    let unit = f64::from(channel) / 255.0;
    if unit <= 0.040_45 {
        unit / 12.92
    } else {
        ((unit + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LetterFileError, LetterPalette, LetterTheme, MIN_CONTRAST, channel, contrast,
        parse_letter_file,
    };

    #[test]
    fn old_words_read_as_clair_and_a_bad_word_is_refused() {
        assert_eq!(LetterTheme::read("vert"), LetterTheme::Clair);
        assert_eq!(LetterTheme::read("encre"), LetterTheme::Clair);
        assert_eq!(LetterTheme::read("sceau"), LetterTheme::Clair);
        assert_eq!(LetterTheme::read("sauge"), LetterTheme::Sauge);
        assert_eq!(LetterTheme::parse("fichier"), Some(LetterTheme::Fichier));
        assert_eq!(LetterTheme::parse("rouge"), None);
        assert_eq!(LetterTheme::Clair.as_str(), "clair");
    }

    #[test]
    fn a_letter_file_names_a_sheet_and_refuses_a_pale_link() {
        let text = "\
nom = Atelier
marge = #f4f1ea
papier = #fbf9f5
encre = #221e18
lien = #2f5d38
";
        let letter = parse_letter_file(text).expect("feuille");
        assert_eq!(letter.name, "Atelier");
        assert_eq!(letter.palette.link, "#2f5d38");
        assert_eq!(letter.palette.paper, "#fbf9f5");
        let pale = "\
nom = Atelier
marge = #f4f1ea
papier = #fbf9f5
encre = #221e18
lien = #8ec07c
";
        assert_eq!(parse_letter_file(pale), Err(LetterFileError::Contrast));
        assert!(parse_letter_file("nom = Atelier\n").is_err());
        assert_eq!(
            LetterTheme::Fichier.resolved(None).paper,
            LetterPalette::clair().paper
        );
    }

    #[test]
    fn clair_and_sauge_are_two_sheets() {
        let clair = LetterPalette::clair();
        let sauge = LetterPalette::sauge();
        assert_eq!(clair.margin, clair.paper);
        assert_eq!(sauge.margin, sauge.paper);
        assert_ne!(clair.paper, sauge.paper);
        assert_ne!(clair.link, sauge.link);
        for palette in [&clair, &sauge] {
            let paper = channel(&palette.paper).expect("papier");
            let ink = channel(&palette.ink).expect("encre");
            let lien = channel(&palette.link).expect("lien");
            assert!(contrast(ink, paper) >= MIN_CONTRAST);
            assert!(contrast(lien, paper) >= MIN_CONTRAST);
        }
    }
}
