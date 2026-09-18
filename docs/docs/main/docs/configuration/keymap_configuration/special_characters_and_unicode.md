# Special Characters and Unicode

RMK ultimately emits HID codes to the operating system. How these codes are interpreted to print a letter depends on the operating system and the keyboard layout setting. For example, pressing the key for `;` on a keyboard with an en-US layout will print `;`. But if you change that to de-DE, it will print `ö` instead.

This documentation and the `KeyCodes` assume an en-US layout.

## Issuing Specific Characters

Entering special characters usually requires a key combination, which depends on your operating system and chosen keyboard layout (setting in the OS). For example, in macOS with an en-US layout, you can define the following sequence to enter an `ä`:

```toml
[[behavior.macro.macros]]
operations = [
    { operation = "down", keycode = "LAlt" },
    { operation = "tap", keycode = "U" },
    { operation = "up", keycode = "LAlt" },
    { operation = "tap", keycode = "A" },
]
```

## Printing unicode

Each unicode symbol has an `code point` (aka alt-sequence) identifying it, usually depicted as `U+` and a hex number, like `U+2764` for ❤. This [wikipedia article](https://en.wikipedia.org/wiki/List_of_Unicode_characters) lists all unicode symbols.

Depending on your Operating System and Keyboard Layout you can enter a specific character by pressing a key combination, usually using the alt modifier.

RMK types that key combination for you. List the codepoints you want under `[behavior.unicode]` and bind `UNICODE(n)` to the n-th entry of the list:

```toml
[behavior.unicode]
default_mode = "linux"                   # linux | macos | windows
codepoints = ["00E9", "2764", "1F44D"]   # hex, no `U+` prefix

[[keymap.layer]]
keys = """
UNICODE(0)  UNICODE(1)  UNICODE(2)  UnicodeModeCycle  ...
"""
```

The table is `&'static`, so it lives in flash and costs 4 bytes per codepoint. That is what makes it a better fit than [macro sequences](./keyboard_macros.md) for a layer full of symbols: macros share the fixed `macro_space_size` RAM buffer, which a few dozen codepoints already exhaust.

## Input modes

`default_mode` picks the input method the codepoint is typed through:

| Mode | Sequence | Requires |
| --- | --- | --- |
| `linux` | `Ctrl+Shift+U`, hex digits, `Enter` | IBus |
| `macos` | Hex digits typed with `LeftAlt` held | The `Unicode Hex Input` keyboard layout |
| `windows` | `RightAlt`, `u`, hex digits, `Enter` | [WinCompose](https://github.com/samhocevar/wincompose) |

`macos` reads exactly four hex digits per UTF-16 code unit, so codepoints above `U+FFFF` are typed as a surrogate pair. The other two take the codepoint itself.

Bind `UnicodeModeCycle` to move between the three modes at runtime. The chosen mode is written to storage and restored on the next boot, so it only has to be set once per host.

## Shifted variants

`UNICODE(n)` does not look at the shift state. Pair two codepoints with a [fork](../behavior.md) when a key should type a different one while shift is held:

```toml
[[behavior.fork.forks]]
trigger = "UNICODE(0)"
negative_output = "UNICODE(0)"
positive_output = "UNICODE(1)"
match_any = "LShift|RShift"
```
