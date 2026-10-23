//! Locating, downloading and verifying whisper.cpp ggml models.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::model_table::{ModelInfo, MODELS, REPO_COMMIT};
use crate::{CancelToken, Error, Result};

/// Where downloaded models live: `$NASTRO_MODEL_DIR`, else the platform cache dir's `whisper` folder
/// (`~/.cache/whisper` on Linux - the same place whisper.cpp's own tooling uses - and
/// `%LOCALAPPDATA%\whisper` on Windows).
pub fn model_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NASTRO_MODEL_DIR") {
        return PathBuf::from(dir);
    }
    directories::BaseDirs::new()
        .map(|b| b.cache_dir().join("whisper"))
        .unwrap_or_else(|| PathBuf::from("whisper-models"))
}

/// Looks a model up by its config name (`large-v3-turbo`, `base.en-q5_1`, ...).
pub fn find(name: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.name == name)
}

/// `true` when the `whisper_model` setting is a file path rather than a model name.
pub fn is_path(spec: &str) -> bool {
    spec.contains('/') || spec.contains('\\') || spec.to_ascii_lowercase().ends_with(".bin")
}

/// Where `spec` (a model name or a path) is expected on disk.
pub fn path_for(spec: &str) -> PathBuf {
    if is_path(spec) {
        PathBuf::from(spec)
    } else {
        model_dir().join(format!("ggml-{spec}.bin"))
    }
}

/// The pinned download URL of a known model.
pub fn download_url(info: &ModelInfo) -> String {
    format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/{REPO_COMMIT}/ggml-{}.bin",
        info.name
    )
}

/// `true` if the model is present and (for a known model) has exactly the expected size. A cheap check that
/// catches a truncated file without re-hashing gigabytes on every run; the hash is verified at download time.
pub fn is_installed(spec: &str) -> bool {
    let path = path_for(spec);
    match (fs::metadata(&path), find(spec)) {
        (Ok(m), Some(info)) if !is_path(spec) => m.is_file() && m.len() == info.size,
        (Ok(m), _) => m.is_file() && m.len() > 0,
        _ => false,
    }
}

