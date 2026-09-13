# Developer Telemetry Profile

## Goal

Give mdkb developers enough local evidence to evaluate real recall behavior without
persisting prompt text or making telemetry the default for end users.

## Requirements

- `mdkb setup developer` enables per-query telemetry for the current initialized
  repository and defaults retention to 30 days.
- Setup edits only the managed `[telemetry]` keys in `.mdkb/config.toml`, preserving
  unrelated settings and comments, and is idempotent.
- Query text is never persisted or exported.
- Query correlation uses HMAC-SHA-256 with a random, repository-local 256-bit key,
  not an unsalted digest.
- The key is stored at `.mdkb/telemetry.key`, outside Git, with owner-only Unix
  permissions. A missing key is generated lazily so existing explicit
  `query_events = true` configurations remain functional.
- Every telemetry write removes events beyond the configured retention window.
- `mdkb metrics status` reports whether collection is enabled, retention, key
  presence, and stored event count without exposing the key.
- `mdkb metrics purge --yes` deletes query events. Without `--yes`, it refuses
  before mutation.
- The developer profile does not enable always-on prompt recall or change hook
  behavior. That is a separate product decision.
- Default configuration remains telemetry-off for end users.

## Approach

- Keep `telemetry.query_events` as the activation flag and add
  `telemetry.retention_days` with a 30-day default.
- Add a small telemetry privacy module backed by `ring` for key generation and
  HMAC-SHA-256.
- Route setup through a TOML-preserving editor and atomic same-directory write.
- Apply retention in the existing guarded telemetry write so daemon writer
  serialization remains the single mutation path.

## Trade-offs

- Per-repository keys prevent comparing identical prompts across repositories.
  This is intentional: cross-project correlation is unnecessary for aggregate
  quality metrics and would enlarge the privacy boundary.
- Retention cleanup adds one indexed delete to each recorded recall. Query volume
  is low and bounded; this is simpler and more reliable than a background job.
- HMAC protects predictable prompts against offline dictionary matching as long as
  the local key remains private. A process with repository filesystem access can
  still read both the key and database; filesystem isolation remains required.

## Validation

- Unit tests cover normalization, HMAC determinism/key separation, retention, and
  destructive-command refusal.
- CLI integration tests cover setup creation, preservation, idempotency, status,
  and purge.
- Existing hook telemetry tests prove no prompt text is persisted.
- Formatting, linting, targeted tests, full test suite, and release build must pass.
