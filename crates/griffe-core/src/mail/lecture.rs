//! Phrases montrées pendant que la boîte lit le compte. Aucune socket.

/// Dossier en cours de lecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LectureFolder {
    /// Boîte de réception.
    Reception,
    /// Dossier des lettres parties.
    Envoyees,
}

/// Où en est la relève, dans la mémoire de la fenêtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LecturePhase {
    /// La liaison s'ouvre.
    Opening,
    /// Un dossier est en cours de lecture.
    Reading {
        /// Réception ou envoyées.
        folder: LectureFolder,
        /// Messages déjà analysés.
        read: u32,
        /// `EXISTS`, dès que `SELECT` l'a donné.
        total: Option<u32>,
    },
    /// Le miroir a grandi. Une seule réponse, puis le repos.
    Arrived {
        /// Lettres nouvelles de ce passage.
        fresh: u32,
    },
    /// Au moins une lettre se pose sur un dossier. Une seule réponse.
    Filing {
        /// Lettres posées sur un dossier.
        on_dossier: u32,
    },
    /// Le compte n'a pas répondu.
    Failed,
    /// Rien n'est en cours.
    Idle,
}

/// Ce que la page demande à la phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LectureAsk {
    /// Phase en mémoire.
    pub phase: LecturePhase,
    /// Adresse du compte. Pas un secret.
    pub address: String,
    /// Le miroir n'a encore aucune lettre.
    pub mirror_empty: bool,
    /// `releve_plan` a un compte lisible.
    pub readable: bool,
}

/// Ce que le sondage emporte. Le paraphe n'est pas dans le bloc sondé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LectureMode {
    /// Grand paraphe, miroir vide, lecture en cours.
    Glyph,
    /// Ligne calme, ou phrase qui remplace celle du plateau pour un instant.
    Quiet,
    /// Le compte n'a pas répondu.
    Fail,
    /// Le compte ne se lit pas.
    Mute,
    /// Repos.
    Idle,
}

impl LectureMode {
    /// Valeur de l'URL du sondage.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Glyph => "glyph",
            Self::Quiet => "quiet",
            Self::Fail => "fail",
            Self::Mute => "mute",
            Self::Idle => "idle",
        }
    }
}

/// Forme de la phrase. Les drapeaux du trait sont dans la variante.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LectureShow {
    /// Colonne de droite : mot, paraphe, mesure.
    Glyph {
        /// Phrase du plateau.
        sentence: String,
        /// Mesure, sous le trait.
        count: String,
        /// Adresse, dossier, ou nouvel essai.
        meta: String,
        /// Le trait s'arrête.
        stall: bool,
    },
    /// Ligne sous la phrase habituelle. Les lettres restent.
    Quiet {
        /// Texte de la ligne.
        line: String,
        /// Italique sceau.
        seal: bool,
    },
    /// Remplace la phrase du plateau.
    Replace {
        /// Phrase affichée.
        sentence: String,
        /// Lien vers Le courrier.
        link: bool,
    },
    /// La phrase habituelle, sans ligne.
    Rest,
}

/// Deux lignes françaises, et ce que la page en fait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LectureFace {
    /// Ce qui s'écrit.
    pub show: LectureShow,
    /// Le bloc se sonde encore.
    pub poll: bool,
    /// Phrase de la liste quand elle est vide.
    pub empty: &'static str,
    /// Mode emporté par l'URL.
    pub mode: LectureMode,
}

const EMPTY_WORKING: &str = "Les lettres arrivent.";
const EMPTY_FAILED: &str = "Aucune lettre pour l'instant.";
const EMPTY_REST: &str = "Aucune lettre dans cette vue.";
const UNREADABLE: &str = "Le compte ne se lit pas.";
const OPENING: &str = "Griffe ouvre le compte.";
const FAILED: &str = "Le compte n'a pas répondu.";
const FAILED_QUIET: &str = "Le compte n'a pas répondu. Nouvel essai dans un instant.";

