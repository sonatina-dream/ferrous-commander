//! Background file operations and the operations queue (SPEC §5.3, F2 "Add to queue").
//!
//! "Start" jobs run immediately on their own thread. "Queued" jobs run one at a time on
//! a single worker. Questions (conflicts, errors) are posted as [`Prompt`]s, and the job
//! thread blocks until the UI answers with [`JobManager::answer`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::archive::{self, CompressionLevel};
use crate::ops::{
    self, ConflictChoice, ErrorChoice, OpError, OpObserver, Progress, Report, TransferKind,
};

pub type JobId = u64;

/// What a job does.
#[derive(Clone, Debug, PartialEq)]
pub enum JobKind {
    Transfer { kind: TransferKind, sources: Vec<PathBuf>, dest: PathBuf, base_dir: PathBuf },
    Delete { paths: Vec<PathBuf>, permanent: bool },
    Compress { items: Vec<PathBuf>, out: PathBuf, level: CompressionLevel },
    Extract { archive: PathBuf, dest_dir: PathBuf },
    Duplicate { items: Vec<PathBuf> },
}

impl JobKind {
    /// Short description for the queue list.
    pub fn title(&self) -> String {
        fn count(n: usize) -> String {
            if n == 1 { "1 item".into() } else { format!("{n} items") }
        }
        match self {
            JobKind::Transfer { kind: TransferKind::Copy, sources, dest, .. } => {
                format!("Copying {} to \"{}\"", count(sources.len()), file_name_or_path(dest))
            }
            JobKind::Transfer { kind: TransferKind::Move, sources, dest, .. } => {
                format!("Moving {} to \"{}\"", count(sources.len()), file_name_or_path(dest))
            }
            JobKind::Delete { paths, permanent: false } => format!("Moving {} to the Trash", count(paths.len())),
            JobKind::Delete { paths, permanent: true } => format!("Deleting {}", count(paths.len())),
            JobKind::Compress { out, .. } => format!("Compressing to \"{}\"", file_name(out)),
            JobKind::Extract { archive, .. } => format!("Extracting \"{}\"", file_name(archive)),
            JobKind::Duplicate { items } => format!("Duplicating {}", count(items.len())),
        }
    }
}

fn file_name_or_path(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Lifecycle of a job.
#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    WaitingForAnswer,
    Done(Report),
    Failed(String),
    Cancelled,
}

impl JobState {
    pub fn is_finished(&self) -> bool {
        matches!(self, JobState::Done(_) | JobState::Failed(_) | JobState::Cancelled)
    }
}

/// UI-facing view of a job.
#[derive(Clone, Debug, PartialEq)]
pub struct JobStatus {
    pub id: JobId,
    pub title: String,
    pub queued: bool,
    pub state: JobState,
    pub progress: Progress,
}

/// A question a job needs answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    Conflict { src: PathBuf, dst: PathBuf, is_dir: bool },
    Error { path: PathBuf, message: String },
}

/// Answer to a [`Prompt`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Conflict(ConflictChoice),
    Error(ErrorChoice),
}

struct PendingPrompt {
    job: JobId,
    prompt: Prompt,
    reply: Sender<Answer>,
}

#[derive(Default)]
struct Inner {
    jobs: Vec<JobStatus>,
    prompts: VecDeque<PendingPrompt>,
    cancels: Vec<(JobId, Arc<AtomicBool>)>,
    live_bytes: Vec<(JobId, Arc<AtomicU64>)>,
    next_id: JobId,
    /// Bumped on every state change so the UI can cheaply detect updates.
    generation: u64,
    /// Bumped when a job finishes (the UI refreshes panels).
    finished: u64,
}

/// Snapshot handed to the UI.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub jobs: Vec<JobStatus>,
    pub prompt: Option<(JobId, Prompt)>,
    pub generation: u64,
    pub finished: u64,
}

impl Snapshot {
    pub fn active(&self) -> impl Iterator<Item = &JobStatus> {
        self.jobs.iter().filter(|j| !j.state.is_finished())
    }
}

