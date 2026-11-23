// Copyright (c) 2026 Stefan Roesch

//! Convert the internal byte representation of an Oracle `CHAR` or
//! `VARCHAR2` value (as shown by `DUMP()`, `DUMP(x, 16)`, `RAWTOHEX()` or a
//! block dump) into its characters.
//!
//! With `--reverse`, it converts ASCII text into that representation instead.
//!
//! This release only supports ASCII data: every byte must be below 0x80.
//! Control characters are printed as escape sequences unless `--raw` is given.

use clap::{Parser, ValueEnum};
use std::fmt;
use std::io::{self, Write};
use std::process::ExitCode;

/// Input format.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Detect the format from the input
    Auto,
    /// Decimal byte list, e.g. "97,98,99" (output of DUMP(x))
    Dec,
    /// Hex bytes, e.g. "616263", "61,62,63" or "col 0: [ 3] 61 62 63"
    /// (output of DUMP(x,16), RAWTOHEX or a block dump)
    Hex,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Auto => "auto",
            Format::Dec => "dec",
            Format::Hex => "hex",
        })
    }
}

/// Convert the Oracle internal representation of a CHAR or VARCHAR2 value
/// into its characters, or with -r characters into the internal representation
/// (ASCII only).
#[derive(Parser, Debug)]
#[command(version, about, long_about = None, after_help = "Copyright (c) 2026 Stefan Roesch")]
struct Args {
    /// Oracle internal value, e.g. "Typ=1 Len=3: 97,98,99", "97,98,99", "616263", "0x616263"
    /// or "col 0: [ 3] 61 62 63"; with -r ASCII text, e.g. "abc"
    input: String,

    /// Convert ASCII text into the internal representation
    #[arg(short, long)]
    reverse: bool,

    /// Input format (output format with -r: hex, the default, or dec)
    #[arg(short, long, value_enum, default_value_t = Format::Auto)]
    format: Format,

    /// Print the characters unchanged instead of escaping control characters and backslashes
    /// (with -r: take the text literally instead of decoding escape sequences)
    #[arg(long)]
    raw: bool,

    /// Print type, length and bytes as well
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Error {
    Empty,
    NullValue,
    BadToken(String),
    NotAscii {
        pos: usize,
        byte: u8,
    },
    BadHeader(String),
    DuplicateField(String),
    UnsupportedType(u32),
    NationalCharset(Option<String>),
    BadPrefix(String),
    LengthMismatch {
        declared: usize,
        actual: usize,
    },
    NullColumn(u32),
    DecBlockDump,
    /// `-r` input character at 1-based position `pos` is not ASCII.
    NotAsciiChar {
        pos: usize,
        ch: char,
    },
    /// `-r` input has a `\\xNN` escape sequence `seq`, starting at 1-based
    /// character position `pos`, for a byte that is not ASCII.
    NotAsciiEscape {
        pos: usize,
        seq: String,
    },
    /// `-r` input has an unknown or incomplete escape sequence.
    BadEscape(String),
    /// `-r` input is empty, which Oracle stores as NULL.
    EmptyText,
    /// Writing the output failed (the message of the `io::Error`).
    Output(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Empty => write!(f, "no bytes given"),
            Error::NullValue => write!(f, "the value is NULL, there are no characters to convert"),
            Error::BadToken(t) => write!(f, "cannot parse '{t}' as a byte"),
            Error::NotAscii { pos, byte } => write!(
                f,
                "byte {pos} (0x{byte:02X}) is not ASCII, only ASCII data is supported"
            ),
            Error::BadHeader(t) => write!(
                f,
                "cannot parse DUMP header '{t}', expected e.g. 'Typ=1 Len=3: 97,98,99'"
            ),
            Error::DuplicateField(key) => {
                write!(f, "DUMP header has the field '{key}' more than once")
            }
            Error::UnsupportedType(typ) => match type_name(*typ) {
                Some(name) if *typ == 2 => {
                    write!(f, "Typ={typ} is {name}, not a character type, use ora-num")
                }
                Some(name) => write!(f, "Typ={typ} is {name}, not a character type"),
                None => write!(
                    f,
                    "Typ={typ} is not a character type, expected Typ=1 (VARCHAR2) or Typ=96 (CHAR)"
                ),
            },
            Error::NationalCharset(cs) => write!(
                f,
                "the data is in {}, the NCHAR/NVARCHAR2 national character set; \
                 NCHAR and NVARCHAR2 are not supported",
                cs.as_deref().unwrap_or("AL16UTF16 (UTF-16)")
            ),
            Error::BadPrefix(t) => write!(
                f,
                "cannot parse block dump prefix in '{t}', expected e.g. 'col 0: [ 3] 61 62 63'"
            ),
            Error::LengthMismatch { declared, actual } => {
                write!(f, "input declares {declared} bytes, but {actual} are given")
            }
            Error::DecBlockDump => write!(
                f,
                "block dump bytes are always hex, -f dec cannot be used with them"
            ),
            Error::NullColumn(col) => write!(
                f,
                "column {col} is NULL in the block dump, there are no characters to convert"
            ),
            Error::NotAsciiChar { pos, ch } => write!(
                f,
                "character {pos} ('{ch}') is not ASCII, only ASCII data is supported"
            ),
            Error::NotAsciiEscape { pos, seq } => write!(
                f,
                "escape sequence '{seq}' at character {pos} is not ASCII, \
                 only ASCII data is supported"
            ),
            Error::BadEscape(seq) => write!(
                f,
                "invalid escape sequence '{seq}', use \\\\ for a backslash or --raw to take \
                 the text literally"
            ),
            Error::EmptyText => write!(
                f,
                "the text is empty, Oracle stores an empty string as NULL"
            ),
            Error::Output(msg) => write!(f, "cannot write output: {msg}"),
        }
    }
}

/// Name of an Oracle internal data type code as shown by `DUMP()`.
fn type_name(typ: u32) -> Option<&'static str> {
    Some(match typ {
        1 => "VARCHAR2",
        2 => "NUMBER",
        12 => "DATE",
        23 => "RAW",
        96 => "CHAR",
        100 => "BINARY_FLOAT",
        101 => "BINARY_DOUBLE",
        _ => return None,
    })
}

/// The fields of a `DUMP()` header such as "Typ=1 Len=3" or
/// "Typ=96 Len=5 CharacterSet=AL32UTF8" (from `DUMP(x, 1016)`).
#[derive(Debug, Default, PartialEq, Eq)]
struct Header {
    typ: Option<u32>,
    len: Option<usize>,
    charset: Option<String>,
}

fn parse_header(text: &str) -> Result<Header, Error> {
    let bad = || Error::BadHeader(text.to_string());
    let mut header = Header::default();
    for field in text.split_whitespace() {
        let (key, value) = field.split_once('=').ok_or_else(bad)?;
        // Each field may appear only once, even with the same value.
        fn set<T>(slot: &mut Option<T>, key: &str, value: T) -> Result<(), Error> {
            match slot.replace(value) {
                Some(_) => Err(Error::DuplicateField(key.to_string())),
                None => Ok(()),
            }
        }
        match key {
            "Typ" => set(&mut header.typ, key, value.parse().map_err(|_| bad())?)?,
            "Len" => set(&mut header.len, key, value.parse().map_err(|_| bad())?)?,
            "CharacterSet" => set(&mut header.charset, key, value.to_string())?,
            _ => return Err(bad()),
        }
    }
    if let Some(typ) = header.typ
        && typ != 1
        && typ != 96
    {
        return Err(Error::UnsupportedType(typ));
    }
    if let Some(cs) = &header.charset
        && (cs.eq_ignore_ascii_case("AL16UTF16") || cs.eq_ignore_ascii_case("UTF16"))
    {
        return Err(Error::NationalCharset(Some(cs.clone())));
    }
    Ok(header)
}

