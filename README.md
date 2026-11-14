# ScrippyScript

Disclaimer: AI code is used in said project, though all ideas are made by a human.

[![ci](https://github.com/ScrippyOffical/ScrippyScript/actions/workflows/ci.yml/badge.svg)](https://github.com/ScrippyOffical/ScrippyScript/actions/workflows/ci.yml)

ScrippyScript (`.scp`) is a small, readable, dynamically typed language with
Rust-shaped syntax. Programs compile to native executables and run without
Minecraft. The same language also has a Paper plugin host for server scripts.

This repository holds:

- `scrippy-script-core` - the Rust lexer, parser and syntax tree
- `scrippy` - the native compiler and command line

The Paper plugin lives outside this repository. It interprets scripts and adds
events, commands, scheduling, imports and Bukkit reflection. Those are host
features, not part of the core language.

The full language reference is [`docs/LANGUAGE.md`](docs/LANGUAGE.md). A
plain-text handout lives in [`docs/SCRIPPYSCRIPT.txt`](docs/SCRIPPYSCRIPT.txt).

## Quick start

You need Rust 1.80 or newer and a C compiler (`cc`, or whatever `$CC` points
at). The compiler emits C and lets the system compiler produce the executable,
so no LLVM libraries are required.

```sh
cargo build --release -p scrippy
./target/release/scrippy run examples/hello.scp
```

To keep the executable:

```sh
./target/release/scrippy build examples/hello.scp -o hello
./hello
```

A program looks like this:

```scp
fun greet(name, greeting = "Hello") {
    return "${greeting}, ${name}!"
}

let scores = { alice: 3, bob: 7 }
for name in scores.keys() {
    print("${greet(name)} score=${scores[name]}")
}
```

## Command line

```
scrippy run <file.scp>                  compile and run
scrippy build <file.scp> [-o path]      compile to a native executable
scrippy check <file.scp>                parse and check without a C compiler
scrippy emit-c <file.scp> [-o path]     write the generated C
```

`build` uses `$CC` when it is set, otherwise `cc`. `--keep-c` keeps the
generated C next to the script for `run`, or next to the output for `build`.

## What the native compiler supports

- `let`, `const`, `global`; assignment to names, members and indexes
- `fun` declarations and `fun(...) { ... }` expressions, defaults, `args`
- `if`, `else if`, `else`, `while`, `for x in ...`, `break`, `continue`, `return`
- integers, floats, booleans, null, strings with `${...}`, lists and maps
- arithmetic, comparisons, `&& || !`, `and or not`, `??`, `?:`
- ranges `1..5` and `0..<5`
- calls, member access, indexing, `?.`
- closures and recursion
- the standard functions listed in [`docs/LANGUAGE.md`](docs/LANGUAGE.md)

Host-only syntax is rejected with a clear error. The shared Rust parser does
not parse `on`, `command`, `every`, `after`, `match`, `try`/`catch`/`throw` or
`import`; the native compiler additionally rejects `new`, because there is no
Java host to construct objects. A native program cannot use `player`,
`ScrippyAI`, `config` or any other plugin object.

The compiler is not a type checker. Type hints are accepted and ignored; the
Paper interpreter converts number, string and boolean parameters at call time.
`const` is enforced at compile time.

## Runtime model

The generated program is C with a small runtime library. Values are dynamic;
integers are 64-bit, floats are doubles and strings are UTF-8. Map keys are
converted to strings. Lists and maps preserve insertion order.

Memory is managed by a mark-sweep collector. Collection runs at statement
boundaries, so values held by a call expression are still reachable while a
called function allocates. Closures capture their defining frame by reference.

This is a native backend, not a native-speed version of Rust. Every value is
tagged, every call goes through a dispatcher and maps use a linear scan. A
compiled program starts fast and avoids the Paper interpreter, but its
throughput is scripting-language throughput.

## Library use

`scrippy-script-core` can be used on its own:

```rust
let src = r#"
fun twice(n: number) {
    return n * 2
}

let name = "Steve"
print("hello ${name}, twice 21 is ${twice(21)}")
"#;

match scrippy_script_core::parse(src) {
    Ok(script) => println!("{script:#?}"),
    Err(errors) => {
        for error in errors {
            eprintln!("{}", error.render(src));
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

## Parser details

`logos` 0.16 builds the tokenizer at compile time. The parser is hand-written
recursive descent with precedence climbing, so spans and messages stay under
our control. MSRV is 1.80, which is what `logos` requires.

It handles statements separated by newlines or `;`, comments (`#`, `//`,
`/* ... */`), decimal and hex integers, underscores, floats, exponents,
double-quoted interpolation and single-quoted literals.

Nesting deeper than 200 levels is rejected with a normal error, so generated
or hostile input cannot overflow the stack.

## Layout

```
src/                 scrippy-script-core
  span.rs            byte spans and line/column lookup
  error.rs           lexer and parser errors with source snippets
  lexer.rs           tokenizer
  ast.rs             Script, Stmt, Expr and friends
  parser.rs          recursive descent parser
crates/scrippy/      native compiler and CLI
  src/codegen.rs     AST to C
  src/runtime.c      dynamic value runtime, GC and standard functions
  src/main.rs        command line
  tests/run.rs       end-to-end compiler tests
examples/            sample programs and the parser example
docs/                language reference and handout
vscode/              editor extension
.scp/                sample scripts
```

## VS Code extension

`vscode/` holds the editor extension: `.scp` syntax highlighting, comment
toggling, bracket pairs and indentation.

```sh
curl -LO https://github.com/ScrippyOffical/ScrippyScript/releases/latest/download/scrippy-vscode-0.1.0.vsix
code --install-extension scrippy-vscode-0.1.0.vsix
```

On the Flatpak build, use
`flatpak run com.visualstudio.code --install-extension "$PWD/scrippy-vscode-0.1.0.vsix"`.
Every `vscode-v*` tag builds a fresh package in `vsix.yml`.

## WASM

The core crate does no I/O, uses no threads and contains no `unsafe`, so
`wasm32-unknown-unknown` is a supported target with no feature flags. Use the
release profile in `Cargo.toml` (`lto`, `opt-level = "s"`) for small binaries.
The native compiler is a separate crate because it calls a C compiler.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --example parse -- .scp/demo.scp
```

The end-to-end tests need a working C compiler. CI runs the same list plus an
MSRV build on 1.80 and a wasm check of the core crate.

## License

GPL-3.0, see [LICENSE](LICENSE).
