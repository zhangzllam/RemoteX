ALTER TABLE devices
    ADD COLUMN connectivity_candidates_json TEXT NOT NULL DEFAULT '[]';

