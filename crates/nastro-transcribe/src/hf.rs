//! Checking a Hugging Face token against the speaker-identification model, before a long run depends on it.
//!
//! WhisperX's speaker models are gated: a valid token is not enough, the account must also have accepted the model's
//! terms. Hugging Face answers a request for one of the model's files with 401 for a missing or rejected token and
//! 403 for a valid token whose account has not accepted the terms, which is exactly the distinction worth making.

use std::time::Duration;

/// A small file in the gated repository; the answer is the same for every file in it.
const GATED_FILE: &str = "/pyannote/speaker-diarization-community-1/resolve/main/config.yaml";
const ENDPOINT: &str = "https://huggingface.co";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HfAccess {
    /// The token works and the model terms are accepted.
    Ready,
    /// 401: Hugging Face does not accept this token.
    TokenRejected,
    /// 403: the token is fine, but the account has not accepted the model's terms.
    TermsNotAccepted,
    /// Could not find out (offline, unexpected answer). The text never contains the token.
    Unknown(String),
}

pub const MODEL_URL: &str = "https://huggingface.co/pyannote/speaker-diarization-community-1";
pub const TOKENS_URL: &str = "https://huggingface.co/settings/tokens";

/// Asks Hugging Face whether `token` may use the speaker model.
pub fn check_access(token: &str) -> HfAccess {
    check_access_at(ENDPOINT, token)
}

pub(crate) fn check_access_at(endpoint: &str, token: &str) -> HfAccess {
    // No redirects: the real file answers with a redirect to a CDN once access is granted, and the token must not be
    // forwarded to another host. Any 3xx therefore already means "allowed".
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let result = agent
        .head(format!("{endpoint}{GATED_FILE}"))
        .header("Authorization", format!("Bearer {token}"))
        .call();
    match result {
        Ok(r) => match r.status().as_u16() {
            200..=399 => HfAccess::Ready,
            401 => HfAccess::TokenRejected,
            403 => HfAccess::TermsNotAccepted,
            other => HfAccess::Unknown(format!("Hugging Face answered with HTTP {other}")),
        },
        // ureq's messages describe the connection, not our headers, but be certain.
        Err(e) => HfAccess::Unknown(e.to_string().replace(token, "<token>")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A one-shot HTTP server answering with `status`; returns its URL and the request it received.
    fn serve(status: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (url, h)
    }

    #[test]
    fn maps_each_answer_and_sends_the_token_only_as_a_header() {
        for (status, want) in [
            ("200 OK", HfAccess::Ready),
            ("302 Found", HfAccess::Ready),
            ("401 Unauthorized", HfAccess::TokenRejected),
            ("403 Forbidden", HfAccess::TermsNotAccepted),
        ] {
            let (url, req) = serve(status);
            assert_eq!(
                check_access_at(&url, "hf_testtokentesttoken"),
                want,
                "{status}"
            );
            let req = req.join().unwrap();
            let first_line = req.lines().next().unwrap();
            assert!(
                !first_line.contains("hf_testtoken"),
                "token in the URL: {first_line}"
            );
            assert!(req
                .to_lowercase()
                .contains("authorization: bearer hf_testtokentesttoken"));
            assert!(first_line.starts_with("HEAD /pyannote/"));
        }
    }

    #[test]
    fn other_answers_and_unreachable_servers_are_unknown_and_never_leak_the_token() {
        let (url, _h) = serve("500 Internal Server Error");
        assert!(
            matches!(check_access_at(&url, "hf_testtokentesttoken"), HfAccess::Unknown(m) if m.contains("500"))
        );
        // nothing is listening here
        let gone = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        match check_access_at(&gone, "hf_testtokentesttoken") {
            HfAccess::Unknown(m) => assert!(!m.contains("hf_testtoken"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}
