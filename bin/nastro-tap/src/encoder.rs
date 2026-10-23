//! Incremental AAC/.m4a encoder, implemented as an `ffmpeg` subprocess.
//!
//! Mixed mono PCM is piped to ffmpeg's stdin and muxed as *fragmented* MP4
//! (`empty_moov` + `frag_keyframe` + `frag_duration`), so the file on disk is a
//! valid, playable `.m4a` after every flushed fragment - a hard kill (or crash)
//! loses at most the last fragment (~1s) rather than the whole recording. This is
//! the equivalent of the original macOS implementation's incrementally-written
//! `AVAudioFile`.
//!
//! ffmpeg is an *external* runtime dependency invoked as a separate process; it is
//! not linked into or redistributed with nastro, so its own license (LGPL/GPL
//! depending on the build) does not attach to this binary. Set `NASTRO_FFMPEG` to
//! use a specific executable instead of the one on `PATH`.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to give ffmpeg to flush its final fragment and exit after stdin is
/// closed, before force-killing it.
const FINISH_TIMEOUT: Duration = Duration::from_secs(10);

const AAC_BITRATE_KBPS: u32 = 64;
/// Fragment length in microseconds (1s): the upper bound on data lost to a hard kill.
const FRAGMENT_DURATION_US: u32 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub enum EncoderError {
    #[error(
        "ffmpeg was not found. Install it and make sure it is on PATH \
         (Windows: `winget install Gyan.FFmpeg`; Debian/Ubuntu: `sudo apt install ffmpeg`), \
         or set NASTRO_FFMPEG to its full path."
    )]
    FfmpegNotFound,
    #[error("failed to start ffmpeg: {0}")]
    Spawn(#[source] io::Error),
    #[error("ffmpeg stopped accepting audio (it may have crashed): {0}")]
    Write(#[source] io::Error),
}

/// Arguments for the ffmpeg process, excluding the executable itself. Pure so it
/// can be unit tested without spawning anything.
pub fn ffmpeg_args(output: &Path, sample_rate: u32) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        // Input: raw little-endian 16-bit mono PCM on stdin.
        "-f".into(),
        "s16le".into(),
        "-ar".into(),
        sample_rate.to_string(),
        "-ac".into(),
        "1".into(),
        "-i".into(),
        "pipe:0".into(),
        // Output: AAC in fragmented MP4.
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        format!("{AAC_BITRATE_KBPS}k"),
        "-f".into(),
        "mp4".into(),
        "-movflags".into(),
        "+empty_moov+default_base_moof+frag_keyframe".into(),
        "-frag_duration".into(),
        FRAGMENT_DURATION_US.to_string(),
        // Without this ffmpeg buffers output internally and nothing reaches the
        // disk until close, which would defeat the crash-safety fragments.
        "-flush_packets".into(),
        "1".into(),
        "-y".into(),
        output.to_string_lossy().into_owned(),
    ]
}

/// Convert `[-1, 1]` float samples to little-endian signed 16-bit PCM bytes.
pub fn f32_to_s16le(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let pcm = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        bytes.extend_from_slice(&pcm.to_le_bytes());
    }
    bytes
}

pub struct M4aEncoder {
    child: Child,
    stdin: Option<ChildStdin>,
    sample_rate: u32,
    frames_written: u64,
}

impl M4aEncoder {
    pub fn start(output: &Path, sample_rate: u32) -> Result<Self, EncoderError> {
        let program = std::env::var_os("NASTRO_FFMPEG").unwrap_or_else(|| "ffmpeg".into());

        let mut command = Command::new(program);
        command
            .args(ffmpeg_args(output, sample_rate))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());

