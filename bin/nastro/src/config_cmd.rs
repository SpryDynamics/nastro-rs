//! `nastro config`: look at and change the config file without opening it in an editor.
//!
//! Exists so the Windows installer (and scripts) can store a setting or a Hugging Face token without editing TOML
//! themselves. The token is read from standard input, never from the command line, where other local users could see it.

use std::io::{IsTerminal, Read};
use std::path::Path;

use anyhow::{bail, Context};
use nastro_transcribe::hf::{self, HfAccess};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Print the path of the config file in use.
    Path,
    /// Set one value: hf_token, whisper_model, transcriber or lang. The token is read from standard input.
    Set {
        /// hf_token | whisper_model | transcriber | lang
        key: String,
        /// The new value. Not accepted for hf_token: pipe it in, e.g. `echo hf_... | nastro config set hf_token`.
        value: Option<String>,
        /// hf_token only: read the token from the first line of this file, and delete the file. For the installer,
        /// which cannot pipe into a program.
        #[arg(long, value_name = "PATH")]
        from_file: Option<std::path::PathBuf>,
    },
    /// Ask Hugging Face whether the configured token can use the speaker model.
    ///
    /// Exit status: 0 ready, 2 token rejected, 3 terms not yet accepted, 4 could not tell (offline).
    CheckHf,
}

fn target(explicit: Option<&Path>) -> anyhow::Result<std::path::PathBuf> {
    match explicit {
        Some(p) => Ok(p.to_path_buf()),
        None => Ok(nastro_config::default_config_path()?),
    }
}

pub fn run(explicit: Option<&Path>, cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Path => {
            outln!("{}", target(explicit)?.display());
            Ok(())
        }
        Cmd::Set {
            key,
            value,
            from_file,
        } => {
            let path = target(explicit)?;
            if from_file.is_some() && key != "hf_token" {
                bail!("--from-file is only for hf_token");
            }
            let value = if key == "hf_token" {
                if value.is_some() {
                    bail!("a token on the command line is visible to other users of this computer; pipe it in instead: echo hf_... | nastro config set hf_token");
                }
                let mut s = String::new();
                if let Some(file) = &from_file {
                    // The file is removed whether or not it can be used: it exists only to hand the token over.
                    let read =
                        std::fs::File::open(file).and_then(|f| f.take(4096).read_to_string(&mut s));
                    let _ = std::fs::remove_file(file);
                    read.context("reading the token file")?;
                } else {
                    if std::io::stdin().is_terminal() {
                        bail!("pipe the token in on standard input: echo hf_... | nastro config set hf_token");
                    }
                    std::io::stdin()
                        .take(4096)
                        .read_to_string(&mut s)
                        .context("reading the token from standard input")?;
                }
                s.lines().next().unwrap_or("").trim().to_string()
            } else {
                value.context("give the new value: nastro config set <key> <value>")?
            };
            nastro_config::set_value(&path, &key, &value)?;
            // Never print the value of a credential.
            if key == "hf_token" {
                outln!("Saved hf_token in {}", path.display());
            } else {
                outln!("Saved {key} = {value} in {}", path.display());
            }
            Ok(())
        }
        Cmd::CheckHf => {
            let config = crate::load_config(explicit)?;
            let token = config.resolved_hf_token();
            if token.is_empty() {
                outln!("No Hugging Face token is set (hf_token in the config file, or HF_TOKEN).");
                std::process::exit(2);
            }
            match hf::check_access(&token) {
                HfAccess::Ready => {
                    outln!("Hugging Face accepts the token and the speaker model's terms are accepted.");
                    Ok(())
                }
                HfAccess::TokenRejected => {
                    outln!(
                        "Hugging Face does not accept this token. Create a new *read* token at {}",
                        hf::TOKENS_URL
                    );
                    std::process::exit(2);
                }
                HfAccess::TermsNotAccepted => {
                    outln!("The token is valid, but its account has not accepted the speaker model's terms yet.");
                    outln!("Open {} while signed in, and accept them.", hf::MODEL_URL);
                    std::process::exit(3);
                }
                HfAccess::Unknown(why) => {
                    outln!("Could not check with Hugging Face ({why}). Try again when this computer is online.");
                    std::process::exit(4);
                }
            }
        }
    }
}
