# Billing operations workspace

We need a small script to total exported invoice lines. Billing uses the CSV
format in `docs/invoice-export.md`; product decisions are recorded in
`meeting-notes/billing-handoff.md`. A sample export is in `examples/`.

The deliverable is `invoice-total.sh` in this folder. It must work when invoked
as `/bin/sh invoice-total.sh path/to/export.csv`. Our machines have shell and
awk installed. Keep any tests and implementation notes here for the next person.
