CREATE TABLE clients (
    id                   TEXT PRIMARY KEY,
    name                 TEXT NOT NULL,
    siren                TEXT,
    vat_number           TEXT,
    address_street       TEXT,
    address_postal_code  TEXT,
    address_city         TEXT,
    address_country      TEXT,
    created_at           TEXT NOT NULL
);

CREATE TABLE contacts (
    id         TEXT PRIMARY KEY,
    client_id  TEXT NOT NULL REFERENCES clients(id),
    name       TEXT NOT NULL,
    email      TEXT,
    phone      TEXT,
    role       TEXT
);
CREATE INDEX idx_contacts_client_id ON contacts(client_id);

CREATE TABLE opportunities (
    id                    TEXT PRIMARY KEY,
    client_id             TEXT NOT NULL REFERENCES clients(id),
    name                  TEXT NOT NULL,
    stage                 TEXT NOT NULL,
    amount_cents          INTEGER NOT NULL,
    probability_percent   INTEGER NOT NULL,
    next_action_at        TEXT,
    source                TEXT,
    loss_reason           TEXT,
    created_at            TEXT NOT NULL
);
CREATE INDEX idx_opportunities_client_id ON opportunities(client_id);
CREATE INDEX idx_opportunities_stage ON opportunities(stage);

CREATE TABLE quotes (
    id              TEXT PRIMARY KEY,
    client_id       TEXT NOT NULL REFERENCES clients(id),
    opportunity_id  TEXT REFERENCES opportunities(id),
    version         INTEGER NOT NULL,
    status          TEXT NOT NULL,
    valid_until     TEXT NOT NULL,
    created_at      TEXT NOT NULL
);
CREATE INDEX idx_quotes_client_id ON quotes(client_id);

CREATE TABLE quote_lines (
    id            INTEGER PRIMARY KEY,
    quote_id      TEXT NOT NULL REFERENCES quotes(id),
    position      INTEGER NOT NULL,
    description   TEXT NOT NULL,
    kind          TEXT NOT NULL,
    vat_rate      TEXT NOT NULL
);
CREATE INDEX idx_quote_lines_quote_id ON quote_lines(quote_id);

CREATE TABLE missions (
    id          TEXT PRIMARY KEY,
    client_id   TEXT NOT NULL REFERENCES clients(id),
    quote_id    TEXT REFERENCES quotes(id),
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    started_on  TEXT NOT NULL,
    ended_on    TEXT
);
CREATE INDEX idx_missions_client_id ON missions(client_id);

CREATE TABLE milestones (
    id          INTEGER PRIMARY KEY,
    mission_id  TEXT NOT NULL REFERENCES missions(id),
    position    INTEGER NOT NULL,
    label       TEXT NOT NULL,
    share_bps   INTEGER NOT NULL,
    due_on      TEXT
);
CREATE INDEX idx_milestones_mission_id ON milestones(mission_id);

CREATE TABLE time_entries (
    id          TEXT PRIMARY KEY,
    mission_id  TEXT NOT NULL REFERENCES missions(id),
    worked_on   TEXT NOT NULL,
    days        REAL NOT NULL,
    category    TEXT NOT NULL,
    note        TEXT
);
CREATE INDEX idx_time_entries_mission_id ON time_entries(mission_id);

CREATE TABLE invoices (
    id             TEXT PRIMARY KEY,
    number         TEXT NOT NULL UNIQUE,
    client_id      TEXT NOT NULL REFERENCES clients(id),
    mission_id     TEXT REFERENCES missions(id),
    status         TEXT NOT NULL,
    issued_on      TEXT NOT NULL,
    due_on         TEXT NOT NULL,
    previous_hash  TEXT,
    hash           TEXT NOT NULL
);
CREATE INDEX idx_invoices_client_id ON invoices(client_id);

CREATE TABLE invoice_lines (
    id                 INTEGER PRIMARY KEY,
    invoice_id         TEXT NOT NULL REFERENCES invoices(id),
    position           INTEGER NOT NULL,
    description        TEXT NOT NULL,
    quantity           REAL NOT NULL,
    unit_price_cents   INTEGER NOT NULL,
    vat_rate           TEXT NOT NULL
);
CREATE INDEX idx_invoice_lines_invoice_id ON invoice_lines(invoice_id);

CREATE TABLE payments (
    id           TEXT PRIMARY KEY,
    invoice_id   TEXT NOT NULL REFERENCES invoices(id),
    amount_cents INTEGER NOT NULL,
    received_on  TEXT NOT NULL,
    method       TEXT NOT NULL
);
CREATE INDEX idx_payments_invoice_id ON payments(invoice_id);

CREATE TABLE expenses (
    id             TEXT PRIMARY KEY,
    label          TEXT NOT NULL,
    amount_cents   INTEGER NOT NULL,
    vat_rate       TEXT NOT NULL,
    incurred_on    TEXT NOT NULL,
    receipt_hash   TEXT
);
