// Integration test: run the lexer over the real LO conformance-suite corpus
// checked into the sibling `lo-testing` repo, not just the hand-picked
// examples in lexer_implementation.md's acceptance table.

use lo_compiler::lexer::{tokenize, LexErrorKind};
use std::fs;
use std::path::{Path, PathBuf};

fn corpus_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <...>/lo-runtime/compiler; lo-testing is a sibling
    // checkout of lo-runtime, not of this crate.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lo-testing")
}

fn collect_lo_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lo_files(&path, out);
        } else if path.extension().map(|e| e == "lo").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// ValidPrograms and RuntimeAbortPrograms are lexically and syntactically
/// well-formed by construction (they fail, if at all, at runtime). The lexer
/// must accept every one of them without error.
#[test]
fn valid_and_runtime_abort_programs_lex_cleanly() {
    let root = corpus_root();
    if !root.exists() {
        eprintln!(
            "skipping: {} not found (lo-testing checkout not present)",
            root.display()
        );
        return;
    }

    let mut files = Vec::new();
    for suite in ["LO-2", "LO-3", "LO-4"] {
        for subdir in ["ValidPrograms", "RuntimeAbortPrograms"] {
            collect_lo_files(&root.join(suite).join(subdir), &mut files);
        }
    }
    assert!(
        !files.is_empty(),
        "expected to find .lo files under {}",
        root.display()
    );

    let mut failures = Vec::new();
    for path in &files {
        let source = fs::read_to_string(path).expect("read .lo file");
        if let Err(e) = tokenize(&source) {
            failures.push(format!("{}: {:?}", path.display(), e));
        }
    }

    assert!(
        failures.is_empty(),
        "{} file(s) failed to lex:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// InvalidPrograms are a mix of parse-phase and semantic-phase failures;
/// most should still lex fine (the grammar violation happens above the
/// lexer). We only assert that lexing never panics and, for the one program
/// in the corpus that is documented as a lex-phase failure, that the lexer
/// reports the specific error the test file's header comment names.
#[test]
fn invalid_programs_never_panic_the_lexer() {
    let root = corpus_root();
    if !root.exists() {
        eprintln!(
            "skipping: {} not found (lo-testing checkout not present)",
            root.display()
        );
        return;
    }

    let mut files = Vec::new();
    for suite in ["LO-2", "LO-3", "LO-4"] {
        collect_lo_files(&root.join(suite).join("InvalidPrograms"), &mut files);
    }
    assert!(
        !files.is_empty(),
        "expected to find InvalidPrograms .lo files"
    );

    for path in &files {
        let source = fs::read_to_string(path).expect("read .lo file");
        let _ = tokenize(&source); // must not panic, Ok or Err both fine
    }
}

#[test]
fn documented_lex_phase_failure_reports_invalid_unicode_escape() {
    let root = corpus_root();
    let path = root
        .join("LO-3")
        .join("InvalidPrograms")
        .join("test_19_invalid_unicode_escape.lo");
    if !path.exists() {
        eprintln!(
            "skipping: {} not found (lo-testing checkout not present)",
            path.display()
        );
        return;
    }

    let source = fs::read_to_string(&path).expect("read test_19_invalid_unicode_escape.lo");
    let err = tokenize(&source).expect_err("expected a lex error for a surrogate \\u escape");
    assert_eq!(err.kind, LexErrorKind::InvalidUnicodeEscape);
}

/// The other documented InvalidPrograms failures in this range are all
/// E_PARSE_PHASE_OTHER (grammar violations above the lexer, e.g. calling a
/// constructor without `new`, defining a method outside a class). They
/// should lex without error since nothing about their *tokens* is invalid.
#[test]
fn parse_phase_invalid_programs_still_lex_cleanly() {
    let root = corpus_root();
    let names = [
        "test_0.lo",
        "test_2.lo",
        "test_3.lo",
        "test_4.lo",
        "test_6.lo",
    ];
    let dir = root.join("LO-3").join("InvalidPrograms");
    if !dir.exists() {
        eprintln!(
            "skipping: {} not found (lo-testing checkout not present)",
            dir.display()
        );
        return;
    }

    for name in names {
        let path = dir.join(name);
        let source = fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {name}"));
        tokenize(&source).unwrap_or_else(|e| panic!("{name} should lex cleanly, got {e:?}"));
    }
}
