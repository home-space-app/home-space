# Home Space

- When the server gains an endpoint, field, query parameter or event a client may rely on, add a
  feature to `shared/protocol/src/features.rs` (`ALL` and a display name). Add it to a client's
  `REQUIRES` only when that client cannot work without it; never when it falls back on older
  servers. A breaking server change bumps the major version. See `docs/versioning.md`.
