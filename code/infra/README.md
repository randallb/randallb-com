# Infrastructure

`export_projects.py` exports committed experiment source into a website content snapshot. From the repository root:

```sh
python3 code/infra/export_projects.py --site /path/to/Website
```

The content folder must already contain its project index and source manifests. The exporter reads `code/projects/` from Git objects and rejects uncommitted changes to those sources or the exporter. Public `/projects/` URLs remain stable; GitHub links point to the committed repository paths. Exporting writes the chosen local content folder and does not deploy.

[Root development environment](development.md) documents direnv, tool pins and environment verification.
