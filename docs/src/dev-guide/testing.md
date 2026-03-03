# Testing

## Running tests

Run the full test suite:

```sh
cargo test
```

Run tests for a specific module:

```sh
cargo test app::tests          # App module tests
cargo test signal::client::tests  # Signal client tests
cargo test db::tests           # Database tests
cargo test input::tests        # Input parsing tests
```

Run a single test by name:

```sh
cargo test test_name
```

## rstest

Tests use [rstest](https://docs.rs/rstest/) for fixtures and parameterization. The crate
is declared in `[dev-dependencies]`.

### Fixtures

Two `#[fixture]` functions provide pre-built test objects:

- **`app()`** in `app.rs` — returns an `App` with an in-memory DB and connected state.
- **`db()`** in `db.rs` — returns an in-memory `Database`.

To use a fixture, mark the test `#[rstest]` and add the fixture as a parameter:

```rust
#[rstest]
fn my_test(mut app: App) {
    app.input_buffer = "/quit".to_string();
    // ...
}
```

### Parameterized tests

When multiple tests share the same assertion logic but differ in inputs, use
`#[case]` to collapse them into a single function:

```rust
#[rstest]
#[case("/quit", InputAction::Quit)]
#[case("/q",    InputAction::Quit)]
#[case("/help", InputAction::Help)]
fn command_returns_expected_action(#[case] input: &str, #[case] expected: InputAction) {
    assert_eq!(parse_input(input), expected);
}
```

Each `#[case]` produces a separate entry in `cargo test` output, so individual
failures are easy to identify.

### When to use what

| Situation | Approach |
|---|---|
| Test needs an `App` or `Database` | Use the fixture (`#[rstest]` + parameter) |
| 3+ tests with identical structure, different data | Parameterize with `#[case]` |
| 2 tests with significantly different setup | Keep them separate |
| Test doesn't need a fixture | Plain `#[test]` is fine |

### Best practices

- **Prefer `#[case]` over copy-paste.** If you're writing a new test and an existing
  parameterized test covers the same assertion pattern, add a `#[case]` instead of a
  new function.
- **Keep case data simple.** Strings, numbers, and booleans work well in `#[case]`
  attributes. For complex types, build them inside the test body using a label parameter
  and `match`.
- **Add a `_label` parameter** when case data alone doesn't make the purpose obvious.
  This shows up in test names (e.g., `my_test::case_basic`).
- **Don't over-parameterize.** If merging tests requires a `match` with completely
  different setup logic per arm, separate tests are clearer.

## Test modules

Tests are defined as `#[cfg(test)] mod tests` blocks within each source file.

### `db.rs` tests

Database tests use `Database::open_in_memory()` for isolated, fast test instances.
Coverage includes:

- Schema migration and table creation
- Conversation upsert and loading
- Name updates on conflict
- Message insertion and retrieval (ordering)
- Unread count with read markers
- System message exclusion from unread counts
- Conversation ordering by most recent message
- Mute flag round-trip
- Last message rowid tracking

### `input.rs` tests

Input parser tests cover:

- Plain text passthrough
- Empty and whitespace-only input
- All commands and their aliases (`/join`, `/j`, `/part`, `/p`, etc.)
- Commands with and without arguments
- Unknown command handling

### `app.rs` tests

Application state tests cover signal event handling, conversation management,
and mode transitions.

### `signal/client.rs` tests

Signal client tests cover JSON-RPC parsing and event routing.

## Demo mode for manual testing

```sh
cargo run -- --demo
```

Demo mode populates the UI with dummy conversations and messages without
requiring signal-cli. This is the easiest way to manually test UI changes,
keybindings, and rendering.

## Linting

The project enforces zero clippy warnings:

```sh
cargo clippy --tests -- -D warnings
```

CI runs this on every push and pull request. Fix all warnings before pushing.
