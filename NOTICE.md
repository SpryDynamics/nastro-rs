# Notice

`nastro-rs` is an independent Rust reimplementation targeting Windows and Linux,
designed and architected with reference to [scaccogatto/nastro](https://github.com/scaccogatto/nastro)
(Go + Swift, macOS-only, MIT licensed) as the original design/behavior spec - in
particular its filesystem-as-database layout, its `nastro`/`nastro-tap` two-process
split and stdio protocol, and the capture-mixer design documented in its own
`docs/stack.md`. No source code from the original project is copied; this is a
from-scratch implementation informed by reading its public repository and documents.

Original project: Copyright (c) 2026 Marco Boffo (GitHub: scaccogatto), MIT License.

## Name and affiliation

**nastro-rs is an unofficial, independent project. It is not affiliated with, endorsed by or sponsored by the author of the original nastro, and it is not the original `nastro`.** It is a separate reimplementation for Windows and Linux, written and
maintained by Spry Dynamics, LLC.

* *Names.* `nastro-rs` is the name of this project (repository, releases, downloads). The command it installs is
  still called `nastro`, after the Italian word for *tape*, which is also what the original calls its command. The
  original targets macOS and this project Windows and Linux, so the two are not expected to meet on one machine.
* *Code.* None of the original's source code is used.
* *Artwork and wording.* The logo and the README demo are original to this project and share nothing with the
  original's. No screenshot or image from the original is included, and the descriptive text is our own wording.
* *Credit.* The design (file layout, the two-process split, the mixer's drift handling) was informed by the original's
  public documents, as described above. The original is MIT licensed and is credited here as that license asks.

See [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for this project's own Rust
dependency licenses.

## External runtime tools (not bundled)

`nastro-tap` (recording) and `nastro transcribe` (reading a recording back for speech recognition) run **FFmpeg** (<https://ffmpeg.org>) as a separate
subprocess. FFmpeg is **not** linked into, compiled into, or redistributed with this
project, so its license does not attach to nastro's own code. FFmpeg builds are
licensed LGPL-2.1+ or GPL-2.0+ depending on how they were configured; whoever
installs FFmpeg is responsible for complying with the license of the build they
chose. If a future release of this project ever *ships* an FFmpeg binary alongside
`nastro`, that release must also carry FFmpeg's license text, its corresponding
source offer, and a note of the exact build configuration.

**WhisperX** (<https://github.com/m-bain/whisperX>, BSD-2-Clause), optional, for speaker labels. `nastro` runs it as a
separate program when `transcriber = "whisperx"` is set. It is **not** bundled, linked or redistributed: it is installed
on request by the user's own `uv` into their own profile. It in turn uses PyTorch, pyannote.audio and faster-whisper, and
downloads models from Hugging Face on first use: notably `pyannote/speaker-diarization-community-1` (CC-BY-4.0, gated: the
user accepts its terms with their own Hugging Face account) and a wav2vec2 alignment model. Their licenses and terms apply
to whoever installs and uses them, not to nastro's code.
**Inno Setup** (<https://jrsoftware.org/isinfo.php>, Copyright (C) 1997-2026 Jordan Russell and Martijn Laan) builds the Windows
installer, and its small setup/uninstall runtime is embedded in `nastro-rs-<version>-windows-x86_64-setup.exe`. Its license
permits use and redistribution for any purpose, including commercial, provided its copyright notices are kept, which they are
(they are part of the embedded runtime).
## Libraries linked dynamically

The Linux build links the PulseAudio client libraries (`libpulse`, `libpulse-simple`;
LGPL-2.1-or-later) as shared libraries from the host system. They are not bundled or
modified, which is how the LGPL is satisfied: a user may replace them with any
compatible version. Their license texts accompany the `libpulse-sys` /
`libpulse-binding` crates listed in `THIRD_PARTY_NOTICES.md`.

## Statically linked native code: whisper.cpp / ggml

Speech recognition (`nastro transcribe` and the TUI's `t` key) uses **whisper.cpp** and **ggml**
(<https://github.com/ggml-org/whisper.cpp>), compiled from source by the `whisper-rs-sys` crate and linked
statically into the `nastro` binary. They are licensed under the MIT License, reproduced here as it requires.
(The Rust wrapper crates `whisper-rs` and `whisper-rs-sys` are listed in `THIRD_PARTY_NOTICES.md`; this
vendored C/C++ code is not covered by that list, so it is reproduced here.)

~~~text
MIT License

Copyright (c) 2023-2024 The ggml authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~

## Speech models (downloaded at run time, not bundled)

`nastro` downloads a Whisper speech model (ggml format, from <https://huggingface.co/ggerganov/whisper.cpp>)
the first time you transcribe. Nothing is shipped with the program. The models are OpenAI's Whisper weights,
released under the MIT License (<https://github.com/openai/whisper>), converted to ggml by the whisper.cpp
project. Each download is verified against a SHA-256 built into `nastro` before it is used.

## Test data

`crates/nastro-transcribe/tests/fixtures/jfk.wav` is an 11-second excerpt of a 1961 address by U.S. President
John F. Kennedy, a work of the United States government and in the public domain. It is the sample that
whisper.cpp itself ships for testing, and is used only by the test suite.
