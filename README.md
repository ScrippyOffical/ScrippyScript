# scrippy-script-core

[![ci](https://github.com/ScrippyOffical/ScrippyScript/actions/workflows/ci.yml/badge.svg)](https://github.com/ScrippyOffical/ScrippyScript/actions/workflows/ci.yml)

Lexer, parser and syntax tree for ScrippyScript (`.scp`), the brace-based
language documented in [`docs/LANGUAGE.md`](docs/LANGUAGE.md). This is the Rust
front end of the Scrippy toolchain, shared by the native builds and a WASM build.

## Dependencies

`logos` 0.16, for the tokenizer. It builds the matcher at compile time, so
tokens are slices of the input with no allocation.

The parser is hand-written recursive descent, with precedence climbing for
binary operators. Keeping it in the crate means spans and error messages are
ours to control. MSRV is 1.80, which is what `logos` requires.

## What the parser handles

- statements separated by newlines or `;`
- `let`, `const`, `global`, with optional type hints (`x: number`, `p: player?`)
- `fun` declarations and `fun(...) {}` expressions, default parameter values
- `if` / `else if` / `else`, `while`, `for x in ...`, `break`, `continue`, `return`
- assignment to names, members and indexes, including `+= -= *= /= %=`
- calls, member access, indexing, `new Type(...)`, safe access `a?.b()`
- arithmetic, comparison, `&& || !` and `and or not`, `??`, ternary `?:`
- ranges `1..5` and `0..<10`, lists, maps
- double-quoted strings with `${...}` interpolation, single-quoted literals,
  `\n \t \r \\ \" \' \$ \{ \} \0` escapes
- comments: `#`, `//`, `/* ... */`
- decimal, `0x` hex, `1_000` separators, floats and exponents
- braces may sit on the same line as the header or the next one
- member names and map keys may be keywords (`player.default`, `{ match: 1 }`)

Strings inside `${...}` should be single-quoted (`"${join('x')}"`), since the
outer double-quote scanner treats a raw `"` as the end of the literal. Map keys
must be static text: `${...}` in a key is a parse error rather than a silently
literal string.

Nesting deeper than 200 levels is rejected with a normal error, so a generated
or hostile script cannot overflow the stack.

Event handlers (`on join`), `command` blocks, `every`/`after` scheduling,
`match`, `try`/`catch` and `import` are recognised but rejected with a specific
message, e.g. `event handlers aren't supported yet`. They have AST shapes in the
Java engine today; adding them here is the next step, and the parser is laid out
so each is a single `parse_*` function.

## Use

```rust
let src = r#"
fun twice(n: number) {
    return n * 2
}

let name = "Steve"
ScrippyAI.ask("hello ${name}, twice 21 is ${twice(21)}")
"#;

match scrippy_script_core::parse(src) {
    Ok(script) => println!("{script:#?}"),
    Err(errors) => {
        for err in errors {
            eprintln!("{}", err.render(src));
        }
    }
}
```

`Error::render` produces something like:

```
2:12: expected an expression, found `}`
  let x = 1 +
            ^
```

## Layout

```
src/
  span.rs     byte spans and line/column lookup
  error.rs    lexer + parser errors and source snippets
  lexer.rs    logos token definitions, tokenize()
  ast.rs      Script, Stmt, Expr and friends
  parser.rs   recursive descent parser
examples/
  parse.rs    prints the AST of a file
docs/
  LANGUAGE.md language reference
.scp/
  demo.scp    sample script used by the example
```

## WASM

The library does no I/O, uses no threads and contains no `unsafe`, so
`wasm32-unknown-unknown` is a supported target with no feature flags. To keep the
binary small, build with the release profile in `Cargo.toml` (`lto`, `opt-level
= "s"`). A `cdylib` crate type and `wasm-bindgen` wrapper belong in a separate
crate so this one stays usable from native Rust.

## Checks

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run --example parse -- .scp/demo.scp
```

CI runs the same list plus an MSRV build on 1.80.

## License

GPL-3.0, see [LICENSE](LICENSE).
