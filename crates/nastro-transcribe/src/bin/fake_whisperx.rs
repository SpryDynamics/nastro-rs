//! A stand-in for the `whisperx` command, used only by nastro-transcribe's tests (it is never shipped).
//!
//! It accepts the arguments nastro passes, writes a result file the way WhisperX does, and can misbehave on
//! request, so the integration around it can be tested without Python, PyTorch or a Hugging Face account.
//!
//! Behaviour is chosen with environment variables:
//!   FAKE_WX_MODE      ok (default) | hang | tree | gated | token | punkt | garbage | nooutput | fail
//!   FAKE_WX_ARGV_LOG  file to write the received arguments to (one per line)
//!   FAKE_WX_ENV_LOG   file to write `HF_TOKEN=<value>` to
//!   FAKE_WX_PIDFILE   (tree mode) file to write the child process's pid to

use std::io::Write;
use std::time::Duration;

fn say(line: &str) {
    // WhisperX logs to stderr.
    eprintln!("2026-10-23 20:26:37 - whisperx - INFO - {line}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = std::env::var("FAKE_WX_MODE").unwrap_or_else(|_| "ok".into());

    if let Ok(path) = std::env::var("FAKE_WX_ARGV_LOG") {
        let _ = std::fs::write(path, args.join("\n"));
    }
    if let Ok(path) = std::env::var("FAKE_WX_ENV_LOG") {
        let token = std::env::var("HF_TOKEN").unwrap_or_default();
        let _ = std::fs::write(path, format!("HF_TOKEN={token}"));
    }

    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let wav = args.first().cloned().unwrap_or_default();
    let out_dir = value_of("--output_dir").unwrap_or_else(|| ".".into());
    let diarize = args.iter().any(|a| a == "--diarize");

    // The noisy warning a real run prints every time. It must not be mistaken for an error.
    eprintln!("C:\\x\\pyannote\\audio\\core\\io.py:48: UserWarning: torchcodec is not installed correctly");

    match mode.as_str() {
        "hang" => {
            say("Performing transcription...");
            loop {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        "tree" => {
            // Start a descendant that outlives a plain kill of this process, and report its pid.
            let exe = std::env::current_exe().expect("current exe");
            // Deliberately never waited on: the point is a descendant that outlives a naive kill of this process.
            #[allow(clippy::zombie_processes)]
            let child = std::process::Command::new(exe)
                .env("FAKE_WX_MODE", "hang")
                .env_remove("FAKE_WX_PIDFILE")
                .spawn()
                .expect("spawn descendant");
            if let Ok(path) = std::env::var("FAKE_WX_PIDFILE") {
                let _ = std::fs::write(path, child.id().to_string());
            }
            say("Performing transcription...");
            loop {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        "gated" => {
            say("Performing diarization...");
            eprintln!("huggingface_hub.errors.GatedRepoError: 403 Client Error.");
            eprintln!("Access to model pyannote/speaker-diarization-community-1 is restricted. You must have access to it and be authenticated to access it.");
            std::process::exit(1);
        }
        "token" => {
            eprintln!("requests.exceptions.HTTPError: 401 Client Error: Unauthorized for url: https://huggingface.co/api/whoami-v2");
            eprintln!("Invalid user token.");
            std::process::exit(1);
        }
        "punkt" => {
            say("Performing alignment...");
            eprintln!("LookupError:");
            eprintln!("  Resource 'punkt_tab' not found.");
            std::process::exit(1);
        }
        "fail" => {
            eprintln!(
                "RuntimeError: something exploded, token was {}",
                std::env::var("HF_TOKEN").unwrap_or_default()
            );
            std::process::exit(3);
        }
        "nooutput" => {
            say("Performing transcription...");
            return;
        }
        "garbage" => {
            let stem = std::path::Path::new(&wav)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let _ = std::fs::write(
                std::path::Path::new(&out_dir).join(format!("{stem}.json")),
                "this is not json",
            );
            return;
        }
        _ => {}
    }

    say("Performing voice activity detection using Pyannote...");
    say("Performing transcription...");
    say("Performing alignment...");
    if diarize {
        say("Performing diarization...");
    }

    let who = |n: usize| {
        if diarize {
            format!(r#","speaker":"SPEAKER_0{n}""#)
        } else {
            String::new()
        }
    };
    let json = format!(
        r#"{{"language":"en","segments":[
  {{"start":0.15,"end":1.2,"text":" Good morning everyone."{a}}},
  {{"start":2.2,"end":5.4,"text":" Let's begin the meeting. Caf\u00e9 \u00f1 \u65e5\u672c\u8a9e"{a}}},
  {{"start":7.2,"end":8.6,"text":" Thanks."{b}}},
  {{"start":9.0,"end":12.0,"text":" The release is on track."{b}}}
],"word_segments":[]}}"#,
        a = who(0),
        b = who(1)
    );
    let stem = std::path::Path::new(&wav)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut f = std::fs::File::create(std::path::Path::new(&out_dir).join(format!("{stem}.json")))
        .expect("write result");
    f.write_all(json.as_bytes()).expect("write result");
}
