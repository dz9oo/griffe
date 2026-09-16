-- Lot 30 : bilan d'ouverture — la reprise du bilan tenu avant l'usage de l'application
-- (typiquement le dernier bilan de l'expert-comptable). Jusqu'ici, la chaîne de clôture
-- (`fiscal_years`, 0010) supposait un report à nouveau et une réserve légale nuls avant le
-- premier exercice clos ici, et le FEC (lot 28) n'avait pas d'à-nouveaux : faux pour toute
-- société préexistante.
--
-- Une seule ligne (`CHECK (id = 1)`, comme `company_profile`) : le bilan d'ouverture est *le*
-- point de départ de l'historique — il n'y en a pas un par exercice, les suivants s'enchaînent
-- depuis les snapshots de `fiscal_years`. Révision optimiste comme les autres entités mutables ;
-- la garde « aucun exercice clos ne doit exister » (record/update/delete) vit dans le cœur
-- (`opening_balance.rs`), pas ici : elle dépend d'une autre table.
CREATE TABLE opening_balance (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    opens_on TEXT NOT NULL,          -- premier jour de l'exercice qui s'ouvre sur ce bilan
    source TEXT,                     -- provenance libre (cabinet, date du bilan repris)
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);

-- Les lignes : une par compte (UNIQUE), un montant strictement positif d'un seul côté. Réécrites
-- en bloc à chaque `UpdateOpeningBalance` (état complet, comme les jalons d'une mission) : elles
-- n'ont pas d'identité externe. Pas d'AUTOINCREMENT : il créerait `sqlite_sequence`, qu'aucun
-- `DROP TABLE` de migration descendante ne retire.
CREATE TABLE opening_balance_lines (
    id INTEGER PRIMARY KEY,
    opening_balance_id INTEGER NOT NULL REFERENCES opening_balance(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    account TEXT NOT NULL,           -- code PCG, 3 à 12 chiffres (domain::AccountCode)
    label TEXT NOT NULL,
    side TEXT NOT NULL CHECK (side IN ('debit', 'credit')),
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
    UNIQUE (opening_balance_id, account)
);
