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

/// Ce que le fichier donne, y compris quand il ne peut pas peindre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomSheet {
    /// Pas de fichier.
    Absent,
    /// Couleurs utilisables.
    Ready(CustomLetter),
    /// Le fichier est là, la lecture a échoué.
    Unreadable,
    /// Lignes manquantes, en trop, ou illisibles.
    IllFormed,
    /// L'encre ou le lien passe sous 4,5 sur le papier. `name` si `nom` se lit.
    Pale { name: Option<String> },
}

/// Un mot du choix, tel qu'il s'affiche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetterChoice {
    pub id: &'static str,
    pub label: String,
    /// Feuille du fichier, à marquer à part de Clair et de Sauge.
    pub own: bool,
    /// On peut la cocher. Une feuille illisible reste visible, et ne se coche pas.
    pub selectable: bool,
    /// Couleur du lien, pour la pastille. Vide si la feuille ne se peint pas.
    pub swatch: Option<String>,
}

/// Ligne sous le choix. `readable` dit si la pastille et le mot peuvent peindre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnSheetNote {
    pub readable: bool,
    pub sentence: String,
}

/// Clair, Sauge, et la feuille du fichier dès qu'il est là.
#[must_use]
pub fn letter_choices() -> Vec<LetterChoice> {
    sheet_choices(&custom_sheet())
}

/// Les mots du choix pour un état déjà lu.
#[must_use]
pub fn sheet_choices(sheet: &CustomSheet) -> Vec<LetterChoice> {
    let mut choices = vec![delivered("clair", "Clair"), delivered("sauge", "Sauge")];
    if let Some(choice) = own_choice(sheet) {
        choices.push(choice);
    }
    choices
}

fn delivered(id: &'static str, label: &str) -> LetterChoice {
    LetterChoice {
        id,
        label: label.to_string(),
        own: false,
        selectable: true,
        swatch: None,
    }
}

fn own_choice(sheet: &CustomSheet) -> Option<LetterChoice> {
    let (label, selectable, swatch) = match sheet {
        CustomSheet::Absent => return None,
        CustomSheet::Ready(letter) => {
            (letter.name.clone(), true, Some(letter.palette.link.clone()))
        }
        CustomSheet::Pale { name: Some(name) } => (name.clone(), false, None),
        CustomSheet::Pale { name: None } | CustomSheet::Unreadable | CustomSheet::IllFormed => {
            ("Votre feuille".to_string(), false, None)
        }
    };
    Some(LetterChoice {
        id: "fichier",
        label,
        own: true,
        selectable,
        swatch,
    })
}

/// Mot coché. Un fichier absent, ou illisible, ne laisse pas `fichier` coché.
#[must_use]
pub fn selected_sheet(theme: LetterTheme) -> &'static str {
    selected_id(theme, &custom_sheet())
}

/// Mot coché pour un état déjà lu.
#[must_use]
pub fn selected_id(theme: LetterTheme, sheet: &CustomSheet) -> &'static str {
    match theme {
        LetterTheme::Sauge => "sauge",
        LetterTheme::Fichier if matches!(sheet, CustomSheet::Ready(_)) => "fichier",
        LetterTheme::Clair | LetterTheme::Fichier => "clair",
    }
}

/// Phrase sous les mots. `selected` est le mot coché (`clair`, `sauge`, `fichier`).
#[must_use]
pub fn own_sheet_note(sheet: &CustomSheet, selected: &str) -> Option<OwnSheetNote> {
    let painted = match selected {
        "sauge" => "Sauge",
        "fichier" => "cette feuille",
        _ => "Clair",
    };
    let (readable, sentence) = match sheet {
        CustomSheet::Absent => return None,
        CustomSheet::Ready(_) if selected == "fichier" => (true, "Lisible.".to_string()),
        CustomSheet::Ready(_) => (true, format!("Lisible. La lettre est sur {painted}.")),
        CustomSheet::Unreadable => (
            false,
            format!("Illisible. Griffe ne peut pas lire le fichier. La lettre est sur {painted}."),
        ),
        CustomSheet::IllFormed => (
            false,
            format!(
                "Illisible. Il faut nom, marge, papier, encre, lien. La lettre est sur {painted}."
            ),
        ),
        CustomSheet::Pale { .. } => (
            false,
            format!(
                "Illisible. L'encre ou le lien est trop pâle sur le papier. La lettre est sur {painted}."
            ),
        ),
    };
    Some(OwnSheetNote { readable, sentence })
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
    match custom_sheet() {
        CustomSheet::Ready(letter) => Some(letter),
        CustomSheet::Absent
        | CustomSheet::Unreadable
        | CustomSheet::IllFormed
        | CustomSheet::Pale { .. } => None,
    }
}