/// Owns jobs, the queue worker and pending prompts. Cheap to clone.
#[derive(Clone)]
pub struct JobManager {
    inner: Arc<Mutex<Inner>>,
    queue_tx: Sender<(JobId, JobKind)>,
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

impl JobManager {
    pub fn new() -> JobManager {
        let inner = Arc::new(Mutex::new(Inner { next_id: 1, ..Default::default() }));
        let (queue_tx, queue_rx) = mpsc::channel::<(JobId, JobKind)>();
        let worker_inner = inner.clone();
        thread::Builder::new()
            .name("fc-queue".into())
            .spawn(move || {
                for (id, kind) in queue_rx {
                    run_job(&worker_inner, id, kind);
                }
            })
            .expect("spawn queue worker");
        JobManager { inner, queue_tx }
    }

    fn register(&self, kind: &JobKind, queued: bool) -> JobId {
        let mut g = self.inner.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        g.jobs.push(JobStatus {
            id,
            title: kind.title(),
            queued,
            state: if queued { JobState::Queued } else { JobState::Running },
            progress: Progress::default(),
        });
        g.cancels.push((id, Arc::new(AtomicBool::new(false))));
        g.live_bytes.push((id, Arc::new(AtomicU64::new(0))));
        g.generation += 1;
        id
    }

    /// Runs a job right away on its own thread.
    pub fn start(&self, kind: JobKind) -> JobId {
        let id = self.register(&kind, false);
        let inner = self.inner.clone();
        thread::Builder::new()
            .name(format!("fc-job-{id}"))
            .spawn(move || run_job(&inner, id, kind))
            .expect("spawn job");
        id
    }

    /// Appends a job to the sequential queue.
    pub fn enqueue(&self, kind: JobKind) -> JobId {
        let id = self.register(&kind, true);
        let _ = self.queue_tx.send((id, kind));
        id
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = self.inner.lock().unwrap();
        let mut jobs = g.jobs.clone();
        for job in &mut jobs {
            if let Some((_, live)) = g.live_bytes.iter().find(|(id, _)| *id == job.id) {
                job.progress.done_bytes = job.progress.done_bytes.max(live.load(Ordering::Relaxed));
            }
        }
        Snapshot {
            jobs,
            prompt: g.prompts.front().map(|p| (p.job, p.prompt.clone())),
            generation: g.generation,
            finished: g.finished,
        }
    }

    /// Answers the oldest pending prompt.
    pub fn answer(&self, answer: Answer) {
        let mut g = self.inner.lock().unwrap();
        if let Some(p) = g.prompts.pop_front() {
            let _ = p.reply.send(answer);
            g.generation += 1;
        }
    }

    /// Requests cancellation; a waiting prompt of that job is answered with Cancel/Abort.
    pub fn cancel(&self, id: JobId) {
        let mut g = self.inner.lock().unwrap();
        if let Some((_, flag)) = g.cancels.iter().find(|(j, _)| *j == id) {
            flag.store(true, Ordering::Relaxed);
        }
        let mut keep = VecDeque::new();
        while let Some(p) = g.prompts.pop_front() {
            if p.job == id {
                let _ = p.reply.send(match p.prompt {
                    Prompt::Conflict { .. } => Answer::Conflict(ConflictChoice::Cancel),
                    Prompt::Error { .. } => Answer::Error(ErrorChoice::Abort),
                });
            } else {
                keep.push_back(p);
            }
        }
        g.prompts = keep;
        g.generation += 1;
    }

    /// Removes finished jobs from the list.
    pub fn clear_finished(&self) {
        let mut g = self.inner.lock().unwrap();
        let finished: Vec<JobId> = g.jobs.iter().filter(|j| j.state.is_finished()).map(|j| j.id).collect();
        g.jobs.retain(|j| !j.state.is_finished());
        g.cancels.retain(|(id, _)| !finished.contains(id));
        g.live_bytes.retain(|(id, _)| !finished.contains(id));
        g.generation += 1;
    }

    pub fn has_active(&self) -> bool {
        self.inner.lock().unwrap().jobs.iter().any(|j| !j.state.is_finished())
    }

