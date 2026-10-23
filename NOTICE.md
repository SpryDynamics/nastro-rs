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

## External runtime tools (not bundled)

`nastro-tap` encodes audio by running **FFmpeg** (<https://ffmpeg.org>) as a separate
subprocess. FFmpeg is **not** linked into, compiled into, or redistributed with this
project, so its license does not attach to nastro's own code. FFmpeg builds are
licensed LGPL-2.1+ or GPL-2.0+ depending on how they were configured; whoever
installs FFmpeg is responsible for complying with the license of the build they
chose. If a future release of this project ever *ships* an FFmpeg binary alongside
`nastro`, that release must also carry FFmpeg's license text, its corresponding
source offer, and a note of the exact build configuration.

## Libraries linked dynamically

The Linux build links the PulseAudio client libraries (`libpulse`, `libpulse-simple`;
LGPL-2.1-or-later) as shared libraries from the host system. They are not bundled or
modified, which is how the LGPL is satisfied: a user may replace them with any
compatible version. Their license texts accompany the `libpulse-sys` /
`libpulse-binding` crates listed in `THIRD_PARTY_NOTICES.md`.
