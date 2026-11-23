## What changed

## Checklist

- [ ] Tests cover the change
- [ ] Added a server feature to `protocol::features::ALL` for anything new a client may rely on, and a client requirement only where a client cannot work without it ([docs/versioning.md](../docs/versioning.md))
- [ ] Bumped the major version if the server breaks something older clients use
