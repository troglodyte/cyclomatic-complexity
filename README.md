# jscx

Cyclomatic complexity for JavaScript, TypeScript, and JSX functions. Single
binary, zero dependencies, no Node toolchain required.

```console
$ cargo build --release
$ ./target/release/jscx src/
src/checkout.ts
    CC   Line  Kind         Name
    14     12  function     validateCart
     3     61  arrow          onEachItem
     7     94  method       applyDiscount

src/utils.ts
    CC   Line  Kind         Name
     2      4  function     formatDate

42 functions in 7 files · mean 3.1 · worst 14 (validateCart at src/checkout.ts:12)
```

Columns are score, line, kind, and name. Nested functions are indented under
their parent.

## Install

```console
cargo install --path .
```

## Usage

```
jscx [OPTIONS] <PATH>...     files or directories (recursive)
jscx [OPTIONS] -             read from stdin
```

| Option | Effect |
| --- | --- |
| `-m, --min <N>` | only report functions scoring N or higher (default 1) |
| `-M, --max <N>` | exit 1 if any function scores above N — for CI |
| `-t, --top <N>` | show only the N worst functions |
| `-s, --sort` | sort by score, highest first |
| `--no-header` | do not print the table header |
| `-j, --json` | machine-readable output |
| `-e, --explain` | list the decision points behind each score |
| `--optional-chain` | count `?.` as a branch (off by default) |
| `--no-logical` | don't count `&&`, `\|\|`, `??` |
| `--jsx` / `--no-jsx` | force JSX lexing (default: on except `.ts`/`.mts`/`.cts`) |
| `--include-dts` | don't skip `.d.ts` files |
| `--ext <EXT>` | extension to assume for stdin (default `tsx`) |

Directories are walked recursively, skipping `node_modules`, `dist`, `build`,
`coverage`, `.next`, `target`, VCS directories and similar.

### As a CI gate

```console
jscx --max 15 src/
```

Exit codes: `0` clean, `1` something exceeded `--max`, `2` a path could not be
read.

### Finding what to refactor first

```console
jscx --top 20 --explain src/
```

`--explain` prints each branch that contributed, so a score of 14 becomes a
list of the fourteen decisions rather than a number you have to hunt for.

## Scoring

Each function starts at 1 and gains a point for every decision point:

| Construct | Points |
| --- | --- |
| `if` | +1 each (`else` adds nothing; `else if` is the nested `if`) |
| `for`, `for..in`, `for..of`, `while`, `do..while` | +1 each |
| `case` | +1 each (`default` adds nothing) |
| `catch` | +1 |
| `&&`, `\|\|`, `??`, `&&=`, `\|\|=`, `??=` | +1 each |
| ternary `?:` | +1 each |
| `?.` | 0 unless `--optional-chain` |

These are the same rules as ESLint's [`complexity`][eslint] rule, so scores are
directly comparable and the crate's test suite checks the examples from that
rule's documentation.

Two consequences worth knowing:

- **Nested functions are scored independently.** A component whose body holds
  five callbacks is not charged for their branches; each callback gets its own
  row. This is what ESLint does, and it keeps the number attached to the unit
  you would actually extract.
- **Parameter defaults count toward the enclosing function.** `function f(a = b || c)`
  scores 2, because that branch runs on every call to `f`.

[eslint]: https://eslint.org/docs/latest/rules/complexity

## Editor integration

`jscx --lsp` speaks just enough of the Language Server Protocol to publish
diagnostics as you type. Every editor integration runs that same server, so scores
and severities cannot drift between them.

One knob, the **threshold**, defaults to **11**. A function scoring at or above it
is a warning; at or above twice it, an error. With the default, 11 through 21 warn
and 22 or above is an error. The other scoring options (`--optional-chain`,
`--no-logical`) are CLI-only.

Every integration needs the binary installed and findable:

```console
cargo install --path .
```

|  | Install | Configure in | After a threshold change |
| --- | --- | --- | --- |
| **Zed** | `zed: install dev extension` | `settings.json` | `editor: restart language server` |
| **JetBrains** | build a zip, install from disk | *Settings \| Tools \| jscx* | automatic |
| **Neovim** | put the directory on the runtimepath | `init.lua` | `:LspRestart jscx` |

That last column exists because the server reads the threshold once, during
`initialize`, and implements no configuration-change handler. The JetBrains plugin
restarts it for you; the other two leave it to you.

All three look for the binary on `PATH`. The JetBrains plugin and the Neovim config
then fall back to `~/.cargo/bin/jscx` — that fallback matters because an
application launched from the Dock does not inherit your shell's `PATH`. Each also
accepts an explicit path; see its section below.

To check an integration is live, open `test.ts` or `test.js` in the repository
root. Each holds functions that are clean, over the threshold, and over double it,
so a working setup shows both a warning and an error.

### Zed

Run `zed: install dev extension` and select this directory:

```text
extensions/jscx
```

Zed compiles the extension to WebAssembly itself, so there is nothing to build
first. Paths here are relative to the repository root.

Configure in `settings.json`:

```json
{
  "lsp": {
    "jscx": {
      "settings": {
        "threshold": 11
      }
    }
  }
}
```

