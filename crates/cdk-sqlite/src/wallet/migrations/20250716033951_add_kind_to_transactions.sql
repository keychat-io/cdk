-- Add migration script here
ALTER TABLE transactions ADD kind TEXT CHECK ( kind IN ('Cashu', 'LN') ) NOT NULL DEFAULT 'Cashu';