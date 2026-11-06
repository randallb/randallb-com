#!/usr/bin/env python3
"""Build a Bel compiler component to Wasm, then compile runtime ASTs with it."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
from compile import HERE, ROOT

OUT = ROOT / "target/bel-emitter-acceptance"
EMITTER = ROOT / "target/bel-emitter/emitter.wasm"
FEATURES = ROOT / "target/bel-emitter/features.wasm"
TOOLS = ROOT / "target/bel-wasm-tools/node_modules/.bin"


def run(*args, success=True, cwd=None):
    print("$", " ".join(map(str, args)), flush=True)
    result = subprocess.run(list(map(str, args)), cwd=cwd, capture_output=True, text=True)
    print(result.stdout, end="", flush=True)
    print(result.stderr, end="", file=sys.stderr, flush=True)
    assert (result.returncode == 0) == success, result
    return result


def inspect(binary, expected, cwd=None):
    result = run("node", "--input-type=module", "-e", """
import {readFileSync} from 'node:fs';
const m = new WebAssembly.Module(readFileSync(process.argv[1]));
const {choose} = new WebAssembly.Instance(m).exports;
console.log(JSON.stringify({imports:WebAssembly.Module.imports(m),results:[choose(0),choose(1)]}));
""", binary, cwd=cwd)
    assert json.loads(result.stdout) == {"imports": [], "results": expected}


def external(source, name, expected):
    source_path = OUT / f"{name}.ast.json"
    source_path.write_text(json.dumps(source))
    # This directory has only the module, generic data adapter and input data.
    # Emission invokes neither Rust nor any project library or host callback.
    with tempfile.TemporaryDirectory(prefix="bel-wasm-emitter-external-") as directory:
        work = Path(directory)
        shutil.copyfile(EMITTER, work / "emitter.wasm")
        shutil.copyfile(HERE / "emit-wat.mjs", work / "transport.mjs")
        shutil.copyfile(source_path, work / "source.json")
        result = run("node", "transport.mjs", "emitter.wasm", "source.json", cwd=work)
        wat = work / "result.wat"
        wat.write_text(result.stdout)
        binary = work / "result.wasm"
        run(TOOLS / "wat2wasm", wat, "-o", binary)
        run(TOOLS / "wasm-validate", binary)
        inspect(binary, expected, cwd=work)
        shutil.copyfile(wat, OUT / f"{name}.wat")
        shutil.copyfile(binary, OUT / f"{name}.wasm")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true", help="reuse previously built emitter/features")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    if not args.skip_build:
        for source, output in [("expression-emitter.bel", EMITTER), ("emitter-features.bel", FEATURES)]:
            run(sys.executable, HERE / "compile.py", "--lists", HERE / source, output)
    run(TOOLS / "wasm-validate", EMITTER)
    run("node", HERE / "verify-emitter-features.mjs", FEATURES)
    cases = [
        ("choose", ["fn", ["x"], ["if", "x", ["quote", "t"], ["quote", "nil"]]], [0, 1]),
        ("mutated", ["fn", ["x"], ["if", "x", ["quote", "nil"], ["quote", "nil"]]], [0, 0]),
        ("reversed", ["fn", ["x"], ["if", "x", "nil", "t"]], [1, 0]),
        ("nested", ["fn", ["arg"], ["if", ["if", "arg", None, "t"], "t", "arg"]], [1, 1]),
        ("constant", ["fn", ["p"], ["if", "t", None, "t"]], [0, 0]),
        ("named", ["fn", ["arg-name"], ["if", "arg-name", "t", None]], [0, 1]),
    ]
    for name, source, expected in cases:
        external(source, name, expected)
    assert (OUT / "choose.wasm").read_bytes() != (OUT / "mutated.wasm").read_bytes()

    native = OUT / "native-wrapper.bel"
    native.write_text("(def emit-native (forms) (if (emit-one forms) (emit-program (car forms)) (emit-reject)))")
    reference = run(ROOT / "target/debug/bel-eval-text", "--call", "emit-native", HERE / "choose.bel",
                    HERE / "expression-emitter.bel", native)
    assert reference.stdout == (OUT / "choose.wat").read_text(), "native and compiled emitters disagree"

    rejected = [
        [], ["fn", [], None], ["fn", ["x", "y"], "x"],
        *(["fn", [p], p] for p in ["nil", "t", "o", "apply", "", "x y", "x)(func", "🙂"]),
        ["fn", ["x"], ["join", "x", "x"]], ["fn", ["x"], ["if", "x", "t"]],
        ["fn", ["x"], ["if", "x", "t", "nil", "t"]],
        ["fn", ["x"], ["quote", "other"]], ["fn", ["x"], ["quote", "t", "nil"]],
        ["fn", ["x"], "missing"], ["fn", ["x"], "x", "nil"],
        {"pair": ["fn", "tail"]}, ["fn", {"pair": ["x", "tail"]}, "x"],
    ]
    output = OUT / "recovery.wasm"
    for index, source in enumerate(rejected):
        path = OUT / f"rejected-{index}.json"
        path.write_text(json.dumps(source))
        output.write_bytes(b"stale")
        output.with_suffix(".wat").write_text("stale")
        result = run(sys.executable, HERE / "emit.py", path, output, success=False)
        assert "Wasm emitter rejected input" in result.stderr
        assert not output.exists() and not output.with_suffix(".wat").exists()

    malformed = OUT / "malformed.json"
    malformed.write_text("{")
    run(sys.executable, HERE / "emit.py", malformed, output, success=False)
    assert not output.exists() and not output.with_suffix(".wat").exists()
    run(sys.executable, HERE / "emit.py", HERE / "choose.ast.json", output,
        "--wat2wasm", OUT / "missing-assembler", success=False)
    assert not output.exists() and not output.with_suffix(".wat").exists()
    run(sys.executable, HERE / "emit.py", HERE / "choose.ast.json", output)
    inspect(output, [0, 1])
    manifest = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in OUT.glob("*.wasm")}
    (OUT / "artifacts.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"PASS: 6 external programs, native equivalence, {len(rejected)} Wasm rejections, failure cleanup/recovery")


if __name__ == "__main__":
    main()
