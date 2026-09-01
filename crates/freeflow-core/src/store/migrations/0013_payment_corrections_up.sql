-- Lot 22 (« corrections d'encaissement ») : persiste la lignée rapprochement → encaissement.
-- `ReconcileTransaction` (lot 5) crée un paiement à partir d'une transaction bancaire sans que
-- rien en base ne relie les deux : impossible de défaire un rapprochement de façon fiable
-- (retrouver « son » paiement par date + montant serait une heuristique, pas une lignée). Même
-- doctrine que `missions.opportunity_id` (0012) : une lignée explicite, posée à la création,
-- jamais éditable. NULL pour un encaissement manuel (`RecordPayment`) ou antérieur à cette
-- migration — pour ces derniers, `UnreconcileTransaction` libère la transaction mais ne peut
-- pas retrouver le paiement : il s'annule séparément via `VoidPayment`.
ALTER TABLE payments ADD COLUMN bank_transaction_id TEXT REFERENCES bank_transactions(id);

-- Volontairement PAS de `revision` sur `payments` : un paiement ne connaît qu'une seule
-- mutation, son annulation (`voided_at`, colonne posée dès 0008), qui n'arrive qu'une fois.
-- `VoidPayment` relit le paiement dans la même transaction IMMEDIATE que son écriture et refuse
-- sur `voided_at IS NOT NULL` — un verrou sémantique plus fort qu'un verrou optimiste pour ce
-- cas précis (même raisonnement que `Advance`/`Win`/`Lose`, voir le commentaire de module de
-- `prospection/commands.rs`). Ne pas « corriger » cette absence sans relire ce commentaire.