    /// Blocks until every job finished (tests and quitting).
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if !self.has_active() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

fn set_state(inner: &Arc<Mutex<Inner>>, id: JobId, state: JobState) {
    let mut g = inner.lock().unwrap();
    let finished = state.is_finished();
    if let Some(j) = g.jobs.iter_mut().find(|j| j.id == id) {
        j.state = state;
    }
    g.generation += 1;
    if finished {
        g.finished += 1;
    }
}

/// Observer that relays prompts to the UI and progress into shared state.
struct JobObserver {
    inner: Arc<Mutex<Inner>>,
    id: JobId,
    cancel: Arc<AtomicBool>,
    live: Arc<AtomicU64>,
}

impl JobObserver {
    fn ask(&self, prompt: Prompt) -> Option<Answer> {
        let (tx, rx): (Sender<Answer>, Receiver<Answer>) = mpsc::channel();
        {
            let mut g = self.inner.lock().unwrap();
            g.prompts.push_back(PendingPrompt { job: self.id, prompt, reply: tx });
            if let Some(j) = g.jobs.iter_mut().find(|j| j.id == self.id) {
                j.state = JobState::WaitingForAnswer;
            }
            g.generation += 1;
        }
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(a) => {
                    set_state(&self.inner, self.id, JobState::Running);
                    return Some(a);
                }
                Err(mpsc::RecvTimeoutError::Timeout) if self.cancel.load(Ordering::Relaxed) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}

impl OpObserver for JobObserver {
    fn progress(&mut self, p: &Progress) {
        let mut g = self.inner.lock().unwrap();
        if let Some(j) = g.jobs.iter_mut().find(|j| j.id == self.id) {
            j.progress = p.clone();
        }
        g.generation += 1;
    }

    fn conflict(&mut self, src: &Path, dst: &Path, is_dir: bool) -> ConflictChoice {
        match self.ask(Prompt::Conflict { src: src.to_path_buf(), dst: dst.to_path_buf(), is_dir }) {
            Some(Answer::Conflict(c)) => c,
            _ => ConflictChoice::Cancel,
        }
    }