/// The bytes of the value and how they were read.
#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    bytes: Vec<u8>,
    /// The format the bytes were read in (never `Auto`).
    format: Format,
    header: Option<Header>,
    /// With `--format auto`: the other format, and the bytes it gives, if
    /// the input is also valid printable text in that format.
    alternative: Option<(Format, Vec<u8>)>,
}

/// Parse the command line text into raw bytes.
fn parse_input(input: &str, format: Format) -> Result<Parsed, Error> {
    let mut text = input.trim();
    // DUMP(NULL) and DUMP('') return "NULL".
    if text.eq_ignore_ascii_case("NULL") {
        return Err(Error::NullValue);
    }
    // Strip a "Typ=1 Len=3:" prefix as produced by DUMP().
    let mut header = None;
    if text.starts_with("Typ=") {
        let (h, rest) = text
            .split_once(':')
            .ok_or_else(|| Error::BadHeader(text.to_string()))?;
        header = Some(parse_header(h)?);
        text = rest.trim();
    }
    // Strip a "col 0: [ 3]" prefix as produced by a block dump; the bytes are hex.
    let mut block_len = None;
    if header.is_none()
        && let Some((rest, len)) = strip_block_dump_prefix(text)?
    {
        text = rest;
        block_len = Some(len);
    }
    let declared_len = block_len.or(header.as_ref().and_then(|h| h.len));

    let tokens: Vec<&str> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err(Error::Empty);
    }

    let has_hex_letters = text.chars().any(|c| matches!(c, 'a'..='f' | 'A'..='F'));
    // Hex-dump style: space separated, no commas, every token exactly two characters ("61 62 63").
    let hex_dump_style =
        tokens.len() > 1 && !text.contains(',') && tokens.iter().all(|t| t.len() == 2);
    let (chosen, other) = match format {
        // Block dumps always print the bytes in hex.
        Format::Dec if block_len.is_some() => return Err(Error::DecBlockDump),
        _ if block_len.is_some() => (Format::Hex, None),
        Format::Auto if tokens.len() > 1 || header.is_some() => {
            if has_hex_letters || hex_dump_style {
                (Format::Hex, Some(Format::Dec))
            } else {
                (Format::Dec, Some(Format::Hex))
            }
        }
        // Single token: contiguous hex string.
        Format::Auto => (Format::Hex, Some(Format::Dec)),
        f => (f, None),
    };

    // A reading in the other format counts only if it is plausible: it
    // parses, has the declared length, and is printable text.
    let plausible =
        |bytes: &[u8]| is_printable(bytes) && declared_len.is_none_or(|len| len == bytes.len());
    let other = other.and_then(|o| parse_tokens(&tokens, o).ok().map(|b| (o, b)));
    let (bytes, format, alternative) = match (parse_tokens(&tokens, chosen), other) {
        // Prefer the other format if only it gives plausible text, e.g.
        // "Typ=1 Len=2: 20,41" is " A" in hex, but control characters in decimal.
        // Non-ASCII bytes are reported instead: "80" is 0x80, not "P".
        (Ok(b), Some((o, alt))) if !plausible(&b) && b.is_ascii() && plausible(&alt) => {
            (alt, o, None)
        }
        // The other reading is only an alternative if it gives different
        // text: "9" is a tab in both formats.
        (Ok(b), alt) => {
            let alt = alt.filter(|(_, a)| plausible(a) && *a != b);
            (b, chosen, alt)
        }
        (Err(_), Some((o, alt))) => (alt, o, None),
        (Err(e), None) => return Err(e),
    };

    if let Some(declared) = declared_len
        && declared != bytes.len()
    {
        return Err(Error::LengthMismatch {
            declared,
            actual: bytes.len(),
        });
    }
    // DUMP(x) of an NCHAR/NVARCHAR2 column has the same Typ as CHAR/VARCHAR2
    // and no character set, so recognize AL16UTF16 data by its bytes.
    if looks_like_utf16(&bytes) {
        return Err(Error::NationalCharset(None));
    }
    if let Some(pos) = bytes.iter().position(|b| !b.is_ascii()) {
        return Err(Error::NotAscii {
            pos: pos + 1,
            byte: bytes[pos],
        });
    }
    Ok(Parsed {
        bytes,
        format,
        header,
        alternative,
    })
}

/// Parse the byte tokens in the given format (`Dec` or `Hex`).
fn parse_tokens(tokens: &[&str], format: Format) -> Result<Vec<u8>, Error> {
    let bad = |t: &str| Error::BadToken(t.to_string());
    match format {
        Format::Dec => tokens
            .iter()
            .map(|t| {
                if !t.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(bad(t));
                }
                t.parse::<u8>().map_err(|_| bad(t))
            })
            .collect(),
        Format::Hex if tokens.len() == 1 => parse_hex_string(tokens[0]),
        // Each token may hold several bytes ("6162 6364", wrapped RAWTOHEX
        // output) and an 0x prefix ("0x41,0x42").
        Format::Hex => tokens.iter().try_fold(Vec::new(), |mut bytes, t| {
            match parse_hex_string(t) {
                Ok(b) => bytes.extend(b),
                // A lone "0x" among other tokens is a bad token, not empty input.
                Err(Error::Empty) => return Err(bad(t)),
                Err(e) => return Err(e),
            }
            Ok(bytes)
        }),
        Format::Auto => unreachable!("format must be resolved before parsing"),
    }
}

/// Strip a block dump prefix such as "col 0: [ 3]" (column number, then the
/// byte count in brackets). Returns the remaining text and the declared byte
/// count, or `None` if the text does not start with "col". A NULL column
/// ("col 1: *NULL*") is reported as `Error::NullColumn`.
fn strip_block_dump_prefix(text: &str) -> Result<Option<(&str, usize)>, Error> {
    let Some(rest) = text.strip_prefix("col") else {
        return Ok(None);
    };
    let bad = || Error::BadPrefix(text.to_string());
    let (col, rest) = rest.split_once(':').ok_or_else(bad)?;
    let col = col.trim().parse::<u32>().map_err(|_| bad())?;
    if rest.trim() == "*NULL*" {
        return Err(Error::NullColumn(col));
    }
    let rest = rest.trim_start().strip_prefix('[').ok_or_else(bad)?;
    let (len, rest) = rest.split_once(']').ok_or_else(bad)?;
    let len = len.trim().parse::<usize>().map_err(|_| bad())?;
    Ok(Some((rest.trim(), len)))
}

/// Parse a contiguous hex string such as "616263" or "0x616263". A single
/// hex digit is one byte, as `DUMP(x, 16)` prints bytes below 0x10 that way.
fn parse_hex_string(s: &str) -> Result<Vec<u8>, Error> {
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if s.is_empty() {
        return Err(Error::Empty);
    }
    if s.len() == 1 {
        return u8::from_str_radix(s, 16)
            .map(|b| vec![b])
            .map_err(|_| Error::BadToken(s.to_string()));
    }
    if !s.len().is_multiple_of(2) || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::BadToken(s.to_string()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| Error::BadToken(s[i..i + 2].to_string()))
        })
        .collect()
}

/// True if the bytes look like AL16UTF16 (big-endian UTF-16) text from the
/// Basic Latin or Latin-1 range: an even number of bytes with every
/// high-order byte zero. Such values are rare in CHAR/VARCHAR2 data, as they
/// mean every other character is NUL.
fn looks_like_utf16(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.len().is_multiple_of(2) && bytes.iter().step_by(2).all(|&b| b == 0)
}

