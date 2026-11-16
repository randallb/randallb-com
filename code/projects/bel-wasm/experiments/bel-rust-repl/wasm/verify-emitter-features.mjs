import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const module = new WebAssembly.Module(readFileSync(process.argv[2]));
assert.deepEqual(WebAssembly.Module.imports(module), []);
const { memory, join, intern, choose } =
  new WebAssembly.Instance(module).exports;
const car = (p) => new DataView(memory.buffer).getInt32(p, true);
const cdr = (p) => new DataView(memory.buffer).getInt32(p + 4, true);
function text(value) {
  let result = "";
  while (value) {
    result += String.fromCodePoint(car(value) >>> 3);
    value = cdr(value);
  }
  return result;
}
function chars(value) {
  return Array.from(value).reduceRight(
    (d, c) => join((c.codePointAt(0) << 3) | 2, d),
    0,
  );
}
for (
  const name of [
    "nil",
    "t",
    "pair",
    "symbol",
    "char",
    "hello",
    "unknown",
    "",
    "\0🙂\u{10ffff}",
  ]
) {
  const symbol = intern(chars(name));
  assert.equal(
    choose(9, symbol),
    symbol,
    "protected global names remain legal parameters",
  );
  assert.equal(
    intern(chars(name)),
    symbol,
    "equal names must intern identically",
  );
  const a = choose(1, symbol);
  const b = choose(1, symbol);
  assert.equal(text(car(a)), name);
  assert.equal(text(car(b)), name);
  if (name.length) {
    assert.notEqual(car(a), car(b), "nom must return fresh pairs");
  }
  assert.equal(
    car(cdr(a)),
    car(cdr(b)),
    "one literal occurrence must retain identity",
  );
  assert.equal(text(car(cdr(a))), "literal");
  assert.equal(text(cdr(cdr(a))), "quoted");
  const identity = choose(0, symbol);
  assert.equal(car(identity), 1, "successive calls must reuse a literal");
  assert.equal(
    car(cdr(identity)),
    0,
    "distinct source literals must remain distinct",
  );
  assert.equal(
    car(cdr(cdr(identity))),
    1,
    "ASCII character literal must match string car",
  );
  assert.equal(
    cdr(cdr(cdr(identity))),
    name === "unknown" ? 1 : 0,
    "a quoted symbol must match its runtime-interned name",
  );
}
assert.deepEqual(
  ["nil", "t", "pair", "symbol", "char"].map((s) => intern(chars(s))),
  [0, 1, 9, 17, 25],
);
for (
  const invalid of [
    1,
    join(1, 0),
    join((0xd800 << 3) | 2, 0),
    join((0x110000 << 3) | 2, 0),
  ]
) {
  assert.throws(() => intern(invalid), WebAssembly.RuntimeError);
}
const cycle = join((97 << 3) | 2, 0);
new DataView(memory.buffer).setInt32(cycle + 4, cycle, true);
assert.throws(
  () => intern(cycle),
  WebAssembly.RuntimeError,
  "cyclic names must stop at the bound",
);
assert.equal(
  text(car(choose(1, intern(chars("after-failure"))))),
  "after-failure",
);
assert.throws(
  () => choose(1, (97 << 3) | 2),
  WebAssembly.RuntimeError,
  "nom of a character must fail",
);
console.log(
  "PASS: multiple/zero arguments, text/quoted literals, identity, Unicode nom/intern, bounds and recovery",
);