/// Returns the path of the model, downloading and verifying it first if needed.
///
/// `on_progress(downloaded, total)` is called while downloading. Only models in the pinned table are ever
/// downloaded (always over HTTPS and always hash-verified); anything else must be a path to a file the user
/// supplied.
pub fn ensure(
    spec: &str,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf> {
    let path = path_for(spec);

    if is_path(spec) {
        return if path.is_file() {
            Ok(path)
        } else {
            Err(Error::ModelMissing {
                name: spec.to_string(),
                path,
            })
        };
    }

    let info = find(spec).ok_or_else(|| Error::UnknownModel(spec.to_string()))?;
    if is_installed(spec) {
        return Ok(path);
    }

    download_verified(
        &download_url(info),
        &path,
        info.size,
        info.sha256,
        cancel,
        on_progress,
    )?;
    Ok(path)
}

/// Removes the partial file unless disarmed, so no failure path leaves a half-downloaded model behind.
struct PartGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for PartGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

/// Streams `url` into `dest`, checking size and SHA-256 *before* the file appears under its final name.
///
/// The bytes go to `<dest>.part` and are renamed into place only once verified, so `dest` is never a
/// half-written or unverified file, and an existing good `dest` is never replaced by a bad download.
pub(crate) fn download_verified(
    url: &str,
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;

    let mut part_name = dest.file_name().unwrap_or_default().to_os_string();
    part_name.push(".part");
    let part = dest.with_file_name(part_name);

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build()
        .into();

    let response = agent
        .get(url)
        .call()
        .map_err(|e| Error::Download(format!("{url}: {e}")))?;

    let total = response
        .body()
        .content_length()
        .filter(|&n| n > 0)
        .unwrap_or(expected_size);

    // Never accept more than the pinned size: a misbehaving server cannot fill the disk.
    let mut reader = response
        .into_body()
        .into_with_config()
        .limit(expected_size.saturating_add(1))
        .reader();

    let mut guard = PartGuard {
        path: &part,
        armed: true,
    };
    let mut out =
        File::create(&part).map_err(|e| Error::io(format!("creating {}", part.display()), e))?;

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut reported: u64 = 0;
    on_progress(0, total);

    loop {
        cancel.check()?;
        let n = reader
            .read(&mut buf)
            .map_err(|e| Error::Download(format!("while reading {url}: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])
            .map_err(|e| Error::io(format!("writing {}", part.display()), e))?;
        downloaded += n as u64;
        // ~1 MiB granularity keeps callbacks (and a UI redrawing on them) cheap.
        if downloaded - reported >= 1 << 20 {
            reported = downloaded;
            on_progress(downloaded, total);
        }
    }
    out.flush()
        .and_then(|_| out.sync_all())
        .map_err(|e| Error::io(format!("writing {}", part.display()), e))?;
    drop(out);

    if downloaded != expected_size {
        return Err(Error::Download(format!(
            "{url}: received {downloaded} bytes, expected {expected_size}"
        )));
    }
    let actual = hex(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(Error::Checksum {
            expected: expected_sha256.to_string(),
            actual,
        });
    }

    fs::rename(&part, dest).map_err(|e| {
        Error::io(
            format!("moving the model into place at {}", dest.display()),
            e,
        )
    })?;
    guard.armed = false;
    on_progress(downloaded, total);
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// A one-shot HTTP server on localhost returning `body` (optionally with a wrong Content-Length).
    fn serve_once(body: Vec<u8>, status: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                    line.clear();
                }
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{addr}/model.bin")
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    #[test]
    fn known_models_have_well_formed_pins() {
        assert!(MODELS.len() >= 30);
        for m in MODELS {
            assert_eq!(m.sha256.len(), 64, "{}", m.name);
            assert!(
                m.sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "{}",
                m.name
            );
            assert!(m.size > 1_000_000, "{}", m.name);
        }
        let mut names: Vec<_> = MODELS.iter().map(|m| m.name).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate model names");
    }

    #[test]
    fn the_default_model_is_in_the_table() {
        assert!(find(nastro_config::DEFAULT_WHISPER_MODEL).is_some());
    }

    #[test]
    fn urls_are_https_and_pinned_to_the_commit() {
        let url = download_url(find("tiny.en").unwrap());
        assert!(url.starts_with("https://huggingface.co/"));
        assert!(url.contains(REPO_COMMIT));
        assert!(url.ends_with("/ggml-tiny.en.bin"));
    }

    #[test]
    fn paths_and_names_are_told_apart() {
        assert!(is_path(r"C:\models\x.bin"));
        assert!(is_path("/opt/models/x"));
        assert!(is_path("custom.BIN"));
        assert!(!is_path("large-v3-turbo"));
        assert!(!is_path("base.en-q5_1"));
    }

    #[test]
    fn unknown_model_names_are_rejected_without_touching_the_network() {
        let err = ensure("../../etc/passwd", &CancelToken::new(), &mut |_, _| {}).unwrap_err();
        // Looks like a path, so it is treated as one - and does not exist.
        assert!(matches!(err, Error::ModelMissing { .. }));
        let err = ensure("not-a-model", &CancelToken::new(), &mut |_, _| {}).unwrap_err();
        assert!(matches!(err, Error::UnknownModel(_)));
    }

    #[test]
    fn an_explicit_model_file_is_used_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("mine.bin");
        fs::write(&f, b"not really a model").unwrap();
        let got = ensure(f.to_str().unwrap(), &CancelToken::new(), &mut |_, _| {}).unwrap();
        assert_eq!(got, f);
    }

    #[test]
    fn download_verifies_and_installs_atomically() {
        let body = vec![7u8; 300_000];
        let url = serve_once(body.clone(), "200 OK");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("sub").join("ggml-x.bin");
        let mut ticks = Vec::new();

        download_verified(
            &url,
            &dest,
            body.len() as u64,
            &sha(&body),
            &CancelToken::new(),
            &mut |d, t| ticks.push((d, t)),
        )
        .unwrap();

        assert_eq!(fs::read(&dest).unwrap(), body);
        assert!(!dir.path().join("sub").join("ggml-x.bin.part").exists());
        assert_eq!(ticks.first().unwrap().0, 0);
        assert_eq!(*ticks.last().unwrap(), (300_000, 300_000));
    }

    #[test]
    fn a_wrong_hash_is_rejected_and_leaves_nothing_behind() {
        let body = vec![1u8; 10_000];
        let url = serve_once(body.clone(), "200 OK");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");

        let err = download_verified(
            &url,
            &dest,
            body.len() as u64,
            &sha(b"something else"),
            &CancelToken::new(),
            &mut |_, _| {},
        )
        .unwrap_err();

        assert!(matches!(err, Error::Checksum { .. }), "{err}");
        assert!(!dest.exists());
        assert!(!dir.path().join("m.bin.part").exists());
    }

    #[test]
    fn a_bad_download_never_replaces_an_existing_good_model() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        fs::write(&dest, b"good existing model").unwrap();
        let body = vec![1u8; 5_000];
        let url = serve_once(body.clone(), "200 OK");

        let r = download_verified(
            &url,
            &dest,
            body.len() as u64,
            &sha(b"x"),
            &CancelToken::new(),
            &mut |_, _| {},
        );

        assert!(r.is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"good existing model");
    }

    #[test]
    fn a_truncated_or_oversized_body_is_rejected() {
        let body = vec![2u8; 4_000];
        for claimed in [body.len() as u64 + 100, body.len() as u64 - 100] {
            let url = serve_once(body.clone(), "200 OK");
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("m.bin");
            let err = download_verified(
                &url,
                &dest,
                claimed,
                &sha(&body),
                &CancelToken::new(),
                &mut |_, _| {},
            )
            .unwrap_err();
            assert!(
                matches!(err, Error::Download(_)),
                "claimed {claimed}: {err}"
            );
            assert!(!dest.exists());
            assert!(!dir.path().join("m.bin.part").exists());
        }
    }

    #[test]
    fn http_errors_are_reported_not_installed() {
        let url = serve_once(b"nope".to_vec(), "404 Not Found");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let err = download_verified(
            &url,
            &dest,
            4,
            &sha(b"nope"),
            &CancelToken::new(),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(matches!(err, Error::Download(_)), "{err}");
        assert!(!dest.exists());
    }

    #[test]
    fn cancellation_stops_the_download_and_cleans_up() {
        let body = vec![3u8; 2_000_000];
        let url = serve_once(body.clone(), "200 OK");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let cancel = CancelToken::new();
        let c2 = cancel.clone();

        let err = download_verified(
            &url,
            &dest,
            body.len() as u64,
            &sha(&body),
            &cancel,
            &mut |d, _| {
                if d > 0 {
                    c2.cancel();
                }
            },
        )
        .unwrap_err();

        assert!(matches!(err, Error::Cancelled), "{err}");
        assert!(!dest.exists());
        assert!(!dir.path().join("m.bin.part").exists());
    }

    #[test]
    fn a_size_mismatch_means_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("NASTRO_MODEL_DIR", dir.path());
        fs::write(dir.path().join("ggml-tiny.en.bin"), b"truncated").unwrap();
        assert!(!is_installed("tiny.en"));
        std::env::remove_var("NASTRO_MODEL_DIR");
    }
}
