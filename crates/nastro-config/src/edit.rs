//! Changing one setting in the config file without disturbing the rest of it.
//!
//! Used by `nastro config set` (and through it by the Windows installer). The file is edited with `toml_edit`, so the
//! user's comments, ordering and other settings survive. Values are validated first, and error messages never repeat
//! a value (it may be a credential) or any text from the existing file.

use std::path::{Path, PathBuf};

/// Keys `nastro config set` accepts.
pub const SETTABLE_KEYS: &[&str] = &["hf_token", "whisper_model", "transcriber", "lang"];

#[derive(Debug, thiserror::Error)]
pub enum SetError {
    #[error("`{0}` cannot be set this way (settable: hf_token, whisper_model, transcriber, lang)")]
    UnknownKey(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the config file {path} is not valid TOML (line {line}); fix or remove it first")]
    ParseExisting { path: PathBuf, line: usize },
    #[error("could not read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not determine the user's config directory")]
    NoConfigDir,
}

/// `%APPDATA%\nastro\config.toml` on Windows, `~/.config/nastro/config.toml` on Linux.
pub fn default_config_path() -> Result<PathBuf, SetError> {
    let dirs = directories::BaseDirs::new().ok_or(SetError::NoConfigDir)?;
    Ok(dirs.config_dir().join("nastro").join("config.toml"))
}

fn validate(key: &str, value: &str) -> Result<(), SetError> {
    let bad = |m: &str| Err(SetError::Invalid(m.to_string()));
    match key {
        "hf_token" => {
            // Hugging Face tokens are `hf_` plus letters and digits. Anything else is a paste mistake.
            if !value.starts_with("hf_")
                || value.len() < 10
                || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return bad("that does not look like a Hugging Face token (they look like hf_ followed by letters and digits)");
            }
            Ok(())
        }
        "whisper_model" | "transcriber" | "lang" => {
            if value.is_empty()
                || value.len() > 64
                || !value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            {
                return bad(&format!(
                    "`{key}` must be a short plain name (letters, digits, '.', '-', '_')"
                ));
            }
            if key == "transcriber" && !matches!(value, "whisper" | "whisper-cli" | "whisperx") {
                return bad("`transcriber` must be whisper or whisperx");
            }
            Ok(())
        }
        other => Err(SetError::UnknownKey(other.to_string())),
    }
}

/// Sets `key = "value"` in the TOML file at `path`, creating the file (and its folder) if needed. Everything else in
/// the file is left as it was. The write goes through a temporary file and a rename, so a crash cannot leave a
/// half-written config behind.
pub fn set_value(path: &Path, key: &str, value: &str) -> Result<(), SetError> {
    validate(key, value)?;
    let io = |source| SetError::Io {
        path: path.to_path_buf(),
        source,
    };

    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io(e)),
    };
    let mut doc: toml_edit::DocumentMut = existing.parse().map_err(|e: toml_edit::TomlError| {
        let line = e
            .span()
            .map(|s| {
                existing[..s.start.min(existing.len())]
                    .matches('\n')
                    .count()
                    + 1
            })
            .unwrap_or(1);
        SetError::ParseExisting {
            path: path.to_path_buf(),
            line,
        }
    })?;
    doc[key] = toml_edit::value(value);

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).map_err(io)?;
    // The file may hold a credential: keep it to the current user where the platform has mode bits.
    let _ = crate::mode::ensure_private_file(&tmp);
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "hf_AbCdEfGhIjKlMnOpQrStUv123456";

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn creates_missing_file_and_folder() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nastro").join("config.toml");
        set_value(&p, "hf_token", TOKEN).unwrap();
        assert_eq!(read(&p), format!("hf_token = \"{TOKEN}\"\n"));
    }

    #[test]
    fn keeps_comments_order_and_other_settings() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        std::fs::write(
            &p,
            "# my recordings\noutput_dir = \"D:/calls\"\nlang = 'en'   # english only\nmax_parallel_transcriptions = 3\n",
        )
        .unwrap();
        set_value(&p, "whisper_model", "small").unwrap();
        let out = read(&p);
        assert!(out.contains("# my recordings\n"));
        assert!(out.contains("output_dir = \"D:/calls\"\n"));
        assert!(out.contains("lang = 'en'   # english only\n"));
        assert!(out.contains("max_parallel_transcriptions = 3\n"));
        assert!(out.contains("whisper_model = \"small\""));
        // and it still loads, with the new value taking effect
        let cfg = crate::Config::load_from(&p, Path::new("/h")).unwrap();
        assert_eq!(cfg.whisper_model, "small");
        assert_eq!(cfg.lang, "en");
    }

    #[test]
    fn replaces_an_existing_value_instead_of_duplicating_it() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        std::fs::write(&p, "hf_token = \"hf_oldoldoldold\"\nlang = \"en\"\n").unwrap();
        set_value(&p, "hf_token", TOKEN).unwrap();
        let out = read(&p);
        assert_eq!(out.matches("hf_token").count(), 1);
        assert!(!out.contains("oldold"));
        assert!(out.contains(TOKEN));
    }

    #[test]
    fn rejects_bad_values_and_unknown_keys_without_echoing_them() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        for (k, v) in [
            ("hf_token", "not-a-token"),
            ("hf_token", "hf_short"),
            ("hf_token", "hf_has space in it 12345"),
            ("hf_token", "hf_abc\"\nmax = 1 12345"),
            ("whisper_model", ""),
            ("whisper_model", "a b"),
            ("whisper_model", "C:\\x.bin;rm"),
            ("transcriber", "whisperz"),
        ] {
            let e = set_value(&p, k, v).unwrap_err().to_string();
            assert!(
                !e.contains(v) || v.is_empty(),
                "{k}: error echoed the value: {e}"
            );
        }
        assert!(matches!(
            set_value(&p, "output_dir", "x"),
            Err(SetError::UnknownKey(_))
        ));
        assert!(!p.exists(), "a rejected value must not create the file");
    }

    #[test]
    fn invalid_existing_file_is_left_untouched_and_not_quoted() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        let broken = "hf_token = \"hf_secretsecretsecret1\"\nthis is = = broken\n";
        std::fs::write(&p, broken).unwrap();
        let e = set_value(&p, "lang", "en").unwrap_err().to_string();
        assert!(e.contains("line 2"), "{e}");
        assert!(!e.contains("secretsecret"), "{e}");
        assert_eq!(read(&p), broken);
    }

    #[test]
    fn leaves_no_temp_file_behind() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        set_value(&p, "lang", "en").unwrap();
        set_value(&p, "lang", "de").unwrap();
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["config.toml"]);
    }
}
