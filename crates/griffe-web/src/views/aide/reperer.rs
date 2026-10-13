//! Se repérer : les trois pièces, et le coffre.

use super::{Letter, Rayon};

pub(super) const PIECES: Letter = Letter {
    slug: "pieces",
    kicker: "Se repérer",
    title: "Les trois pièces.",
    lede: "Griffe est une lettre du matin. Trois pièces suffisent. L'aide n'en est pas une.",
    steps: &[
        "Le jour dit quoi faire aujourd'hui, et où en est l'argent.",
        "Les affaires dit avec qui vous en êtes. Un nom ouvre le dossier.",
        "La société dit si cela tient : la piste, le relevé, ce que vous vous versez, les échéances, la clôture, les réglages.",
    ],
    does_not: Some("Il n'y a pas d'écran Factures, ni Devis, ni Banque."),
    href: Some("/jour"),
    action: Some("Aller au jour"),
    rayon: Rayon::Atelier,
};

pub(super) const COFFRE: Letter = Letter {
    slug: "coffre",
    kicker: "Se repérer",
    title: "Le coffre.",
    lede: "Tout est sur cette machine, chiffré. La passphrase n'est écrite nulle part.",
    steps: &[
        "À l'ouverture, la case dit : « Pendant 12 h, ne pas redemander la passphrase, et laisser la fenêtre ouverte. » Cochée, et le trousseau accepte la clé, la fenêtre reste ouverte pendant ces douze heures, y compris si vous la rouvrez.",
        "Verrouiller, dans le pied, ferme tout de suite et oublie la clé.",
        "Sans la case, ou si le trousseau refuse la clé, quinze minutes sans geste referment le coffre. Le journal qui se rafraîchit ne compte pas comme un geste.",
        "Le pied nomme la version, à côté de l'état du coffre. L'écran verrouillé la nomme sous la carte.",
    ],
    does_not: Some("Griffe ne demande pas la passphrase dans une lettre, ni dans l'aide."),
    href: None,
    action: None,
    rayon: Rayon::Atelier,
};
