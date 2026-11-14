# The Scrippy language (`.scp`)

Scrippy scripts are plain text. Statements end at a newline or `;`. Blocks are
wrapped in `{ }`, and a trailing comma is allowed inside lists, maps and calls.

The language has two implementations:

- the native compiler in this repository, which turns a program into an
  executable and has no Minecraft dependency
- the Paper plugin, which interprets scripts on a server and adds events,
  commands, scheduling and the Bukkit API

Most of this document describes the shared core. The last sections cover the
Paper-only features and the constructs the native compiler does not parse.

## Comments

```scp
# hash comment
// slash comment
/* block
   comment */
```

## Values

- number — integers (`42`, `0xFF`) and decimals (`3.14`). Whole numbers stay
  integers until a decimal is needed.
- string — `"double quoted"` allows `${...}` interpolation, `'single quoted'`
  is literal.
- boolean — `true`, `false`.
- null — `null`.
- list — `[1, 2, 3]`.
- map — `{ name: "Steve", level: 3 }`.
- function — see below.

`typeof(x)` returns `"number"`, `"string"`, `"boolean"`, `"null"`, `"list"`,
`"map"` or `"function"`.

## Variables

```scp
let x = 10         // reassignable
const name = "hi"  // not reassignable
global visits = 0  // lives in the script's global scope
```

Assignment works on plain names, members and indexes:

```scp
x = x + 1
x += 5
scores["alice"] = 5
settings.name = "Steve"
```

The compound forms are `+= -= *= /= %=`. `const` is checked by the native
compiler; the Paper interpreter also rejects writes to a constant.

`${...}` interpolation builds a string from expressions:

```scp
let x = 3
print("You have ${x} coins.")
```

An interpolation cannot contain a double-quoted string of its own. Compute the
value first if you need one:

```scp
let items = "a,b".split(",")
print("count: ${len(items)}")
```

## Operators

- arithmetic: `+ - * / %`
- comparison: `== != < <= > >=`
- logic: `&& || !` (also `and`, `or`, `not`)
- ternary: `cond ? a : b`
- null coalescing: `a ?? b`
- safe access: `a?.b` and `a?.b()`
- ranges: `1..5` (inclusive), `0..<10` (exclusive)

Precedence follows the usual order: unary, `* / %`, `+ -`, ranges,
comparisons, `&&`, `||`, `??`, ternary.

`+` concatenates when either side is a string. Other arithmetic converts
numeric strings, so `"5" - 2` is `3`. Comparing a number with a string using
`<` or `>` is an error; compare numbers with numbers and strings with strings.

Division by zero and modulo by zero stop the program with an error. Integer
division stays an integer when it divides evenly, otherwise it produces a
decimal: `8 / 4` is `2`, `10 / 4` is `2.5`.

Truthiness: `null`, `false`, `0`, `""`, `[]` and `{}` are false. Everything
else is true, including functions.

## Control flow

```scp
if x > 5 {
    print("big")
} else if x == 5 {
    print("exact")
} else {
    print("small")
}

while running {
    tick()
}

for item in [1, 2, 3] {
    print(item)
}

for i in 1..10 {
    print(i)
}
```

`break` and `continue` work inside loops. `return` returns from a function and
otherwise ends the program.

`for` walks a list, a map (its keys), a string (one character per step) or a
range. Ranges are capped at one million elements.

The native runtime allows 1000 nested calls. Deeper recursion stops with an
error instead of crashing.

## Functions

```scp
fun twice(n) {
    return n * 2
}

fun greet(name, greeting = "Hello") {
    print("${greeting}, ${name}!")
}

let add = fun(a, b) { return a + b }
print(add(1, 2))
```

Parameters may have defaults. A default is evaluated when the argument is
missing and can refer to earlier parameters. Every function also sees `args`,
the full list of values it was called with. Parameters and `args` are
read-only, like a `const` binding.

Functions capture their defining scope, so closures and recursion work:

```scp
fun counter(start) {
    let value = start
    return fun(step = 1) {
        value += step
        return value
    }
}

let next = counter(10)
print(next())   // 11
print(next(5))  // 16
```

Calling with too many arguments is an error. A missing argument without a
default is an error too.

## Strings

Strings are UTF-8 and index by character, not byte. Negative indexes count
from the end.

