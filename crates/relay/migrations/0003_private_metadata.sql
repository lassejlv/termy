-- Device names are encrypted on the owning device. Old plaintext cannot be
-- retroactively encrypted without its private key; remove it and let updated
-- apps publish ciphertext without changing registrations or pairing links.
ALTER TABLE remote_devices DROP COLUMN name;
ALTER TABLE remote_devices ADD COLUMN encrypted_name bytea
    CHECK (encrypted_name IS NULL OR (length(encrypted_name) = 285 AND get_byte(encrypted_name, 0) = 1));
ALTER TABLE remote_pairings ADD COLUMN encrypted_client_name bytea
    CHECK (encrypted_client_name IS NULL OR (length(encrypted_client_name) = 285 AND get_byte(encrypted_client_name, 0) = 1));

-- Explicitly authorized removal of the retired v1 sign-in data. These tables
-- are not referenced by v2 device registrations or QR pairing links.
DROP TABLE sessions;
DROP TABLE devices;
DROP TABLE users;
