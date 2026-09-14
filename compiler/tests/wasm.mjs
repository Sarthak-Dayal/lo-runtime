// Exact i32 results and memory checks supplement the course host's process status.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const work = mkdtempSync(join(tmpdir(), 'lo-wasm-'));
const compiler = join(root, 'compiler/target/release/lo-compiler');
const runtime = process.env.LO_RUNTIME_ARCHIVE ?? join(root, 'rust/target/wasm32-unknown-unknown/release/liblo_runtime.a');

function run(command, args) {
  const result = spawnSync(command, args, { encoding: 'utf8', timeout: 60_000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, `${command}: ${result.stderr}`);
  return result.stdout;
}

function compile(source, observeStack = false) {
  const wasm = join(work, 'program.wasm');
  if (observeStack) {
    const assembly = join(work, 'program.s');
    const object = join(work, 'program.o');
    writeFileSync(assembly, run(compiler, ['--emit-wasm', source]));
    run(process.env.LLVM_MC ?? 'llvm-mc', ['-triple=wasm32-unknown-unknown', '-filetype=obj', assembly, '-o', object]);
    run(process.env.WASM_LD ?? 'wasm-ld', ['--no-entry', '--export=lo_entry', '--export-memory', '--export=__stack_pointer', '--allow-undefined', object, runtime, '-o', wasm]);
  } else {
    run(compiler, ['--compile', source, wasm]);
  }
  const bytes = readFileSync(wasm);
  assert.ok(WebAssembly.validate(bytes), 'linked module validates');
  return new WebAssembly.Module(bytes);
}

function hostImports(module) {
  const host = {};
  for (const imported of WebAssembly.Module.imports(module)) {
    assert.equal(imported.module, 'host');
    assert.equal(imported.kind, 'function');
    host[imported.name] = () => { throw new Error(`Unexpected I/O: ${imported.name}`); };
  }
  return host;
}

try {
  const fixture = readFileSync(new URL('fixtures/return-42.lo', import.meta.url), 'utf8');
  const source = join(work, 'program.lo');
  // 64 crosses the signed LEB128 one-byte boundary; MAX crosses several more.
  for (const [expression, expected] of [['42', 42], ['64', 64], ['2147483647', 2147483647], ['(20 + 22)', 42]]) {
    writeFileSync(source, fixture.replace('42', expression));
    const module = compile(source);
    assert.deepEqual(WebAssembly.Module.exports(module).sort((a, b) => a.name.localeCompare(b.name)), [
      { name: 'lo_entry', kind: 'function' },
      { name: 'memory', kind: 'memory' },
    ]);
    const instance = new WebAssembly.Instance(module, { host: hostImports(module) });
    assert.equal(instance.exports.lo_entry(), expected);
    console.log(`Full pipeline: ${expression} returns ${expected}`);
  }

  for (const [replacement, diagnostic] of [
    ['@', /lex line/],
    ['42 42', /parse line/],
    ['true', /type check line.*E_RETURN_TYPE_MISMATCH/],
  ]) {
    writeFileSync(source, fixture.replace('42', replacement));
    const result = spawnSync(compiler, ['--emit-wasm', source], { encoding: 'utf8', timeout: 10_000 });
    assert.ifError(result.error);
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.match(result.stderr, diagnostic);
  }
  console.log('Lex, parse, and type-check rejection checks passed.');

  const module = compile(join(root, 'compiler/tests/fixtures/LO-4/ValidPrograms/gc-roots.lo'), true);
  const host = hostImports(module);
  let emptyPrints = 0;
  let instance;
  host.host_print_bytes = (pointer, length) => {
    assert.equal(length, 0);
    // String.data starts 16 bytes after its header on wasm32.
    const memory = new DataView(instance.exports.memory.buffer);
    assert.notEqual(memory.getUint32(pointer - 16, true), 0, 'empty String has a descriptor');
    assert.equal(memory.getUint32(pointer - 4, true), 0, 'empty String has length zero');
    emptyPrints += 1;
  };
  instance = new WebAssembly.Instance(module, { host });
  const stackBefore = instance.exports.__stack_pointer.value;
  assert.equal(instance.exports.lo_entry(), 49);
  assert.equal(instance.exports.__stack_pointer.value, stackBefore, 'LEAVE restores the memory stack');
  assert.equal(emptyPrints, 1, 'String field default is an actual empty object, not null');
  console.log('Runtime ABI: String default, moving GC result, and stack restoration passed.');
} catch (error) {
  console.error(`Pipeline artifacts retained at ${work}`);
  throw error;
}
rmSync(work, { recursive: true });