/// True if every byte is printable ASCII, a tab, a newline or a carriage return.
fn is_printable(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes
            .iter()
            .all(|&b| matches!(b, 0x20..=0x7E | b'\t' | b'\n' | b'\r'))
}

/// Render ASCII bytes as text, escaping backslashes and control characters
/// (`\\`, `\n`, `\t`, `\r`, `\0`, otherwise `\xNN`). With `quoted`, double
/// quotes are escaped as well and the result is put in double quotes.
fn escape(bytes: &[u8], quoted: bool) -> String {
    let mut out = String::new();
    if quoted {
        out.push('"');
    }
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' if quoted => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0 => out.push_str("\\0"),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02X}")),
        }
    }
    if quoted {
        out.push('"');
    }
    out
}

/// Bytes of the ASCII text `text` for `--reverse`. Unless `raw`, the escape
/// sequences that the forward conversion prints (`\\`, `\n`, `\t`, `\r`,
/// `\0`, `\xNN`, and `\"` from verbose output) are decoded.
fn encode_text(text: &str, raw: bool) -> Result<Vec<u8>, Error> {
    if text.is_empty() {
        return Err(Error::EmptyText);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut bytes = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !c.is_ascii() {
            return Err(Error::NotAsciiChar { pos: i + 1, ch: c });
        }
        if raw || c != '\\' {
            bytes.push(c as u8);
            i += 1;
            continue;
        }
        let bad =
            |len: usize| Error::BadEscape(chars[i..(i + len).min(chars.len())].iter().collect());
        let (byte, len) = match chars.get(i + 1) {
            Some('\\') => (b'\\', 2),
            Some('"') => (b'"', 2),
            Some('n') => (b'\n', 2),
            Some('t') => (b'\t', 2),
            Some('r') => (b'\r', 2),
            Some('0') => (0, 2),
            Some('x') => {
                let digits = chars.get(i + 2..i + 4).ok_or_else(|| bad(4))?;
                if !digits.iter().all(char::is_ascii_hexdigit) {
                    return Err(bad(4));
                }
                let byte = u8::from_str_radix(&digits.iter().collect::<String>(), 16)
                    .map_err(|_| bad(4))?;
                if !byte.is_ascii() {
                    return Err(Error::NotAsciiEscape {
                        pos: i + 1,
                        seq: chars[i..i + 4].iter().collect(),
                    });
                }
                (byte, 4)
            }
            _ => return Err(bad(2)),
        };
        bytes.push(byte);
        i += len;
    }
    Ok(bytes)
}

/// Write the internal representation of `bytes` (`--reverse`) to `out`:
/// hex bytes separated by spaces, or a decimal byte list with `-f dec`.
fn write_reverse(out: &mut impl Write, bytes: &[u8], args: &Args) -> io::Result<()> {
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
    let hex = hex.join(" ");
    let dec: Vec<String> = bytes.iter().map(u8::to_string).collect();
    let dec = dec.join(",");
    if args.verbose {
        writeln!(out, "value:    {}", escape(bytes, true))?;
        writeln!(out, "length:   {}", bytes.len())?;
        writeln!(out, "dump:     Typ=1 Len={}: {dec}", bytes.len())?;
        writeln!(out, "bytes:    {hex}")
    } else if args.format == Format::Dec {
        writeln!(out, "{dec}")
    } else {
        writeln!(out, "{hex}")
    }
}

fn run(args: &Args) -> Result<(), Error> {
    let mut out = io::stdout().lock();
    let written = if args.reverse {
        let bytes = encode_text(&args.input, args.raw)?;
        write_reverse(&mut out, &bytes, args)
    } else {
        let parsed = parse_input(&args.input, args.format)?;
        if let Some((other, alt)) = &parsed.alternative {
            eprintln!(
                "note: input is ambiguous, read as {} giving {}; as {other} it is {}, use -f to choose",
                parsed.format,
                escape(&parsed.bytes, true),
                escape(alt, true)
            );
        }
        write_output(&mut out, &parsed, args)
    };
    match written.and_then(|()| out.flush()) {
        // The reader went away (e.g. `| head -c 1`): nothing left to do.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result.map_err(|e| Error::Output(e.to_string())),
    }
}