/// Phrases de la lecture. `readable` faux l'emporte sur la phase.
#[must_use]
pub fn lecture_face(ask: &LectureAsk) -> LectureFace {
    if !ask.readable {
        return LectureFace {
            show: LectureShow::Replace {
                sentence: UNREADABLE.to_string(),
                link: true,
            },
            poll: false,
            empty: EMPTY_REST,
            mode: LectureMode::Mute,
        };
    }
    match ask.phase {
        LecturePhase::Idle
        | LecturePhase::Arrived { fresh: 0 }
        | LecturePhase::Filing { on_dossier: 0 } => rest(),
        LecturePhase::Opening if ask.mirror_empty => {
            glyph(OPENING, "Le compte", &ask.address, false)
        }
        LecturePhase::Opening => quiet(OPENING, false),
        LecturePhase::Reading {
            folder,
            read,
            total,
        } if ask.mirror_empty => {
            let (count, meta) = measure(folder, read, total, &ask.address);
            glyph(reading_sentence(folder), &count, &meta, false)
        }
        LecturePhase::Reading { folder, .. } => quiet(rereading(folder), false),
        LecturePhase::Arrived { fresh } => quiet_owned(arrived_line(fresh), false),
        LecturePhase::Filing { on_dossier } => LectureFace {
            show: LectureShow::Replace {
                sentence: filing_sentence(on_dossier),
                link: false,
            },
            poll: true,
            empty: EMPTY_REST,
            mode: LectureMode::Quiet,
        },
        LecturePhase::Failed if ask.mirror_empty => {
            let meta = format!("{} · nouvel essai dans un instant.", ask.address);
            LectureFace {
                show: LectureShow::Glyph {
                    sentence: FAILED.to_string(),
                    count: "Le compte".to_string(),
                    meta,
                    stall: true,
                },
                poll: true,
                empty: EMPTY_FAILED,
                mode: LectureMode::Fail,
            }
        }
        LecturePhase::Failed => {
            let mut face = quiet(FAILED_QUIET, true);
            face.mode = LectureMode::Fail;
            face
        }
    }
}

/// Annonce du `SELECT` ou d'un message analysé. Ni secret, ni corps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullTick {
    /// `inbox` ou `sent`, côté serveur.
    pub role: PullRole,
    /// Messages analysés dans ce dossier.
    pub read: u32,
    /// `EXISTS`. Absent tant que la réponse ne l'a pas dit.
    pub total: Option<u32>,
}

/// Dossier annoncé par la liaison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRole {
    /// Réception.
    Inbox,
    /// Envoyées.
    Sent,
}

impl PullRole {
    /// `sent` est le dossier des lettres parties. Le reste est la réception.
    #[must_use]
    pub fn parse(role: &str) -> Self {
        if role.eq_ignore_ascii_case("sent") {
            Self::Sent
        } else {
            Self::Inbox
        }
    }
}

fn rest() -> LectureFace {
    LectureFace {
        show: LectureShow::Rest,
        poll: false,
        empty: EMPTY_REST,
        mode: LectureMode::Idle,
    }
}

fn glyph(sentence: &str, count: &str, meta: &str, stall: bool) -> LectureFace {
    LectureFace {
        show: LectureShow::Glyph {
            sentence: sentence.to_string(),
            count: count.to_string(),
            meta: meta.to_string(),
            stall,
        },
        poll: true,
        empty: EMPTY_WORKING,
        mode: LectureMode::Glyph,
    }
}

fn quiet(line: &str, seal: bool) -> LectureFace {
    quiet_owned(line.to_string(), seal)
}

fn quiet_owned(line: String, seal: bool) -> LectureFace {
    LectureFace {
        show: LectureShow::Quiet { line, seal },
        poll: true,
        empty: EMPTY_REST,
        mode: LectureMode::Quiet,
    }
}

