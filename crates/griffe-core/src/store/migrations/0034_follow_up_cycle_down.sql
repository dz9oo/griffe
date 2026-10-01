PRAGMA defer_foreign_keys = ON;

ALTER TABLE follow_up_events RENAME TO follow_up_events_old;

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

INSERT INTO follow_up_events (
    id, subject_kind, subject_id, fact, at, until_on,
    rendered_subject, rendered_body, retracts, interaction_id
)
SELECT
    id, subject_kind, subject_id, fact, at, until_on,
    rendered_subject, rendered_body, retracts, interaction_id
FROM follow_up_events_old
WHERE fact <> 'cycle_opened';

DROP TABLE follow_up_events_old;

CREATE INDEX idx_follow_up_events_subject ON follow_up_events(subject_kind, subject_id, at);
