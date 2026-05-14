---
title: "Testing env-var-reading Rust code in edition 2024 (`temp-env`) and the `std::env::home_dir` gotcha"
date: 2026-05-14
category: best-practices
module: src/config.rs
problem_type: best_practice
component: testing_framework
severity: medium
applies_when:
  - "Writing unit tests in Rust edition 2024 that need to set or unset process env vars"
  - "Adopting a crate that internally calls `std::env::home_dir` (e.g. `etcetera`) on Rust 1.85+"
  - "Replacing parameter-injected env helpers with code that reads `std::env` directly"
  - "Tests must run under default `cargo test` (cannot rely on `#[ignore]`'d integration tests)"
tags:
  - rust
  - edition-2024
  - testing
  - env-vars
  - temp-env
  - etcetera
  - home-dir
related_components:
  - tooling
  - documentation
---

# Testing env-var-reading Rust code in edition 2024 (`temp-env`) and the `std::env::home_dir` gotcha

## Context

Rust 1.85 re-stabilised `std::env::home_dir()` with a behaviour change that bites silently: when
`$HOME` is unset, it no longer returns `None`. It falls back to `getpwuid_r` against `/etc/passwd`
and hands back whatever the passwd database says. Crates that delegate to it inherit the new
contract — including `etcetera`, whose `etcetera::home_dir()` and `base_strategy::Xdg::new()`
therefore stop erroring on unset `$HOME`.

If your CLI promises operator-facing wording like
`Cannot determine config directory: $HOME is not set`, that promise breaks the moment you swap a
hand-rolled XDG resolver for `etcetera`. The breakage is silent: tests that assert "errors when HOME
is unset" only fail in environments where `getpwuid_r` happens to also fail, which is almost never
CI.

The downstream problem is testing. Helpers that read the process env directly (via `etcetera`,
`std::env::var_os`, etc.) can't be unit-tested by passing fake values in — the only lever is
mutating the real process env. In edition 2024 that means `unsafe` (which an `unsafe_code = "deny"`
crate rejects) and a parallel-test data race on shared env vars.

## Guidance

**1. Pre-check `$HOME` before calling any XDG resolver.** Treat empty as unset, matching XDG 0.8
semantics for `XDG_*_HOME`:

```rust
fn xdg_strategy(purpose: &str) -> anyhow::Result<Xdg> {
    if std::env::var_os("HOME").is_none_or(|v| v.is_empty()) {
        return Err(anyhow::anyhow!(
            "Cannot determine {purpose} directory: $HOME is not set. \
             Use --config to specify a path."
        ));
    }
    Xdg::new().map_err(|_| {
        anyhow::anyhow!(
            "Cannot determine {purpose} directory: $HOME is not set. \
             Use --config to specify a path."
        )
    })
}
```

Don't trust the resolver crate to surface the error you want. Gate it yourself.

**2. Use `temp-env` for any unit test that needs to mutate the process env.** Add as dev-dep:

```toml
[dev-dependencies]
temp-env = "0.3"
```

Wrap the env-sensitive call:

```rust
#[test]
fn xdg_config_home_overrides_default() {
    temp_env::with_vars(
        [
            ("XDG_CONFIG_HOME", Some("/custom/config")),
            ("HOME", Some("/home/user")),
        ],
        || {
            let path = default_config_path().unwrap();
            assert_eq!(path, PathBuf::from("/custom/config/lore/lore.toml"));
        },
    );
}
```

`with_vars` takes `[(K, Option<V>)]` — `Some` sets, `None` unsets. It serialises against other
`temp_env` callers via a reentrant mutex (`SERIAL_TEST`) and restores prior values on drop, even on
panic. Nested `with_vars` calls are safe.

**3. Pin every invariant the resolver crate gives you for free.** Don't assume — assert. Empty
`XDG_CONFIG_HOME` falling back to `$HOME/.config` is an invariant that survives a crate swap only if
a test holds it down:

```rust
#[test]
fn empty_xdg_config_home_falls_back_to_home() {
    temp_env::with_vars(
        [("XDG_CONFIG_HOME", Some("")), ("HOME", Some("/home/user"))],
        || {
            let path = default_config_path().unwrap();
            assert_eq!(path, PathBuf::from("/home/user/.config/lore/lore.toml"));
        },
    );
}
```

## Why This Matters

The `home_dir` change is a textbook silent contract drift: the function still exists, still returns
the same type, still works most of the time. The failure mode is "your error message is wrong in
exactly the operator-debugging scenario where the message matters most". Pre-checking `$HOME` keeps
the operator-facing contract independent of upstream crate behaviour and immune to future
re-stabilisations.

The testing piece matters because the alternatives are worse:

- **Wrap in `unsafe`** — requires lifting `unsafe_code = "deny"`, even for test-only code, which
  weakens a useful crate-wide invariant for an unrelated reason.
