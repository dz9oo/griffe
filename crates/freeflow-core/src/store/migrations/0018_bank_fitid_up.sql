-- Lot 38 : identifiant unique donné par la banque (`FITID` en OFX, « Transaction ID » de
-- certains CSV). Quand il existe, c'est la clé de dédoublonnage d'un ré-import : plus fiable
-- que le triplet date/montant/libellé, qui diffère entre l'export CSV et l'OFX d'une même
-- opération. Index unique partiel : les transactions sans identifiant gardent l'ancienne règle.
ALTER TABLE bank_transactions ADD COLUMN fitid TEXT;
CREATE UNIQUE INDEX idx_bank_transactions_fitid ON bank_transactions(fitid) WHERE fitid IS NOT NULL;
