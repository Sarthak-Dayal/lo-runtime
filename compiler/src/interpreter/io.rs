use std::cell::RefCell;
use std::io::{self, BufRead, Write};
use std::rc::Rc;

use super::abort::AbortKind;

/// The interpreter's three I/O streams.
pub struct Io {
    stdin: Box<dyn BufRead>,
    stdout: Box<dyn Write>,
    stderr: Box<dyn Write>,
}

impl Io {
    pub fn new(stdin: Box<dyn BufRead>, stdout: Box<dyn Write>, stderr: Box<dyn Write>) -> Io {
        Io {
            stdin,
            stdout,
            stderr,
        }
    }

    /// Reads no input (immediate EOF) and discards output — used when a run's I/O
    /// is not under inspection.
    pub fn sink() -> Io {
        Io {
            stdin: Box::new(io::empty()),
            stdout: Box::new(io::sink()),
            stderr: Box::new(io::sink()),
        }
    }

    fn stream(&mut self, to_err: bool) -> &mut dyn Write {
        if to_err {
            self.stderr.as_mut()
        } else {
            self.stdout.as_mut()
        }
    }

    // --- writes: byte-exact, flushed so output survives a later abort ---------

    pub fn print_int(&mut self, n: i32, to_err: bool) {
        let w = self.stream(to_err);
        let _ = write!(w, "{n}");
        let _ = w.flush();
    }

    pub fn print_bool(&mut self, b: bool, to_err: bool) {
        let w = self.stream(to_err);
        let _ = w.write_all(if b { b"true" } else { b"false" });
        let _ = w.flush();
    }

    pub fn print_string(&mut self, s: &str, to_err: bool) {
        let w = self.stream(to_err);
        let _ = w.write_all(s.as_bytes());
        let _ = w.flush();
    }

    pub fn println(&mut self, to_err: bool) {
        let w = self.stream(to_err);
        let _ = w.write_all(b"\n");
        let _ = w.flush();
    }

    // --- reads (ABI §3.7) -----------------------------------------------------

    /// Next integer token; EOF before any token aborts 111, a non-integer or an
    /// out-of-range token aborts 110.
    pub fn read_int(&mut self) -> Result<i32, AbortKind> {
        let token = self.read_token();
        if token.is_empty() {
            return Err(AbortKind::ReadIntEof);
        }
        token
            .parse::<i32>()
            .map_err(|_| AbortKind::ReadIntMalformed)
    }

    /// Next whitespace-delimited token; only `true`/`false` are valid, anything
    /// else (EOF included) aborts 112.
    pub fn read_bool(&mut self) -> Result<bool, AbortKind> {
        match self.read_token().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(AbortKind::ReadBoolInvalid),
        }
    }

    /// Read to the next newline; the newline is consumed and excluded. Returns the
    /// empty string on immediate end-of-input (use `eof` to disambiguate).
    pub fn read_string(&mut self) -> String {
        let mut buf = Vec::new();
        let _ = self.stdin.read_until(b'\n', &mut buf);
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// True iff at end-of-input, consuming nothing. A robust guard for *line*
    /// reads but not for *token* reads (a trailing newline leaves it false) — the
    /// interpreter reproduces that quirk; it does not paper over it.
    pub fn eof(&mut self) -> bool {
        match self.stdin.fill_buf() {
            Ok(bytes) => bytes.is_empty(),
            Err(_) => true,
        }
    }

    /// The next whitespace-delimited token, or the empty string at end-of-input.
    fn read_token(&mut self) -> String {
        self.skip_whitespace();
        let mut token: Vec<u8> = Vec::new();
        loop {
            let (consumed, stop) = {
                let buf = match self.stdin.fill_buf() {
                    Ok(b) => b,
                    Err(_) => break,
                };
                if buf.is_empty() {
                    break;
                }
                let n = buf.iter().take_while(|b| !b.is_ascii_whitespace()).count();
                token.extend_from_slice(&buf[..n]);
                (n, n < buf.len())
            };
            self.stdin.consume(consumed);
            if stop {
                break;
            }
        }
        String::from_utf8_lossy(&token).into_owned()
    }

    fn skip_whitespace(&mut self) {
        loop {
            let n = {
                let buf = match self.stdin.fill_buf() {
                    Ok(b) => b,
                    Err(_) => return,
                };
                if buf.is_empty() {
                    return;
                }
                buf.iter().take_while(|b| b.is_ascii_whitespace()).count()
            };
            self.stdin.consume(n);
            if n == 0 {
                return; // the next byte is non-whitespace
            }
        }
    }
}

/// A shared in-memory sink for tests: hand a clone to `Io`, read `contents()`
/// after the run.
#[derive(Clone)]
pub struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl SharedBuf {
    pub fn new() -> SharedBuf {
        SharedBuf(Rc::new(RefCell::new(Vec::new())))
    }

    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.borrow()).into_owned()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
