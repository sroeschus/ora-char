# ora-char

Convert an Oracle `CHAR` or `VARCHAR2` value from its internal byte
representation (as shown by `DUMP()`, `DUMP(x, 16)`, `RAWTOHEX()` or a block
dump) into its characters. With `-r`, it converts ASCII text into the internal
representation.

This release supports ASCII data only: every byte must be below `0x80`.
`NCHAR` and `NVARCHAR2` are not supported.

## Building

```sh
cargo build --release
```

The binary is written to `target/release/ora-char`.

## Man page

A man page is in `man/ora-char.1`. View it without installing:

```sh
man -l man/ora-char.1
```

Install it together with the binary:

```sh
install -Dm755 target/release/ora-char /usr/local/bin/ora-char
install -Dm644 man/ora-char.1 /usr/local/share/man/man1/ora-char.1
```

## Usage

```
ora-char [OPTIONS] <INPUT>
```

| Option                  | Description                                                       |
|-------------------------|-------------------------------------------------------------------|
| `-r, --reverse`         | Convert ASCII text into the internal representation               |
| `-f, --format <FORMAT>` | Input format: `auto` (default), `dec` or `hex` (output with `-r`) |
| `--raw`                 | Print the characters unchanged, without escaping (`-r`: take the text literally) |
| `-v, --verbose`         | Also print the type, length and bytes                             |
| `-h, --help`            | Show help                                                         |
| `-V, --version`         | Show version                                                      |

### Accepted input

All of the following decode to `Hello`:

```sh
ora-char "Typ=1 Len=5: 72,101,108,108,111"   # DUMP(x)
ora-char "Typ=1 Len=5: 48,65,6c,6c,6f"       # DUMP(x, 16)
ora-char "Typ=1 Len=5 CharacterSet=AL32UTF8: 72,101,108,108,111"   # DUMP(x, 1010)
ora-char 72,101,108,108,111                  # decimal byte list
ora-char "72 101 108 108 111"
ora-char 48656C6C6F                          # RAWTOHEX / contiguous hex
ora-char 0x48656C6C6F
ora-char "48 65 6c 6c 6f"                    # hex-dump style
ora-char "col 0: [ 5] 48 65 6c 6c 6f"        # partial block dump
```

