#!/usr/bin/env python3
"""Compile Bel list programs, then execute copied Wasm outside the checkout."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from threading import Lock

sys.dont_write_bytecode = True
from compile import HERE, ROOT

OUT = ROOT / "target/bel-list-acceptance"
LOG = Lock()


def run(*args, success=True, cwd=None):
    result = subprocess.run(list(map(str, args)), capture_output=True, text=True, cwd=cwd)
    with LOG:
        print("$", " ".join(map(str, args)), flush=True)
        print(result.stdout, end="", flush=True)
        print(result.stderr, end="", file=sys.stderr, flush=True)
    assert (result.returncode == 0) == success, result
    return result


def compile_source(source, output, success=True):
    return run(sys.executable, HERE / "compile.py", "--lists", source, output, success=success)


def execute(output, mode):
    run(ROOT / "target/bel-wasm-tools/node_modules/.bin/wasm-validate", output)
    with tempfile.TemporaryDirectory(prefix="bel-list-external-") as directory:
        binary = Path(directory) / "program.wasm"
        shutil.copyfile(output, binary)
        # stdin is a standalone test driver using only Node standard modules.
        result = subprocess.run(
            ["node", "--input-type=module", "-", str(binary), mode],
            input=(HERE / "verify-lists.mjs").read_text(),
            cwd=directory, capture_output=True, text=True,
        )
        print(result.stdout, end="", flush=True)
        print(result.stderr, end="", file=sys.stderr, flush=True)
        assert result.returncode == 0, result


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    reference = OUT / "host-reference.bel"
    reference.write_text(r'''
(let x '(t (nil sym . \a))
  (let result (copy-tree x)
    (if (and (= x result) (no (id x result))
             (id (type nil) 'symbol) (id (type \a) 'char)
             (id (car nil) nil) (id (cdr nil) nil))
        "PASS: bootstrap copy-tree semantics"
        (err 'reference-mismatch))))
''')
    run(ROOT / "target/debug/bel-eval-text", HERE / "copy-tree.bel", reference)
    fixtures = {
        "copy": (HERE / "copy-tree.bel").read_text(),
        "mutated": (HERE / "copy-tree.bel").read_text().replace("      x))", "      t))"),
        "calls": "(def list-first (x) (helper x)) (def helper (x) (car x))",
        "identity": "(def same (x) (id (car x) (cdr x)))",
        "type": "(def classify (x) (type x))",
        "truth": "(def truth (x) (if x t nil))",
    }
    for mode, source in fixtures.items():
        path = OUT / f"{mode}.bel"
        path.write_text(source)
        output = path.with_suffix(".wasm")
        compile_source(path, output)
        execute(output, mode)

    rejected = [
        "", "(def copy (o) o)", "(def copy (apply) apply)",
        "(def copy (x y) (copy x))", "(def copy (x) (missing x))",
        "(def copy (x) (join x))", "(def copy (x) (car x x))",
        "(def copy (x) (if x nil))", "(def copy (x) '(unknown pair))",
        "(def copy (x) 0)", "(def copy (x) x) (def copy (y) y)",
        "(def copy (helper) (helper helper)) (def helper (x) x)",
        "(def car (x) x)", "(def copy (x) x) (err 'must-not-run)",
        "(def lit (x) x)", "(def dyn (x) x)",
        *[f"(def {name} (x) x) (def other (x) x)"
          for name in ["no", "list", "append", "cons", "reduce", "scope", "globe", "map", "hug", "letu", "first", "chars"]],
        "(def copy (x) (quote nil t))", "(def copy (x) x . other)",
        "(def copy (x) x) . other", ") (err 'must-not-run)",
    ]
    output = OUT / "recovery.wasm"
    def check_rejection(item):
        index, text = item
        path = OUT / f"rejected-{index}.bel"
        path.write_text(text)
        artifact = output if index == 0 else OUT / f"rejected-{index}-output.wasm"
        artifact.write_bytes(b"stale")
        artifact.with_suffix(".wat").write_text("stale")
        compile_source(path, artifact, success=False)
        assert not artifact.exists() and not artifact.with_suffix(".wat").exists()

    # Every fresh host and artifact pair is independent; retain a complete
    # command/output block per case and recover into the first failed path.
    with ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(check_rejection, enumerate(rejected)))
    compile_source(HERE / "copy-tree.bel", output)
    execute(output, "copy")
    print(f"PASS: 6 programs, external execution, mutation, {len(rejected)} rejections, recovery")


if __name__ == "__main__":
    main()