fn reading_sentence(folder: LectureFolder) -> &'static str {
    match folder {
        LectureFolder::Reception => "Griffe lit la réception.",
        LectureFolder::Envoyees => "Griffe lit les envoyées.",
    }
}

fn rereading(folder: LectureFolder) -> &'static str {
    match folder {
        LectureFolder::Reception => "Griffe relit la réception.",
        LectureFolder::Envoyees => "Griffe relit les envoyées.",
    }
}

fn folder_label(folder: LectureFolder) -> &'static str {
    match folder {
        LectureFolder::Reception => "La réception",
        LectureFolder::Envoyees => "Les envoyées",
    }
}

fn measure(
    folder: LectureFolder,
    read: u32,
    total: Option<u32>,
    address: &str,
) -> (String, String) {
    let label = folder_label(folder);
    match total {
        None => (label.to_string(), address.to_string()),
        Some(0) => ("Aucune lettre".to_string(), format!("{label} · {address}")),
        Some(total) => (
            format!("{read} sur {total}"),
            format!("{label} · {address}"),
        ),
    }
}

fn arrived_line(fresh: u32) -> String {
    if fresh == 1 {
        "Une lettre nouvelle.".to_string()
    } else {
        format!("{fresh} lettres nouvelles.")
    }
}

fn filing_sentence(count: u32) -> String {
    if count == 1 {
        "Une lettre se pose sur un dossier.".to_string()
    } else {
        format!("{count} se posent sur un dossier.")
    }
}

/// Ce que le passage vient de ranger, pour choisir la phrase d'après.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbsorbBeat {
    /// Lettres posées sur un dossier.
    pub on_dossier: u32,
    /// Lettres nouvelles du miroir.
    pub stored: u32,
    /// Le miroir était vide avant ce passage.
    pub was_empty: bool,
}