/// Fichier lu, y compris quand il ne peut pas peindre.
///
/// Même chemin que [`read_custom_letter`].
#[must_use]
pub fn custom_sheet() -> CustomSheet {
    let Some(path) = letter_path() else {
        return CustomSheet::Absent;
    };
    match fs::read_to_string(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CustomSheet::Absent,
        Err(_) => CustomSheet::Unreadable,
        Ok(text) => sheet_from_text(&text),
    }
}

/// Interprète le texte d'une feuille, sans ouvrir de fichier.
#[must_use]
pub fn sheet_from_text(text: &str) -> CustomSheet {
    match parse_letter_file(text) {
        Ok(letter) => CustomSheet::Ready(letter),
        Err(LetterFileError::Contrast) => CustomSheet::Pale {
            name: loose_name(text),
        },
        Err(LetterFileError::Shape) => CustomSheet::IllFormed,
    }
}

fn loose_name(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "nom" {
            return checked_name(value);
        }
    }
    None
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
        CustomSheet, LetterFileError, LetterPalette, LetterTheme, MIN_CONTRAST, channel, contrast,
        own_sheet_note, parse_letter_file, selected_id, sheet_choices, sheet_from_text,
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
        let pale_sheet = sheet_from_text(pale);
        assert_eq!(
            pale_sheet,
            CustomSheet::Pale {
                name: Some("Atelier".into())
            }
        );
        let choices = sheet_choices(&pale_sheet);
        let own = choices.iter().find(|choice| choice.own).expect("feuille");
        assert_eq!(own.label, "Atelier");
        assert!(!own.selectable);
        assert!(own.swatch.is_none());
        let note = own_sheet_note(&pale_sheet, "clair").expect("ligne");
        assert!(!note.readable);
        assert!(note.sentence.contains("trop pâle"));
        assert!(note.sentence.contains("Clair"));
    }

    #[test]
    fn a_readable_file_is_a_sheet_one_can_choose() {
        let text = "\
nom = Signal
marge = #f4f1ea
papier = #fbf9f5
encre = #221e18
lien = #2f5d38
";
        let sheet = sheet_from_text(text);
        let CustomSheet::Ready(letter) = &sheet else {
            panic!("feuille lisible");
        };
        assert_eq!(letter.name, "Signal");
        let choices = sheet_choices(&sheet);
        assert_eq!(choices.len(), 3);
        let own = &choices[2];
        assert!(own.own && own.selectable);
        assert_eq!(own.label, "Signal");
        assert_eq!(own.swatch.as_deref(), Some("#2f5d38"));
        assert_eq!(selected_id(LetterTheme::Fichier, &sheet), "fichier");
        assert_eq!(
            own_sheet_note(&sheet, "fichier").expect("ligne").sentence,
            "Lisible."
        );
        let waiting = own_sheet_note(&sheet, "clair").expect("ligne");
        assert!(waiting.readable);
        assert!(waiting.sentence.contains("La lettre est sur Clair."));
        assert!(own_sheet_note(&CustomSheet::Absent, "clair").is_none());
        assert!(
            own_sheet_note(&CustomSheet::IllFormed, "sauge")
                .expect("ligne")
                .sentence
                .contains("Sauge")
        );
        assert!(
            own_sheet_note(&CustomSheet::Unreadable, "clair")
                .expect("ligne")
                .sentence
                .contains("ne peut pas lire")
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
