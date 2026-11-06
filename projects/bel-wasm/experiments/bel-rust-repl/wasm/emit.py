#!/usr/bin/env python3
"""Run the Wasm emitter on a runtime AST, then assemble its returned WAT."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def emit_module(source, output, emitter, assembler):
    source, output, emitter = source.resolve(), output.resolve(), emitter.resolve()
    wat = output.with_suffix(".wat")
    if output.suffix != ".wasm":
        raise ValueError("output must end in .wasm")
    if source in (output, wat) or emitter in (output, wat):
        raise ValueError("output must not overwrite input or emitter")
    output.unlink(missing_ok=True)
    wat.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix="bel-emitted-") as directory:
        work = Path(directory)
        result = subprocess.run(
            ["node", str(HERE / "emit-wat.mjs"), str(emitter), str(source)],
            capture_output=True, check=False,
        )
        if result.returncode:
            raise ValueError(result.stderr.decode("utf8", errors="replace").strip())
        candidate = work / "program.wat"
        binary = work / "program.wasm"
        candidate.write_bytes(result.stdout)
        subprocess.run([str(assembler), str(candidate), "-o", str(binary)], check=True)
        output.parent.mkdir(parents=True, exist_ok=True)
        wat.write_bytes(result.stdout)
        output.write_bytes(binary.read_bytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--emitter", type=Path, default=ROOT / "target/bel-emitter/emitter.wasm")
    parser.add_argument("--wat2wasm", type=Path,
                        default=ROOT / "target/bel-wasm-tools/node_modules/.bin/wat2wasm")
    args = parser.parse_args()
    try:
        emit_module(args.source, args.output, args.emitter, args.wat2wasm)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"emission failed: {error}", file=sys.stderr)
        return 1
    print(f"emitted {args.source} -> {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
