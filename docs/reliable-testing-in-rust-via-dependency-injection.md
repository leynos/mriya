# 🛡️ Reliable testing in Rust via dependency injection

Writing robust, reliable, and parallelizable tests requires an intentional
approach to handling external dependencies such as environment variables, the
filesystem, or the system clock. Functions that directly call `std::env::var` or
`SystemTime::now()` are difficult to test because they depend on global,
non-deterministic state.

This leads to several problems:

- **Flaky Tests:** A test might pass or fail depending on the environment it
  runs in.
- **Parallel Execution Conflicts:** Tests that modify the same global
  environment variable (`std::env::set_var`) will interfere with each other
  when run with `cargo test`.
- **State Corruption:** A test that panics can fail to clean up its changes to
  the environment, poisoning subsequent tests.

The solution is a classic software design pattern: **Dependency Injection
(DI)**. Instead of a function reaching out to the global state, its
dependencies are provided as arguments. The `mockable` crate offers a
convenient set of traits (`Env`, and `Clock` behind its `clock` feature) to
implement this pattern for common system interactions in Rust.

______________________________________________________________________

## ✨ Mocking environment variables

### 1. Add `mockable`

Add the crate as a regular dependency (it supplies `Env` and `DefaultEnv`) and
enable the `mock` feature for tests only, so `MockEnv` never reaches release
builds. The examples below were checked against `mockable` 3.0.0.

```toml
[dependencies]
mockable = { version = "3.0.0", default-features = false }

[dev-dependencies]
mockable = { version = "3.0.0", default-features = false, features = ["mock"] }
```

### 2. The untestable code (before)

Directly calling `std::env` makes it difficult to test all logic paths
exhaustively.

```rust,no_run
pub fn get_api_key() -> Option<String> {
    match std::env::var("API_KEY") {
        Ok(key) if !key.is_empty() => Some(key),
        _ => None,
    }
}
```

### 3. Refactoring for testability (after)

The function is refactored to accept a generic type that implements the
`mockable::Env` trait.

```rust,no_run
use mockable::Env;

pub fn get_api_key(env: &impl Env) -> Option<String> {
    env.string("API_KEY").filter(|key| !key.is_empty())
}
```

The function's core logic remains unchanged, but its dependency on the
environment is now explicit and injectable.

### 4. Writing isolated unit tests

Tests can use `MockEnv`, a `mockall`-generated mock, to simulate any
environmental condition without touching the actual process environment. Set an
expectation on the accessor the code under test calls (`string` here).

```rust,no_run
#[cfg(test)]
mod tests {
    use super::*;
    use mockable::MockEnv;

    fn env_with(value: Option<&'static str>) -> MockEnv {
        let mut env = MockEnv::new();
        env.expect_string()
            .returning(move |_| value.map(str::to_owned));
        env
    }

    #[test]
    fn test_get_api_key_present() {
        let env = env_with(Some("secret123"));
        assert_eq!(get_api_key(&env), Some("secret123".to_string()));
    }

    #[test]
    fn test_get_api_key_missing() {
        assert_eq!(get_api_key(&env_with(None)), None);
    }

    #[test]
    fn test_get_api_key_present_but_empty() {
        assert_eq!(get_api_key(&env_with(Some(""))), None);
    }
}
```

These tests are fast, completely isolated from each other, and will never fail
due to external state.

### 5. Usage in production code

In production code, inject the default implementation, `DefaultEnv`, which
calls the actual `std::env` functions.

```rust,no_run
use mockable::DefaultEnv;

fn main() {
    let env = DefaultEnv;
    if let Some(api_key) = get_api_key(&env) {
        println!("API Key found!");
    } else {
        println!("API Key not configured.");
    }
}
```

______________________________________________________________________

## 🔩 Handling other non-deterministic dependencies

This dependency injection pattern also applies to other non-deterministic
dependencies, such as the system clock. The `mockable` crate provides a `Clock`
trait for this purpose, behind the `clock` feature. That feature pulls in
`chrono`, so the trait deals in `chrono`'s `DateTime<Utc>` and
`DateTime<Local>` rather than `std::time::SystemTime`:

### Untestable code

```rust,no_run
use chrono::{DateTime, Duration, Utc};

fn is_cache_entry_stale(creation_time: DateTime<Utc>) -> bool {
    let timeout = Duration::seconds(300);
    Utc::now() - creation_time > timeout
}
```

### Testable refactor

```rust,no_run
use chrono::{DateTime, Duration, Utc};
use mockable::Clock;

fn is_cache_entry_stale(creation_time: DateTime<Utc>, clock: &impl Clock) -> bool {
    let timeout = Duration::seconds(300);
    clock.utc() - creation_time > timeout
}
```

### Testing with `MockClock`

`MockClock` is a `mockall` mock of the trait, not a self-advancing clock. Each
test sets the reading it wants, which is what makes the two cases independent:

```rust,no_run
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_is_not_stale() {
        let creation_time = Utc::now();
        let mut clock = MockClock::new();
        clock
            .expect_utc()
            .return_const(creation_time + Duration::seconds(100));
        assert!(!is_cache_entry_stale(creation_time, &clock));
    }

    #[test]
    fn test_cache_is_stale() {
        let creation_time = Utc::now();
        let mut clock = MockClock::new();
        clock
            .expect_utc()
            .return_const(creation_time + Duration::seconds(301));
        assert!(is_cache_entry_stale(creation_time, &clock));
    }
}
```

In production, `DefaultClock` is the implementation to pass.

The `clock` feature is separate from `mock`, so a crate that uses the trait
needs both features in its dev-dependencies. `mockable` does not re-export
`chrono`, so the types in the function signature must be imported from `chrono`
declared as a direct dependency:

```toml
[dependencies]
chrono = "0.4"
mockable = { version = "3.0.0", default-features = false, features = ["clock"] }

[dev-dependencies]
mockable = { version = "3.0.0", default-features = false, features = ["clock", "mock"] }
```

______________________________________________________________________

## 📌 Key takeaways

- **The Problem is Non-Determinism:** Directly accessing global state like
  `std::env` or `SystemTime::now` makes code difficult to test exhaustively.
- **The Solution is Dependency Injection:** Pass dependencies into functions as
  arguments.
- **Use** `mockable` **Traits:** Abstract dependencies behind traits such as
  `impl Env` or `impl Clock`.
- **`Mock*` for Tests:** Use `MockEnv` and `MockClock` in unit tests for
  isolated, deterministic control.
- **`Default*` for Production:** Use `DefaultEnv` and `DefaultClock` in the
  application to interact with the actual system.
- **`DefaultEnv` is read-only:** `Env` exposes no setters, so it cannot change
  the process environment. Tests that genuinely need the live process
  environment (for example, code that reads `HOME` through a seam-less
  dependency) should use scoped `temp_env::with_var` closures, never direct
  `set_var`. For unit tests, `MockEnv` is preferable.
- **`Clock` is `utc()` and `local()`:** `mockable` 3.x returns `chrono`
  `DateTime` values, and `MockClock` is a `mockall` mock whose reading each
  test sets, so both features are needed in dev-dependencies.