/// Phrase d'après le rangement. Zéro dossier et une boîte déjà pleine : les nouvelles.
/// Une boîte qui était vide ne dit pas « lettre nouvelle » : les cartes arrivent ensemble.
#[must_use]
pub fn beat_after_absorb(beat: AbsorbBeat) -> LecturePhase {
    if beat.on_dossier > 0 {
        LecturePhase::Filing {
            on_dossier: beat.on_dossier,
        }
    } else if !beat.was_empty && beat.stored > 0 {
        LecturePhase::Arrived { fresh: beat.stored }
    } else {
        LecturePhase::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LectureAsk, LectureFace, LectureFolder, LectureMode, LecturePhase, LectureShow,
        lecture_face,
    };

    fn ask(phase: LecturePhase, empty: bool, readable: bool) -> LectureAsk {
        LectureAsk {
            phase,
            address: "nicolas@lumen-conseil.fr".to_string(),
            mirror_empty: empty,
            readable,
        }
    }

    fn glyph(face: &LectureFace) -> (&str, &str, &str, bool) {
        match &face.show {
            LectureShow::Glyph {
                sentence,
                count,
                meta,
                stall,
            } => (sentence, count, meta, *stall),
            other => panic!("paraphe attendu, obtenu {other:?}"),
        }
    }

    #[test]
    fn an_unreadable_account_is_a_sentence_without_a_stroke() {
        let face = lecture_face(&ask(
            LecturePhase::Reading {
                folder: LectureFolder::Reception,
                read: 3,
                total: Some(14),
            },
            true,
            false,
        ));
        assert_eq!(
            face.show,
            LectureShow::Replace {
                sentence: "Le compte ne se lit pas.".to_string(),
                link: true,
            }
        );
        assert!(!face.poll);
        assert_eq!(face.empty, "Aucune lettre dans cette vue.");
        assert_eq!(face.mode, LectureMode::Mute);
    }

    #[test]
    fn opening_an_empty_mirror_names_the_account() {
        let face = lecture_face(&ask(LecturePhase::Opening, true, true));
        let (sentence, count, meta, stall) = glyph(&face);
        assert_eq!(sentence, "Griffe ouvre le compte.");
        assert_eq!(count, "Le compte");
        assert_eq!(meta, "nicolas@lumen-conseil.fr");
        assert!(!stall);
        assert!(face.poll);
        assert_eq!(face.empty, "Les lettres arrivent.");
        assert_eq!(face.mode, LectureMode::Glyph);
    }

    #[test]
    fn opening_a_filled_mirror_is_a_quiet_line() {
        let face = lecture_face(&ask(LecturePhase::Opening, false, true));
        assert_eq!(
            face.show,
            LectureShow::Quiet {
                line: "Griffe ouvre le compte.".to_string(),
                seal: false,
            }
        );
        assert!(face.poll);
        assert_eq!(face.mode, LectureMode::Quiet);
        assert_eq!(face.empty, "Aucune lettre dans cette vue.");
    }

    #[test]
    fn reading_the_inbox_counts_what_select_said() {
        let face = lecture_face(&ask(
            LecturePhase::Reading {
                folder: LectureFolder::Reception,
                read: 3,
                total: Some(14),
            },
            true,
            true,
        ));
        let (sentence, count, meta, stall) = glyph(&face);
        assert_eq!(sentence, "Griffe lit la réception.");
        assert_eq!(count, "3 sur 14");
        assert_eq!(meta, "La réception · nicolas@lumen-conseil.fr");
        assert!(!stall);
        assert_eq!(face.mode, LectureMode::Glyph);
    }

    #[test]
    fn reading_sent_mail_without_a_total_names_the_folder() {
        let face = lecture_face(&ask(
            LecturePhase::Reading {
                folder: LectureFolder::Envoyees,
                read: 0,
                total: None,
            },
            true,
            true,
        ));
        let (sentence, count, meta, _) = glyph(&face);
        assert_eq!(sentence, "Griffe lit les envoyées.");
        assert_eq!(count, "Les envoyées");
        assert_eq!(meta, "nicolas@lumen-conseil.fr");
    }

    #[test]
    fn an_empty_folder_says_there_is_no_letter() {
        let face = lecture_face(&ask(
            LecturePhase::Reading {
                folder: LectureFolder::Reception,
                read: 0,
                total: Some(0),
            },
            true,
            true,
        ));
        let (_, count, meta, _) = glyph(&face);
        assert_eq!(count, "Aucune lettre");
        assert_eq!(meta, "La réception · nicolas@lumen-conseil.fr");
    }

    #[test]
    fn rereading_keeps_the_open_letter() {
        let inbox = lecture_face(&ask(
            LecturePhase::Reading {
                folder: LectureFolder::Reception,
                read: 2,
                total: Some(2),
            },
            false,
            true,
        ));
        assert_eq!(
            inbox.show,
            LectureShow::Quiet {
                line: "Griffe relit la réception.".to_string(),
                seal: false,
            }
        );
        let sent = lecture_face(&ask(
            LecturePhase::Reading {
                folder: LectureFolder::Envoyees,
                read: 1,
                total: Some(4),
            },
            false,
            true,
        ));
        assert_eq!(
            sent.show,
            LectureShow::Quiet {
                line: "Griffe relit les envoyées.".to_string(),
                seal: false,
            }
        );
        assert_eq!(sent.mode, LectureMode::Quiet);
    }

    #[test]
    fn new_letters_are_counted_once() {
        let one = lecture_face(&ask(LecturePhase::Arrived { fresh: 1 }, false, true));
        assert_eq!(
            one.show,
            LectureShow::Quiet {
                line: "Une lettre nouvelle.".to_string(),
                seal: false,
            }
        );
        assert!(one.poll);
        let several = lecture_face(&ask(LecturePhase::Arrived { fresh: 3 }, false, true));
        assert_eq!(
            several.show,
            LectureShow::Quiet {
                line: "3 lettres nouvelles.".to_string(),
                seal: false,
            }
        );
        let none = lecture_face(&ask(LecturePhase::Arrived { fresh: 0 }, false, true));
        assert_eq!(none.show, LectureShow::Rest);
        assert!(!none.poll);
        assert_eq!(none.mode, LectureMode::Idle);
    }

    #[test]
    fn filing_replaces_the_habitual_sentence_for_one_beat() {
        let one = lecture_face(&ask(LecturePhase::Filing { on_dossier: 1 }, false, true));
        assert_eq!(
            one.show,
            LectureShow::Replace {
                sentence: "Une lettre se pose sur un dossier.".to_string(),
                link: false,
            }
        );
        assert!(one.poll);
        assert_eq!(one.mode, LectureMode::Quiet);
        let several = lecture_face(&ask(LecturePhase::Filing { on_dossier: 4 }, false, true));
        assert_eq!(
            several.show,
            LectureShow::Replace {
                sentence: "4 se posent sur un dossier.".to_string(),
                link: false,
            }
        );
        let none = lecture_face(&ask(LecturePhase::Filing { on_dossier: 0 }, true, true));
        assert_eq!(none.show, LectureShow::Rest);
        assert!(!none.poll);
        assert_eq!(none.empty, "Aucune lettre dans cette vue.");
    }

    #[test]
    fn a_silent_account_stalls_the_stroke_when_the_mirror_is_empty() {
        let face = lecture_face(&ask(LecturePhase::Failed, true, true));
        let (sentence, count, meta, stall) = glyph(&face);
        assert_eq!(sentence, "Le compte n'a pas répondu.");
        assert_eq!(count, "Le compte");
        assert_eq!(
            meta,
            "nicolas@lumen-conseil.fr · nouvel essai dans un instant."
        );
        assert!(stall);
        assert!(face.poll);
        assert_eq!(face.empty, "Aucune lettre pour l'instant.");
        assert_eq!(face.mode, LectureMode::Fail);
    }

    #[test]
    fn a_silent_account_stays_under_the_letters() {
        let face = lecture_face(&ask(LecturePhase::Failed, false, true));
        assert_eq!(
            face.show,
            LectureShow::Quiet {
                line: "Le compte n'a pas répondu. Nouvel essai dans un instant.".to_string(),
                seal: true,
            }
        );
        assert!(face.poll);
        assert_eq!(face.mode, LectureMode::Fail);
        assert_eq!(face.empty, "Aucune lettre dans cette vue.");
    }

    #[test]
    fn the_beat_after_absorb_skips_a_first_arrival_and_a_zero() {
        use super::{AbsorbBeat, LecturePhase, beat_after_absorb};
        assert_eq!(
            beat_after_absorb(AbsorbBeat {
                on_dossier: 0,
                stored: 3,
                was_empty: true,
            }),
            LecturePhase::Idle
        );
        assert_eq!(
            beat_after_absorb(AbsorbBeat {
                on_dossier: 2,
                stored: 3,
                was_empty: true,
            }),
            LecturePhase::Filing { on_dossier: 2 }
        );
        assert_eq!(
            beat_after_absorb(AbsorbBeat {
                on_dossier: 0,
                stored: 1,
                was_empty: false,
            }),
            LecturePhase::Arrived { fresh: 1 }
        );
        assert_eq!(
            beat_after_absorb(AbsorbBeat {
                on_dossier: 1,
                stored: 4,
                was_empty: false,
            }),
            LecturePhase::Filing { on_dossier: 1 }
        );
        assert_eq!(
            beat_after_absorb(AbsorbBeat {
                on_dossier: 0,
                stored: 0,
                was_empty: false,
            }),
            LecturePhase::Idle
        );
    }

    #[test]
    fn idle_is_the_habitual_sentence() {
        let face = lecture_face(&ask(LecturePhase::Idle, false, true));
        assert_eq!(face.show, LectureShow::Rest);
        assert!(!face.poll);
        assert_eq!(face.mode, LectureMode::Idle);
        assert_eq!(face.mode.as_str(), "idle");
        assert_eq!(face.empty, "Aucune lettre dans cette vue.");
    }
}
