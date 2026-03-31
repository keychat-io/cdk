-- Add 'Split' to direction CHECK constraint by recreating the table
-- SQLite does not support ALTER COLUMN to modify CHECK constraints

CREATE TABLE transactions_new (
    id BLOB PRIMARY KEY,
    mint_url TEXT NOT NULL,
    direction TEXT CHECK (direction IN ('Incoming', 'Outgoing', 'Split')) NOT NULL,
    amount INTEGER NOT NULL,
    fee INTEGER NOT NULL,
    unit TEXT NOT NULL,
    ys BLOB NOT NULL,
    timestamp INTEGER NOT NULL,
    memo TEXT,
    metadata TEXT,
    quote_id TEXT,
    payment_request TEXT,
    payment_proof TEXT,
    payment_method TEXT,
    saga_id TEXT,
    kind TEXT CHECK (kind IN ('Cashu', 'LN')) NOT NULL DEFAULT 'Cashu',
    token TEXT DEFAULT '',
    status TEXT CHECK (status IN ('Success', 'Pending', 'Expired', 'Failed')) NOT NULL DEFAULT 'Success'
);

INSERT INTO transactions_new SELECT
    id, mint_url, direction, amount, fee, unit, ys, timestamp, memo, metadata,
    quote_id, payment_request, payment_proof, payment_method, saga_id,
    kind, token, status
FROM transactions;

DROP TABLE transactions;
ALTER TABLE transactions_new RENAME TO transactions;

CREATE INDEX IF NOT EXISTS mint_url_index ON transactions(mint_url);
CREATE INDEX IF NOT EXISTS direction_index ON transactions(direction);
CREATE INDEX IF NOT EXISTS unit_index ON transactions(unit);
CREATE INDEX IF NOT EXISTS timestamp_index ON transactions(timestamp);
CREATE INDEX IF NOT EXISTS transactions_kind_index ON transactions(kind);
CREATE INDEX IF NOT EXISTS transactions_status_index ON transactions(status);
CREATE INDEX IF NOT EXISTS transactions_saga_id_index ON transactions(saga_id);
