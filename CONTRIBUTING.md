# Contributing

1. `make verify` must pass: fmt-check, clippy with warnings denied, the
   tests, the smoke test.
2. Fixtures are synthetic. A new answer shape becomes a file in
   `tests/fixtures/site/`, written by hand from the shapes
   `examples/shape.rs` prints and the dummies in `tests/fixtures/README.md`,
   never copied from a real answer.
3. No personal data anywhere: code, docs, commit messages, pull requests,
   issues. `tests/no_personal_data.rs` enforces the shapes; review the rest.
4. A new read is an experience the site's own pages call for the same
   data, added to `EXPERIENCES` with its page named, and documented in
   `docs/api.md`. Nothing that writes.
5. Squash merges only, with an approval and green CI.
