#!/usr/bin/env python3
"""Export committed experiment sources and ZIPs into a website content folder."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import zipfile

SLUGS = ("bel-wasm", "ink-on-deno", "cwr-vcs")
REPOSITORY = "https://github.com/randallb/randallb-com"


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args])


def export(repo, site):
    site = site.resolve()
    if not site.is_dir():
        raise ValueError("Website content folder must already exist")
    config_path = site / "site.json"
    if not config_path.exists():
        config_path = site / "routes.json"
    config = json.loads(config_path.read_text())
    index_path = site / "projects/index.json"
    index = json.loads(index_path.read_text())
    revision = git(repo, "rev-parse", "HEAD").decode().strip()
    if git(repo, "status", "--porcelain", "--", "projects", "scripts/export_projects.py"):
        raise ValueError("Commit project sources and exporter before exporting")
    replacements = {}
    for slug in SLUGS:
        prefix = f"projects/{slug}/"
        names = git(repo, "ls-tree", "-rz", "--name-only", revision, "--", prefix)
        names = [name.decode() for name in names.split(b"\0") if name]
        if not names:
            raise ValueError(f"Missing committed project: {slug}")
        destination = site / "projects" / slug
        manifest_path = destination / "source-manifest.json"
        manifest = json.loads(manifest_path.read_text())
        # Read Git objects, never arbitrary untracked files or build output.
        files = []
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for name in sorted(names):
                relative = name[len(prefix):]
                body = git(repo, "show", f"{revision}:{name}")
                output = destination / "source" / relative
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(body)
                url = f"/projects/{slug}/source/{relative}"
                files.append({"path": relative, "url": url, "bytes": len(body),
                              "sha256": hashlib.sha256(body).hexdigest()})
                config["routes"][url] = {"file": output.relative_to(site).as_posix(),
                                         "content_type": "text/plain; charset=utf-8",
                                         "noindex": True}
                info = zipfile.ZipInfo(f"{slug}/{relative}", (2026, 11, 5, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o100644 << 16
                bundle.writestr(info, body)
        current = {f["path"] for f in files}
        for previous in manifest["files"]:
            if previous["path"] not in current:
                # Old source routes must not retain deleted code. Old ZIPs stay available.
                config["routes"].pop(previous["url"], None)
        digest = hashlib.sha256(archive.getvalue()).hexdigest()
        download = f"/projects/{slug}/downloads/{revision[:12]}-{digest[:12]}.zip"
        output = site / download.lstrip("/")
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(archive.getvalue())
        config["routes"][download] = {"file": output.relative_to(site).as_posix(),
                                       "content_type": "application/zip", "noindex": True}
        replacements[manifest["download_url"]] = download
        github = f"{REPOSITORY}/tree/{revision}/projects/{slug}"
        manifest.update(files=files, download_url=download, download_sha256=digest,
                        maintained_source={"repository": REPOSITORY, "revision": revision,
                                           "path": f"projects/{slug}", "url": github})
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        for project in index["projects"]:
            if project["id"] == slug:
                project.update(download_url=download, github_source_url=github)
        for page in (site / "pages/projects").glob(f"{slug}*.html"):
            text = page.read_text()
            # Replace only our existing link on a subsequent export.
            import re
            text = re.sub(r'<a data-github-source="true" href="[^"]*">GitHub source</a>',
                          '', text)
            marker = '<p class="project-links">'
            if marker in text:
                text = text.replace(marker, marker +
                    f'<a data-github-source="true" href="{github}">GitHub source</a>', 1)
            page.write_text(text)
    index_path.write_text(json.dumps(index, indent=2) + "\n")
    for page in (site / "pages/projects").glob("*.html"):
        text = page.read_text()
        for old, new in replacements.items():
            text = text.replace(old, new)
        page.write_text(text)
    config_path.write_text(json.dumps(config, indent=2) + "\n")
    return revision


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site", required=True, type=Path,
                        help="Website content folder containing site.json or routes.json")
    args = parser.parse_args()
    print(export(Path(__file__).resolve().parents[1], args.site))
