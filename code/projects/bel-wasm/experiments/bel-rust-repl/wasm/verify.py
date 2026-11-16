#!/usr/bin/env python3
"""Exercise the complete Bel -> WAT -> Wasm -> ordinary Node runtime path."""
import json
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
from compile import HERE, ROOT

OUT = ROOT / "target/bel-wasm-acceptance"
TOOLS = ROOT / "target/bel-wasm-tools/node_modules/.bin"


def run(*args, success=True):
    result = subprocess.run(
        [str(arg) for arg in args], text=True, capture_output=True, check=False
    )
    print("$", " ".join(str(arg) for arg in args), flush=True)
    print(result.stdout, end="")
    print(result.stderr, end="", file=sys.stderr)
    assert (result.returncode == 0) == success, result
    return result


def compile_source(source, output, success=True):
    return run(sys.executable, HERE / "compile.py", source, output, success=success)


def inspect(output, expected):
    run(TOOLS / "wasm-validate", output)
    result = run("node", "--input-type=module", "-e", """
import {readFileSync} from 'node:fs';
const bytes = readFileSync(process.argv[1]);
const module = new WebAssembly.Module(bytes);
const instance = new WebAssembly.Instance(module);
console.log(JSON.stringify({valid: WebAssembly.validate(bytes),
  imports: WebAssembly.Module.imports(module),
  exports: WebAssembly.Module.exports(module),
  results: [0,1].map(x => instance.exports.choose(x))}));
""", output)
    observed = json.loads(result.stdout)
    assert observed == {
        "valid": True, "imports": [],
        "exports": [{"name": "choose", "kind": "function"}],
        "results": expected,
    }, observed


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    run("node", "--version")
    run(TOOLS / "wat2wasm", "--version")
    output = OUT / "choose.wasm"
    compile_source(HERE / "choose.bel", output)
    inspect(output, [0, 1])
    run(TOOLS / "wasm-objdump", "-x", output)

    cases = [
        ("mutated", "(fn (x) (if x 'nil 'nil))", [0, 0]),
        ("nested", "(fn (arg) (if (if arg nil t) 't arg))", [1, 1]),
        ("constant-test", "(fn (x) (if 't nil t))", [0, 0]),
    ]
    for name, text, expected in cases:
        source = OUT / f"{name}.bel"
        source.write_text(text + "\n", encoding="utf-8")
        artifact = OUT / f"{name}.wasm"
        compile_source(source, artifact)
        inspect(artifact, expected)

    rejected = [
        "(fn (x) (join x x))", "(fn (x) (if x 0 nil))",
        "(fn (x) 'other)", "(fn (x) missing)", "(fn (x y) x)",
        "(fn (o) (if o nil t))", "(fn (apply) (if apply nil t))",
        "(fn (if) if)", "(fn (t) t)", "(fn (nil) nil)", "(fn ((x)) x)",
        "(fn (x) (if x t))", "(fn (x) (if x t nil t))",
        "(fn (x) (quote t nil))", "(fn (x) x nil)",
        "(fn (x) (if x t . nil)) (fn (x) x)",
        "(fn (x) (if x t . other))", "(fn (x) x) (err 'extra)",
        ") (list \\x)",
    ]
    for index, text in enumerate(rejected):
        source = OUT / f"rejected-{index}.bel"
        source.write_text(text + "\n", encoding="utf-8")
        # Seed old products to prove rejection cannot leave a stale success.
        output.write_bytes(b"stale wasm")
        output.with_suffix(".wat").write_text("stale wat")
        result = compile_source(source, output, success=False)
        assert "unsupported-wasm-source" in result.stderr or "unexpected closing" in result.stderr
        assert not output.exists() and not output.with_suffix(".wat").exists()

    compile_source(HERE / "choose.bel", output)
    inspect(output, [0, 1])
    print(f"PASS: external branches, mutation, nested/constant tests, {len(rejected)} rejections, recovery")
    print(f"Artifacts and source variants retained in {OUT}")


if __name__ == "__main__":
    main()
