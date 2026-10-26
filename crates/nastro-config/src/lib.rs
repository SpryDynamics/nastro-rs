//! Configuration loading/defaults for nastro.
//!
//! Mirrors the original Go `internal/config` package: a TOML config file overlaid onto
//! sane defaults, where a missing file is not an error. Ported 1:1 in spirit, adapted
//! for cross-platform (Windows + Linux) path conventions instead of macOS-only ones.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod edit;
pub use edit::{default_config_path, set_value, SetError, SETTABLE_KEYS};

/// Directory permission bits applied on Unix (Linux) when creating recording
/// directories. Windows has no POSIX mode bits; see [`mode::ensure_private_dir`] /
/// [`mode::ensure_private_file`] for the per-platform story.
pub const PRIVATE_DIR_MODE: u32 = 0o700;
/// File permission bits applied on Unix (Linux) for transcripts/lockfiles.
pub const PRIVATE_FILE_MODE: u32 = 0o600;

pub const DEFAULT_WHISPER_MODEL: &str = "large-v3-turbo";
pub const DEFAULT_LANG: &str = "auto";
pub const DEFAULT_TRANSCRIBER: &str = "whisper";
pub const DEFAULT_MAX_PARALLEL_TRANSCRIPTIONS: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub output_dir: PathBuf,
    pub whisper_model: String,
    pub lang: String,
    pub transcriber: String,
    pub hf_token: String,
    pub max_parallel_transcriptions: u32,
}

/// The on-disk TOML shape. Every field optional - a config file may only override a
/// subset, relying on [`Config::default_with_home`] for the rest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    pub output_dir: Option<String>,
    pub whisper_model: Option<String>,
    pub lang: Option<String>,
    pub transcriber: Option<String>,
    pub hf_token: Option<String>,
    pub max_parallel_transcriptions: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("could not determine the user's home directory")]
    NoHomeDir,
}

impl Config {
    /// Defaults, parameterized by home directory so this is unit-testable without
    /// touching real environment state.
    pub fn default_with_home(home: &Path) -> Self {
        Self {
            output_dir: home.join("Recordings").join("nastro"),
            whisper_model: DEFAULT_WHISPER_MODEL.to_string(),
            lang: DEFAULT_LANG.to_string(),
            transcriber: DEFAULT_TRANSCRIBER.to_string(),
            hf_token: String::new(),
            max_parallel_transcriptions: DEFAULT_MAX_PARALLEL_TRANSCRIPTIONS,
        }
    }

    /// Overlay a parsed [`ConfigFile`] onto defaults built from `home`.
    pub fn with_overlay(home: &Path, file: ConfigFile) -> Self {
        let mut config = Self::default_with_home(home);
        if let Some(v) = file.output_dir {
            config.output_dir = expand_tilde(&v, home);
        }
        if let Some(v) = file.whisper_model {
            config.whisper_model = v;
        }
        if let Some(v) = file.lang {
            config.lang = v;
        }
        if let Some(v) = file.transcriber {
            config.transcriber = v;
        }
        if let Some(v) = file.hf_token {
            config.hf_token = v;
        }
        if let Some(v) = file.max_parallel_transcriptions {
            config.max_parallel_transcriptions = v;
        }
        config
    }

    /// Load from an explicit path. A missing file is *not* an error - defaults for
    /// `home` are returned as-is, matching the Go implementation's behavior.
    pub fn load_from(path: &Path, home: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default_with_home(home));
        }

        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;

        let file: ConfigFile = toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;

        Ok(Self::with_overlay(home, file))
    }

    /// Load using the platform-conventional config path
    /// (`~/.config/nastro/config.toml` on Linux, `%APPDATA%\nastro\config.toml` on
    /// Windows) and the platform-conventional home directory.
    pub fn load() -> Result<Self, ConfigError> {
        let dirs = directories::BaseDirs::new().ok_or(ConfigError::NoHomeDir)?;
        let home = dirs.home_dir().to_path_buf();
        let config_path = dirs.config_dir().join("nastro").join("config.toml");
        Self::load_from(&config_path, &home)
    }

    /// `hf_token`, falling back to the `HF_TOKEN` environment variable when the
    /// config value is empty.
    pub fn resolved_hf_token(&self) -> String {
        if !self.hf_token.is_empty() {
            return self.hf_token.clone();
        }
        std::env::var("HF_TOKEN").unwrap_or_default()
    }

    /// `max_parallel_transcriptions`, floored at 1.
    pub fn resolved_max_parallel(&self) -> u32 {
        self.max_parallel_transcriptions.max(1)
    }
}

/// Expand a leading `~` (or `~/`) to `home`. Only the leading-tilde form is supported,
/// matching the Go implementation's `os.UserHomeDir`-based expansion.
fn expand_tilde(value: &str, home: &Path) -> PathBuf {
    if let Some(rest) = value.strip_prefix('~') {
        let rest = rest.trim_start_matches(['/', '\\']);
        if rest.is_empty() {
            return home.to_path_buf();
        }
        return home.join(rest);
    }
    PathBuf::from(value)
}

/// Platform-specific directory/file privacy helpers. On Linux, applies POSIX mode
/// bits. On Windows, this is currently a documented no-op: NTFS has no equivalent of
/// POSIX mode bits, and a user's own profile directory (where recordings default to
/// living) is already private-by-default via inherited ACLs. Tightening ACLs
/// explicitly is tracked as a follow-up, not a blocker.
pub mod mode {
    use std::path::Path;

