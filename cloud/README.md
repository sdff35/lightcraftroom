# LightCraft Cloud — stage 3

An independent Rust REST service, PostgreSQL 17 catalog, immutable filesystem originals and
Keycloak OIDC. This is a working server foundation; desktop/mobile synchronization and rendering
are separate work. It shares no database, storage, credentials or containers with Immich.

The server is a separate Cargo workspace, so PostgreSQL and HTTP dependencies never enter the
LightCraft desktop/WASM builds. Rust 1.95+, Linux amd64/arm64, Docker Engine and Compose v2 are
required. Ubuntu 24.04 is a supported deployment target; host-specific OCI/NPM changes are manual.

## Local start

Run from this directory on an isolated development machine:

```sh
python3 scripts/init-local.py
docker compose -p lightcraft-cloud-dev -f compose.yaml -f compose.local.yaml config --quiet
docker compose -p lightcraft-cloud-dev -f compose.yaml -f compose.local.yaml up -d --build --wait
curl --fail http://127.0.0.1:18880/health/ready
LC_COMPOSE_PROJECT=lightcraft-cloud-dev python3 tests/e2e.py
```

The first image build and Keycloak database initialization take time. The local override publishes
API `127.0.0.1:18880` and Keycloak `127.0.0.1:18881`. PostgreSQL has **no host port**.
The generator creates a 128 MiB development quota, a 64 MiB free-space reserve and a 32 MiB
single-upload cap, explicitly enables a loopback HTTP issuer, and retains existing secrets.
Those small limits are for tests, not an estimate of your Oracle server's available space.

`init-local.py` creates five random password files under ignored `.secrets/` and never prints them.
The host directory is `0700`; individually mounted files are readable by the services' different
unprivileged UIDs. Compose file-backed secrets do not reliably remap UID/mode. Protect the parent
directory and Docker socket; never mount the whole secret directory into a service. Do not rerun
initialization as a password-rotation mechanism or remove volumes to troubleshoot a real library.

Keycloak's bootstrap admin is `lc-bootstrap`; obtain its generated password locally through your
secure tooling. No account or password is committed. Application clients use **Authorization Code
+ PKCE S256**, not password grants. Public client `lightcraft-desktop` has the exact redirect URI
`http://127.0.0.1:47861/callback`; registration, implicit flow and direct grants are disabled.
Production requires a permanent administrator with MFA and removal of temporary bootstrap access.

The integration tests create their own users, use real browser-form login + PKCE, exercise two
account ACLs and refresh rotation, and simulate an interrupted finalize transaction. They also
recreate containers. Run them only against an isolated development stack, never a real library.

## API

All `/v1` endpoints require `Authorization: Bearer <access token>`. Reads require
`lightcraft:read`, writes require `lightcraft:write`. The API validates RS256, issuer, audience
`lightcraft-cloud`, expiration/not-before, access-token type and subject. Identity is `(iss, sub)`;
email and username are not identity keys. The included `lightcraft-identity` client scope
explicitly maps the stable `sub`; include it on future clients as well. JWT keys have a five-minute bounded cache, with a
throttled refresh for unknown key IDs. Token-provided key URLs are ignored. A cached token can
remain valid until its expiry after session revocation; refresh is enforced by Keycloak.

| Method | Endpoint | Behavior |
|---|---|---|
| GET | `/health/live` | Process liveness, public |
| GET | `/health/ready` | PostgreSQL, storage and OIDC key readiness, public |
| GET | `/v1/me` | Authenticated identity and scopes |
| POST / GET | `/v1/libraries` | Create / list the owner's libraries |
| GET | `/v1/libraries/{id}` | Read one owned library |
| POST / GET | `/v1/libraries/{id}/photos` | Register / list photos |
| GET | `/v1/photos/{id}` | Metadata, state, revision, size and checksum |
| PUT | `/v1/photos/{id}/original` | Stream one complete original; finalize after verification |
| GET | `/v1/photos/{id}/original` | Authenticated byte-identical download with SHA-256/ETag |
| POST | `/v1/photos/{id}/verify` | Re-hash original and verify its metadata manifest |
| DELETE | `/v1/photos/{id}/upload` | Cancel a pending intent; cannot delete a completed original |

Lists use `after=<UUID>&limit=1..100`, with `items` and `next_after`. Another owner's resources
return 404. Errors use `{"error":{"code":"...","message":"..."}}`, with no credentials or SQL
in responses. Useful statuses: 401, 403, 404, 409, 411, 413, 415, 422, 429, 503 and 507.

Create a library with `{"name":"My photos"}`. Register an original before sending bytes:

```json
{
  "operation_id": "<client-generated UUID>",
  "original_name": "IMG_0001.NEF",
  "media_type": "application/octet-stream",
  "byte_length": 12345678,
  "sha256": "<64 lower-case hexadecimal characters>",
  "metadata": {
    "title": "Mountain",
    "camera_make": "Nikon",
    "camera_model": "My camera",
    "captured_at": "2026-10-20T12:00:00Z",
    "width": 6000,
    "height": 4000,
    "rating": 4,
    "tags": ["trip"]
  }
}
```

