CREATE TABLE users (
    id uuid PRIMARY KEY,
    issuer text NOT NULL CHECK (length(issuer) BETWEEN 1 AND 2048),
    subject text NOT NULL CHECK (length(subject) BETWEEN 1 AND 255),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (issuer, subject)
);
CREATE TABLE libraries (
    id uuid PRIMARY KEY,
    owner_id uuid NOT NULL REFERENCES users(id),
    name text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    change_seq bigint NOT NULL DEFAULT 0 CHECK (change_seq >= 0),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX libraries_owner ON libraries(owner_id, id);
CREATE TABLE storage_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    used_bytes bigint NOT NULL DEFAULT 0 CHECK (used_bytes >= 0),
    reserved_bytes bigint NOT NULL DEFAULT 0 CHECK (reserved_bytes >= 0),
    pending_count integer NOT NULL DEFAULT 0 CHECK (pending_count >= 0)
);
INSERT INTO storage_budget(singleton) VALUES (true);
CREATE TABLE photos (
    id uuid PRIMARY KEY,
    library_id uuid NOT NULL REFERENCES libraries(id),
    object_id uuid NOT NULL UNIQUE,
    operation_id uuid NOT NULL,
    original_name text NOT NULL CHECK (length(original_name) BETWEEN 1 AND 255),
    media_type text NOT NULL CHECK (media_type IN ('image/jpeg', 'application/octet-stream')),
    byte_length bigint NOT NULL CHECK (byte_length > 0 AND byte_length <= 536870912),
    sha256 text NOT NULL CHECK (sha256 ~ '^[a-f0-9]{64}$'),
    metadata jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(metadata) = 'object'),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'ready', 'cancelled')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    stored_at timestamptz,
    deleted_at timestamptz,
    UNIQUE(library_id, operation_id),
    CHECK ((status = 'ready') = (stored_at IS NOT NULL))
);
CREATE INDEX photos_library ON photos(library_id, id);
CREATE INDEX photos_pending ON photos(created_at) WHERE status = 'pending';
-- No PostgreSQL sequence allocated before commit: writers advance the library row
-- and commit under its row lock, preserving visible order for the future sync feed.
CREATE TABLE changes (
    library_id uuid NOT NULL REFERENCES libraries(id),
    seq bigint NOT NULL CHECK (seq > 0),
    entity_id uuid NOT NULL,
    kind text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(library_id, seq)
);
GRANT USAGE ON SCHEMA public TO lightcraft_api;
GRANT SELECT, INSERT, UPDATE ON users, libraries, photos, storage_budget, changes TO lightcraft_api;
