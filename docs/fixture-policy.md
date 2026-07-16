# Fixture Policy

Persistent test and fuzz inputs are untrusted project artifacts. Every file
under `fuzz/corpus` must have exactly one entry in `fuzz/fixtures.toml`.

Each entry records SHA-256 and exact byte size; license, provenance, and
generator version; guest architecture, endianness, and page size where
meaningful; physical-range context; and expected parser behavior.

Run the validator after adding or intentionally changing a seed:

```console
python scripts/verify-fixtures.py
python -m unittest scripts/test_fixtures.py
```

The validator rejects malformed metadata, duplicate paths, undeclared files,
stale declarations, path escapes, non-files, size changes, and hash changes.
Do not update a digest until the changed bytes and provenance have been
reviewed. Preserve minimized fuzz failures as new regression seeds with their
own entries rather than silently replacing an existing seed.

Large or licensed guest images must remain outside Git in access-controlled,
immutable CI artifact storage. Their external catalog should carry the same
metadata plus artifact-store identity and retention policy.