    fn error(&mut self, path: &Path, err: &std::io::Error) -> ErrorChoice {
        match self.ask(Prompt::Error { path: path.to_path_buf(), message: err.to_string() }) {
            Some(Answer::Error(e)) => e,
            _ => ErrorChoice::Abort,
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn byte_counter(&self) -> Option<Arc<AtomicU64>> {
        Some(self.live.clone())
    }

    fn cancel_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(self.cancel.clone())
    }
}

fn run_job(inner: &Arc<Mutex<Inner>>, id: JobId, kind: JobKind) {
    let (cancel, live) = {
        let g = inner.lock().unwrap();
        let cancel = g.cancels.iter().find(|(j, _)| *j == id).map(|(_, c)| c.clone());
        let live = g.live_bytes.iter().find(|(j, _)| *j == id).map(|(_, c)| c.clone());
        match (cancel, live) {
            (Some(c), Some(l)) => (c, l),
            _ => return,
        }
    };
    if cancel.load(Ordering::Relaxed) {
        set_state(inner, id, JobState::Cancelled);
        return;
    }
    set_state(inner, id, JobState::Running);
    let mut observer = JobObserver { inner: inner.clone(), id, cancel: cancel.clone(), live };

    let result: Result<Report, OpError> = match kind {
        JobKind::Transfer { kind, sources, dest, base_dir } => {
            ops::plan_transfer(kind, &sources, &dest, &base_dir).and_then(|plan| ops::execute_transfer(&plan, &mut observer))
        }
        JobKind::Delete { paths, permanent } => ops::delete_items(&paths, permanent, &mut observer),
        JobKind::Duplicate { items } => ops::duplicate(&items, &mut observer).map(|outputs| Report {
            done: outputs.len() as u64,
            outputs,
            ..Default::default()
        }),
        JobKind::Compress { items, out, level } => {
            let total = items.len() as u64;
            let mut done = 0u64;
            let res = archive::compress_zip(&items, &out, level, &cancel, |p| {
                done += 1;
                observer.progress(&Progress { total_items: total.max(done), done_items: done, current: p.to_path_buf(), ..Default::default() });
            });
            res.map(|_| Report { done: 1, outputs: vec![out.clone()], ..Default::default() }).map_err(OpError::from)
        }
        JobKind::Extract { archive: a, dest_dir } => archive::extract(&a, &dest_dir, &cancel)
            .map(|tops| Report { done: 1, outputs: tops, ..Default::default() })
            .map_err(OpError::from),
    };

    let state = match result {
        Ok(report) => JobState::Done(report),
        Err(_) if cancel.load(Ordering::Relaxed) => JobState::Cancelled,
        Err(OpError::Aborted) => JobState::Cancelled,
        Err(e) => JobState::Failed(e.to_string()),
    };
    set_state(inner, id, state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn wait_prompt(m: &JobManager) -> Prompt {
        for _ in 0..500 {
            if let Some((_, p)) = m.snapshot().prompt {
                return p;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("no prompt");
    }

    #[test]
    fn immediate_copy_with_conflict_prompt() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::write(r.join("a.txt"), "new").unwrap();
        fs::create_dir(r.join("dst")).unwrap();
        fs::write(r.join("dst/a.txt"), "old").unwrap();

        let m = JobManager::new();
        let id = m.start(JobKind::Transfer {
            kind: TransferKind::Copy,
            sources: vec![r.join("a.txt")],
            dest: r.join("dst"),
            base_dir: r.to_path_buf(),
        });
        match wait_prompt(&m) {
            Prompt::Conflict { dst, is_dir, .. } => {
                assert_eq!(dst, r.join("dst/a.txt"));
                assert!(!is_dir);
            }
            other => panic!("unexpected {other:?}"),
        }
        m.answer(Answer::Conflict(ConflictChoice::Overwrite));
        assert!(m.wait_idle(Duration::from_secs(5)));
        let snap = m.snapshot();
        let job = snap.jobs.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(&job.state, JobState::Done(r) if r.done == 1));
        assert_eq!(fs::read_to_string(r.join("dst/a.txt")).unwrap(), "new");
        assert_eq!(snap.finished, 1);
        m.clear_finished();
        assert!(m.snapshot().jobs.is_empty());
    }

    #[test]
    fn queue_runs_sequentially_and_cancel_answers_prompt() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::write(r.join("x"), "1").unwrap();
        fs::create_dir(r.join("d")).unwrap();
        fs::write(r.join("d/x"), "0").unwrap();
        let m = JobManager::new();
        let first = m.enqueue(JobKind::Transfer {
            kind: TransferKind::Copy,
            sources: vec![r.join("x")],
            dest: r.join("d"),
            base_dir: r.to_path_buf(),
        });
        let second = m.enqueue(JobKind::Delete { paths: vec![r.join("x")], permanent: true });
        wait_prompt(&m);
        {
            let snap = m.snapshot();
            let s = snap.jobs.iter().find(|j| j.id == second).unwrap();
            assert_eq!(s.state, JobState::Queued, "second waits for the first");
        }
        m.cancel(first);
        assert!(m.wait_idle(Duration::from_secs(5)));
        let snap = m.snapshot();
        assert_eq!(snap.jobs.iter().find(|j| j.id == first).unwrap().state, JobState::Cancelled);
        assert!(matches!(snap.jobs.iter().find(|j| j.id == second).unwrap().state, JobState::Done(_)));
        assert!(!r.join("x").exists());
        assert_eq!(fs::read_to_string(r.join("d/x")).unwrap(), "0");
    }

    #[test]
    fn failed_plan_reports_reason() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir(r.join("folder")).unwrap();
        let m = JobManager::new();
        let id = m.start(JobKind::Transfer {
            kind: TransferKind::Copy,
            sources: vec![r.join("folder")],
            dest: r.join("folder/inside"),
            base_dir: r.to_path_buf(),
        });
        assert!(m.wait_idle(Duration::from_secs(5)));
        let snap = m.snapshot();
        let job = snap.jobs.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(&job.state, JobState::Failed(msg) if msg.contains("into itself")));
    }

    #[test]
    fn compress_and_extract_jobs() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir(r.join("data")).unwrap();
        fs::write(r.join("data/f.txt"), "content").unwrap();
        let m = JobManager::new();
        m.start(JobKind::Compress { items: vec![r.join("data")], out: r.join("data.zip"), level: CompressionLevel::Normal });
        assert!(m.wait_idle(Duration::from_secs(5)));
        m.start(JobKind::Extract { archive: r.join("data.zip"), dest_dir: r.join("out") });
        assert!(m.wait_idle(Duration::from_secs(5)));
        assert_eq!(fs::read_to_string(r.join("out/data/f.txt")).unwrap(), "content");
    }
}
