-- Add kind, token, status columns to transactions table and update direction constraint
ALTER TABLE transactions ADD COLUMN kind TEXT CHECK (kind IN ('Cashu', 'LN')) NOT NULL DEFAULT 'Cashu';
ALTER TABLE transactions ADD COLUMN token TEXT DEFAULT '';
ALTER TABLE transactions ADD COLUMN status TEXT CHECK (status IN ('Success', 'Pending', 'Expired', 'Failed')) NOT NULL DEFAULT 'Success';

CREATE INDEX IF NOT EXISTS transactions_kind_index ON transactions(kind);
CREATE INDEX IF NOT EXISTS transactions_status_index ON transactions(status);
