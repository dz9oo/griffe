-- Compte d'envoi. Une seule ligne. Le secret est dans le coffre chiffré,
-- jamais dans le journal d'audit.
CREATE TABLE mail_account (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    from_name TEXT,
    from_address TEXT,
    smtp_host TEXT,
    smtp_port INTEGER,
    smtp_tls TEXT NOT NULL DEFAULT 'starttls',
    smtp_username TEXT,
    secret TEXT,
    preset TEXT NOT NULL DEFAULT 'custom',
    auto_send INTEGER NOT NULL DEFAULT 0 CHECK (auto_send IN (0, 1)),
    updated_at TEXT NOT NULL
);

-- File d'envoi. Le réseau n'écrit pas ici : une commande arme, une autre constate.
CREATE TABLE outbound_mail (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    anchor TEXT,
    from_address TEXT NOT NULL,
    from_name TEXT,
    to_address TEXT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT NOT NULL,
    message_id TEXT NOT NULL,
    send_after TEXT NOT NULL,
    session_token TEXT,
    follow_subject TEXT,
    follow_subject_id TEXT,
    follow_cycle TEXT,
    follow_step TEXT,
    error TEXT,
    smtp_response TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_outbound_mail_follow_open
    ON outbound_mail (follow_subject, follow_subject_id, follow_cycle, follow_step)
    WHERE follow_subject IS NOT NULL
      AND status IN ('armed', 'sending', 'sent', 'uncertain');
