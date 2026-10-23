//! Minimal incremental WAV (16-bit PCM) writer.
//!
//! **Interim format** - the roadmap's Phase 4 replaces this with incremental
//! AAC/M4A muxing via an `ffmpeg` subprocess (fragmented MP4, for true
//! crash-safety: a valid file after every flushed fragment even on `kill -9`).
//! Until then, this gives real, playable recordings today: samples are appended
//! as they're mixed, and the RIFF header is rewritten with up-to-date sizes
//! periodically (see [`WavWriter::flush_header`]) rather than only once at close,
//! so a hard kill loses at most the last update interval rather than the whole
//! file being unplayable.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

pub struct WavWriter {
    file: File,
    sample_rate: u32,
    channels: u16,
    frames_written: u64,
}

impl WavWriter {
    pub fn create(path: &Path, sample_rate: u32, channels: u16) -> io::Result<Self> {
        let mut file = File::create(path)?;
        write_header(&mut file, sample_rate, channels, 0)?;
        Ok(Self {
            file,
            sample_rate,
            channels,
            frames_written: 0,
        })
    }

    /// Append mono `f32` samples in `[-1, 1]`, converting to 16-bit PCM.
    pub fn write_samples(&mut self, samples: &[f32]) -> io::Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let clamped = s.clamp(-1.0, 1.0);
            let pcm = (clamped * i16::MAX as f32) as i16;
            bytes.extend_from_slice(&pcm.to_le_bytes());
        }
        self.file.write_all(&bytes)?;
        self.frames_written += samples.len() as u64;
        Ok(())
    }

    pub fn duration_seconds(&self) -> f64 {
        self.frames_written as f64 / self.sample_rate as f64
    }

    /// Rewrite the RIFF header with current sizes, then seek back to the append
    /// position. Safe to call periodically while still writing.
    pub fn flush_header(&mut self) -> io::Result<()> {
        self.file.flush()?;
        let current_pos = self.file.stream_position()?;
        self.file.seek(SeekFrom::Start(0))?;
        write_header(
            &mut self.file,
            self.sample_rate,
            self.channels,
            self.frames_written,
        )?;
        self.file.seek(SeekFrom::Start(current_pos))?;
        Ok(())
    }

    /// Finalize: same as [`Self::flush_header`], named separately for call-site
    /// clarity at actual shutdown.
    pub fn finalize(mut self) -> io::Result<()> {
        self.flush_header()
    }
}

fn write_header(file: &mut File, sample_rate: u32, channels: u16, frames: u64) -> io::Result<()> {
    let bits_per_sample: u16 = 16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * block_align as u32;
    let data_size = frames * block_align as u64;
    let riff_size = 36 + data_size;

    file.write_all(b"RIFF")?;
    file.write_all(&(riff_size as u32).to_le_bytes())?;
    file.write_all(b"WAVE")?;

    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // fmt chunk size
    file.write_all(&1u16.to_le_bytes())?; // PCM
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&bits_per_sample.to_le_bytes())?;

    file.write_all(b"data")?;
    file.write_all(&(data_size as u32).to_le_bytes())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_minimal_wav_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let mut writer = WavWriter::create(&path, 48_000, 1).unwrap();
        writer.write_samples(&[0.0, 0.5, -0.5, 1.0, -1.0]).unwrap();
        writer.finalize().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(&bytes[36..40], b"data");

        let data_size = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        assert_eq!(data_size, 5 * 2); // 5 samples * 2 bytes (16-bit)
        assert_eq!(bytes.len(), 44 + data_size as usize);
    }

    #[test]
    fn duration_seconds_reflects_frames_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let mut writer = WavWriter::create(&path, 48_000, 1).unwrap();
        writer.write_samples(&vec![0.0; 48_000]).unwrap();

        assert!((writer.duration_seconds() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn flush_header_mid_write_leaves_a_valid_playable_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let mut writer = WavWriter::create(&path, 48_000, 1).unwrap();
        writer.write_samples(&[0.1, 0.2, 0.3]).unwrap();
        writer.flush_header().unwrap();

        // Simulate a crash right here: read the file back without ever calling
        // finalize() again, and confirm the header already reflects 3 frames.
        let bytes = std::fs::read(&path).unwrap();
        let data_size = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        assert_eq!(data_size, 3 * 2);
    }
}
