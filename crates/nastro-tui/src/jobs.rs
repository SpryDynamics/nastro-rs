//! Background transcription jobs for the TUI.
//!
//! Each job runs [`nastro_transcribe::transcribe`] on its own thread; at most `max_parallel` run at once and the
//! rest wait in a first-in-first-out queue. The event loop polls [`Jobs::poll`] to turn progress and
//! completions into UI updates. Dropping [`Jobs`] cancels everything and waits for the threads, so quitting (or
//! a panic) never leaves a transcription running, and since transcripts are written atomically, never leaves a
//! half-written one either.

use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use nastro_transcribe::{CancelToken, Error, Options, Outcome, Progress};

use crate::app::JobState;

/// What a job tells the UI.
#[derive(Debug)]
pub enum JobMsg {
    State {
        dir_name: String,
        state: JobState,
    },
    Done {
        dir_name: String,
        result: Result<Outcome, Error>,
    },
}

struct Pending {
    dir_name: String,
    path: PathBuf,
    options: Options,
}

struct Running {
    cancel: CancelToken,
    handle: Option<JoinHandle<()>>,
}

pub struct Jobs {
    max_parallel: usize,
    threads_per_job: usize,
    tx: Sender<JobMsg>,
    rx: Receiver<JobMsg>,
    running: HashMap<String, Running>,
    queue: VecDeque<Pending>,
}

/// Maps the library's progress events onto the UI's job states.
pub fn state_of(progress: Progress) -> JobState {
    match progress {
        Progress::DownloadingModel { done, total } => JobState::Downloading(
            done.saturating_mul(100)
                .checked_div(total)
                .map_or(0, |p| p.min(100) as u8),
        ),
        Progress::Decoding => JobState::Reading,
        Progress::Phase(phase) => JobState::Working(phase),
        Progress::Transcribing { percent } => JobState::Transcribing(percent.min(100)),
        Progress::Saving => JobState::Saving,
    }
}

impl Jobs {
    pub fn new(max_parallel: usize) -> Self {
        let max_parallel = max_parallel.max(1);
        let (tx, rx) = mpsc::channel();
        Jobs {
            max_parallel,
            // Share the cores between simultaneous jobs rather than letting each assume it has them all.
            threads_per_job: (nastro_transcribe::default_threads() / max_parallel).max(1),
            tx,
            rx,
            running: HashMap::new(),
            queue: VecDeque::new(),
        }
    }

    pub fn is_busy(&self) -> bool {
        !self.running.is_empty() || !self.queue.is_empty()
    }

    /// Starts the job now if there is a free slot, otherwise queues it. Returns the state to show.
    pub fn submit(&mut self, dir_name: String, path: PathBuf, options: Options) -> JobState {
        let pending = Pending {
            dir_name,
            path,
            options,
        };
        if self.running.len() < self.max_parallel {
            self.spawn(pending);
            JobState::Reading
        } else {
            self.queue.push_back(pending);
            JobState::Queued
        }
    }

    /// Cancels a job. Returns `true` if it was still queued (so it is gone immediately); a running job is told to
    /// stop and reports [`Error::Cancelled`] shortly after, through [`Jobs::poll`].
    pub fn cancel(&mut self, dir_name: &str) -> bool {
        if let Some(i) = self.queue.iter().position(|p| p.dir_name == dir_name) {
            self.queue.remove(i);
            return true;
        }
        if let Some(r) = self.running.get(dir_name) {
            r.cancel.cancel();
        }
        false
    }

