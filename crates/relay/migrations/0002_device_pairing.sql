-- Additive upgrade: legacy account tables remain intact for rollback.
CREATE TABLE remote_devices (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('host', 'client')),
    public_key bytea NOT NULL CHECK (length(public_key) = 32),
    token_hash bytea NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz
);
CREATE TABLE remote_links (
    host_id uuid NOT NULL REFERENCES remote_devices(id) ON DELETE CASCADE,
    client_id uuid NOT NULL REFERENCES remote_devices(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (host_id, client_id),
    CHECK (host_id <> client_id)
);
CREATE INDEX remote_links_client ON remote_links(client_id);
CREATE TABLE remote_pairings (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    host_id uuid NOT NULL REFERENCES remote_devices(id) ON DELETE CASCADE,
    client_id uuid REFERENCES remote_devices(id) ON DELETE CASCADE,
    init_packet bytea,
    proof bytea,
    response_packet bytea,
    expires_at timestamptz NOT NULL DEFAULT now() + interval '2 minutes'
);
CREATE INDEX remote_pairings_host ON remote_pairings(host_id);
