use std::sync::Arc;

/// Progress events emitted during long-running operations (downloads, etc.).
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// A named stage started (e.g. "ライブラリをダウンロード中").
    Stage(String),
    /// Progress within the current stage.
    Progress { done: usize, total: usize },
    /// A free-form status message.
    Message(String),
}

pub type ProgressSink = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

/// A progress reporter that can be cheaply cloned and passed around.
#[derive(Clone)]
pub struct Progress {
    sink: Option<ProgressSink>,
}

impl Progress {
    pub fn none() -> Self {
        Self { sink: None }
    }

    pub fn new(sink: ProgressSink) -> Self {
        Self { sink: Some(sink) }
    }

    pub fn stage(&self, name: impl Into<String>) {
        self.emit(ProgressEvent::Stage(name.into()));
    }

    pub fn progress(&self, done: usize, total: usize) {
        self.emit(ProgressEvent::Progress { done, total });
    }

    pub fn message(&self, msg: impl Into<String>) {
        self.emit(ProgressEvent::Message(msg.into()));
    }

    fn emit(&self, event: ProgressEvent) {
        if let Some(sink) = &self.sink {
            sink(event);
        }
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::none()
    }
}
