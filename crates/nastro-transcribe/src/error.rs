use std::path::PathBuf;

/// Everything that can go wrong transcribing a recording. The `Display` text is written for end users: it says
/// what happened and, where there is one, what to do about it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("recording not found: {0}")]
    RecordNotFound(String),

    #[error("{0} has no audio.m4a to transcribe")]
    NoAudio(PathBuf),

    #[error(
        "this recording is still being recorded; stop the recording first (transcribing a partial file would give a partial transcript)"
    )]
    InProgress,

    #[error("{0} is already transcribed (use --force to overwrite it)")]
    AlreadyTranscribed(PathBuf),

    #[error(
        "ffmpeg was not found, and it is needed to read the recording. Install it \
         (Windows: `winget install Gyan.FFmpeg`; Debian/Ubuntu: `sudo apt install ffmpeg`) or set NASTRO_FFMPEG to its full path"
    )]
    FfmpegNotFound,

    #[error("ffmpeg could not read the audio - the recording may be corrupt or empty: {0}")]
    FfmpegFailed(String),

    #[error("the recording contains no audio")]
    EmptyAudio,

    #[error(
        "the recording is longer than {max} hours; transcription holds the whole recording in memory, so it is limited to {max} hours"
    )]
    TooLong { max: u32 },

    #[error(
        "whisper model {name:?} is not installed at {path}. Run `nastro transcribe` again to download it, \
         or place the file there yourself (known models are listed in `nastro transcribe --list-models`)"
    )]
    ModelMissing { name: String, path: PathBuf },

    #[error("unknown whisper model {0:?}; choose one of `nastro transcribe --list-models`, or give the path to a ggml .bin file")]
    UnknownModel(String),

    #[error("model download failed: {0}")]
    Download(String),

    #[error("downloaded model failed verification (expected sha256 {expected}, got {actual}); it was discarded")]
    Checksum { expected: String, actual: String },

    #[error(
        "this CPU lacks AVX2/FMA/F16C, which the bundled speech engine requires (any x86-64 CPU from about 2013 on has them)"
    )]
    UnsupportedCpu,

    #[error("unknown language {0:?}: use a language code such as `en`, `es`, `de`, or `auto` to detect it")]
    UnknownLanguage(String),

    #[error("the model {model:?} only supports English, but language {lang:?} was requested")]
    EnglishOnlyModel { model: String, lang: String },

    #[error(
        "speaker identification needs WhisperX, which is not installed (looked for `{searched}`).\n\
         Run `nastro transcribe --setup-whisperx` to install it (needs the `uv` tool: https://docs.astral.sh/uv/), \
         or install it yourself with `uv tool install whisperx`"
    )]
    WhisperxMissing { searched: String },

    #[error(
        "speaker identification needs a Hugging Face access token.\n\n\
         1. Create a free *read* token at https://huggingface.co/settings/tokens\n\
         2. Accept the model terms at https://huggingface.co/pyannote/speaker-diarization-community-1\n\
         3. Put it in the config file as `hf_token = \"hf_...\"` (or set the HF_TOKEN environment variable)"
    )]
    HfTokenMissing,

    #[error(
        "Hugging Face did not accept the access token. Check that `hf_token` (or HF_TOKEN) is a valid *read* token \
         from https://huggingface.co/settings/tokens"
    )]
    HfTokenRejected,

    #[error(
        "your Hugging Face account has not been granted access to the speaker model yet.\n\
         Open https://huggingface.co/{repo} while signed in, accept its terms, then try again"
    )]
    DiarizationAccess { repo: String },

    #[error("WhisperX cannot use the model {0:?}: it takes a model name (such as `small` or `large-v3-turbo`), not a file path or an unknown name")]
    WhisperxModel(String),

    #[error("WhisperX is missing something it needs: {0}")]
    WhisperxSetup(String),

    #[error("WhisperX failed ({status}).\n{detail}")]
    WhisperxFailed { status: String, detail: String },

    #[error("could not read WhisperX's result: {0}")]
    WhisperxOutput(String),

    #[error("unknown transcriber {0:?} in the config file; use \"whisper\" or \"whisperx\"")]
    UnknownTranscriber(String),

    #[error("speech recognition failed: {0}")]
    Whisper(String),

    #[error("cancelled")]
    Cancelled,

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
