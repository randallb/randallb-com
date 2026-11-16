#!/usr/bin/env python3
"""Transport source to Bel and assemble its text output; no language lowering."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def compile_module(source, output, host, assembler, lists=False):
    source, output = source.resolve(), output.resolve()
    wat = output.with_suffix(".wat")
    if output.suffix != ".wasm":
        raise ValueError("output must end in .wasm")
    protected = [source, *(HERE / name for name in
                           ("compiler.bel", "list-compiler.bel", "list-runtime.bel",
                            "list-text.bel", "text-runtime.bel", "bootstrap-boundary.bel"))]
    if any(path in (output, wat) for path in protected):
        raise ValueError("output must not overwrite source or compiler")
    # Remove previous products before attempting compilation, including failure.
    output.unlink(missing_ok=True)
    wat.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix="bel-wasm-") as temporary:
        work = Path(temporary)
        libraries = [HERE / "compiler.bel"]
        if lists:
            libraries = [HERE / "bootstrap-boundary.bel", *libraries,
                         HERE / "list-runtime.bel", HERE / "text-runtime.bel",
                          HERE / "list-text.bel", HERE / "list-compiler.bel"]
        result = subprocess.run(
            [str(host), "--call", "wasm-compile-lists" if lists else "wasm-compile-forms",
             str(source), *(str(path) for path in libraries)],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        if result.returncode:
            raise ValueError(result.stderr.decode("utf-8", errors="replace").strip())
        candidate = work / "module.wat"
        candidate.write_bytes(result.stdout)
        binary = work / "module.wasm"
        subprocess.run([str(assembler), str(candidate), "-o", str(binary)], check=True)
        # Publish only complete successful products.
        output.parent.mkdir(parents=True, exist_ok=True)
        wat.write_bytes(result.stdout)
        output.write_bytes(binary.read_bytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--lists", action="store_true", help="compile first-order list functions")
    parser.add_argument("--host", type=Path, default=ROOT / "target/debug/bel-eval-text")
    parser.add_argument(
        "--wat2wasm", type=Path,
        default=ROOT / "target/bel-wasm-tools/node_modules/.bin/wat2wasm",
    )
    args = parser.parse_args()
    try:
        compile_module(args.source, args.output, args.host, args.wat2wasm, args.lists)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"compile failed: {error}", file=sys.stderr)
        return 1
    print(f"compiled {args.source} -> {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
