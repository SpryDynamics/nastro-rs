<!--
Thanks for the PR. Please read CONTRIBUTING.md first.

How PRs are handled: this GitHub repository is a mirror of the maintainers' Azure DevOps repository, which is
fast-forward only. A maintainer will port an accepted change by hand, credit you as a co-author, and close this
PR with a link to the commit. Your PR will therefore not show as "Merged".
-->

## What and why

## Checklist

- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] Every commit is signed off (`git commit -s`), see CONTRIBUTING.md
- [ ] No new dependency, or its license is allowed by `deny.toml`
- [ ] Behaviour changes are covered by tests, and `CHANGELOG.md` is updated
