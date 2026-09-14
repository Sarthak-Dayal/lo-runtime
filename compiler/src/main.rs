#![allow(dead_code)]

mod add_io_classes;
mod ast;
mod interpreter;
mod lexer;
mod parser;
mod token;
mod type_checker;
mod wasm;
mod wasm_toolchain;

fn main() -> std::process::ExitCode {
    // `--run FILE.lo` interprets the program. It is handled here, ahead of `run()`,
    // so the interpreter can own the process exit status (main's return value, or an
    // abort code) — which `run()`'s String result cannot express — while `run()` and
    // the P1 grading contract (`--check`/`--compile`/`--emit-wasm`) stay untouched.
    let args: Vec<_> = std::env::args_os().collect();
    if let [_, mode, path] = args.as_slice() {
        if mode == "--run" {
            return run_interpreter(path);
        }
    }
    match run() {
        Ok(assembly) => {
            print!("{assembly}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run_interpreter(path: &std::ffi::OsStr) -> std::process::ExitCode {
    let front_end = || {
        let source = std::fs::read_to_string(path).map_err(|e| format!("read source: {e}"))?;
        let tokens = lexer::tokenize(&source)
            .map_err(|e| format!("lex line {} [{}]: {:?}", e.line, e.kind.as_str(), e.kind))?;
        let program = parser::parse_program(&tokens)
            .map_err(|e| format!("parse line {} [{}]: {}", e.line, e.code.as_str(), e.message))?;
        type_checker::check_program(program).map_err(|e| {
            format!(
                "type check line {} [{}]: {}",
                e.line,
                e.code.as_str(),
                e.message
            )
        })
    };
    let (program, classes) = match front_end() {
        Ok(checked) => checked,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let io = interpreter::Io::new(
        Box::new(std::io::BufReader::new(std::io::stdin())),
        Box::new(std::io::stdout()),
        Box::new(std::io::stderr()),
    );
    match interpreter::interpret(&program, &classes, interpreter::DEFAULT_HEAP_LIMIT, io) {
        // A native exit status is one byte; the OS truncates like the wasm runtime.
        interpreter::Outcome::Exit(code) => std::process::ExitCode::from(code as u8),
        interpreter::Outcome::Abort(abort) => {
            eprintln!("{}", abort.message());
            std::process::ExitCode::from(abort.code() as u8)
        }
    }
}

fn run() -> Result<String, String> {
    let args: Vec<_> = std::env::args_os().collect();
    let usage =
        "usage: lo-compiler --check FILE.lo | --emit-wasm FILE.lo | --compile FILE.lo OUTPUT.wasm";
    let (mode, path, output) = match args.as_slice() {
        [_, mode, path] if mode == "--check" || mode == "--emit-wasm" => (mode, path, None),
        [_, mode, path, output] if mode == "--compile" => (mode, path, Some(output)),
        _ => return Err(usage.into()),
    };
    let source = std::fs::read_to_string(path).map_err(|e| format!("read source: {e}"))?;
    let tokens = lexer::tokenize(&source)
        .map_err(|e| format!("lex line {} [{}]: {:?}", e.line, e.kind.as_str(), e.kind))?;
    let program = parser::parse_program(&tokens)
        .map_err(|e| format!("parse line {} [{}]: {}", e.line, e.code.as_str(), e.message))?;
    let (program, classes) = type_checker::check_program(program).map_err(|e| {
        format!(
            "type check line {} [{}]: {}",
            e.line,
            e.code.as_str(),
            e.message
        )
    })?;
    if mode == "--check" {
        return Ok(String::new());
    }
    let assembly = wasm::p1_program(&program, &classes);
    if let Some(output) = output {
        wasm_toolchain::compile(&assembly, std::path::Path::new(output))?;
        Ok(String::new())
    } else {
        Ok(assembly)
    }
}
