# Conformité légale et fiscale

Ce n’est **pas** un cours, ni le dossier d’une société.
C’est la politique pour qu’un contributeur (ou son agent)
ne rende pas Griffe moins fiable.

Un PR qui change une règle fiscale, un document Cerfa, le FEC ou la liasse
**sans source primaire** est refusé.

Le détail d’un calcul vit dans le doc-comment du module et dans le test.
On ne le recopie pas ici. Aucun dossier réel n’entre dans ce dépôt :
pas de raison sociale, pas de SIREN, pas d’adresse, pas de pièce déposée,
pas de montant repris d’un exercice réel.

## Doctrine

1. **Disclaimer.** Griffe calcule et rappelle. Ce n’est pas un avis fiscal,
   pas une attestation DGFiP, pas un SAE. Le FEC est l’extrait des écritures
   du coffre. Ce n’est pas une attestation, ni un dépôt. Rien n’est
   télétransmis. Pas d’EDI, pas de PDF Cerfa. La phrase « on recopie, rien
   n’est télétransmis » figure dans le produit.
   Une écriture déjà posée ne se réécrit pas. Une correction est une extourne.
2. **Source primaire.** Toute règle nouvelle ou changée cite BOFIP, CGI,
   C. com., ou notice Cerfa (extrait `pdftotext` d’un PDF officiel, pas un
   souvenir). La citation vit dans le doc-comment du type / de la fonction
   **ou** dans le test, pas dans un commentaire orphelin.
3. **Chiffre avant l’assert.** Les montants attendus sont écrits à la main
   dans le test (patron `crates/griffe-cli/tests/closing_scenario.rs`,
   `settlement_scenario.rs`). Le test se plie au chiffre de
   l’expert-comptable, jamais l’inverse. Ce chiffre est un exemple du test,
   pas un extrait d’un dossier.
4. **Limite connue à côté de la règle.** Si on ne modélise pas (DAS2
   natures de somme, cession d’immo, régime 2027 non précisé…), on le dit
   dans le même module. Pas une checklist perso dans le README.
5. **Périmètre actuel.** SASU à l’IS, TVA au réel (normal, trimestriel,
   simplifié jusqu’à sa fin). EURL comme régime distinct, micro, IR,
   multi-associés, stock, étranger : hors PR « au passage ».
6. **Loi datée.** Une bascule (ex. suppression du RSI au 1er janv. 2027)
   est une constante nommée (`SIMPLIFIED_REGIME_REPEAL`), pas un
   `if year > 2026` magique.
7. **Arrondis.** IS à l’euro (art. 1657 CGI) déjà dans le cœur ; un PR ne
   « simplifie » pas en flottant.
8. **Culture générale interdite.** Un agent qui « sait » que la déclaration
   de TVA tombe le 15 d’après le SIREN a tort. S’il n’a pas la source sous
   la main, il s’arrête.

## Limites

Une créance irrécouvrable n’est pas un avoir : le chiffre d’affaires de la
vente reste. La source et l’écriture sont dans le module des ventes.

## CODEOWNERS

Le ruleset `MasterProtect` exige une revue CODEOWNERS (`@dz9oo`) sur
toute PR. Le fichier `.github/CODEOWNERS` commence par `* @dz9oo`.

Les chemins fiscaux restent listés pour le lecteur humain :

```
/crates/griffe-core/src/accounting.rs    @dz9oo
/crates/griffe-core/src/fiscal.rs        @dz9oo
/crates/griffe-core/src/fiscal_year.rs   @dz9oo
/crates/griffe-core/src/ledger.rs        @dz9oo
/crates/griffe-core/src/journal.rs       @dz9oo
/crates/griffe-core/src/fec/             @dz9oo
/crates/griffe-core/src/closing.rs       @dz9oo
/crates/griffe-core/src/opening_balance.rs @dz9oo
/crates/griffe-core/src/opening_balance/ @dz9oo
/crates/griffe-core/src/fixed_assets.rs  @dz9oo
/crates/griffe-core/src/domain/vat.rs    @dz9oo
/crates/griffe-core/src/domain/vat_filing.rs @dz9oo
/crates/griffe-core/src/domain/period.rs @dz9oo
/crates/griffe-core/src/domain/asset.rs  @dz9oo
/crates/griffe-core/src/society/         @dz9oo
/crates/griffe-docs/                     @dz9oo
/crates/griffe-invoice/                  @dz9oo
/docs/legal.md                          @dz9oo
```

Une PR qui touche ces fichiers, ou n’importe quel autre chemin, attend
une revue `@dz9oo` avant merge.
