# Security policy

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on this repository
(Security → Report a vulnerability). Please do not open public issues for
security reports. You will get an acknowledgment within a week; fixes ship as
patch releases with a CHANGELOG entry.

## Supported versions

The latest released `0.x` version. Pre-1.0, fixes are not backported.

## Threat model

proef is a **test tool with a deliberately modest threat model**: it protects
secret material *at rest* and keeps it *out of every output*, and it does not
attempt to defend a compromised host.

**What proef guarantees:**

- The secret store (`.proef-secrets.json`) holds only XChaCha20-Poly1305
  ciphertext (`enc:v1:` envelope) — safe to commit and share.
- Secret **values never appear in any sink**: artifacts carry `{{name}}`
  placeholders, events/logs/reports are value-redacted at the sink boundary
  (property-tested), and a `saveAs: global` capture whose value equals a
  known secret is refused rather than persisted to the plaintext
  `.proef-state.json`.
- Sensitive files (`.proef-secrets.json`, the key file, `.proef-state.json`)
  are created `0600`, private from the first byte. `proef doctor` warns when
  permissions have drifted.
- Request file bodies are confined to the suite directory (hurl's
  `context_dir` sandbox); artifact asset copying rejects absolute and `..`
  paths.
- A **fragment corpus is read, never written** (ADR-0018). Pointing
  `[run] fragments` at `.hurl` files somebody else owns is one-directional:
  `proef fmt` refuses them in both discovery branches, and the declared root
  is the confinement boundary — nothing outside it is scanned. Files come back
  byte-identical, which an integration test asserts.
- A **renamed secret is still never materialized**. `bind: { token: "${secret:x}" }`
  lets a foreign corpus keep its own variable name; the value still travels via
  `insert_secret` and never enters the artifact. Mixing a secret into a larger
  bound value is *refused* (`lower::secret_in_composite_bind`) rather than
  quietly written out, because injecting the joined string would require putting
  it in the artifact.
- **TLS verification is on unless a profile says otherwise, and saying so is
  loud.** `[http] insecure = true` exists because staging environments really do
  present self-signed certificates, but a suite that goes green without
  verifying one has not proved what a green suite normally proves. Every run
  with it active prints a warning naming the profile that set it. The run record
  deliberately carries no config, so that warning is the whole audit trail —
  which is why it cannot be suppressed.
- **mTLS credentials are file paths, not values.** `[http] client-cert` /
  `client-key` name files; proef reads no key material into its own memory and
  writes none into any artifact. A `client-key` without a `client-cert` is exit
  2 rather than a silent pass-through: libcurl would accept the pair and then
  present nothing, so the failure would surface at the server as an
  authentication error naming nothing about the cause.
- Release binaries are built with `cargo auditable` (dependency trees stay
  scannable) on cache-isolated CI runners.

**What proef does not defend against:**

- A compromised host or user account: the key file lives on disk, decrypted
  values live in process memory (no zeroize — hurl holds its own copies),
  and `PROEF_KEY`/`PROEF_SECRET_*` are readable from the process
  environment.
- Malicious suites: packs execute arbitrary HTTP requests by design; run
  suites you trust.
- **Credentials written into `proef.toml`.** There is deliberately no
  `[http] user` or `netrc` key — a password belongs in the secret store, where
  it is encrypted at rest and masked out of every sink. `[http] proxy` is the
  one edge: a proxy URL embedding credentials is plaintext in a file you
  probably commit, and proef cannot mask a value it was never told is a secret.

If your environment needs more than this, inject secrets per run via
`PROEF_SECRET_<NAME>` from a real secret manager and skip the store entirely.
