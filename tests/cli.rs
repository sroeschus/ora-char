// Copyright (c) 2026 Stefan Roesch

//! End-to-end tests: run the `ora-char` binary and check its exit status,
//! standard output and standard error.

use std::io::Read;
use std::process::{Command, Output, Stdio};

fn ora_char(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ora-char"))
        .args(args)
        .output()
        .expect("failed to run ora-char")
}

/// Run ora-char and return (exit code, stdout, stderr).
fn run(args: &[&str]) -> (i32, String, String) {
    let out = ora_char(args);
    (
        out.status.code().expect("terminated by a signal"),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn ok(args: &[&str], stdout: &str) {
    assert_eq!(
        run(args),
        (0, stdout.to_string(), String::new()),
        "{args:?}"
    );
}

fn fails(args: &[&str], stderr: &str) {
    assert_eq!(
        run(args),
        (1, String::new(), format!("error: {stderr}\n")),
        "{args:?}"
    );
}

#[test]
fn readme_accepted_input() {
    for input in [
        "Typ=1 Len=5: 72,101,108,108,111",
        "Typ=1 Len=5: 48,65,6c,6c,6f",
        "Typ=1 Len=5 CharacterSet=AL32UTF8: 72,101,108,108,111",
        "72,101,108,108,111",
        "72 101 108 108 111",
        "48656C6C6F",
        "0x48656C6C6F",
        "48 65 6c 6c 6f",
        "col 0: [ 5] 48 65 6c 6c 6f",
    ] {
        ok(&[input], "Hello\n");
    }
}

#[test]
fn readme_examples() {
    ok(
        &["Typ=1 Len=11: 79,114,97,99,108,101,32,50,51,97,105"],
        "Oracle 23ai\n",
    );
    ok(&["Typ=96 Len=5: 4f,4b,20,20,20"], "OK   \n");
    ok(&["4F7261636C65"], "Oracle\n");
    ok(&["4f 72 61 63 6c 65"], "Oracle\n");
    ok(&["col  1: [ 6]  4f 72 61 63 6c 65"], "Oracle\n");
    ok(&["-f", "hex", "65,66,67"], "efg\n");
    ok(&["-f", "hex", "Typ=1 Len=3: 61,62,63"], "abc\n");
    ok(&["610962"], "a\\tb\n");
    ok(
        &["-v", "Typ=96 Len=5: 104,105,32,32,32"],
        "type:     CHAR (Typ=96)\n\
         format:   dec\n\
         length:   5\n\
         bytes:    68 69 20 20 20\n\
         value:    \"hi   \"\n",
    );
    ok(
        &["-v", "Typ=1 Len=5 CharacterSet=AL32UTF8: 48,65,6c,6c,6f"],
        "type:     VARCHAR2 (Typ=1)\n\
         charset:  AL32UTF8\n\
         format:   hex\n\
         length:   5\n\
         bytes:    48 65 6C 6C 6F\n\
         value:    \"Hello\"\n",
    );
}

#[test]
fn readme_errors() {
    fails(
        &["Typ=1 Len=4: 79,114,97"],
        "input declares 4 bytes, but 3 are given",
    );
    fails(
        &["Typ=1 Len=1 Len=2: 104,105"],
        "DUMP header has the field 'Len' more than once",
    );
    fails(
        &["Typ=2 Len=3: 194,2,24"],
        "Typ=2 is NUMBER, not a character type, use ora-num",
    );
    fails(
        &["NULL"],
        "the value is NULL, there are no characters to convert",
    );
    fails(
        &["col 1: *NULL*"],
        "column 1 is NULL in the block dump, there are no characters to convert",
    );
    fails(
        &["-f", "dec", "col 0: [ 2] 41 42"],
        "block dump bytes are always hex, -f dec cannot be used with them",
    );
    fails(
        &["C3A4"],
        "byte 1 (0xC3) is not ASCII, only ASCII data is supported",
    );
    fails(
        &["Typ=1 Len=4: 0,72,0,105"],
        "the data is in AL16UTF16 (UTF-16), the NCHAR/NVARCHAR2 national character set; \
         NCHAR and NVARCHAR2 are not supported",
    );
}

#[test]
fn ambiguity_note() {
    let (code, stdout, stderr) = run(&["65,66,67"]);
    assert_eq!((code, stdout.as_str()), (0, "ABC\n"));
    assert_eq!(
        stderr,
        "note: input is ambiguous, read as dec giving \"ABC\"; as hex it is \"efg\", \
         use -f to choose\n"
    );
    // The note goes to stderr only, also in verbose mode.
    let (code, stdout, stderr) = run(&["-v", "41 42"]);
    assert_eq!(code, 0);
    assert!(stdout.ends_with("value:    \"AB\"\n"), "{stdout}");
    assert!(!stdout.contains("note"));
    assert!(stderr.starts_with("note: input is ambiguous, read as hex giving \"AB\""));
    // A forced format or identical readings give no note.
    ok(&["-f", "dec", "65,66,67"], "ABC\n");
    ok(&["Typ=1 Len=1: 9"], "\\t\n");
}

#[test]
fn raw_output() {
    let out = ora_char(&["--raw", "00410a5c7f"]);
    assert!(out.status.success());
    assert_eq!(out.stdout, b"\0A\n\\\x7f\n");
    assert!(out.stderr.is_empty());
    // Escaped by default.
    ok(&["00410a5c7f"], "\\0A\\n\\\\\\x7F\n");
}

#[test]
fn options_combine() {
    ok(
        &["-v", "-f", "hex", "--raw", "41"],
        "format:   hex\nlength:   1\nbytes:    41\nvalue:    A\n",
    );
    ok(&["--format=dec", "65"], "A\n");
    ok(&["-fhex", "65"], "e\n");
    ok(
        &["-vfhex", "41"],
        "format:   hex\nlength:   1\nbytes:    41\nvalue:    \"A\"\n",
    );
    // Options may follow the input.
    ok(&["65,66", "-f", "hex"], "ef\n");
}

#[test]
fn hyphen_input_is_an_option() {
    // No valid input starts with '-', so it is parsed as an (unknown) option.
    let (code, _, stderr) = run(&["-1"]);
    assert_eq!(code, 2);
    assert!(
        stderr.starts_with("error: unexpected argument '-1' found"),
        "{stderr}"
    );
    // "--" still passes it through as input.
    fails(&["--", "-1"], "cannot parse '-1' as a byte");
}

#[test]
fn usage_errors() {
    // Missing input, unknown option and bad format value are clap errors (exit 2).
    for args in [
        &[][..],
        &["--bogus", "41"],
        &["-f", "oct", "41"],
        &["41", "42"],
    ] {
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 2, "{args:?}");
        assert!(stdout.is_empty(), "{args:?}");
        assert!(stderr.starts_with("error:"), "{args:?}: {stderr}");
    }
    let (_, _, stderr) = run(&["-f", "oct", "41"]);
    assert!(
        stderr.contains("[possible values: auto, dec, hex]"),
        "{stderr}"
    );
}

#[test]
fn help_and_version() {
    let (code, stdout, _) = run(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Usage: ora-char [OPTIONS] <INPUT>"));
    assert!(stdout.contains("--raw"));
    assert!(stdout.contains("-r, --reverse"));
    assert!(stdout.contains("Copyright (c) 2026 Stefan Roesch"));
    ok(
        &["--version"],
        &format!("ora-char {}\n", env!("CARGO_PKG_VERSION")),
    );
}

#[test]
fn reverse() {
    ok(&["-r", "Hello"], "48 65 6C 6C 6F\n");
    ok(&["--reverse", "Hello"], "48 65 6C 6C 6F\n");
    ok(&["-r", "-f", "hex", "Hello"], "48 65 6C 6C 6F\n");
    ok(&["-r", "-f", "dec", "Hello"], "72,101,108,108,111\n");
    ok(&["-rfdec", "AB"], "65,66\n");
    ok(&["Hello", "-r"], "48 65 6C 6C 6F\n");
    ok(
        &["-r", "-v", "hi   "],
        "value:    \"hi   \"\nlength:   5\ndump:     Typ=1 Len=5: 104,105,32,32,32\nbytes:    68 69 20 20 20\n",
    );
    // Escapes are decoded, so output of the forward direction converts back.
    ok(&["-r", r"a\tb"], "61 09 62\n");
    ok(&["-r", "--raw", r"a\tb"], "61 5C 74 62\n");
    // Text starting with '-' needs "--".
    ok(&["-r", "--", "-1"], "2D 31\n");
}

#[test]
fn reverse_round_trip() {
    for text in ["Oracle 23ai", "OK   ", r"a\tb\\c\x01", "~!@#$%^&*()"] {
        let (code, hex, _) = run(&["-r", text]);
        assert_eq!(code, 0);
        ok(&[hex.trim_end()], &format!("{text}\n"));
        let (code, dec, _) = run(&["-r", "-f", "dec", text]);
        assert_eq!(code, 0);
        ok(&["-f", "dec", dec.trim_end()], &format!("{text}\n"));
    }
}

#[test]
fn reverse_errors() {
    fails(
        &["-r", "Grüße"],
        "character 3 ('ü') is not ASCII, only ASCII data is supported",
    );
    fails(
        &["-r", ""],
        "the text is empty, Oracle stores an empty string as NULL",
    );
    fails(
        &["-r", r"C:\Users"],
        r"invalid escape sequence '\U', use \\ for a backslash or --raw to take the text literally",
    );
    fails(
        &["-r", r"\x80"],
        r"escape sequence '\x80' at character 1 is not ASCII, only ASCII data is supported",
    );
}

#[test]
fn reverse_options() {
    ok(&["-r", "-f", "auto", "AB"], "41 42\n");
    ok(&["-r", "--format=dec", "A"], "65\n");
    let verbose = "value:    \"A\"\nlength:   1\ndump:     Typ=1 Len=1: 65\nbytes:    41\n";
    ok(&["-rv", "A"], verbose);
    ok(&["--verbose", "--reverse", "A"], verbose);
    // Verbose output is the same for every format.
    ok(&["-r", "-v", "-f", "dec", "A"], verbose);
    ok(&["-r", "-v", "-f", "hex", "A"], verbose);
    // --raw reads the text literally, the output is still escaped.
    ok(
        &["-r", "--raw", "-v", r"a\t"],
        "value:    \"a\\\\t\"\nlength:   3\ndump:     Typ=1 Len=3: 97,92,116\nbytes:    61 5C 74\n",
    );
    ok(&["--raw", "-r", r"\"], "5C\n");
    ok(&["-r", "--raw", "-f", "dec", r"\\"], "92,92\n");
}

#[test]
fn reverse_usage_errors() {
    // Missing text, extra text, a bad format and text that looks like an option.
    for args in [
        &["-r"][..],
        &["-r", "a", "b"],
        &["-r", "-f", "oct", "a"],
        &["-r", "-x"],
        &["-r", "--raw-text", "a"],
    ] {
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 2, "{args:?}");
        assert!(stdout.is_empty(), "{args:?}");
        assert!(stderr.starts_with("error:"), "{args:?}: {stderr}");
    }
    // After "--", anything is text.
    ok(&["-r", "--", "--raw"], "2D 2D 72 61 77\n");
    ok(&["-r", "--", "-r"], "2D 72\n");
}

#[test]
fn reverse_literal_text() {
    ok(&["-r", "a b"], "61 20 62\n");
    ok(&["-r", "   "], "20 20 20\n");
    ok(&["-r", " "], "20\n");
    ok(&["-r", "a\tb"], "61 09 62\n");
    ok(
        &["-r", "line1\nline2"],
        "6C 69 6E 65 31 0A 6C 69 6E 65 32\n",
    );
    ok(&["-r", "\x7f"], "7F\n");
    // Input that looks like another format is converted as text.
    ok(&["-r", "NULL"], "4E 55 4C 4C\n");
    ok(&["-r", "0x41"], "30 78 34 31\n");
    ok(
        &["-r", "-f", "dec", "Typ=1 Len=1: 65"],
        "84,121,112,61,49,32,76,101,110,61,49,58,32,54,53\n",
    );
}

#[test]
fn reverse_inverts_forward() {
    for hex in [
        "48 65 6C 6C 6F",
        "00 41 0A 5C 7F",
        "09 0D 22 27 1B",
        "20 20 20",
    ] {
        // The plain output of the forward conversion converts back.
        let (code, text, stderr) = run(&["-f", "hex", hex]);
        assert_eq!((code, stderr.as_str()), (0, ""), "{hex}");
        ok(
            &["-r", text.strip_suffix('\n').unwrap()],
            &format!("{hex}\n"),
        );
        // So does the quoted value of the verbose output.
        let (code, out, _) = run(&["-v", "-f", "hex", hex]);
        assert_eq!(code, 0);
        let value = out
            .lines()
            .last()
            .unwrap()
            .strip_prefix("value:    ")
            .unwrap();
        ok(&["-r", &value[1..value.len() - 1]], &format!("{hex}\n"));
    }
}

#[test]
fn reverse_more_errors() {
    // Errors exit with 1 and print nothing on stdout, also with -v.
    fails(
        &["-r", "--raw", "Grüße"],
        "character 3 ('ü') is not ASCII, only ASCII data is supported",
    );
    fails(
        &["-r", "-v", "naïve"],
        "character 3 ('ï') is not ASCII, only ASCII data is supported",
    );
    fails(
        &["-r", "日本"],
        "character 1 ('日') is not ASCII, only ASCII data is supported",
    );
    fails(
        &["-r", "-f", "dec", "a\u{a0}b"],
        "character 2 ('\u{a0}') is not ASCII, only ASCII data is supported",
    );
    for (text, seq) in [
        (r"a\", r"\"),
        (r"\x4", r"\x4"),
        (r"\q", r"\q"),
        (r"\X41", r"\X"),
    ] {
        fails(
            &["-r", text],
            &format!(
                r"invalid escape sequence '{seq}', use \\ for a backslash or --raw to take the text literally"
            ),
        );
    }
    fails(
        &["-r", r"AB\xFF"],
        r"escape sequence '\xFF' at character 3 is not ASCII, only ASCII data is supported",
    );
    // The position counts input characters, escapes included.
    fails(
        &["-r", r"abc\t\x41\x80"],
        r"escape sequence '\x80' at character 10 is not ASCII, only ASCII data is supported",
    );
    fails(
        &["-r", "--raw", ""],
        "the text is empty, Oracle stores an empty string as NULL",
    );
    fails(
        &["-r", "-v", ""],
        "the text is empty, Oracle stores an empty string as NULL",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn reverse_write_error_exit_status() {
    let full = std::fs::File::create("/dev/full").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ora-char"))
        .args(["-r", "AB"])
        .stdout(full)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with("error: cannot write output: "),
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn reverse_closed_pipe_is_not_an_error() {
    // 100000 characters print as 300000 bytes, more than a pipe buffer.
    let input = "A".repeat(100000);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ora-char"))
        .args(["-r", &input])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 6];
    stdout.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"41 41 ");
    drop(stdout);
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert_eq!(stderr, "");
}

#[cfg(target_os = "linux")]
#[test]
fn write_error_exit_status() {
    let full = std::fs::File::create("/dev/full").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ora-char"))
        .arg("4142")
        .stdout(full)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with("error: cannot write output: "),
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn closed_pipe_is_not_an_error() {
    // 30000 bytes of 0x01 print as 120000 characters, more than a pipe buffer.
    let input = "01".repeat(30000);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ora-char"))
        .arg(&input)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 8];
    stdout.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"\\x01\\x01");
    drop(stdout);
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(stderr, "");
}
