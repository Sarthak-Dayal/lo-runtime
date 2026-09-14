#![allow(dead_code)]

mod add_io_classes;
mod ast;
mod lexer;
mod parser;
mod token;
mod type_checker;
mod wasm;
mod wasm_toolchain;

fn main() -> std::process::ExitCode {
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
