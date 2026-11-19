-- New device authorizations separate the human-entered user code from the
-- high-entropy polling credential. Existing pending rows remain compatible
-- for their ten-minute lifetime; every newly issued poll secret is Argon2
-- hashed and addressed by the row UUID embedded in the returned credential.
ALTER TABLE login_codes ADD COLUMN device_secret_hash TEXT;
