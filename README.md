# Griffe

Alpha. Lettre du matin pour un indépendant en SASU à l’IS : local-only,
chiffré, sans serveur. Linux x86_64 pour l’instant.

Vitrine : [https://dz9oo.github.io/griffe/](https://dz9oo.github.io/griffe/).
Captures : [`site/shots/`](site/shots/).

## Aujourd’hui

Prospection, devis, missions, factures (nées ailleurs ou émises),
dépenses, relevé, relances en `.eml`, TVA au réel, se payer, clôture
indicative, coffre SQLCipher sur ta machine.

## Ce que ça ne fait pas

Pas d’avis fiscal, pas d’envoi de mail, pas de télétransmission, pas une
plateforme agréée. Griffe calcule et rappelle ; tu déposes ailleurs.

## Statut

Les tags `v0.*` sont de l’alpha. Aujourd’hui : **0.3.x**, Linux x86_64,
SASU à l’impôt sur les sociétés.

### Vers une v1.0.0

Même périmètre (SASU à l’IS, Linux). Le produit quitte l’alpha :

- parcours Atelier (copy, densité du mois, seuil d’alarme de la piste) ;
- facture née dans Griffe (Factur-X) pour un dépôt humain sur une PA, si
  on n’émet plus ailleurs ;
- mises à jour guidées (lien Releases, signatures), sans socket au
  démarrage ;
- paquet AUR `griffe-bin` soumis ;
- notes de version et `griffe --version` alignés.

### Ensuite

Même en v1.0.0 :

- Windows et macOS ;
- EURL comme régime distinct (gérant, TNS), micro-entreprise, impôt sur
  le revenu, multi-associés, stock, étranger ;
- EDI / télédéclaration, plateforme agréée, thème sombre, autre langue,
  coffre certifié NF Z42-013.

## Installation

Linux (x86_64) : télécharger l’AppImage de la
[dernière version](https://github.com/dz9oo/griffe/releases/latest),
`chmod +x`, puis l’installer dans `~/.local` et le menu / Walker :

```bash
chmod +x ./Griffe_*.AppImage
scripts/install-griffe.sh ./Griffe_*.AppImage
```

Le script copie l’AppImage vers `~/.local/bin/griffe` (un fichier, pas un
lien), écrit l’icône et la fiche `.desktop`. Pas de `sudo`, pas de réseau.
WebKit et Typst sont dans l’AppImage. Tes données restent dans
`~/.local/share/griffe/` : rejouer le script ne touche pas au coffre.
Un coffre déjà présent dans `~/.local/share/freeflow/` est déplacé vers
`griffe/` au premier lancement.

Mise à jour : télécharger le nouvel AppImage et rejouer le script (même
chemin). Désinstall : `rm` des trois chemins, coffre intact.

```bash
rm ~/.local/bin/griffe \
  ~/.local/share/applications/io.github.dz9oo.griffe.desktop \
  ~/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png
```

Sans le clone, copie [`scripts/install-griffe.sh`](scripts/install-griffe.sh)
depuis ce dépôt, ou colle les trois `install` / `cat` :

```sh
chmod +x ./Griffe_*.AppImage
install -D ./Griffe_*.AppImage "$HOME/.local/bin/griffe"
chmod +x "$HOME/.local/bin/griffe"
tmp=$(mktemp -d)
(cd "$tmp" && "$HOME/.local/bin/griffe" --appimage-extract >/dev/null)
install -D "$tmp"/squashfs-root/usr/share/icons/hicolor/256x256/apps/*.png \
  "$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png"
rm -rf "$tmp"
mkdir -p "$HOME/.local/share/applications"
cat >"$HOME/.local/share/applications/io.github.dz9oo.griffe.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Griffe
Exec=$HOME/.local/bin/griffe
Icon=io.github.dz9oo.griffe
StartupWMClass=Griffe-desktop
Terminal=false
Categories=Office;Finance;
EOF
```

### Arch / Omarchy (paquet pacman)

Daily driver : le `.pkg.tar.zst` de la Release. Si une install XDG
native (`~/.local/lib/griffe/`) existe, la retirer **avant** `pacman -U`
(même id de fiche `io.github.dz9oo.griffe.desktop`) :

```bash
rm -f ~/.local/bin/griffe-desktop \
  ~/.local/lib/griffe/griffe-desktop ~/.local/lib/griffe/typst
sudo pacman -U ./griffe-bin-0.3.1-1-x86_64.pkg.tar.zst
```

Désinstall : `sudo pacman -Rns griffe-bin`. Le coffre
`~/.local/share/griffe/` ne bouge pas. Mise à jour : `pacman -U` le
nouveau fichier.

Sans pacman, le tarball `griffe-*-x86_64-linux.tar.xz` et
`scripts/install-griffe-native.sh` restent le geste `~/.local`.

L’AUR `griffe-bin` n’est pas soumis pour l’instant : le binaire pacman
vit sur la GitHub Release, pas dans l’AUR.

### Depuis les sources

```bash
nix develop
just check
cargo run -p griffe-desktop
```

## Architecture

Une seule couche applicative (`griffe-core::app`). CLI, MCP et fenêtre sont
trois adaptateurs équivalents : une règle métier n’y entre pas. Mutations via
`Command` (dry-run, idempotence, confirmation, audit chaîné) ; lectures via
`Query`. Aucun port TCP — Tauri exécute `griffe://` en mémoire contre le
routeur axum. Coffre SQLCipher, WAL multi-process, zéro connexion sortante.

Détail : [`docs/architecture.md`](docs/architecture.md). Invariants pour un
agent : [`AGENTS.md`](AGENTS.md).

## Licence

Le code est ouvert. Tu peux le lire, le modifier, t’en servir pour ta propre
activité. Tu ne peux pas vendre Griffe, le proposer en service hébergé, ni le
présenter comme un substitut, sans accord. Licence PolyForm Shield 1.0.0
([`LICENSE`](LICENSE), [`NOTICE`](NOTICE)).

« Griffe » désigne ce projet. Un fork se renomme.

## Contribuer

[`CONTRIBUTING.md`](CONTRIBUTING.md). Critère : `just check`.

## Sécurité

[`SECURITY.md`](SECURITY.md).

## Contact

[hello@nicolascollier.dev](mailto:hello@nicolascollier.dev)
