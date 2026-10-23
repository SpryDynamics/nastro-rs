//! Decoding a recording to the 16 kHz mono f32 PCM that whisper expects, using the `ffmpeg` already required
//! for recording (never linked, only run as a subprocess).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::{CancelToken, Error, Result};

/// whisper.cpp's fixed input rate.
pub const SAMPLE_RATE: u32 = 16_000;

/// The whole recording is held in memory (whisper needs it in one piece), 4 bytes per sample.
/// 6 hours is ~1.4 GB, which is already a lot; beyond that, fail clearly rather than thrash or crash.
pub const MAX_HOURS: u32 = 6;

/// `NASTRO_FFMPEG`, else `ffmpeg` from `PATH` - the same rule the recorder uses.
fn ffmpeg_program() -> std::ffi::OsString {
    std::env::var_os("NASTRO_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

/// ffmpeg arguments to decode `input` to raw little-endian f32, mono, 16 kHz on stdout.
pub fn ffmpeg_args(input: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> =
        ["-nostdin", "-hide_banner", "-loglevel", "error", "-i"]
            .iter()
            .map(Into::into)
            .collect();
    args.push(input.as_os_str().to_os_string());
    args.extend(
        ["-vn", "-ac", "1", "-ar", "16000", "-f", "f32le", "pipe:1"]
            .iter()
            .map(std::ffi::OsString::from),
    );
    args
}

/// Decodes `input` (any format ffmpeg can read, normally `audio.m4a`) to samples.
pub fn decode(input: &Path, cancel: &CancelToken) -> Result<Vec<f32>> {
    let mut child = Command::new(ffmpeg_program())
        .args(ffmpeg_args(input))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::FfmpegNotFound
            } else {
                Error::io("starting ffmpeg", e)
            }
        })?;

    // Drain stderr concurrently: if ffmpeg filled that pipe while we only read stdout, both would block.
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.by_ref().take(64 * 1024).read_to_string(&mut text);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        text
    });

    let max_bytes = MAX_HOURS as usize * 3600 * SAMPLE_RATE as usize * 4;
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut bytes: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 256 * 1024];
    let mut too_long = false;
    let mut cancelled = false;

    loop {
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }
        let n = stdout
            .read(&mut chunk)
            .map_err(|e| Error::io("reading decoded audio from ffmpeg", e))?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > max_bytes {
            too_long = true;
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }

    if cancelled || too_long {
        let _ = child.kill();
    }
    drop(stdout);
    let status = child
        .wait()
        .map_err(|e| Error::io("waiting for ffmpeg", e))?;
    let stderr_text = stderr_reader.join().unwrap_or_default();

    if cancelled {
        return Err(Error::Cancelled);
    }
    if too_long {
        return Err(Error::TooLong { max: MAX_HOURS });
    }
    if !status.success() {
        let msg = stderr_text.trim();
        return Err(Error::FfmpegFailed(if msg.is_empty() {
            format!("ffmpeg exited with {status}")
        } else {
            msg.to_string()
        }));
    }

    let samples = bytes_to_f32(&bytes);
    if samples.is_empty() {
        return Err(Error::EmptyAudio);
    }
    Ok(samples)
}

/// Little-endian f32 bytes -> samples (a trailing partial sample is ignored).
pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_request_16k_mono_float_on_stdout() {
        let args: Vec<String> = ffmpeg_args(Path::new("a b.m4a"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        assert!(joined.contains("-ac 1"));
        assert!(joined.contains("-ar 16000"));
        assert!(joined.contains("-f f32le pipe:1"));
        assert!(joined.contains("-nostdin"));
        // The input path is one argument even when it contains a space.
        assert!(args.contains(&"a b.m4a".to_string()));
    }

    #[test]
    fn converts_little_endian_floats_and_ignores_a_trailing_fragment() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0.5f32.to_le_bytes());
        bytes.extend_from_slice(&(-1.0f32).to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        assert_eq!(bytes_to_f32(&bytes), vec![0.5, -1.0]);
        assert!(bytes_to_f32(&[]).is_empty());
    }

    #[test]
    fn a_missing_file_is_reported_as_an_ffmpeg_failure_not_a_panic() {
        // Requires ffmpeg; when it is absent the error is FfmpegNotFound, which is equally acceptable here.
        let err = decode(Path::new("definitely/not/here.m4a"), &CancelToken::new()).unwrap_err();
        assert!(
            matches!(err, Error::FfmpegFailed(_) | Error::FfmpegNotFound),
            "{err}"
        );
    }
}
