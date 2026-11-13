# The Scrippy language (`.scp`)

Scrippy scripts are plain text. Statements end at a newline or `;`. Blocks are
wrapped in `{ }`, and a trailing comma is allowed inside lists, maps and calls.

## Comments

```scp
# hash comment
// slash comment
/* block
   comment */
```

## Values

- number — integers (`42`, `0xFF`) and decimals (`3.14`). Whole numbers are kept
  as integers until a decimal is needed.
- string — `"double quoted"` allows `${...}` interpolation, `'single quoted'` is
  literal.
- boolean — `true`, `false`.
- null — `null`.
- list — `[1, 2, 3]`.
- map — `{ name: "Steve", level: 3 }`.
- function — see below.

## Variables

```scp
let x = 10        // reassignable
const name = "hi" // not reassignable
global visits = 0  // lives in the script's global scope
```

Assignment works on plain names, members and indexes:

```scp
x = x + 1
x += 5
player.health = 20
scores["alice"] = 5
```

`${...}` interpolation builds a string from expressions:

```scp
message(sender, "You have ${x} coins.")
```

## Operators

- arithmetic: `+ - * / %`
- comparison: `== != < <= > >=`
- logic: `&& || !` (also `and`, `or`, `not`)
- ternary: `cond ? a : b`
- null coalescing: `a ?? b`
- safe access: `a?.b` and `a?.b()`
- ranges: `1..5` (inclusive), `0..<10` (exclusive)

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

match kind {
    case "a", "b":
        print("first")
    case "c":
        print("second")
    default:
        print("other")
}
```

`break` and `continue` work inside loops. `return` returns from a function.
`throw` and `try`/`catch`/`finally` handle errors:

```scp
try {
    risky()
} catch err {
    print("failed: ${err}")
} finally {
    cleanup()
}
```

## Functions

```scp
fun twice(n) {
    return n * 2
}

fun greet(name, greeting = "Hello") {
    print("${greeting}, ${name}!")
}

let add = fun(a, b) { return a + b }  // anonymous
print(add(1, 2))
```

Parameters may declare optional types (`n: number`) and defaults. Extra values
are reachable through the host when needed.

## Events

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

The handler receives the Bukkit event as `event`; access anything the event
exposes with normal member syntax. Cancellable events can be cancelled with
`event.cancelled = true` or `event.cancel()`.

Lifecycle hooks run when a script is loaded and unloaded:

```scp
on load {
    print("starting up")
}

on disable {
    print("shutting down")
}
```

## Commands

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

Argument types drive both parsing and tab completion: `player`, `world`,
`number`, `integer`, `double`, `boolean`, `material`, `entity`, `string`, `text`
(consumes the rest of the line). A `?` marks an argument optional; `= expr` gives
a default.

## Scheduling

```scp
every 30 seconds {
    broadcast("&7Still here.")
}

after 5 minutes {
    broadcast("&7Five minutes have passed.")
}
```

Units are `ticks`, `seconds`, `minutes`, `hours` and `days`.

## Accessing the server API

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

## Standard library

`len`/`size`, `list`, `add`, `remove`, `sort`, `reverse`, `join`, `split`,
`contains`, `keys`, `values`, `range`, `upper`, `lower`, `trim`, `replace`,
`startsWith`, `endsWith`, `substring`, `color`, `abs`, `floor`, `ceil`, `round`,
`sqrt`, `pow`, `min`, `max`, `random`, `randomInt`, `now`, `uuid`, `str`, `num`,
`int`, `bool`, `typeof`, `isNull`, `print`/`log`.

## Script configuration

A script can ship a YAML file next to it. If `heal.scp` exists, a file named
`plugins/Scrippy/configs/heal.yml` is readable from inside the script:

```scp
let settings = config("")
message(sender, "Heal amount: ${settings.amount}")
```

`config("path.to.section")` returns that section as a map; `config("")` returns
the whole file.

## Imports

```scp
import "lib/util.scp"
import "lib/colors.scp" as colors
```

Imports are resolved relative to the scripts folder and share the importer's
globals. Circular imports are rejected.