    /// Collects everything the jobs have reported, reaps finished threads and starts queued jobs in the freed
    /// slots (announcing them with a [`JobMsg::State`]).
    pub fn poll(&mut self) -> Vec<JobMsg> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            if let JobMsg::Done { dir_name, .. } = &msg {
                if let Some(mut r) = self.running.remove(dir_name) {
                    if let Some(h) = r.handle.take() {
                        let _ = h.join();
                    }
                }
            }
            out.push(msg);
        }
        while self.running.len() < self.max_parallel {
            let Some(next) = self.queue.pop_front() else {
                break;
            };
            let dir_name = next.dir_name.clone();
            self.spawn(next);
            out.push(JobMsg::State {
                dir_name,
                state: JobState::Reading,
            });
        }
        out
    }

    fn spawn(&mut self, pending: Pending) {
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let tx = self.tx.clone();
        let Pending {
            dir_name,
            path,
            mut options,
        } = pending;
        options.threads = Some(self.threads_per_job);
        let name = dir_name.clone();

        let spawned = std::thread::Builder::new()
            .name(format!("transcribe {name}"))
            .spawn(move || {
                let progress_tx = tx.clone();
                let progress_name = dir_name.clone();
                // A panic in the speech engine must not take the whole TUI down with it.
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    nastro_transcribe::transcribe(&path, &options, &worker_cancel, &mut |p| {
                        let _ = progress_tx.send(JobMsg::State {
                            dir_name: progress_name.clone(),
                            state: state_of(p),
                        });
                    })
                }))
                .unwrap_or_else(|_| {
                    Err(Error::Whisper(
                        "internal error: the transcription thread panicked".into(),
                    ))
                });
                let _ = tx.send(JobMsg::Done { dir_name, result });
            });

        match spawned {
            Ok(handle) => {
                self.running.insert(
                    name,
                    Running {
                        cancel,
                        handle: Some(handle),
                    },
                );
            }
            Err(e) => {
                let _ = self.tx.send(JobMsg::Done {
                    dir_name: name,
                    result: Err(Error::Whisper(format!(
                        "could not start a transcription thread: {e}"
                    ))),
                });
            }
        }
    }

    /// Cancels every job and waits for the threads to finish (cancelling is prompt, well under a second).
    pub fn shutdown(&mut self) {
        self.queue.clear();
        for r in self.running.values() {
            r.cancel.cancel();
        }
        for (_, mut r) in self.running.drain() {
            if let Some(h) = r.handle.take() {
                let _ = h.join();
            }
        }
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn opts() -> Options {
        Options {
            model: "tiny.en".into(),
            ..Options::default()
        }
    }

    /// A directory that does not exist: the job fails immediately with `NoAudio`, which is all the lifecycle
    /// tests need (no model or ffmpeg involved).
    fn missing(name: &str) -> PathBuf {
        PathBuf::from("this-directory-does-not-exist").join(name)
    }

    fn drain(jobs: &mut Jobs, want_done: usize) -> Vec<JobMsg> {
        let mut all = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut done = 0;
        while done < want_done {
            assert!(Instant::now() < deadline, "jobs did not finish");
            for m in jobs.poll() {
                if matches!(m, JobMsg::Done { .. }) {
                    done += 1;
                }
                all.push(m);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        all
    }

    #[test]
    fn progress_maps_onto_job_states() {
        assert_eq!(state_of(Progress::Decoding), JobState::Reading);
        assert_eq!(state_of(Progress::Saving), JobState::Saving);
        assert_eq!(
            state_of(Progress::Transcribing { percent: 42 }),
            JobState::Transcribing(42)
        );
        assert_eq!(
            state_of(Progress::DownloadingModel {
                done: 50,
                total: 200
            }),
            JobState::Downloading(25)
        );
        assert_eq!(
            state_of(Progress::DownloadingModel { done: 1, total: 0 }),
            JobState::Downloading(0)
        );
        assert_eq!(
            state_of(Progress::DownloadingModel {
                done: 300,
                total: 200
            }),
            JobState::Downloading(100)
        );
    }

    #[test]
    fn a_job_runs_and_reports_its_result() {
        let mut jobs = Jobs::new(2);
        assert_eq!(
            jobs.submit("a".into(), missing("a"), opts()),
            JobState::Reading
        );
        assert!(jobs.is_busy());

        let msgs = drain(&mut jobs, 1);

        assert!(matches!(
            msgs.last(),
            Some(JobMsg::Done { dir_name, result: Err(Error::NoAudio(_)) }) if dir_name == "a"
        ));
        assert!(!jobs.is_busy());
    }

    #[test]
    fn jobs_beyond_the_limit_queue_and_then_run_in_order() {
        let mut jobs = Jobs::new(1);
        assert_eq!(
            jobs.submit("a".into(), missing("a"), opts()),
            JobState::Reading
        );
        assert_eq!(
            jobs.submit("b".into(), missing("b"), opts()),
            JobState::Queued
        );
        assert_eq!(
            jobs.submit("c".into(), missing("c"), opts()),
            JobState::Queued
        );
        assert!(jobs.running.len() <= 1);

        let msgs = drain(&mut jobs, 3);

        let order: Vec<&str> = msgs
            .iter()
            .filter_map(|m| match m {
                JobMsg::Done { dir_name, .. } => Some(dir_name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["a", "b", "c"]);
        // queued jobs were announced as started when their slot freed up
        assert!(msgs.iter().any(
            |m| matches!(m, JobMsg::State { dir_name, state: JobState::Reading } if dir_name == "b")
        ));
    }

    #[test]
    fn a_queued_job_can_be_cancelled_before_it_starts() {
        let mut jobs = Jobs::new(1);
        jobs.submit("a".into(), missing("a"), opts());
        jobs.submit("b".into(), missing("b"), opts());

        assert!(jobs.cancel("b"), "queued jobs are removed immediately");
        assert!(!jobs.cancel("nope"));

        let msgs = drain(&mut jobs, 1);
        assert!(!msgs
            .iter()
            .any(|m| matches!(m, JobMsg::Done { dir_name, .. } if dir_name == "b")));
        assert!(!jobs.is_busy());
    }

    #[test]
    fn shutdown_cancels_everything_and_leaves_nothing_running() {
        let mut jobs = Jobs::new(1);
        jobs.submit("a".into(), missing("a"), opts());
        jobs.submit("b".into(), missing("b"), opts());
        jobs.shutdown();
        assert!(!jobs.is_busy());
        assert!(
            jobs.poll().iter().all(|m| !matches!(
                m,
                JobMsg::State {
                    state: JobState::Reading,
                    ..
                }
            )),
            "nothing new may start"
        );
    }

    #[test]
    fn dropping_jobs_waits_for_threads() {
        let mut jobs = Jobs::new(2);
        jobs.submit("a".into(), missing("a"), opts());
        drop(jobs); // must not hang or panic
    }
}