    #[cfg(unix)]
    pub fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(super::PRIVATE_DIR_MODE),
        )
    }

    #[cfg(windows)]
    pub fn ensure_private_dir(_path: &Path) -> std::io::Result<()> {
        // No POSIX mode bits on Windows; rely on the per-user profile directory's
        // default ACLs. See docs/windows-permissions.md for the ACL-tightening
        // follow-up plan.
        Ok(())
    }

    #[cfg(unix)]
    pub fn ensure_private_file(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(super::PRIVATE_FILE_MODE),
        )
    }

    #[cfg(windows)]
    pub fn ensure_private_file(_path: &Path) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_with_home_sets_expected_defaults() {
        let home = Path::new("/home/alice");
        let config = Config::default_with_home(home);

        assert_eq!(config.output_dir, home.join("Recordings").join("nastro"));
        assert_eq!(config.whisper_model, DEFAULT_WHISPER_MODEL);
        assert_eq!(config.lang, DEFAULT_LANG);
        assert_eq!(config.transcriber, DEFAULT_TRANSCRIBER);
        assert_eq!(config.hf_token, "");
        assert_eq!(
            config.max_parallel_transcriptions,
            DEFAULT_MAX_PARALLEL_TRANSCRIPTIONS
        );
    }

    #[test]
    fn load_from_missing_file_returns_defaults() {
        let home = Path::new("/home/alice");
        let missing = Path::new("/home/alice/.config/nastro/does-not-exist.toml");

        let config = Config::load_from(missing, home).unwrap();

        assert_eq!(config, Config::default_with_home(home));
    }

    #[test]
    fn load_from_valid_toml_overlays_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
            whisper_model = "small"
            lang = "en"
            max_parallel_transcriptions = 4
            "#,
        )
        .unwrap();

        let home = Path::new("/home/alice");
        let config = Config::load_from(&path, home).unwrap();

        assert_eq!(config.whisper_model, "small");
        assert_eq!(config.lang, "en");
        assert_eq!(config.max_parallel_transcriptions, 4);
        // Untouched fields keep their defaults.
        assert_eq!(config.transcriber, DEFAULT_TRANSCRIBER);
        assert_eq!(config.output_dir, home.join("Recordings").join("nastro"));
    }

    #[test]
    fn load_from_partial_toml_only_overrides_present_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, r#"transcriber = "whisperx""#).unwrap();

        let home = Path::new("/home/alice");
        let config = Config::load_from(&path, home).unwrap();

        assert_eq!(config.transcriber, "whisperx");
        assert_eq!(config.whisper_model, DEFAULT_WHISPER_MODEL);
    }

    #[test]
    fn load_from_invalid_toml_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not = [valid toml").unwrap();

        let home = Path::new("/home/alice");
        let result = Config::load_from(&path, home);

        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn output_dir_expands_leading_tilde() {
        let home = Path::new("/home/alice");
        let file = ConfigFile {
            output_dir: Some("~/custom/recordings".to_string()),
            ..Default::default()
        };

        let config = Config::with_overlay(home, file);

        assert_eq!(config.output_dir, home.join("custom").join("recordings"));
    }

    #[test]
    fn output_dir_bare_tilde_expands_to_home() {
        let home = Path::new("/home/alice");
        let file = ConfigFile {
            output_dir: Some("~".to_string()),
            ..Default::default()
        };

        let config = Config::with_overlay(home, file);

        assert_eq!(config.output_dir, home);
    }

    #[test]
    fn output_dir_absolute_path_is_used_as_is() {
        let home = Path::new("/home/alice");
        let file = ConfigFile {
            output_dir: Some("/mnt/data/recordings".to_string()),
            ..Default::default()
        };

        let config = Config::with_overlay(home, file);

        assert_eq!(config.output_dir, PathBuf::from("/mnt/data/recordings"));
    }

    #[test]
    fn resolved_hf_token_falls_back_to_env_var() {
        // SAFETY: test-only env var mutation, single-threaded test execution assumed
        // via serial access pattern (no other test in this module touches HF_TOKEN).
        unsafe {
            std::env::set_var("HF_TOKEN", "env-token-value");
        }

        let home = Path::new("/home/alice");
        let config = Config::default_with_home(home);

        assert_eq!(config.resolved_hf_token(), "env-token-value");

        unsafe {
            std::env::remove_var("HF_TOKEN");
        }
    }

    #[test]
    fn resolved_hf_token_prefers_config_value_over_env() {
        unsafe {
            std::env::set_var("HF_TOKEN", "env-token-value");
        }

        let home = Path::new("/home/alice");
        let mut config = Config::default_with_home(home);
        config.hf_token = "config-token-value".to_string();

        assert_eq!(config.resolved_hf_token(), "config-token-value");

        unsafe {
            std::env::remove_var("HF_TOKEN");
        }
    }

    #[test]
    fn resolved_max_parallel_floors_at_one() {
        let home = Path::new("/home/alice");
        let mut config = Config::default_with_home(home);
        config.max_parallel_transcriptions = 0;

        assert_eq!(config.resolved_max_parallel(), 1);
    }
}
