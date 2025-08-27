-- Add migration script here
ALTER TABLE transactions ADD status TEXT CHECK ( status IN ('Success', 'Pending', 'Expired', 'Failed') ) NOT NULL DEFAULT 'Success';