- **Move to integration tests** (`tests/<name>.rs` spawning the binary with
  `Command::cargo_bin("lore").env(...)`) — each test gets a fresh process, no race. Legitimate
  option, but hides invariants behind `cargo test --test <name>` and adds compile-link cost per
  case. Worse if your existing integration tests are `#[ignore]`'d behind external services (e.g. a
  running Ollama), because path-resolution coverage then disappears from default `cargo test` runs.
- **Roll your own mutex around `set_var`** — reinvents `temp-env`, badly, and still needs `unsafe`.

`temp-env` keeps env-driven coverage in unit tests, off the `unsafe` blast radius, and serialised
against itself — which is enough when your tests are the only env-mutating code in the module.

## When to Apply

Apply the **`$HOME` pre-check** whenever:

- You depend on `etcetera`, `dirs`, `directories`, `xdg`, or any crate that ultimately calls
  `std::env::home_dir()` or `getpwuid_r`.
- Your CLI documents or tests an exact error message for unset `$HOME`.
- You're on Rust 1.85+ (i.e. effectively now).

Apply **`temp-env`** whenever:

- A unit test needs a specific env var value and the code under test reads env directly.
- You're on Rust edition 2024 and have `unsafe_code = "deny"` (or want to keep it that way).
- The env-mutating code in the module is contained — i.e. all readers go through `temp_env`-wrapped
  tests, or run on a single thread you control.

Do **not** rely on `temp-env`'s serialisation when:

- Other code in the same test binary mutates env via `std::env::set_var` directly.
- Background threads spawned by the code under test read env outside the closure.
- You need cross-process isolation — use integration tests with `Command::env(...)` instead.

## Examples

**Before** — silent contract breakage after swapping to `etcetera`:

```rust
fn config_dir() -> anyhow::Result<PathBuf> {
    let xdg = Xdg::new()
        .map_err(|_| anyhow::anyhow!("Cannot determine config directory: $HOME is not set"))?;
    Ok(xdg.config_dir().join("lore"))
}
// On Rust 1.85+ with HOME unset: Xdg::new() succeeds via /etc/passwd fallback.
// Operator sees a config written under /var/lib/some-system-user/.config/lore. Confusing.
```

**After** — explicit pre-check, contract held:

```rust
fn config_dir() -> anyhow::Result<PathBuf> {
    Ok(xdg_strategy("config")?.config_dir().join("lore"))
}
// Unset or empty HOME -> deterministic, documented error. /etc/passwd never consulted.
```

**Before** — un-runnable in edition 2024 with `unsafe_code = "deny"`:

```rust
#[test]
fn xdg_config_home_overrides_default() {
    std::env::set_var("XDG_CONFIG_HOME", "/custom/config"); // error: call to unsafe function
    std::env::set_var("HOME", "/home/user");
    let path = default_config_path().unwrap();
    assert_eq!(path, PathBuf::from("/custom/config/lore/lore.toml"));
    // also: races against any sibling test touching the same vars
}
```

**After** — safe, serialised, restores prior state on drop or panic:

```rust
#[test]
fn xdg_config_home_overrides_default() {
    temp_env::with_vars(
        [
            ("XDG_CONFIG_HOME", Some("/custom/config")),
            ("HOME", Some("/home/user")),
        ],
        || {
            let path = default_config_path().unwrap();
            assert_eq!(path, PathBuf::from("/custom/config/lore/lore.toml"));
        },
    );
}

#[test]
fn unset_home_is_a_clear_error() {
    temp_env::with_vars(
        [("HOME", None::<&str>), ("XDG_CONFIG_HOME", None::<&str>)],
        || {
            let err = default_config_path().unwrap_err().to_string();
            assert!(err.contains("$HOME is not set"));
            assert!(err.contains("Use --config"));
        },
    );
}

#[test]
fn empty_xdg_config_home_falls_back_to_home() {
    temp_env::with_vars(
        [("XDG_CONFIG_HOME", Some("")), ("HOME", Some("/home/user"))],
        || {
            let path = default_config_path().unwrap();
            assert_eq!(path, PathBuf::from("/home/user/.config/lore/lore.toml"));
        },
    );
}
```

## Related

- [`docs/solutions/test-failures/insta-snapshot-cargo-pkg-version-redaction-2026-05-01.md`](../test-failures/insta-snapshot-cargo-pkg-version-redaction-2026-05-01.md)
  — sibling Rust 1.85 + edition 2024 testing concern; covers env-derived snapshot brittleness while
  this entry covers env-var mutation in unit tests.
- PR [#52](https://github.com/attila/lore/pull/52) — original refactor where both findings surfaced,
  including the `xdg_strategy` helper landing in `src/config.rs`.
- [`temp-env` on crates.io](https://crates.io/crates/temp-env) — current version 0.3.6,
  `MIT OR Apache-2.0`, single transitive dep `parking_lot`.
- [Rust 1.85 release notes](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0.html) —
  `std::env::home_dir` re-stabilisation.
