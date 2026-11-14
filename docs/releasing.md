# Release checklist

Review the version, changelog, documentation, license notices and demo preview.
Confirm that package contents contain only intended source and documentation.

From a clean checkout, run:

```bash
bash tools/check-release
bash tools/cargo-local publish --dry-run --locked
bash tools/cargo-local package --list --locked
```

Complete physical display testing separately on an unused supported device.
For an approved release, publish the reviewed package, create the matching
`vVERSION` tag and write release notes. These are separate release actions;
the checks above do not publish or create tags.
