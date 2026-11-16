-- One APNs device token per phone, so a paired Mac can wake it. Notification
-- content never reaches the relay: Macs send a blob sealed for the phone.
CREATE TABLE remote_push_tokens (
    device_id uuid PRIMARY KEY REFERENCES remote_devices(id) ON DELETE CASCADE,
    token bytea NOT NULL UNIQUE CHECK (length(token) BETWEEN 32 AND 100),
    environment text NOT NULL CHECK (environment IN ('sandbox', 'production')),
    updated_at timestamptz NOT NULL DEFAULT now()
);