The returned photo has `pending` state. PUT the raw bytes, **not multipart**, with exact
`Content-Length` and matching `Content-Type` (`image/jpeg` for JPEG,
`application/octet-stream` for RAW). Success returns the photo in `ready` state. The default
maximum is 512 MiB, with two active file operations, a 30-second inactivity timeout and a
300-second overall upload deadline. A failed/interrupted PUT is retried **from byte zero**.
This stage does not implement tus, chunks, Range downloads or previews.

The same `(library_id, operation_id)` and request return the same photo; a different request with
that operation ID returns 409. A repeated completed PUT verifies and returns the existing object
without overwriting it. Cancel abandoned pending intents to release reserved original bytes.
Cancelled metadata remains charged against the quota. Metadata is validated client-supplied data;
this stage does not automatically extract EXIF or decode camera sensor data.

## Durability and isolation

- PostgreSQL bootstrap creates separate migration, API and Keycloak roles/databases. The API role
  cannot perform DDL or change migration history. A one-shot migration service runs before API.
  PostgreSQL health includes a completed-bootstrap marker, not merely an accepting TCP socket.
- Original paths use server-generated UUIDs; user filenames are metadata only. Storage refuses a
  second API writer. Reads reject symbolic/hard links and verify length, SHA-256 and a bounded
  JSON manifest. RAW/JPEG **envelope screening is not complete camera-format validation**.
- A pending SQL record and quota reservation exist before upload. Bytes are streamed to the same
  volume, checked, `fsync`ed, renamed and directory-synced. A manifest is written durably before
  the final PostgreSQL transaction and HTTP success. The DB and filesystem are not one atomic
  transaction: startup reconciles valid final files whose SQL record is still pending. Incomplete
  staging files are discarded; unrecognized originals are never garbage-collected automatically.
- A corrupt pending final file prevents startup reconciliation; retain it and investigate/restore
  it. A corrupt ready object returns `integrity_error`, not a successful download. No original is
  modified during rendering because rendering is not part of this service.
- Stable UUIDs, operation IDs, revisions, cancellation tombstones and transactional `changes`
  are present for future sync. Sequence advancement and commit serialize on each library row.
  There is no client outbox, public change-feed contract, edit-conflict resolver or ready-photo
  deletion API yet. Manifests retain the original metadata snapshot; future editable metadata
  needs its own version/migration rather than rewriting an original.
- `pgdata` and `originals` are persistent named volumes. `up --force-recreate` retains both.
  Persistent volumes are not a backup: restore requires a consistent PostgreSQL + originals set
  and the independent Keycloak database/signing keys.

## Resources and NPM readiness

Default production Compose publishes no ports. `backend` is a new internal network; `edge` is a
new project network. Connect the existing NPM only to this new edge network after deployment is
authorized. Configure separate Proxy Hosts:

- API hostname → `http://lightcraft-api:8080`.
- Auth hostname → `http://lightcraft-auth:8080`.

Use the existing public HTTPS 443 and Let's Encrypt. Do not expose PostgreSQL or start a second
proxy. For this one-shot stage use `client_max_body_size 512m`, disable request buffering and set
proxy timeouts above the 300-second upload deadline. Stage 4 chunks will use a smaller per-request
limit. Do not enable proxy caching of authorized responses. Restrict the new auth host's admin
paths to administrators while leaving the realm login/token endpoints accessible.

Copy `.env.example` to a private `.env` and choose real HTTPS issuer/hostnames. Keep
`LC_ALLOW_HTTP_OIDC=false`. HTTP JWKS are accepted in production **only** with the explicit
`LC_ALLOW_HTTP_JWKS` flag and exact private `lc-auth:8080` host; public issuer and user traffic use
HTTPS. HTTPS JWKS can instead be configured. Never deploy the local override or promote its
HTTP-only test realm and users as production accounts.

API: 0.5 CPU/512 MiB; PostgreSQL: 0.5 CPU/1 GiB; Keycloak: 0.5 CPU/1.5 GiB; migration job:
0.5 CPU/512 MiB. Compose also caps PIDs, temporary space and log rotation. The API enforces a
shared byte quota including pending uploads and a conservative metadata charge, plus a measured
free-space reserve. This does not create a filesystem hard quota for PostgreSQL/WAL/other
services: set host filesystem limits and monitoring during approved deployment. Measure free
space first; the roughly 200 GB OCI disk is not all available to LightCraft.

## Build and validation

```sh
cargo fmt --manifest-path Cargo.toml -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s tests -p 'test_*.py'
python3 tests/e2e.py                 # isolated running Compose stack, see project variable above
```

`Dockerfile` cross-builds a static Rust binary from source for amd64/arm64 using Rust's bundled
musl target and `rust-lld`, without native codec/C++ libraries. An optional `proxy_ca` BuildKit
secret supplies a combined CA bundle in managed environments; it is never copied into the image.
`Dockerfile.keycloak` optimizes the pinned official image without secrets or a live database.
`Dockerfile.prebuilt` packages a separately built static artifact from `dist/<arch>/lightcraft-cloud`.

Example ARM64 source build on a capable builder:

```sh
docker buildx build --platform linux/arm64 --load -t lightcraft-cloud:arm64 .
```

Native execution, emulation and image manifest inspection are distinct checks. See the
[stage 3 verification report](../docs/cloud-stage3.md) for the checks actually run in Codex. No command here connects to OCI or
changes Immich. Source and integration tests are configured in `.github/workflows/cloud.yml`.
