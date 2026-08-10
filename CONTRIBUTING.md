# Contributing

One maintainer, but the repository is public and the workflow is written down so
that every session — human or agent — picks it up the same way.

## Work items live in issues

Every unit of work is a GitHub issue. Not a branch name, not a TODO comment, not
a line in someone's notes.

An issue carries:

- a title that names the defect or the wish, not the area it lives in
- the measurable facts — counts, timings, the corpus or run they came from
- an acceptance criterion, written as **"Fertig, wenn …" / "Done when …"**
- one label from the small set below

Labels: `bug`, `scraper`, `wörterbuch`, `harness`, `ux-idea`, plus `prio-hoch`
when it goes before the rest. The app repo uses the same set with `ui` in place
of `scraper`.

Measure before you name a cause. A tagging change is proven by comparing two
`dump_tags` runs — *no previously tagged line may change its tag* — not by the
count of untagged lines alone. "Probably X" belongs in an issue; it does not
belong in a commit message.

## Pull requests close their issue

A pull request body says `fixes #N` (or `closes #N`) so the merge closes the
issue by itself — no manual ticking off. One issue per PR wherever the change
allows it.

If a PR fixes something else along the way, that gets its own issue, even after
the fact. A closed issue is the only record that the thing was ever open.

## This repository is public

Keep personal data out of issues, PR descriptions, commit messages, code
comments and test fixtures:

- no tester names, no towns, no postcodes tied to a person
- no verbatim feedback that identifies who wrote it
- no install IDs, no account identifiers, no API keys

Beta-test feedback is sanitized before it becomes an issue: the issue gets a
neutral summary of the finding plus the pointer *"Details: Vault-Backlog"*, and
the identifying context stays in the maintainer's private notes. When in doubt,
sanitize harder — an issue can always be edited to add technical detail, never
to unpublish a name.

Chain names, product names and leaflet data are fine. Those are public already.
A single branch ID is fine as data; the fact that a named person shops there is
not.

## Where the findings come from

Device-test rounds are dictated, cleaned up into a private note, and then split
into one issue per point. That flow stays: the note keeps the narrative and the
personal context, the issues carry the work.

Decisions that are the maintainer's to make — whether non-food belongs in the
offers at all, whether a delete migration runs against production — are not
issues until they are decided. They live in the private roadmap.
