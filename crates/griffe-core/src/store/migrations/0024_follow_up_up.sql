-- Journal de relances (lot 47) : faits append-only, file dérivée. L'identité d'envoi
-- (From) est un singleton à part du profil société — le mail de relance n'est pas
-- une mention légale.
CREATE TABLE follow_up_events (
    id                  TEXT PRIMARY KEY,
    subject_kind        TEXT NOT NULL CHECK (subject_kind IN ('opportunity', 'invoice')),
    subject_id          TEXT NOT NULL,
    fact                TEXT NOT NULL CHECK (fact IN (
                            'draft_prepared', 'marked_sent', 'step_skipped',
                            'snoozed', 'date_set', 'retracted'
                        )),
    at                  TEXT NOT NULL,
    until_on            TEXT,
    rendered_subject    TEXT,
    rendered_body       TEXT,
    retracts            TEXT REFERENCES follow_up_events(id),
    interaction_id      TEXT
);
CREATE INDEX idx_follow_up_events_subject ON follow_up_events(subject_kind, subject_id, at);

CREATE TABLE follow_up_settings (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    sender_email    TEXT,
    sender_name     TEXT,
    updated_at      TEXT NOT NULL
);
