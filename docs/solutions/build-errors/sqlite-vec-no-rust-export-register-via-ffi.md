---
title: "sqlite-vec has no Rust-level export — register via extern C and transmute"
date: 2026-03-30
category: build-errors
module: database
problem_type: build_error
component: database
symptoms:
  - "sqlite_vec::sqlite3_vec_init does not exist as a Rust function — crate only links a C static library"
  - "The scaffold's load_extension API does not exist in rusqlite 0.39"
  - "unsafe_code = deny blocks naive unsafe usage without targeted #[allow(unsafe_code)]"
root_cause: wrong_api
resolution_type: code_fix
severity: high
tags:
  - sqlite-vec
  - rusqlite
  - ffi
  - unsafe
  - auto-extension
  - transmute
  - extern-c
---

# sqlite-vec has no Rust-level export — register via extern C and transmute

## Problem

The `sqlite-vec` 0.1.7 crate needs to be registered with `rusqlite` 0.39 as a SQLite extension, but
the crate does not provide a callable Rust function. The scaffold's approach
(`conn.load_extension()`) no longer exists in rusqlite 0.39, and assuming
`sqlite_vec::sqlite3_vec_init` is a Rust export is wrong.

## Symptoms

- `conn.load_extension(sqlite_vec::loadable_extension_path(), None)` fails to compile — the
  `load_extension` method does not exist on `rusqlite::Connection` in rusqlite 0.39.
- Attempting to call `sqlite_vec::sqlite3_vec_init(...)` directly as a Rust function fails because
  the symbol exists only at the C ABI level after static linking of `libsqlite_vec0.a`.

## What Didn't Work

- **The scaffold's `load_extension` approach:**
  `conn.load_extension(sqlite_vec::loadable_extension_path(), None)` — this API was removed in
  rusqlite 0.39.
- **Assuming sqlite-vec provides a Rust-level export:** It doesn't. The crate's `build.rs` uses the
  `cc` crate to compile C source files and link `libsqlite_vec0.a` statically. The
  `sqlite3_vec_init` symbol is only available through the C linker, not as a normal Rust function.

## Solution

Use `extern "C"` to declare the FFI symbol, `std::mem::transmute` to convert the function pointer to
the type expected by `sqlite3_auto_extension`, and `std::sync::Once` to ensure registration happens
exactly once per process.

```rust
use std::sync::Once;

static SQLITE_VEC_INIT: Once = Once::new();

#[allow(unsafe_code)]
fn register_sqlite_vec() {
    SQLITE_VEC_INIT.call_once(|| {
        // SAFETY: sqlite3_vec_init is the documented entrypoint for the sqlite-vec
        // extension. transmute converts the bare function pointer into the type
        // expected by sqlite3_auto_extension. This is the pattern used in the
        // sqlite-vec crate's own test suite.
        unsafe {
            type AutoExtFn = unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::ffi::c_int;
            rusqlite::ffi::sqlite3_auto_extension(Some(
                std::mem::transmute::<*const (), AutoExtFn>(
                    sqlite_vec::sqlite3_vec_init as *const (),
                ),
            ));
        }
    });
}
```

Call `register_sqlite_vec()` at the top of `KnowledgeDB::open()`, before `Connection::open()`. After
registration, every subsequent connection automatically loads the vec0 virtual table module.

## Why This Works

The `sqlite-vec` crate's `build.rs` compiles C source code via the `cc` crate and produces
`libsqlite_vec0.a`, which is linked statically. The `sqlite3_vec_init` symbol exists at the C ABI
level but has no Rust function wrapper. The `sqlite_vec::sqlite3_vec_init` path resolves to this
extern symbol.

`rusqlite::ffi::sqlite3_auto_extension` expects `Option<unsafe extern "C" fn(...)>` with the
standard SQLite extension init signature. `std::mem::transmute` bridges the gap by casting
`*const ()` (the erased function pointer) to the exact `AutoExtFn` type alias. This is sound because
the C function's actual signature matches the declared type.

The `Once` guard ensures registration happens exactly once per process. This is process-global — all
connections opened afterward get sqlite-vec automatically. This replaces the old per-connection
`load_extension` approach entirely.

## Prevention

- **Write a minimal FFI verification test first.** Before building the full database layer, write a
  test that creates a `vec0` virtual table, inserts a row, and reads it back. This catches
  registration failures immediately.
- **Check what a build-only crate actually provides.** Read the crate's `build.rs` and `lib.rs`
  before assuming it exports callable Rust functions. If `build.rs` uses `cc::Build` and `lib.rs`
  only has `extern "C"` declarations, you're dealing with a C static library, not a Rust API.
- **Use targeted `#[allow(unsafe_code)]` with SAFETY comments.** Scope the allow to the single
  function that needs it, not the entire module. Document in a `// SAFETY:` comment exactly why the
  transmute is sound.
- **Verify rusqlite API compatibility early.** When upgrading rusqlite across major versions (e.g.,
  0.31 to 0.39), check the changelog for removed methods before writing code against the old API.

## Related Issues

- Origin plan: `docs/plans/2026-03-27-001-feat-scaffold-porting-plan.md` (Key Technical Decisions:
  sqlite-vec registration)
- Phase 0 requirements: R5 (`unsafe_code = "deny"`) established the policy requiring targeted allows
- Implementation: `src/database.rs` (`register_sqlite_vec()` function)
- Versions: rusqlite 0.39.0, sqlite-vec 0.1.7, Rust 1.85 (edition 2024)