        // Keep ffmpeg out of our console process group, so the graceful-stop signal
        // aimed at nastro-tap (CTRL_BREAK on Windows, a terminal Ctrl+C / SIGINT on
        // Unix) doesn't also reach ffmpeg and make it shut down on its own schedule.
        // Shutdown ordering then stays under our control: we stop capture, then close
        // stdin, then ffmpeg finalizes. (Precautionary: on Windows a negative-control
        // run showed ffmpeg also finalizes cleanly when it does receive CTRL_BREAK, so
        // this is hygiene rather than a fix for an observed failure.)
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            command.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                EncoderError::FfmpegNotFound
            } else {
                EncoderError::Spawn(e)
            }
        })?;

        let stdin = child.stdin.take();
        Ok(Self {
            child,
            stdin,
            sample_rate,
            frames_written: 0,
        })
    }

    pub fn write_samples(&mut self, samples: &[f32]) -> Result<(), EncoderError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(EncoderError::Write(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "encoder already finished",
            )));
        };
        stdin
            .write_all(&f32_to_s16le(samples))
            .map_err(EncoderError::Write)?;
        self.frames_written += samples.len() as u64;
        Ok(())
    }

    /// Duration of audio handed to the encoder so far.
    pub fn duration_seconds(&self) -> f64 {
        self.frames_written as f64 / self.sample_rate as f64
    }

    /// Close the pipe so ffmpeg flushes its final fragment, then wait for it to
    /// exit (force-killing after [`FINISH_TIMEOUT`]).
    pub fn finish(mut self) -> io::Result<()> {
        self.finish_inner()
    }

    fn finish_inner(&mut self) -> io::Result<()> {
        // Dropping stdin sends EOF, which is ffmpeg's cue to finalize.
        self.stdin.take();

        let deadline = Instant::now() + FINISH_TIMEOUT;
        loop {
            match self.child.try_wait()? {
                Some(_) => return Ok(()),
                None if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "ffmpeg did not exit after stdin was closed",
                    ));
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

impl Drop for M4aEncoder {
    fn drop(&mut self) {
        // Best effort so an early return never leaves an orphaned ffmpeg running.
        if self.stdin.is_some() {
            let _ = self.finish_inner();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn args_describe_s16le_mono_input_on_stdin() {
        let args = ffmpeg_args(&PathBuf::from("out.m4a"), 48_000);
        let joined = args.join(" ");
        assert!(joined.contains("-f s16le -ar 48000 -ac 1 -i pipe:0"));
    }

    #[test]
    fn args_request_aac_in_fragmented_mp4() {
        let joined = ffmpeg_args(&PathBuf::from("out.m4a"), 48_000).join(" ");
        assert!(joined.contains("-c:a aac"));
        assert!(joined.contains("-f mp4"));
        assert!(joined.contains("+empty_moov+default_base_moof+frag_keyframe"));
        assert!(joined.contains("-frag_duration 1000000"));
    }

    #[test]
    fn args_flush_packets_so_fragments_reach_disk_promptly() {
        let joined = ffmpeg_args(&PathBuf::from("out.m4a"), 48_000).join(" ");
        assert!(joined.contains("-flush_packets 1"));
    }

    #[test]
    fn args_end_with_overwrite_flag_and_output_path() {
        let args = ffmpeg_args(&PathBuf::from("out.m4a"), 48_000);
        assert_eq!(&args[args.len() - 2..], ["-y", "out.m4a"]);
    }

    #[test]
    fn args_use_the_given_sample_rate() {
        let joined = ffmpeg_args(&PathBuf::from("o.m4a"), 44_100).join(" ");
        assert!(joined.contains("-ar 44100"));
    }

    #[test]
    fn f32_to_s16le_maps_full_scale_and_silence() {
        let bytes = f32_to_s16le(&[0.0, 1.0, -1.0]);
        assert_eq!(bytes.len(), 6);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 0);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[4], bytes[5]]), -i16::MAX);
    }

    #[test]
    fn f32_to_s16le_clamps_out_of_range_input() {
        let bytes = f32_to_s16le(&[5.0, -5.0]);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), -i16::MAX);
    }

    #[test]
    fn f32_to_s16le_empty_input_is_empty() {
        assert!(f32_to_s16le(&[]).is_empty());
    }

    fn ffmpeg_available() -> bool {
        Command::new(std::env::var_os("NASTRO_FFMPEG").unwrap_or_else(|| "ffmpeg".into()))
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// End-to-end against a real ffmpeg. Skipped (with a note) where ffmpeg isn't
    /// installed, so the suite still passes on machines without it - unless
    /// `NASTRO_REQUIRE_FFMPEG` is set (CI does), in which case a missing ffmpeg is a
    /// failure, so the test can never silently skip where it is meant to run.
    #[test]
    fn encodes_a_real_playable_m4a_when_ffmpeg_is_available() {
        if !ffmpeg_available() {
            assert!(
                std::env::var_os("NASTRO_REQUIRE_FFMPEG").is_none(),
                "NASTRO_REQUIRE_FFMPEG is set but ffmpeg is not available"
            );
            eprintln!("skipping: ffmpeg not available on this machine");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.m4a");

        let mut enc = M4aEncoder::start(&path, 48_000).expect("ffmpeg should start");
        let tone: Vec<f32> = (0..48_000)
            .map(|i| 0.25 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        enc.write_samples(&tone).unwrap();
        assert!((enc.duration_seconds() - 1.0).abs() < 1e-9);
        enc.finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 1_000, "expected real encoded data");
        assert_eq!(&bytes[4..8], b"ftyp", "should be an MP4-family container");
        assert!(
            bytes.windows(4).any(|w| w == b"moov"),
            "should contain a moov box"
        );
        assert!(
            bytes.windows(4).any(|w| w == b"moof"),
            "should be fragmented (moof boxes)"
        );
    }

    #[test]
    fn missing_ffmpeg_reports_an_actionable_error() {
        // Confirms the OS-level "not found" kind that `M4aEncoder::start` maps to
        // `FfmpegNotFound`, and that the user-facing message carries the install
        // hints. (No env var is mutated, so this is safe alongside parallel tests.)
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("definitely-not-ffmpeg");
        let result = {
            let mut command = Command::new(&bogus);
            command.stdin(Stdio::piped());
            command.spawn()
        };
        let err = result.expect_err("spawn of a missing binary must fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        let msg = EncoderError::FfmpegNotFound.to_string();
        assert!(msg.contains("winget install"));
        assert!(msg.contains("apt install ffmpeg"));
        assert!(msg.contains("NASTRO_FFMPEG"));
    }
}
