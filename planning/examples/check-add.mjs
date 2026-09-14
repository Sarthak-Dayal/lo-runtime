import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

// Accept a modified copy of add.hex for the guide's experiments.
const path = process.argv[2] ?? new URL("./add.hex", import.meta.url);
const text = await readFile(path, "utf8");
assert.match(text, /^[\da-f\s]+$/i, "Expected whitespace-separated hex bytes");
const hex = text.replace(/\s/g, "");
assert.equal(hex.length % 2, 0, "A byte needs two hex digits");
const bytes = Buffer.from(hex, "hex");

assert.ok(WebAssembly.validate(bytes), "WASM validation failed");
const { module, instance } = await WebAssembly.instantiate(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), []);
assert.deepEqual(WebAssembly.Module.exports(module), [{ name: "add", kind: "function" }]);
console.log(`Validated ${bytes.length} bytes; exports add; no imports.`);

for (const [left, right, expected] of [
  [20, 22, 42],
  [-7, 2, -5],
  [2147483647, 1, -2147483648],
]) {
  const actual = instance.exports.add(left, right);
  assert.equal(actual, expected, `add(${left}, ${right})`);
  console.log(`add(${left}, ${right}) = ${actual}`);
}