```scp
let s = "Hello"
print(s[0])          // H
print(s[-1])         // o
print(s.size)        // 5
print(s.upper())     // HELLO
print(s.substring(1, 3)) // el
```

Methods: `upper`, `toUpperCase`, `lower`, `toLowerCase`, `trim`, `size`,
`length`, `isEmpty`, `toString`, `contains`, `startsWith`, `endsWith`,
`indexOf`, `charAt`, `at`, `split`, `replace`, `substring`.

`split` keeps empty parts, so `"a,,b".split(",")` is `["a", "", "b"]`.
Splitting on an empty separator returns one character per item.

## Lists

```scp
let items = [3, 1, 2]
items.add(5)
items.sort()
print(items.join("-"))  // 1-2-3-5
print(items.first)      // 1
print(items.last)       // 5
```

Methods: `add`, `get`, `set`, `remove`, `contains`, `indexOf`, `slice`,
`join`, `sort`, `reverse`, `size`, `isEmpty`, `first`, `last`.

`sort` and `reverse` change the list in place. The global `sort(list)` and
`reverse(list)` return a sorted or reversed copy instead.

`remove` takes an index for numbers and removes every matching value for
anything else.

Members `size`, `length`, `isEmpty`, `first` and `last` are available without
parentheses. Parentheses also work.

## Maps

```scp
let scores = { alice: 3, bob: 7 }
scores["carol"] = 9
scores.dave = 1
print(scores["alice"])   // 3
print(scores.bob)        // 7
print(scores.missing ?? 0)
```

Keys are converted to strings. Insertion order is preserved.

Methods: `get`, `put`, `set`, `containsKey`, `contains`, `remove`,
`getOrDefault`, `keys`, `values`, `size`, `isEmpty`.

Reading a missing key returns `null`. Member access and index access behave the
same for maps, and assigning to either creates or replaces the key.

Maps are plain data, not objects. A method name that matches a key still calls
the method when you add parentheses: `m.size` reads a key called `size`, while
`m.size()` returns the entry count. A zero-argument call whose name is not a
method returns the value stored under that key, so `m.handler()` hands back a
stored function.

## Numbers and booleans

Numbers answer `abs`, `floor`, `ceil`, `round`, `toInt` and `toString` as
methods. Booleans answer `toString`.

## Standard library

Type and conversion: `str`/`toString`, `num`, `int`, `bool`, `typeof`,
`isNull`.

Collections: `len`/`size`, `list`, `add`, `remove`, `sort`, `reverse`, `join`,
`split`, `contains`, `keys`, `values`, `range`.

Strings: `upper`, `lower`, `trim`, `replace`, `startsWith`, `endsWith`,
`substring`, `color`/`colorize` (turns `&` into `§`, for Minecraft chat).

Math: `abs`, `floor`, `ceil`, `round`, `sqrt`, `pow`, `min`, `max`,
`random`, `randomInt`.

Other: `now` (Unix milliseconds), `uuid`, `print`/`log`.

`print` writes each argument separated by a space and ends with a newline.
`log` is the same in the native runtime.

## Type hints

Parameters and variables may carry a hint:

```scp
fun twice(n: number) {
    return n * 2
}

let player: player? = null
```

The native compiler accepts and ignores hints. They do not change runtime
behavior, and a wrong value is not rejected. The Paper interpreter is not so
lenient: it converts number, string and boolean parameters to the hinted type
at call time.

## The native compiler

```sh
scrippy run <file.scp>              compile and run
scrippy build <file.scp> [-o path]  write an executable
scrippy check <file.scp>            parse and check only
scrippy emit-c <file.scp> [-o path] write the generated C
```

`build` and `run` need a C compiler. It runs `$CC` when set and `cc`
otherwise. The generated C is compiled with `-O2` and removed afterwards,
unless `--keep-c` is passed. `run` uses a temporary directory and cleans up
after itself.

Compile errors show the file, line and column, the offending line and a caret:

```
scrippy: hello.scp:1:7: unknown variable or function `missing`
  print(missing)
        ^^^^^^^
```

Runtime errors show the location and a call stack:

```
boom.scp:2:12: division by zero
  at inner (boom.scp:6:12)
  at outer (boom.scp:9:7)
```

