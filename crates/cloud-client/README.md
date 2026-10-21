# lightcraft-cloud-client

Native, UI-independent (L0) client for the independent LightCraft Cloud REST API.
All methods block and must run on a worker thread. The egui adapter is in
`crates/ui-egui/src/cloud/`; this crate does not depend on the local editor or catalog.

- HTTPS root server addresses; HTTP only on loopback for local development.
- Bounded configuration/discovery/API responses, explicit timeouts, no HTTP redirects.
- OIDC authorization code + PKCE S256 through the host's system browser. State, nonce,
  issuer, audience and RS256 ID-token validation; fixed loopback callback on port 47861.
- Access tokens in memory; refresh tokens in macOS Keychain, Windows Credential Manager
  or Linux Secret Service through `keyring`. No plaintext fallback. A locked/unavailable
  store permits explicitly reported session-only login. Logout removes the saved credential
  and attempts Keycloak session revocation; failed removal does not report successful logout.
- Paged libraries/photos, library creation, immutable one-shot RAW/JPEG transfers with
  SHA-256, progress and cooperative cancellation. A stalled network read may delay cancellation
  until the transfer timeout. No chunks, persistent outbox or automatic synchronization.
- Downloads use a temporary file in the destination directory, verify length/checksum,
  sync it and persist without replacing an existing file. A 64 MiB free-space reserve is required.

`cargo test -p lightcraft-cloud-client` runs protocol and file-safety tests without an
external service. The UI crate has an opt-in real Keycloak/PostgreSQL/storage test;
see [desktop testing](../../docs/cloud-desktop.md). The crate is empty on WebAssembly.
