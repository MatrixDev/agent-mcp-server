use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::tools::other::rdk_qemu::CONSOLE_CAPACITY;

////////////////////////////////////////////////////////////////////////////////
#[derive(Clone)]
pub struct RdkQemuConsoleBuffer {
    inner: Arc<Mutex<VecDeque<String>>>,
}

impl RdkQemuConsoleBuffer {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(CONSOLE_CAPACITY))),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn push(&self, line: String) {
        let mut console = self.inner.lock();
        if console.len() >= CONSOLE_CAPACITY {
            console.pop_front();
        }
        console.push_back(line);
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn last_line(&self) -> Option<String> {
        let console = self.inner.lock();
        console.back().cloned()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn dump(&self) -> String {
        let console = self.inner.lock();
        console.iter().map(|e| e.as_str()).collect::<Vec<_>>().join("\n")
    }
}