The native compiler rejects host-only syntax that the core language does not
define. `new` is the exception: the shared parser accepts it, but the compiler
reports that there is no host to construct objects. A native program also
cannot use `player`, `ScrippyAI`, `config` or any other plugin object.

The Paper parser understands more syntax than the native compiler. `match`,
`try`/`catch`/`finally`, `throw` and `import` parse only on the Paper side. The
native compiler reports a normal parse error for them.

## Paper-only features

Everything below is implemented by the Paper plugin. Native programs cannot use
these features, either because they need a server or because the shared parser
does not accept the syntax.

### Events

```scp
on join {
    message(event.player, "&aWelcome!")
}

on break(priority: HIGH) {
    if event.block.type == material("DIAMOND_ORE") {
        event.player.send("&bNice find.")
    }
}
```

The handler receives the Bukkit event as `event`. Cancellable events can be
cancelled with `event.cancelled = true` or `event.cancel()`.

Lifecycle hooks run when a script is loaded and unloaded:

```scp
on load {
    print("starting up")
}

on disable {
    print("shutting down")
}
```

### Commands

```scp
command "heal" {
    permission: "scrippy.heal"
    aliases: ["h", "restore"]
    description: "Restore health."
    args: [target: player?, amount: number = 20]
    run {
        let who = target != null ? target : sender
        heal(who, amount)
    }
}
```

Argument types drive parsing and tab completion: `player`, `world`, `number`,
`integer`, `double`, `boolean`, `material`, `entity`, `string`, `text`
(consumes the rest of the line). A `?` marks an argument optional; `= expr`
gives a default.

### Scheduling

```scp
every 30 seconds {
    broadcast("&7Still here.")
}

after 5 minutes {
    broadcast("&7Five minutes have passed.")
}
```

Units are `ticks`, `seconds`, `minutes`, `hours` and `days`.

### Server API

Any member (`player.name`, `block.type`, `event.item`) and any method
(`player.sendMessage(...)`, `world.getBlockAt(...)`) on a Bukkit object is
reachable through reflection. Names are matched case-insensitively and tolerate
snake_case, so `display_name` resolves to `getDisplayName()`.

Some common operations are provided as shorter host functions:

| Function | Effect |
| --- | --- |
| `broadcast(text)` | Message to the whole server. |
| `message(target, text)` / `msg` | Message to a player or sender. |
| `console(text)` | Message to the console. |
| `player(name)` | Online player by name, or null. |
| `players()` | List of online players. |
| `world(name)` | World by name. |
| `give(target, material, amount)` | Add an item to a player. |
| `item(material, amount)` | Build an ItemStack. |
| `heal(target, amount?)` | Restore health and hunger. |
| `kill(target)` | Kill a player or remove an entity. |
| `teleport(target, world, x, y, z)` | Teleport. |
| `sound(target, name, volume, pitch)` | Play a sound. |
| `particle(name, world, x, y, z, count)` | Spawn particles. |
| `setBlock(world, x, y, z, material)` | Set a block. |
| `getBlock(world, x, y, z)` | Read a block. |
| `spawn(world, x, y, z, type)` | Spawn an entity. |
| `dispatch(command)` | Run a command as the console. |
| `material(name)` | Resolve a Bukkit material. |
| `location(world, x, y, z)` / `new Location(...)` | Build a location. |
| `isPlayer(value)` | Whether a value is a player. |

### Match blocks

```scp
match kind {
    case "a", "b":
        print("first")
    case "c":
        print("second")
    default:
        print("other")
}
```

### Error handling

```scp
try {
    risky()
} catch err {
    print("failed: ${err}")
} finally {
    cleanup()
}
```

`throw` raises a value, which the matching `catch` block receives. Errors
raised by the runtime arrive as a message string.

### Script configuration

A script can ship a YAML file next to it. If `heal.scp` exists, a file named
`plugins/Scrippy/configs/heal.yml` is readable from inside the script:

```scp
let settings = config("")
message(sender, "Heal amount: ${settings.amount}")
```

`config("path.to.section")` returns that section as a map; `config("")` returns
the whole file.

### Imports

```scp
import "lib/util.scp"
```

Imports are resolved relative to the scripts folder and share the importer's
globals. Circular imports are rejected. Import aliases are not implemented, so
`import "x.scp" as y` behaves like a plain import.