The detection rules are the same as in
[ora-num](https://github.com/sroeschus/ora-num). Block dump input is always
read as hex; unlike in ora-num, `-f dec` is an error for it. Other input is treated as hex if it is a
single contiguous token, contains hex letters, or is space-separated
two-character pairs; otherwise it is read as decimal bytes. Use `-f dec` or
`-f hex` to override the detection.

Many short ASCII strings are valid in both formats, because most ASCII codes
are two digits in both decimal and hex. `ora-char` handles this as follows:

* If only one reading gives printable text (and, with a `Len=` or `[n]`
  prefix, the declared length), that reading is used.
* If both readings give printable text, the detected format is used and a
  note on stderr shows the other reading:

```sh
$ ora-char 65,66,67
note: input is ambiguous, read as dec giving "ABC"; as hex it is "efg", use -f to choose
ABC

$ ora-char -f hex 65,66,67
efg
```

`DUMP(x, 16)` output without any of the letters `a`–`f`, e.g.
`Typ=1 Len=3: 61,62,63`, looks exactly like `DUMP(x)` output. Pass `-f hex`
for it.

### Examples by format

#### `DUMP(x)` output (decimal bytes)

Paste the result of `SELECT DUMP(x) FROM ...` as is. The `Typ=n Len=n:`
prefix is checked: the type must be `1` (`VARCHAR2`) or `96` (`CHAR`), the
number of bytes must match `Len`, and no field may appear more than once.

```sh
$ ora-char "Typ=1 Len=11: 79,114,97,99,108,101,32,50,51,97,105"
Oracle 23ai

$ ora-char "Typ=1 Len=4: 79,114,97"
error: input declares 4 bytes, but 3 are given

$ ora-char "Typ=2 Len=3: 194,2,24"
error: Typ=2 is NUMBER, not a character type, use ora-num

$ ora-char NULL
error: the value is NULL, there are no characters to convert
```

A `CharacterSet=` field, as printed by `DUMP(x, 1010)` and `DUMP(x, 1016)`, is
accepted.

`NCHAR` and `NVARCHAR2` are not supported. `DUMP()` shows them with the same
type codes as `CHAR` and `VARCHAR2`, so `ora-char` recognizes their usual
national character set, `AL16UTF16`, by its bytes: an even number of bytes in
which every other byte, starting with the first, is zero. Such input is
rejected, as is `CharacterSet=AL16UTF16`:

```sh
$ ora-char "Typ=1 Len=4: 0,72,0,105"
error: the data is in AL16UTF16 (UTF-16), the NCHAR/NVARCHAR2 national character set; NCHAR and NVARCHAR2 are not supported
```

With the `UTF8` national character set, ASCII text in an `NCHAR` or
`NVARCHAR2` column has the same bytes as in a `CHAR` or `VARCHAR2` column,
so it cannot be detected.

#### `DUMP(x, 16)` output (hex bytes)

```sh
$ ora-char "Typ=96 Len=5: 4f,4b,20,20,20"
OK   
```

#### Contiguous hex (`RAWTOHEX`, `UTL_RAW`)

An optional `0x` prefix is allowed, and hex digits may be upper or lower case.
Hex input may also be split into several tokens of any even length, e.g.
wrapped output (`4865 6C6C 6F`) or a list of `0x` values (`0x48,0x65`).

```sh
$ ora-char 4F7261636C65
Oracle
```

#### Hex-dump style

```sh
$ ora-char "4f 72 61 63 6c 65"
Oracle
```

#### Partial block dump

A column entry from a block dump (`ALTER SYSTEM DUMP DATAFILE ...`): `col`,
the column number, `:`, the byte count in brackets, then the bytes in hex.
Values that the dump wraps over several lines can be passed as one quoted
argument.

```sh
$ ora-char "col  1: [ 6]  4f 72 61 63 6c 65"
Oracle

$ ora-char -f dec "col 0: [ 2] 41 42"
error: block dump bytes are always hex, -f dec cannot be used with them

$ ora-char "col 1: *NULL*"
error: column 1 is NULL in the block dump, there are no characters to convert
```

### Output

By default, control characters and backslashes are escaped so the output
stays readable and unambiguous: `\\`, `\n`, `\t`, `\r`, `\0`, and `\xNN` for
other control characters (including `0x7F`). `--raw` prints the bytes
unchanged.

```sh
$ ora-char 610962
a\tb
```

Bytes of `0x80` and above are rejected:

```sh
$ ora-char C3A4
error: byte 1 (0xC3) is not ASCII, only ASCII data is supported
```

### Verbose output

The value is shown in double quotes so the blank padding of `CHAR` columns is
visible:

```sh
$ ora-char -v "Typ=96 Len=5: 104,105,32,32,32"
type:     CHAR (Typ=96)
format:   dec
length:   5
bytes:    68 69 20 20 20
value:    "hi   "
```

### Reverse conversion

With `-r`, the input is ASCII text and the output is its internal
representation: hex bytes separated by spaces (e.g. `48 65 6C`) by default, or
a decimal byte list as in `DUMP(x)` with `-f dec`.

```sh
$ ora-char -r Hello
48 65 6C 6C 6F

$ ora-char -r -f dec Hello
72,101,108,108,111
```

The escape sequences that the forward conversion prints (`\\`, `\n`, `\t`,
`\r`, `\0`, `\xNN`, and `\"` from verbose output) are decoded, so its output
converts back to the same bytes. Write a backslash as `\\`, or use `--raw` to
take the text literally:

```sh
$ ora-char -r 'a\tb'
61 09 62

$ ora-char -r --raw 'C:\temp'
43 3A 5C 74 65 6D 70
```

Text that starts with `-` must follow `--`, e.g. `ora-char -r -- -1`.
Non-ASCII characters and the empty string (which Oracle stores as `NULL`) are
rejected:

```sh
$ ora-char -r Grüße
error: character 3 ('ü') is not ASCII, only ASCII data is supported
```

With `-v`, the value, its length, the `DUMP()` output and the hex bytes are
shown:

```sh
$ ora-char -r -v 'hi   '
value:    "hi   "
length:   5
dump:     Typ=1 Len=5: 104,105,32,32,32
bytes:    68 69 20 20 20
```

## Testing

```sh
cargo test
```
