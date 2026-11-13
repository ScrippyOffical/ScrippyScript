# ScrippyScript for VS Code

Syntax highlighting and editing support for `.scp` scripts.

## Install

Download the latest `.vsix` from
[releases](https://github.com/ScrippyOffical/ScrippyScript/releases) and either:

```sh
code --install-extension scrippy-vscode-0.1.0.vsix
```

or drop the file onto the VS Code window, or run
`Extensions: Install from VSIX...` from the command palette.

If you use the Flatpak build, `code` lives inside the sandbox:

```sh
flatpak run com.visualstudio.code --install-extension "$PWD/scrippy-vscode-0.1.0.vsix"
```

## What it highlights

- `let`, `const`, `global`, `fun` declarations, with the declared name picked out
- control flow: `if`, `else`, `while`, `for`, `in`, `break`, `continue`, `return`
- `and`, `or`, `not`, `new`, `true`, `false`, `null`
- double-quoted strings, including `${...}` interpolation, and single-quoted
  literals; `\n`, `\t` and the rest are marked as escapes
- integers, `0x` hex and floats with `_` separators
- `#`, `//` and `/* ... */` comments
- builtins such as `broadcast`, `message`, `give`, `teleport`, `print`
- type hints after `:` (`number`, `string`, `player`, ...)
- method calls and member access: `ScrippyAI.ask(...)`, `player.health`
- operators, punctuation, brackets

Scopes are the standard TextMate ones (`keyword.control`, `string.quoted.double`,
`entity.name.function`, ...), so themes colour them without extra configuration.

Comment toggling, bracket matching, auto-closing pairs and indent-on-`{` come
from `language-configuration.json`.

## Build

```sh
npm ci
npm run compile
npx @vscode/vsce package --no-dependencies
```

Pushing a `vscode-v*` tag runs `.github/workflows/vsix.yml`, which rebuilds the
package and attaches it to a GitHub release.

## License

GPL-3.0, see [LICENSE](LICENSE).
