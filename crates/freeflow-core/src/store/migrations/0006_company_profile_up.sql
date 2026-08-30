-- Identité légale de l'émetteur (SASU/EURL) : nécessaire aux mentions obligatoires d'une
-- facture (lot 10). Table à une seule ligne (id fixé à 1) plutôt qu'un schéma clé-valeur
-- générique : ses colonnes sont connues et stables, pas un ensemble de préférences ouvert.
CREATE TABLE company_profile (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    name TEXT NOT NULL,
    legal_form TEXT NOT NULL,
    siren TEXT NOT NULL,
    vat_number TEXT,
    address_street TEXT NOT NULL,
    address_postal_code TEXT NOT NULL,
    address_city TEXT NOT NULL,
    address_country TEXT NOT NULL,
    share_capital_cents INTEGER,
    rcs_city TEXT,
    iban TEXT,
    updated_at TEXT NOT NULL
);
