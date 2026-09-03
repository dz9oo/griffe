-- Lot 37 : règlement d'un compte de bilan depuis le relevé. Une transaction importée pouvait
-- être l'encaissement d'une facture (`matched_invoice_id`) ou le paiement d'une dépense
-- (`matched_expense_id`) ; elle peut désormais être le règlement d'un compte de bilan —
-- dette reprise au bilan d'ouverture (401, 444, 4455), compte courant d'associé (455),
-- dividendes (457), virement interne (580), emprunt (164). Pas de table nouvelle : un règlement
-- n'a pas d'identité propre, c'est une lecture du mouvement, portée par la transaction comme
-- les deux autres rapprochements (exclusivité garantie par les commandes).
ALTER TABLE bank_transactions ADD COLUMN settlement_account TEXT;
ALTER TABLE bank_transactions ADD COLUMN settlement_label TEXT;
