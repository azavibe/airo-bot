## What and why

<!-- What does this change, and why? Link the issue: "Fixes #123". -->

## How it was tested

<!-- Tests added or updated, and what you checked by hand. -->

## Checklist

- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] `npm run build` in `web/` passes (if `web/` changed)
- [ ] New or changed endpoints have integration tests
- [ ] Schema changes are in a new migration (no edits to existing ones)
- [ ] User-visible changes are under "Unreleased" in `CHANGELOG.md`