The `settings` nesting is required — `"jscx": { "threshold": 11 }` is silently
ignored.

### JetBrains

Works in WebStorm, IntelliJ IDEA Ultimate, and the other paid JetBrains IDEs. The
IntelliJ LSP API is unavailable in IDEA Community and Android Studio, neither of
which supports JavaScript or TypeScript anyway.

Requires a 2026.2 or newer build; see `extensions/jetbrains/gradle.properties` for
why, and what it takes to go back further.

Build the plugin, which needs [Gradle](https://gradle.org):

```console
cd extensions/jetbrains
gradle wrapper        # first time only, or open the directory in your IDE
./gradlew buildPlugin
```

Then *Settings | Plugins | ⚙ | Install Plugin from Disk* and select the zip — not
either jar inside it:

```text
extensions/jetbrains/build/distributions/jscx-intellij-0.1.0.zip
```

The version in that filename comes from `pluginVersion` in
`extensions/jetbrains/gradle.properties`. In the macOS file dialog, `Cmd+Shift+G`
takes a pasted path.

Restart the IDE, then set the threshold under *Settings | Tools | jscx*. The same
panel takes an explicit binary path, for when neither `PATH` nor `~/.cargo/bin`
finds it.

### Neovim

Requires Neovim 0.11 or newer, for `vim.lsp.config` and `lsp/` runtimepath
discovery. Put the directory on the runtimepath — with lazy.nvim:

```lua
{ dir = "~/code/jscx/extensions/nvim" },
```

Then enable it, alongside any other servers:

```lua
vim.lsp.enable({ "ts_ls", "rust_analyzer", "jscx" })
```

Do not lazy-load that spec on `ft`, `event`, or `cmd`. `vim.lsp.enable` runs at
startup and needs `lsp/jscx.lua` on the runtimepath already.

Configure the threshold as a merge over the shipped default:

```lua
vim.lsp.config("jscx", { init_options = { threshold = 11 } })
```

Override `cmd` to point at a binary somewhere else:

```lua
vim.lsp.config("jscx", { cmd = { "/path/to/jscx", "--lsp" } })
```

See `extensions/nvim/README.md` for the rest.

## How it works

There is no `swc`/`oxc`/`tree-sitter` dependency and no AST. The tool is two
passes over a token stream:

1. **`src/lexer.rs`** — a tokenizer with a mode stack. It gets right the things
   that break naive scanners: line and block comments, strings with escapes,
   template literals with arbitrarily nested `${}` substitutions, regex literals
   versus division, and JSX (tags, attributes, `{}` containers, and text — so
   the `&&` in `<span>a && b</span>` is text, not an operator).

2. **`src/analyze.rs`** — locates function boundaries by bracket matching and
   local token patterns (`function`, `=>`, and the method shape
   `name(...) {`), builds the nesting tree by containment, then counts decision
   tokens in each function's range minus its descendants' ranges.

TypeScript is handled by masking type positions — parameter annotations, return
types, generic parameter lists, `type`/`interface`/`enum` declarations, and
`as`/`satisfies` assertions — so that a conditional type `T extends U ? X : Y`
or a callback type `(x: number) => void` is not mistaken for a ternary or a
function. Two disambiguations are worth calling out, because they are where a
token-level tool has to be clever:

- Inside a parameter list, `:` normally starts a type — but not after a
  top-level `=`, where it is the middle of a ternary in a default value. So
  `(a = b ? 1 : c && d)` keeps both of its branches.
- A concise arrow body followed by `=` is a type annotation, not a function:
  in `handler: (e: Event) => void = (e) => {...}` the first `=>` is a type and
  the second is the real function. JSX attribute `=` is excluded from this rule,
  so `(i) => <Row on={i.ok} />` still parses as an arrow.

## Limitations

Being a token-level tool rather than a parser buys speed and a tiny dependency
tree at the cost of some edge cases:

- A bare class field with a function-type annotation and no modifier
  (`onChange: (v: string) => void;` inside a class body) is
  lexically identical to an object-literal property whose value is an arrow.
  The tool treats it as an arrow. Adding any modifier (`readonly`, `private`)
  or an initializer resolves it.
- Function-type annotations in unusual positions — inside an `as` cast that
  spans a statement boundary, or in explicit call-site type arguments such as
  `useMemo<(x: number) => void>(...)` — can produce one extra row scoring 1.
  Filter with `--min 2` if this is noisy in your codebase.
- Decorators are treated as ordinary tokens; they do not affect scoring.
- A call expression immediately followed by a block statement with no
  semicolon (`doThing()\n{ ... }`) can be read as a method. This is vanishingly
  rare in real code.
- Cyclomatic complexity itself is a blunt instrument: it counts branches, not
  how tangled they are. A flat `switch` with 20 cases scores 21 and is usually
  fine; a nested 8 is often worse. Read it as a place to look, not a verdict.

## Development

```console
cargo test          # unit tests, including the ESLint reference cases
cargo run -- fixtures/ --explain
cargo clippy
```

The `fixtures/` directory holds hand-checked samples covering nesting,
generators, private methods, template literals, regex-versus-division, JSX text,
TypeScript overloads and type positions, and Unicode identifiers.
