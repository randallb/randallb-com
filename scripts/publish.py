#!/usr/bin/env python3
"""Build and publish an explicit snapshot of a synced Google Drive folder."""
import argparse
import hashlib
import json
import re
from pathlib import Path
import subprocess
import shutil
import tempfile
from urllib.parse import urlsplit, unquote

ROOT = Path(__file__).resolve().parent.parent
MAX_SOURCE_BYTES = 100_000_000
MAX_ASSET_BYTES = 25_000_000


def build(source: Path, output: Path):
    source = source.resolve(strict=True)
    if source == ROOT or ROOT in source.parents:
        raise ValueError("Content must be outside the Git checkout")
    hashes = {}
    total = 0
    files = {}

    def read_file(name):
        nonlocal total
        if not isinstance(name, str) or not name or Path(name).is_absolute():
            raise ValueError("Expected a relative source file")
        path = (source / name).resolve(strict=True)
        if not path.is_relative_to(source) or not path.is_file():
            raise ValueError("Source file escapes the content folder")
        data = path.read_bytes()
        total += len(data)
        if total > MAX_SOURCE_BYTES:
            raise ValueError("Publisher supports at most 100 MB of mapped source")
        if len(data) > MAX_ASSET_BYTES:
            raise ValueError("Each static asset must be at most 25 MB")
        hashes[name] = hashlib.sha256(data).hexdigest()
        return data

    manifest = json.loads(read_file("site.json"))
    if manifest.get("version") != 1 or type(manifest.get("noindex")) is not bool:
        raise ValueError("Expected manifest version 1 and explicit boolean noindex")
    routes = manifest.get("routes")
    if not isinstance(routes, dict) or "/" not in routes:
        raise ValueError("A routes object with a homepage is required")

    analytics = manifest.get("posthog")
    analytics_tag = None
    if analytics is not None:
        if (not isinstance(analytics, dict)
                or set(analytics) != {"host", "project_token"}
                or not isinstance(analytics["host"], str)
                or not isinstance(analytics["project_token"], str)
                or not re.fullmatch(r"phc_[A-Za-z0-9]+", analytics["project_token"])):
            raise ValueError("Invalid PostHog configuration")
        host = urlsplit(analytics["host"])
        if (host.scheme != "https" or not host.hostname or host.username
                or host.password or host.query or host.fragment
                or host.path not in ("", "/")):
            raise ValueError("PostHog host must be an HTTPS origin")
        template = (ROOT / "scripts/posthog.js").read_text()
        hashes["publisher:posthog.js"] = hashlib.sha256(template.encode()).hexdigest()
        def js_string(value):
            return json.dumps(value).replace("<", "\\u003c")
        script = template.replace("__PROJECT_TOKEN__", js_string(analytics["project_token"]))
        script = script.replace("__API_HOST__", js_string(analytics["host"].rstrip("/")))
        analytics_tag = '<script data-randallb-posthog>\n' + script + '</script>\n'

    def entry(row):
        if not isinstance(row, dict):
            raise ValueError("Expected a file entry")
        name = row.get("file")
        content_type = row.get("content_type")
        if not isinstance(content_type, str) or not content_type or any(
            ord(char) < 32 or ord(char) > 126 for char in content_type
        ):
            raise ValueError("Invalid content type")
        if name not in files:
            data = read_file(name)
            if analytics_tag is not None and content_type.split(";")[0] == "text/html":
                html = data.decode("utf-8")
                if "data-randallb-posthog" in html:
                    raise ValueError("PostHog must be injected by the publisher only")
                if not re.search(r"</head\s*>", html, re.I):
                    raise ValueError("Analytics HTML requires a closing head")
                data = re.sub(r"</head\s*>", lambda match: analytics_tag + match[0],
                              html, count=1, flags=re.I).encode("utf-8")
                if len(data) > MAX_ASSET_BYTES:
                    raise ValueError("Each static asset must be at most 25 MB")
            files[name] = data
        noindex = row.get("noindex", manifest["noindex"])
        if type(noindex) is not bool:
            raise ValueError("Expected boolean noindex")
        return {"asset": "/" + hashlib.sha256(files[name]).hexdigest() + ".bin",
                "content_type": content_type, "noindex": noindex}

    public = {}
    for route, row in routes.items():
        parsed = urlsplit(route)
        decoded = unquote(route)
        if (not route.startswith("/") or route.startswith("//") or parsed.query
                or parsed.fragment or parsed.netloc or "\\" in decoded
                or any(part in (".", "..") for part in decoded.split("/"))
                or any(ord(char) < 33 or ord(char) > 126 for char in route)):
            raise ValueError(f"Invalid URL path: {route!r}")
        public[route] = entry(row)
    not_found = entry(manifest["not_found"])
    revision = hashlib.sha256(json.dumps(hashes, sort_keys=True).encode()).hexdigest()
    site = {"revision": revision, "noindex": manifest["noindex"],
            "routes": public, "not_found": not_found}
    # Write only after the full source has passed validation.
    output.mkdir(parents=True, exist_ok=True)
    asset_dir = output / "assets"
    staged = Path(tempfile.mkdtemp(prefix="assets-", dir=output))
    for data in files.values():
        (staged / (hashlib.sha256(data).hexdigest() + ".bin")).write_bytes(data)
    if asset_dir.exists():
        shutil.rmtree(asset_dir)
    staged.rename(asset_dir)
    (output / "site-content.json").write_text(json.dumps(site) + "\n")
    (output / "entry.ts").write_text(
        'import { createWorker } from "../worker.ts";\n'
        'import site from "./site-content.json" with { type: "json" };\n'
        'export default createWorker(site);\n')
    receipt = {"source": str(source), "revision": revision, "hashes": hashes,
               "route_count": len(public), "source_bytes": total, "limit_bytes": MAX_SOURCE_BYTES}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path,
                        help="Synced Drive Website folder, outside the Git checkout")
    parser.add_argument("--target", choices=["build", "dry-run", "preview", "production"],
                        default="build")
    args = parser.parse_args()
    output = ROOT / ".drive-build"
    receipt = build(args.source, output)
    print(json.dumps(receipt, indent=2), flush=True)
    if args.target == "build":
        return
    subprocess.run(["deno", "task", "check"], cwd=ROOT, check=True)
    subprocess.run(["deno", "check", str(output / "entry.ts")], cwd=ROOT, check=True)
    config = "wrangler.jsonc" if args.target == "production" else "wrangler.preview.jsonc"
    command = ["npx", "--yes", "wrangler@4.157.0", "deploy", "--config", config]
    if args.target == "dry-run":
        command.append("--dry-run")
    subprocess.run(command, cwd=ROOT, check=True)
    receipt["target"] = args.target
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
