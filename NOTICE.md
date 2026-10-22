# Notice

`nastro-rs` is an independent Rust reimplementation targeting Windows and Linux,
designed and architected with reference to [scaccogatto/nastro](https://github.com/scaccogatto/nastro)
(Go + Swift, macOS-only, MIT licensed) as the original design/behavior spec - in
particular its filesystem-as-database layout, its `nastro`/`nastro-tap` two-process
split and stdio protocol, and the capture-mixer design documented in its own
`docs/stack.md`. No source code from the original project is copied; this is a
from-scratch implementation informed by reading its public repository and documents.

Original project: Copyright (c) scaccogatto, MIT License.

See [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for this project's own Rust
dependency licenses.
