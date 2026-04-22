# Contributing

Lore is in active early development and is **not accepting external contributions** at this time.

This may change once the project reaches a stable release. If you have questions or feedback, please
use [GitHub Discussions](https://github.com/attila/lore/discussions).

**Security vulnerabilities** should not be reported through Discussions. See
[SECURITY.md](SECURITY.md) for reporting instructions.

## Design invariants

Architectural rules that shape how the codebase is structured live in
[`docs/architecture.md`](docs/architecture.md). Read it before introducing a new read surface, a new
write path, or any runtime disk access — a few of the design choices look odd without the motivating
incidents.