/// Write the converted value (and with `--verbose` its details) to `out`.
fn write_output(out: &mut impl Write, parsed: &Parsed, args: &Args) -> io::Result<()> {
    let bytes = &parsed.bytes;
    if args.verbose {
        if let Some(typ) = parsed.header.as_ref().and_then(|h| h.typ) {
            writeln!(
                out,
                "type:     {} (Typ={typ})",
                type_name(typ).unwrap_or("?")
            )?;
        }
        if let Some(cs) = parsed.header.as_ref().and_then(|h| h.charset.as_ref()) {
            writeln!(out, "charset:  {cs}")?;
        }
        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
        writeln!(out, "format:   {}", parsed.format)?;
        writeln!(out, "length:   {}", bytes.len())?;
        writeln!(out, "bytes:    {}", hex.join(" "))?;
        write!(out, "value:    ")?;
    }
    if args.raw {
        out.write_all(bytes)?;
        out.write_all(b"\n")
    } else {
        // Quote in verbose mode so that trailing blanks (CHAR padding) are visible.
        writeln!(out, "{}", escape(bytes, args.verbose))
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(input: &str) -> String {
        escape(&parse_input(input, Format::Auto).unwrap().bytes, false)
    }

    fn conv_as(input: &str, format: Format) -> String {
        escape(&parse_input(input, format).unwrap().bytes, false)
    }

    fn err(input: &str) -> Error {
        parse_input(input, Format::Auto).unwrap_err()
    }

    #[test]
    fn input_formats() {
        // All of these are "Hello".
        for input in [
            "Typ=1 Len=5: 72,101,108,108,111",
            "Typ=1 Len=5: 48,65,6c,6c,6f",
            "Typ=96 Len=5: 72,101,108,108,111",
            "Typ=1 Len=5 CharacterSet=AL32UTF8: 48,65,6c,6c,6f",
            "72,101,108,108,111",
            "72 101 108 108 111",
            "48656C6C6F",
            "48656c6c6f",
            "0x48656C6C6F",
            "0X48656c6c6f",
            "48 65 6c 6c 6f",
            "  48  65 6c 6c 6f ",
            "col 0: [ 5] 48 65 6c 6c 6f",
            "col  1: [ 5]  48 65 6C 6C 6F",
        ] {
            assert_eq!(conv(input), "Hello", "{input}");
        }
    }

    #[test]
    fn forced_format() {
        assert_eq!(conv_as("72,101,108,108,111", Format::Dec), "Hello");
        assert_eq!(conv_as("48,65,6c,6c,6f", Format::Hex), "Hello");
        assert_eq!(conv_as("41 42", Format::Dec), ")*");
        assert_eq!(conv_as("65", Format::Dec), "A");
        assert_eq!(conv_as("65", Format::Hex), "e");
        assert_eq!(
            parse_input("48,65,6c", Format::Dec),
            Err(Error::BadToken("6c".into()))
        );
    }

    #[test]
    fn block_dump() {
        assert_eq!(conv("col 0: [ 1] 41"), "A");
        // Block dump bytes are hex even without hex letters.
        assert_eq!(conv("col 2: [ 3] 31 32 33"), "123");
        // CHAR columns are blank padded.
        assert_eq!(conv("col 3: [ 5] 61 62 20 20 20"), "ab   ");
        // Longer values are wrapped over several lines.
        let wrapped = "col  0: [26]\n 61 62 63 64 65 66 67 68 69 6a 6b 6c 6d 6e 6f 70 71 72 73 74 75 76 77 78 79\n 7a";
        assert_eq!(conv(wrapped), "abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn block_dump_errors() {
        assert_eq!(
            parse_input("col 0: [ 2] 41 42", Format::Dec),
            Err(Error::DecBlockDump)
        );
        assert_eq!(
            parse_input("col 0: [ 2] 41 42", Format::Hex).unwrap().bytes,
            b"AB"
        );
        assert_eq!(
            err("col 0: [ 3] 61 62"),
            Error::LengthMismatch {
                declared: 3,
                actual: 2
            }
        );
        assert_eq!(err("col 0: [ 0]"), Error::Empty);
        assert_eq!(err("col 1: *NULL*"), Error::NullColumn(1));
        assert_eq!(err("col  7:  *NULL* "), Error::NullColumn(7));
        for bad in [
            "col 0 [ 2] 61 62",
            "col x: [ 2] 61 62",
            "col 0: 2] 61 62",
            "col 0: [ 2 61 62",
        ] {
            assert!(matches!(err(bad), Error::BadPrefix(_)), "{bad}");
        }
    }

    #[test]
    fn dump_header() {
        assert_eq!(
            err("Typ=1 Len=3: 97,98"),
            Error::LengthMismatch {
                declared: 3,
                actual: 2
            }
        );
        assert_eq!(err("Typ=2 Len=3: 194,2,24"), Error::UnsupportedType(2));
        assert_eq!(
            err("Typ=12 Len=7: 120,126,11,22,1,1,1"),
            Error::UnsupportedType(12)
        );
        assert_eq!(err("Typ=999 Len=1: 65"), Error::UnsupportedType(999));
        assert_eq!(
            err("Typ=1 Len=2 CharacterSet=AL16UTF16: 0,41"),
            Error::NationalCharset(Some("AL16UTF16".into()))
        );
        for bad in [
            "Typ=1 Len=3 97,98,99",
            "Typ=x Len=1: 65",
            "Typ=1 Len=y: 65",
            "Typ=1 Foo=1: 65",
            "Typ=1 Len: 65",
        ] {
            assert!(matches!(err(bad), Error::BadHeader(_)), "{bad}");
        }
        // Repeated fields are rejected, also with the same value and
        // wherever the repetition is.
        for (bad, key) in [
            ("Typ=1 Len=1 Len=2: 104,105", "Len"),
            ("Typ=1 Len=2 Len=2: 104,105", "Len"),
            ("Typ=1 Typ=1 Len=2: 104,105", "Typ"),
            ("Typ=1 Len=2 Typ=96: 104,105", "Typ"),
            (
                "Typ=1 Len=2 CharacterSet=AL32UTF8 CharacterSet=AL32UTF8: 104,105",
                "CharacterSet",
            ),
            (
                "Typ=1 CharacterSet=US7ASCII Len=2 CharacterSet=AL16UTF16: 0,65",
                "CharacterSet",
            ),
        ] {
            assert_eq!(err(bad), Error::DuplicateField(key.into()), "{bad}");
        }
        // A repeated field with an unparsable value is a syntax error first.
        assert!(matches!(
            err("Typ=1 Len=2 Len=x: 104,105"),
            Error::BadHeader(_)
        ));
        let parsed = parse_input("Typ=96 Len=1 CharacterSet=US7ASCII: 65", Format::Auto).unwrap();
        assert_eq!(
            parsed.header,
            Some(Header {
                typ: Some(96),
                len: Some(1),
                charset: Some("US7ASCII".into())
            })
        );
        // A comma list with a header is decimal unless it contains hex letters.
        assert_eq!(conv("Typ=1 Len=1: 65"), "A");
        // A contiguous token after a header is not a decimal byte: read as hex.
        assert_eq!(conv("Typ=1 Len=2: 4142"), "AB");
        assert_eq!(
            err("Typ=1 Len=3: 4142"),
            Error::LengthMismatch {
                declared: 3,
                actual: 2
            }
        );
    }

    #[test]
    fn null_and_empty() {
        assert_eq!(err("NULL"), Error::NullValue);
        assert_eq!(err(" null "), Error::NullValue);
        assert_eq!(err(""), Error::Empty);
        assert_eq!(err("   "), Error::Empty);
        assert_eq!(err("Typ=1 Len=0:"), Error::Empty);
        assert_eq!(err(","), Error::Empty);
    }

    #[test]
    fn national_charset() {
        // DUMP of N'Hi' in an NVARCHAR2 column: Typ=1, but AL16UTF16 bytes.
        for input in [
            "Typ=1 Len=4: 0,72,0,105",
            "Typ=96 Len=4: 0,48,0,69",
            "00480069",
            "00 48 00 69",
            "col 0: [ 4] 00 48 00 69",
            "Typ=1 Len=2: 0,65",
            // Latin-1 characters beyond ASCII: N'ä'.
            "00E4",
        ] {
            assert_eq!(err(input), Error::NationalCharset(None), "{input}");
        }
        for cs in ["AL16UTF16", "al16utf16", "UTF16"] {
            assert_eq!(
                err(&format!("Typ=1 Len=2 CharacterSet={cs}: 0,65")),
                Error::NationalCharset(Some(cs.into()))
            );
        }
        // A NUL that is not part of the UTF-16 pattern is CHAR/VARCHAR2 data.
        assert_eq!(conv("410042"), "A\\0B");
        assert_eq!(conv("004142"), "\\0AB");
        assert_eq!(conv("00"), "\\0");
        assert_eq!(conv("41004200"), "A\\0B\\0");
    }

    #[test]
    fn ascii_only() {
        assert_eq!(err("C3A4"), Error::NotAscii { pos: 1, byte: 0xC3 });
        assert_eq!(
            err("Typ=1 Len=3: 97,98,200"),
            Error::NotAscii { pos: 3, byte: 200 }
        );
        assert_eq!(
            err("col 0: [ 2] 41 80"),
            Error::NotAscii { pos: 2, byte: 0x80 }
        );
        assert_eq!(conv("7F"), "\\x7F");
    }

    #[test]
    fn bad_tokens() {
        assert_eq!(err("6162636"), Error::BadToken("6162636".into()));
        assert_eq!(err("0x"), Error::Empty);
        assert_eq!(err("0xg"), Error::BadToken("g".into()));
        assert_eq!(err("0x123"), Error::BadToken("123".into()));
        // A single hex digit is one byte (DUMP(x, 16) prints 0x09 as "9").
        assert_eq!(conv_as("Typ=1 Len=3: 61,9,62", Format::Hex), "a\\tb");
        assert_eq!(conv_as("9", Format::Hex), "\\t");
        assert_eq!(err("61,256"), Error::BadToken("256".into()));
        assert_eq!(err("61,-1"), Error::BadToken("-1".into()));
        assert_eq!(err("61,+1"), Error::BadToken("+1".into()));
        assert_eq!(
            parse_input("61 6g", Format::Hex),
            Err(Error::BadToken("6g".into()))
        );
        assert_eq!(
            parse_input("+6", Format::Hex),
            Err(Error::BadToken("+6".into()))
        );
        assert_eq!(err("hello"), Error::BadToken("hello".into()));
        // Non-ASCII input must not panic when slicing the hex string.
        assert!(parse_input("61ä2", Format::Auto).is_err());
        // Bad tokens among several hex tokens.
        assert_eq!(
            parse_input("6162 636", Format::Hex),
            Err(Error::BadToken("636".into()))
        );
        assert_eq!(
            parse_input("0x41 0x", Format::Hex),
            Err(Error::BadToken("0x".into()))
        );
        assert_eq!(
            parse_input("0x41,0xg", Format::Hex),
            Err(Error::BadToken("g".into()))
        );
    }

    #[test]
    fn multi_byte_hex_tokens() {
        // Wrapped or grouped RAWTOHEX output, 0x prefixes, mixed token sizes.
        for input in [
            "4865 6C6C 6F",
            "48656C\n6C6F",
            "0x48,0x65,0x6c,0x6c,0x6f",
            "0x4865 0x6C6C6F",
            "4865,6c,6c6f",
            "col 0: [ 5] 4865 6c6c 6f",
        ] {
            assert_eq!(conv(input), "Hello", "{input}");
        }
        // The byte count is checked against the bytes, not the tokens.
        assert_eq!(
            err("col 0: [ 2] 4865 6c"),
            Error::LengthMismatch {
                declared: 2,
                actual: 3
            }
        );
        // Several bytes per token cannot be decimal, so there is no note.
        let p = parse_input("6162 6364", Format::Auto).unwrap();
        assert_eq!((p.format, p.bytes.as_slice()), (Format::Hex, &b"abcd"[..]));
        assert_eq!(p.alternative, None);
    }

    #[test]
    fn identical_readings() {
        // Single digits are the same byte in both formats: no ambiguity note.
        for input in ["Typ=1 Len=1: 9", "Typ=1 Len=3: 9,9,9", "9 9", "9"] {
            let p = parse_input(input, Format::Auto).unwrap();
            assert_eq!(p.alternative, None, "{input}");
        }
        // Different readings are still reported.
        let p = parse_input("Typ=1 Len=2: 9,65", Format::Auto).unwrap();
        assert_eq!(p.alternative, Some((Format::Hex, b"\te".to_vec())));
    }

    /// A writer that fails every write, like a full disk.
    struct FailingWriter(io::ErrorKind);

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(self.0.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn args(raw: bool, verbose: bool) -> Args {
        Args {
            input: String::new(),
            reverse: false,
            format: Format::Auto,
            raw,
            verbose,
        }
    }

    #[test]
    fn output() {
        let parsed = parse_input("Typ=96 Len=3: 104,105,32", Format::Auto).unwrap();
        let render = |raw, verbose| {
            let mut out = Vec::new();
            write_output(&mut out, &parsed, &args(raw, verbose)).unwrap();
            String::from_utf8(out).unwrap()
        };
        assert_eq!(render(false, false), "hi \n");
        assert_eq!(render(true, false), "hi \n");
        assert!(render(false, true).ends_with("value:    \"hi \"\n"));
        let parsed = parse_input("610962", Format::Auto).unwrap();
        let mut out = Vec::new();
        write_output(&mut out, &parsed, &args(true, false)).unwrap();
        assert_eq!(out, b"a\tb\n");
    }

    #[test]
    fn write_errors() {
        let parsed = parse_input("4142", Format::Auto).unwrap();
        for (raw, verbose) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut out = FailingWriter(io::ErrorKind::StorageFull);
            let result = write_output(&mut out, &parsed, &args(raw, verbose));
            assert_eq!(
                result.map_err(|e| e.kind()),
                Err(io::ErrorKind::StorageFull),
                "raw={raw} verbose={verbose}"
            );
        }
    }

    #[test]
    fn ambiguity() {
        // Uppercase letters in decimal are valid hex, too: "ABC" or "efg".
        let p = parse_input("65,66,67", Format::Auto).unwrap();
        assert_eq!((p.format, p.bytes.as_slice()), (Format::Dec, &b"ABC"[..]));
        assert_eq!(p.alternative, Some((Format::Hex, b"efg".to_vec())));
        // Space separated pairs are read as hex first.
        let p = parse_input("41 42", Format::Auto).unwrap();
        assert_eq!((p.format, p.bytes.as_slice()), (Format::Hex, &b"AB"[..]));
        assert_eq!(p.alternative, Some((Format::Dec, b")*".to_vec())));
        // A single two-digit token is hex.
        let p = parse_input("65", Format::Auto).unwrap();
        assert_eq!(p.bytes, b"e");
        assert_eq!(p.alternative, Some((Format::Dec, b"A".to_vec())));
        // Unambiguous inputs have no alternative.
        for input in [
            "72,101,108,108,111",
            "48,65,6c,6c,6f",
            "48656C6C6F",
            "col 0: [ 2] 41 42",
        ] {
            let p = parse_input(input, Format::Auto).unwrap();
            assert_eq!(p.alternative, None, "{input}");
        }
        // A forced format never has an alternative.
        assert_eq!(parse_input("65", Format::Dec).unwrap().alternative, None);
    }

    #[test]
    fn prefer_printable() {
        // Decimal gives control characters, hex gives text: hex wins.
        let p = parse_input("Typ=1 Len=2: 20,41", Format::Auto).unwrap();
        assert_eq!((p.format, p.bytes.as_slice()), (Format::Hex, &b" A"[..]));
        assert_eq!(p.alternative, None);
        // Hex gives control characters, decimal gives text: decimal wins.
        let p = parse_input("10 32", Format::Auto).unwrap();
        assert_eq!((p.format, p.bytes.as_slice()), (Format::Dec, &b"\n "[..]));
        // Neither is printable: keep the detected format.
        let p = parse_input("01,02", Format::Auto).unwrap();
        assert_eq!((p.format, p.bytes.as_slice()), (Format::Dec, &[1u8, 2][..]));
        // A forced format is never switched.
        let p = parse_input("Typ=1 Len=2: 20,41", Format::Dec).unwrap();
        assert_eq!(p.bytes, [20, 41]);
    }

    #[test]
    fn escaping() {
        assert_eq!(escape(b"abc", false), "abc");
        assert_eq!(escape(b"a\\b", false), "a\\\\b");
        assert_eq!(escape(b"a\"b", false), "a\"b");
        assert_eq!(escape(b"a\"b", true), "\"a\\\"b\"");
        assert_eq!(escape(b"\t\n\r\0", false), "\\t\\n\\r\\0");
        assert_eq!(escape(&[0x01, 0x1B, 0x7F], false), "\\x01\\x1B\\x7F");
        assert_eq!(escape(b"ab  ", true), "\"ab  \"");
        assert_eq!(conv("610962"), "a\\tb");
    }

    #[test]
    fn all_ascii_bytes() {
        // Every ASCII byte decodes to itself in every format.
        for b in 0..0x80u8 {
            for (input, format) in [
                (format!("{b:02X}"), Format::Hex),
                (format!("{b}"), Format::Dec),
                (format!("col 0: [ 1] {b:02x}"), Format::Auto),
                (format!("Typ=1 Len=1: {b}"), Format::Dec),
                (format!("Typ=1 Len=1: {b:x}"), Format::Hex),
            ] {
                assert_eq!(parse_input(&input, format).unwrap().bytes, [b], "{input}");
            }
        }
    }

    #[test]
    fn long_values() {
        // VARCHAR2 can hold up to 32767 bytes with MAX_STRING_SIZE=EXTENDED.
        let text = "x".repeat(32767);
        let hex: String = text.bytes().map(|b| format!("{b:02X}")).collect();
        assert_eq!(conv(&hex), text);
        let dec: Vec<String> = text.bytes().map(|b| b.to_string()).collect();
        assert_eq!(conv(&format!("Typ=1 Len=32767: {}", dec.join(","))), text);
    }

    #[test]
    fn separators() {
        // Commas and any whitespace separate bytes; empty tokens are ignored.
        for input in [
            "104,105",
            "104, 105",
            "104 ,105",
            " 104 , 105 ",
            "104,,105",
            "104,105,",
            ",104,105",
            "104\t105",
            "104\n105",
            "104\r\n105",
        ] {
            assert_eq!(conv(input), "hi", "{input:?}");
        }
        assert_eq!(conv("68\t69\n6a"), "hij");
    }

    #[test]
    fn header_variants() {
        // Len and CharacterSet are optional, spacing is free.
        assert_eq!(conv("Typ=1: 104,105"), "hi");
        assert_eq!(conv("Typ=96: 104,105"), "hi");
        assert_eq!(conv("Typ=1 Len=2 : 104,105"), "hi");
        assert_eq!(conv("Typ=1  Len=2:104,105"), "hi");
        assert_eq!(conv("Typ=1 Len=2:\n104,105"), "hi");
        let p = parse_input("Typ=1: 104,105", Format::Auto).unwrap();
        assert_eq!(
            p.header,
            Some(Header {
                typ: Some(1),
                len: None,
                charset: None
            })
        );
        // The header is case sensitive, as printed by Oracle.
        assert_eq!(err("typ=1 Len=1: 65"), Error::BadToken("typ=1".into()));
        assert!(matches!(err("Typ=1 len=1: 65"), Error::BadHeader(_)));
        // Other character sets are accepted; non-ASCII bytes are still rejected.
        for cs in ["US7ASCII", "WE8MSWIN1252", "WE8ISO8859P1", "UTF8"] {
            assert_eq!(
                conv(&format!("Typ=1 Len=2 CharacterSet={cs}: 104,105")),
                "hi"
            );
        }
        assert_eq!(
            err("Typ=1 Len=1 CharacterSet=WE8MSWIN1252: 228"),
            Error::NotAscii { pos: 1, byte: 228 }
        );
        // Typ is checked before the bytes are parsed.
        assert_eq!(err("Typ=23 Len=2: zz"), Error::UnsupportedType(23));
    }

    #[test]
    fn block_dump_variants() {
        assert_eq!(conv("col 0:[2]41 42"), "AB");
        assert_eq!(conv("col 123: [ 2] 41 42"), "AB");
        assert_eq!(conv("col 0: [  2]  41   42  "), "AB");
        assert_eq!(conv("col 0: [2]\n41\n42"), "AB");
        assert_eq!(conv("col 0: [ 2] 41,42"), "AB");
        // Hex letters in either case, and digits only.
        assert_eq!(conv("col 0: [ 2] 6A 6b"), "jk");
        assert_eq!(conv("col 0: [ 2] 30 39"), "09");
        // A block dump after a DUMP header is not a block dump.
        assert!(parse_input("Typ=1 Len=2: col 0: [ 2] 41 42", Format::Auto).is_err());
        // Block dump errors win over byte errors.
        assert_eq!(
            parse_input("col 0: [ 2] zz", Format::Dec),
            Err(Error::DecBlockDump)
        );
        assert_eq!(err("col 9: *NULL*"), Error::NullColumn(9));
    }

    #[test]
    fn null_variants() {
        for input in ["NULL", "null", "Null", "  NULL\n"] {
            assert_eq!(err(input), Error::NullValue, "{input:?}");
        }
        // NULL is only recognized on its own.
        assert_eq!(err("NULL,41"), Error::BadToken("NULL".into()));
    }

    #[test]
    fn national_charset_edges() {
        // Odd length or a non-zero high byte: not UTF-16.
        assert_eq!(conv("004100"), "\\0A\\0");
        assert_eq!(conv("00410142"), "\\0A\\x01B");
        // Two NULs are rejected as UTF-16 (U+0000).
        assert_eq!(err("0000"), Error::NationalCharset(None));
        // The UTF-16 check runs before the ASCII check.
        assert_eq!(err("00E400F6"), Error::NationalCharset(None));
        // Forced formats are checked as well.
        assert_eq!(
            parse_input("0,65", Format::Dec),
            Err(Error::NationalCharset(None))
        );
    }

    #[test]
    fn length_mismatch_after_switch() {
        // The header length is checked against the bytes actually used.
        assert_eq!(
            err("Typ=1 Len=3: 4142"),
            Error::LengthMismatch {
                declared: 3,
                actual: 2
            }
        );
        assert_eq!(
            parse_input("Typ=1 Len=1: 104,105", Format::Dec),
            Err(Error::LengthMismatch {
                declared: 1,
                actual: 2
            })
        );
        // A length that fits only one reading does not make it ambiguous.
        let p = parse_input("Typ=1 Len=2: 4142", Format::Auto).unwrap();
        assert_eq!((p.format, p.alternative), (Format::Hex, None));
    }

    #[test]
    fn not_ascii_position() {
        assert_eq!(err("80"), Error::NotAscii { pos: 1, byte: 0x80 });
        assert_eq!(err("41FF"), Error::NotAscii { pos: 2, byte: 0xFF });
        assert_eq!(
            err("Typ=1 Len=4: 104,105,33,128"),
            Error::NotAscii { pos: 4, byte: 128 }
        );
        // The first non-ASCII byte is reported.
        assert_eq!(err("4180FF"), Error::NotAscii { pos: 2, byte: 0x80 });
        // Hex bytes 0x80..=0x99 are digits only, but must not be reread as
        // decimal text ("80" would be "P").
        assert_eq!(err("90 91"), Error::NotAscii { pos: 1, byte: 0x90 });
        assert_eq!(
            err("col 0: [ 1] 99"),
            Error::NotAscii { pos: 1, byte: 0x99 }
        );
        assert_eq!(conv_as("80", Format::Dec), "P");
    }

    /// The escaped output must be unambiguous: different byte strings never
    /// give the same text.
    #[test]
    fn escape_is_injective() {
        let mut seen = std::collections::HashMap::new();
        for a in 0..0x80u8 {
            for b in 0..0x80u8 {
                for bytes in [vec![a], vec![a, b]] {
                    for quoted in [false, true] {
                        let text = escape(&bytes, quoted);
                        if let Some(prev) = seen.insert((text.clone(), quoted), bytes.clone()) {
                            assert_eq!(prev, bytes, "{text}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn escape_table() {
        for (byte, text) in [
            (0x00, "\\0"),
            (0x01, "\\x01"),
            (0x07, "\\x07"),
            (0x08, "\\x08"),
            (0x09, "\\t"),
            (0x0A, "\\n"),
            (0x0B, "\\x0B"),
            (0x0C, "\\x0C"),
            (0x0D, "\\r"),
            (0x1F, "\\x1F"),
            (0x20, " "),
            (0x22, "\""),
            (0x27, "'"),
            (0x5C, "\\\\"),
            (0x7E, "~"),
            (0x7F, "\\x7F"),
        ] {
            assert_eq!(escape(&[byte], false), text, "0x{byte:02X}");
        }
        assert_eq!(escape(b"", false), "");
        assert_eq!(escape(b"", true), "\"\"");
        // Escaped output contains only printable ASCII.
        let all: Vec<u8> = (0..0x80).collect();
        assert!(
            escape(&all, true)
                .bytes()
                .all(|c| (0x20..=0x7E).contains(&c))
        );
    }

    fn verbose(input: &str, format: Format, raw: bool) -> String {
        let parsed = parse_input(input, format).unwrap();
        let mut out = Vec::new();
        let args = Args {
            input: input.into(),
            reverse: false,
            format,
            raw,
            verbose: true,
        };
        write_output(&mut out, &parsed, &args).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn verbose_output() {
        assert_eq!(
            verbose(
                "Typ=1 Len=5 CharacterSet=AL32UTF8: 48,65,6c,6c,6f",
                Format::Auto,
                false
            ),
            "type:     VARCHAR2 (Typ=1)\n\
             charset:  AL32UTF8\n\
             format:   hex\n\
             length:   5\n\
             bytes:    48 65 6C 6C 6F\n\
             value:    \"Hello\"\n"
        );
        // Without a header there are no type and charset lines.
        assert_eq!(
            verbose("6122095C", Format::Auto, false),
            "format:   hex\n\
             length:   4\n\
             bytes:    61 22 09 5C\n\
             value:    \"a\\\"\\t\\\\\"\n"
        );
        // The format line shows the format the bytes were read in.
        assert!(verbose("65,66", Format::Hex, false).contains("format:   hex\n"));
        assert!(verbose("65,66", Format::Auto, false).contains("format:   dec\n"));
        assert!(verbose("Typ=1 Len=2: 20,41", Format::Auto, false).contains("format:   hex\n"));
        // With --raw the value is unquoted and unescaped.
        assert!(verbose("610962", Format::Auto, true).ends_with("value:    a\tb\n"));
    }

    fn rev(text: &str, format: Format, verbose: bool) -> String {
        let bytes = encode_text(text, false).unwrap();
        let args = Args {
            input: text.into(),
            reverse: true,
            format,
            raw: false,
            verbose,
        };
        let mut out = Vec::new();
        write_reverse(&mut out, &bytes, &args).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn reverse_output() {
        assert_eq!(rev("Hello", Format::Auto, false), "48 65 6C 6C 6F\n");
        assert_eq!(rev("Hello", Format::Hex, false), "48 65 6C 6C 6F\n");
        assert_eq!(rev("Hello", Format::Dec, false), "72,101,108,108,111\n");
        assert_eq!(
            rev("hi   ", Format::Auto, true),
            "value:    \"hi   \"\n\
             length:   5\n\
             dump:     Typ=1 Len=5: 104,105,32,32,32\n\
             bytes:    68 69 20 20 20\n"
        );
        // Verbose output is the same for every format.
        assert_eq!(rev("a", Format::Dec, true), rev("a", Format::Hex, true));
    }

    #[test]
    fn reverse_escapes() {
        let enc = |t| encode_text(t, false);
        assert_eq!(enc(r"a\tb"), Ok(b"a\tb".to_vec()));
        assert_eq!(
            enc(r#"\\\n\r\0\"\x7f\x7F\x01"#),
            Ok(b"\\\n\r\0\"\x7f\x7f\x01".to_vec())
        );
        // Characters that are not escaped stay as they are, including quotes.
        assert_eq!(enc("say \"hi\""), Ok(b"say \"hi\"".to_vec()));
        // --raw takes the text literally.
        assert_eq!(encode_text(r"C:\temp", true), Ok(br"C:\temp".to_vec()));
        assert_eq!(encode_text(r"a\", true), Ok(br"a\".to_vec()));
        for (text, seq) in [
            (r"a\", r"\"),
            (r"\q", r"\q"),
            (r"\x", r"\x"),
            (r"\x4", r"\x4"),
            (r"\x4g", r"\x4g"),
            (r"\x+1", r"\x+1"),
            (r"\xä1", r"\xä1"),
        ] {
            assert_eq!(enc(text), Err(Error::BadEscape(seq.into())), "{text}");
        }
        let not_ascii = |pos, seq: &str| {
            Err(Error::NotAsciiEscape {
                pos,
                seq: seq.into(),
            })
        };
        assert_eq!(enc(r"ab\x80"), not_ascii(3, r"\x80"));
        // The sequence is shown as typed.
        assert_eq!(enc(r"\xFF"), not_ascii(1, r"\xFF"));
        assert_eq!(enc(r"\xff"), not_ascii(1, r"\xff"));
    }

    #[test]
    fn reverse_errors() {
        assert_eq!(encode_text("", false), Err(Error::EmptyText));
        assert_eq!(encode_text("", true), Err(Error::EmptyText));
        assert_eq!(
            encode_text("aäb", false),
            Err(Error::NotAsciiChar { pos: 2, ch: 'ä' })
        );
        assert_eq!(
            encode_text("€", true),
            Err(Error::NotAsciiChar { pos: 1, ch: '€' })
        );
    }

    #[test]
    fn reverse_round_trip() {
        // Every ASCII byte survives escape -> encode_text -> hex -> parse_input.
        let all: Vec<u8> = (0..0x80).collect();
        for quoted in [false, true] {
            let mut text = escape(&all, quoted);
            if quoted {
                text = text[1..text.len() - 1].to_string();
            }
            let bytes = encode_text(&text, false).unwrap();
            assert_eq!(bytes, all, "quoted={quoted}");
            let hex = rev(&text, Format::Hex, false);
            assert_eq!(parse_input(hex.trim_end(), Format::Hex).unwrap().bytes, all);
            let dec = rev(&text, Format::Dec, false);
            assert_eq!(parse_input(dec.trim_end(), Format::Dec).unwrap().bytes, all);
        }
    }

    #[test]
    fn reverse_escape_edges() {
        let enc = |t| encode_text(t, false);
        let bad = |seq: &str| Err(Error::BadEscape(seq.into()));
        // \0 is a single NUL byte, not the start of an octal escape.
        assert_eq!(enc(r"\012"), Ok(b"\x0012".to_vec()));
        // \x takes exactly two hex digits.
        assert_eq!(enc(r"\x414"), Ok(b"A4".to_vec()));
        assert_eq!(enc(r"\x00\x0a"), Ok(vec![0, 0x0A]));
        // An escaped backslash does not start a new escape.
        assert_eq!(enc(r"\\n"), Ok(br"\n".to_vec()));
        assert_eq!(enc(r"\\\\"), Ok(br"\\".to_vec()));
        assert_eq!(enc(r"\\\t"), Ok(b"\\\t".to_vec()));
        // Escapes alone, at the start and at the end.
        assert_eq!(enc(r"\t"), Ok(b"\t".to_vec()));
        assert_eq!(enc(r"\tx\t"), Ok(b"\tx\t".to_vec()));
        // Escape letters are case-sensitive.
        for seq in [r"\N", r"\T", r"\R", r"\X"] {
            assert_eq!(encode_text(&format!("{seq}41"), false), bad(seq), "{seq}");
        }
        // Escapes known from other languages are not accepted.
        for seq in [
            r"\a", r"\b", r"\e", r"\f", r"\v", r"\'", r"\ ", r"\u", r"\1",
        ] {
            assert_eq!(enc(seq), bad(seq), "{seq}");
        }
        // A backslash before a non-ASCII character is an invalid escape.
        assert_eq!(enc(r"\ä"), bad(r"\ä"));
        // The first problem is reported.
        assert_eq!(enc(r"\q\x80"), bad(r"\q"));
        assert_eq!(
            enc(r"\x80\q"),
            Err(Error::NotAsciiEscape {
                pos: 1,
                seq: r"\x80".into()
            })
        );
        assert_eq!(enc(r"ä\q"), Err(Error::NotAsciiChar { pos: 1, ch: 'ä' }));
    }

    #[test]
    fn reverse_error_positions() {
        let enc = |t| encode_text(t, false);
        // A non-ASCII character is reported by its position in the input,
        // counting escape sequences character by character.
        assert_eq!(
            enc(r"\t\x41ä"),
            Err(Error::NotAsciiChar { pos: 7, ch: 'ä' })
        );
        // A multi-byte character counts as one.
        assert_eq!(enc("ab€"), Err(Error::NotAsciiChar { pos: 3, ch: '€' }));
        assert_eq!(enc("€ä"), Err(Error::NotAsciiChar { pos: 1, ch: '€' }));
        assert_eq!(enc("日本"), Err(Error::NotAsciiChar { pos: 1, ch: '日' }));
        // A non-ASCII \xNN escape is reported by the input position where it
        // starts, the same way, not by its position in the resulting bytes.
        assert_eq!(
            enc(r"\t\x41\x80"),
            Err(Error::NotAsciiEscape {
                pos: 7,
                seq: r"\x80".into()
            })
        );
        assert_eq!(
            enc(r"abc\t\x41\x80"),
            Err(Error::NotAsciiEscape {
                pos: 10,
                seq: r"\x80".into()
            })
        );
        // With --raw the backslash is an ordinary character.
        assert_eq!(
            encode_text(r"\tä", true),
            Err(Error::NotAsciiChar { pos: 3, ch: 'ä' })
        );
    }

    #[test]
    fn reverse_literal_text() {
        for raw in [false, true] {
            // Control characters and whitespace typed literally are kept.
            for text in ["a\tb\nc\r\0\x7f", "   ", " a ", "\n"] {
                assert_eq!(encode_text(text, raw), Ok(text.as_bytes().to_vec()));
            }
            // Text that looks like another input format is still text.
            for text in [
                "Typ=1 Len=3: 97,98,99",
                "NULL",
                "0x41",
                "41",
                "col 0: [ 1] 41",
                "col 1: *NULL*",
            ] {
                assert_eq!(encode_text(text, raw), Ok(text.as_bytes().to_vec()));
            }
        }
        // Every ASCII character other than the backslash maps to itself.
        let all: String = (0u8..0x80)
            .filter(|&b| b != b'\\')
            .map(char::from)
            .collect();
        assert_eq!(encode_text(&all, false), Ok(all.as_bytes().to_vec()));
        // With --raw, every ASCII character does.
        let all: String = (0u8..0x80).map(char::from).collect();
        assert_eq!(encode_text(&all, true), Ok(all.as_bytes().to_vec()));
    }

    #[test]
    fn reverse_output_variants() {
        // A single byte.
        assert_eq!(rev("A", Format::Hex, false), "41\n");
        assert_eq!(rev("A", Format::Dec, false), "65\n");
        // Hex is upper case and zero-padded; decimal has no padding.
        assert_eq!(rev(r"\x00\x0a\x7f", Format::Hex, false), "00 0A 7F\n");
        assert_eq!(rev(r"\x00\x0a\x7f", Format::Dec, false), "0,10,127\n");
        // In verbose output, the value is quoted and escaped.
        assert_eq!(
            rev(r#"a"b\t\\"#, Format::Auto, true),
            "value:    \"a\\\"b\\t\\\\\"\n\
             length:   5\n\
             dump:     Typ=1 Len=5: 97,34,98,9,92\n\
             bytes:    61 22 62 09 5C\n"
        );
        // --raw only changes how the input is read, not the output.
        let mut out = Vec::new();
        let args = Args {
            input: String::new(),
            reverse: true,
            format: Format::Auto,
            raw: true,
            verbose: true,
        };
        write_reverse(&mut out, br"a\t", &args).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "value:    \"a\\\\t\"\n\
             length:   3\n\
             dump:     Typ=1 Len=3: 97,92,116\n\
             bytes:    61 5C 74\n"
        );
        // A VARCHAR2 at its standard maximum of 4000 bytes.
        let text = "x".repeat(4000);
        let hex = rev(&text, Format::Hex, false);
        assert_eq!(hex, format!("{}78\n", "78 ".repeat(3999)));
        let dec = rev(&text, Format::Dec, false);
        assert_eq!(dec, format!("{}120\n", "120,".repeat(3999)));
        assert!(
            rev(&text, Format::Auto, true)
                .contains("\nlength:   4000\ndump:     Typ=1 Len=4000: 120,")
        );
    }

    #[test]
    fn reverse_write_errors() {
        for format in [Format::Auto, Format::Dec, Format::Hex] {
            for verbose in [false, true] {
                let args = Args {
                    input: String::new(),
                    reverse: true,
                    format,
                    raw: false,
                    verbose,
                };
                let mut out = FailingWriter(io::ErrorKind::StorageFull);
                let result = write_reverse(&mut out, b"AB", &args);
                assert_eq!(
                    result.map_err(|e| e.kind()),
                    Err(io::ErrorKind::StorageFull),
                    "format={format} verbose={verbose}"
                );
            }
        }
    }

    #[test]
    fn reverse_inverts_forward() {
        // The plain and the verbose (quoted) output of the forward conversion
        // both convert back to the original bytes.
        for hex in [
            "48656C6C6F",
            "00410A5C7F",
            "090D22271B",
            "202020",
            "5C5C78343122",
        ] {
            let bytes = parse_input(hex, Format::Hex).unwrap().bytes;
            assert_eq!(
                encode_text(&escape(&bytes, false), false),
                Ok(bytes.clone())
            );
            let out = verbose(hex, Format::Hex, false);
            let value = out
                .lines()
                .last()
                .unwrap()
                .strip_prefix("value:    ")
                .unwrap();
            let unquoted = &value[1..value.len() - 1];
            assert_eq!(encode_text(unquoted, false), Ok(bytes), "{hex}");
        }
    }

    #[test]
    fn error_messages() {
        for (error, text) in [
            (Error::Empty, "no bytes given"),
            (
                Error::NullValue,
                "the value is NULL, there are no characters to convert",
            ),
            (Error::BadToken("zz".into()), "cannot parse 'zz' as a byte"),
            (
                Error::NotAscii { pos: 3, byte: 0xC3 },
                "byte 3 (0xC3) is not ASCII, only ASCII data is supported",
            ),
            (
                Error::DuplicateField("Len".into()),
                "DUMP header has the field 'Len' more than once",
            ),
            (
                Error::UnsupportedType(2),
                "Typ=2 is NUMBER, not a character type, use ora-num",
            ),
            (
                Error::UnsupportedType(12),
                "Typ=12 is DATE, not a character type",
            ),
            (
                Error::UnsupportedType(999),
                "Typ=999 is not a character type, expected Typ=1 (VARCHAR2) or Typ=96 (CHAR)",
            ),
            (
                Error::NationalCharset(None),
                "the data is in AL16UTF16 (UTF-16), the NCHAR/NVARCHAR2 national character set; \
                 NCHAR and NVARCHAR2 are not supported",
            ),
            (
                Error::NationalCharset(Some("UTF16".into())),
                "the data is in UTF16, the NCHAR/NVARCHAR2 national character set; \
                 NCHAR and NVARCHAR2 are not supported",
            ),
            (
                Error::LengthMismatch {
                    declared: 3,
                    actual: 2,
                },
                "input declares 3 bytes, but 2 are given",
            ),
            (
                Error::NullColumn(1),
                "column 1 is NULL in the block dump, there are no characters to convert",
            ),
            (
                Error::DecBlockDump,
                "block dump bytes are always hex, -f dec cannot be used with them",
            ),
            (
                Error::NotAsciiChar { pos: 2, ch: 'ä' },
                "character 2 ('ä') is not ASCII, only ASCII data is supported",
            ),
            (
                Error::NotAsciiEscape {
                    pos: 7,
                    seq: r"\x80".into(),
                },
                r"escape sequence '\x80' at character 7 is not ASCII, only ASCII data is supported",
            ),
            (
                Error::BadEscape(r"\q".into()),
                r"invalid escape sequence '\q', use \\ for a backslash or --raw to take the text literally",
            ),
            (
                Error::EmptyText,
                "the text is empty, Oracle stores an empty string as NULL",
            ),
            (
                Error::Output("disk full".into()),
                "cannot write output: disk full",
            ),
        ] {
            assert_eq!(error.to_string(), text);
        }
        assert!(
            Error::BadHeader("Typ=x".into())
                .to_string()
                .contains("'Typ=x'")
        );
        assert!(
            Error::BadPrefix("col x".into())
                .to_string()
                .contains("'col x'")
        );
    }
}
