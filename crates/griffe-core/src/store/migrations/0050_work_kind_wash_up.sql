-- Teinte et note d'un type. L'encre et la note vide sont le départ.
ALTER TABLE work_kinds ADD COLUMN tint TEXT NOT NULL DEFAULT 'encre';
ALTER TABLE work_kinds ADD COLUMN note TEXT NOT NULL DEFAULT '';
