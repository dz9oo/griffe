-- Fondations du lot 15 (« gestion des données : écrire, modifier, supprimer ») : jusqu'ici,
-- aucune table mutable n'avait de garde-fou contre l'écriture concurrente, et rien ne
-- distinguait un client/une mission qu'on peut supprimer d'un qu'on doit seulement retirer des
-- listes actives sans casser les références passées (factures, devis).
--
-- `revision` : les commandes `Update*`/`Delete*`/`Archive*` du lot 15 écrivent
-- `WHERE id = ?1 AND revision = ?2` ; zéro ligne modifiée signale un conflit d'écriture
-- concurrente (GUI, CLI et serveur MCP peuvent écrire simultanément dans le même fichier).
-- Posée dès maintenant sur toutes les tables du Tier 1 (clients, contacts, opportunités,
-- missions, temps, dépenses) pour que les lots 16-18 n'aient pas à rouvrir une migration rien
-- que pour ce garde-fou.
ALTER TABLE clients ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE contacts ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE opportunities ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE missions ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE time_entries ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE expenses ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;

-- `archived_at` : un client ou une mission déjà référencé par une facture/un devis ne peut pas
-- être supprimé (les triggers de `0004`/`0005` interdisent déjà de toucher ces références) —
-- il se retire des listes actives par archivage plutôt que par suppression.
ALTER TABLE clients ADD COLUMN archived_at TEXT;
ALTER TABLE missions ADD COLUMN archived_at TEXT;

-- `voided_at` : prépare le régime « contre-écriture » du Tier 2 pour les paiements (un
-- encaissement saisi à tort n'est aujourd'hui ni corrigible ni annulable) — introduit ici pour
-- la même raison que `revision` ci-dessus, la commande `VoidPayment` elle-même est hors
-- périmètre du lot 15.
ALTER TABLE payments ADD COLUMN voided_at TEXT;
