# Signatures des binaires publiés

Une clé minisign (Ed25519), distincte de la clé GPG des commits. Elle signe les octets de la Release. Les `.sha256` restent le contrôle d'intégrité calculé dans le même job.

Trois fichiers, chacun avec un voisin `fichier.minisig` :

- l'AppImage ;
- `griffe-<version>-x86_64-linux.tar.xz` ;
- `griffe-bin-<version>-1-x86_64.pkg.tar.zst`.

Le commentaire de confiance est le nom du fichier. La vérification exige une signature pré-hachée (`minisign -V -H`), le mode par défaut depuis minisign 0.11.

La clé prouve que ces octets ont été signés par le mainteneur. Elle ne dit pas que le fichier est la dernière version.

## Cérémonie

Sur la machine du mainteneur. La clé secrète ne va pas dans le dépôt, ni dans un ticket, ni dans les logs.

```sh
pacman -S minisign
mkdir -p ~/.config/griffe
minisign -G -c "griffe release" \
  -p packaging/minisign.pub \
  -s ~/.config/griffe/minisign.key
```

`-c` nomme la clé secrète. Le fichier public porte toujours `untrusted comment: minisign public key` suivi de l'identifiant. Copier `packaging/minisign.pub` vers `site/minisign.pub`. Reporter les deux lignes dans `MINISIGN_PUB_COMMENT` et `MINISIGN_PUB_KEY` de `scripts/install-griffe.sh` et `scripts/install-griffe-native.sh`.

La copie de travail reste chiffrée, avec une sauvegarde hors ligne. Le secret GitHub est la forme sans phrase de passe : au moment de signer, minisign lit une phrase sur un TTY si la clé est chiffrée, et `-W` ne saute pas cette demande.

```sh
cp ~/.config/griffe/minisign.key /tmp/minisign.key
minisign -C -W -s /tmp/minisign.key
gh secret set MINISIGN_SECRET_KEY --repo dz9oo/griffe < /tmp/minisign.key
rm -P /tmp/minisign.key
```

`packaging/minisign.key` est ignoré par git.

## Publication

Le job `sign-release` de `.github/workflows/bundle.yml` ne tourne que sur un tag `v*`. Il refuse de continuer si `MINISIGN_SECRET_KEY` est vide, signe, puis publie la Release. Les jobs de build ne publient pas. La clé n'entre pas dans le conteneur Arch privilégié. `minisign` vient du paquet Ubuntu du runner (0.11 ou plus récent).

Sans le secret, le tag ne publie rien. `workflow_dispatch` ne publie pas non plus.

## Vérification

`scripts/install-griffe.sh` et, pour une archive, `scripts/install-griffe-native.sh` vérifient avant d'écrire sous `~/.local`. Un script copié seul utilise la clé embarquée. Dans un clone, `packaging/minisign.pub` doit être identique, sinon le script s'arrête.

`pacman -U` ne lit pas un `.minisig`. La commande est dans le README.

## AUR, plus tard

`griffe-bin` sur l'AUR reconstruira le paquet depuis le tarball, pas depuis le `.pkg.tar.zst`. Le PKGBUILD gagnera l'URL du `.minisig`, une copie de `minisign.pub` vendue dans le dépôt AUR (pas téléchargée depuis `master` au moment du build), les `sha256sums`, et un `prepare()` qui lance le même `minisign -V -H`. `makepkg` ne vérifie pas minisign tout seul.

Les commits de ce dépôt AUR seront signés avec la clé GPG git (`git commit -S`). On ne pousse pas l'AUR tant qu'une Release signée n'existe pas. `publish_aur` reste manuel et éteint.

Changer de clé : la nouvelle clé publique arrive dans le dépôt et sur le site, les scripts embarqués changent en même temps. Une Release de transition peut encore être signée par l'ancienne clé le temps que les copies du script suivent.
