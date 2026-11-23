// Copyright (c) 2026 Stefan Roesch

//! Convert the internal byte representation of an Oracle `CHAR` or
//! `VARCHAR2` value (as shown by `DUMP()`, `DUMP(x, 16)`, `RAWTOHEX()` or a
//! block dump) into its characters.
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
/// into its characters (ASCII only).
#[derive(Parser, Debug)]
#[command(version, about, long_about = None, after_help = "Copyright (c) 2026 Stefan Roesch")]
struct Args {
    /// Oracle internal value, e.g. "Typ=1 Len=3: 97,98,99", "97,98,99", "616263", "0x616263"
    /// or "col 0: [ 3] 61 62 63"
    #[arg(allow_hyphen_values = true)]
    input: String,

    /// Input format
    #[arg(short, long, value_enum, default_value_t = Format::Auto)]
    format: Format,

    /// Print the characters unchanged instead of escaping control characters and backslashes
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
    UnsupportedType(u32),
    NationalCharset(Option<String>),
    BadPrefix(String),
    LengthMismatch {
        declared: usize,
        actual: usize,
    },
    NullColumn(u32),
    DecBlockDump,
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
        match key {
            "Typ" => header.typ = Some(value.parse().map_err(|_| bad())?),
            "Len" => header.len = Some(value.parse().map_err(|_| bad())?),
            "CharacterSet" => header.charset = Some(value.to_string()),
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
        (Ok(b), Some((o, alt))) if !plausible(&b) && plausible(&alt) => (alt, o, None),
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

fn run(args: &Args) -> Result<(), Error> {
    let parsed = parse_input(&args.input, args.format)?;
    if let Some((other, alt)) = &parsed.alternative {
        eprintln!(
            "note: input is ambiguous, read as {} giving {}; as {other} it is {}, use -f to choose",
            parsed.format,
            escape(&parsed.bytes, true),
            escape(alt, true)
        );
    }
    let mut out = io::stdout().lock();
    match write_output(&mut out, &parsed, args).and_then(|()| out.flush()) {
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
}
