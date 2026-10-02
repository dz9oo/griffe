-- Les phrases de prospection vivent dans le coffre. Le semis est le texte
-- d'aujourd'hui : rien ne change tant qu'on ne les réécrit pas.
CREATE TABLE prospect_phrases (
    id           TEXT PRIMARY KEY,
    position     INTEGER NOT NULL UNIQUE CHECK (position >= 0),
    label        TEXT NOT NULL,
    offset_days  INTEGER NOT NULL CHECK (offset_days >= 0),
    subject      TEXT NOT NULL,
    body         TEXT NOT NULL,
    revision     INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1)
);

INSERT INTO prospect_phrases (id, position, label, offset_days, subject, body) VALUES
(
    'hello', 0, 'Premier message', 0, '{{sujet}}',
    'Bonjour {{prenom}},

Je me permets de revenir vers vous au sujet de {{sujet}}.

Auriez-vous un créneau cette semaine pour en parler ?

Bien à vous,
{{moi}}
{{societe}}
'
),
(
    'bump', 1, 'Petit rappel', 3, '{{sujet}} — je me permets un rappel',
    'Bonjour {{prenom}},

Je me permets un court rappel au sujet de {{sujet}}.

Dites-moi simplement si le moment n''est pas le bon.

Bien à vous,
{{moi}}
'
),
(
    'value', 2, 'Relance utile', 7, '{{sujet}}',
    'Bonjour {{prenom}},

Je reviens vers vous concernant {{sujet}} ({{montant}}).

Je reste disponible pour un échange de quinze minutes, à votre convenance.

Bien à vous,
{{moi}}
{{societe}}
'
),
(
    'close', 3, 'Dernier mot', 14, '{{sujet}} — je clos le dossier de mon côté ?',
    'Bonjour {{prenom}},

Je n''ai pas eu de retour au sujet de {{sujet}}.

Si le timing n''est pas le bon, dites-le-moi et je referme le dossier sans insister.

Bien à vous,
{{moi}}
'
);
