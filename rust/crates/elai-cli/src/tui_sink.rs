//! `ProgressSink` que envia updates de tasks pelo canal `mpsc<TuiMsg>` do
//! TUI. Registrado via `runtime::set_default_sink` no startup do `tui::run`.
//!
//! Quando uma task chama `with_task_default(...)`, o reporter resolve este
//! sink e cada `report(msg)` vira `TuiMsg::TaskProgress { task_id, label, msg }`.
//! O drain do TUI (`apply_tui_msg`) substitui in-place a `ChatEntry::TaskProgress`
//! existente em vez de empilhar uma nova entry — é assim que as 50+ linhas de
//! "Embedding [...] X%" colapsam em uma única linha viva.

use std::sync::mpsc;
use std::sync::Mutex;

use runtime::{ProgressSink, TaskStatus};

use crate::tui::TuiMsg;

pub struct ChannelSink {
    tx: Mutex<mpsc::Sender<TuiMsg>>,
}

impl ChannelSink {
    #[must_use]
    pub fn new(tx: mpsc::Sender<TuiMsg>) -> Self {
        Self { tx: Mutex::new(tx) }
    }
}

impl ProgressSink for ChannelSink {
    fn emit(&self, task_id: &str, label: &str, msg: &str) {
        // Erros de send (canal fechado, drain desligou) são silenciosos —
        // a task continua, só não aparece no TUI.
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(TuiMsg::TaskProgress {
                task_id: task_id.to_string(),
                label: label.to_string(),
                msg: msg.to_string(),
            });
        }
    }

    fn finalize(&self, task_id: &str, label: &str, status: TaskStatus, summary: Option<&str>) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(TuiMsg::TaskProgressEnd {
                task_id: task_id.to_string(),
                label: label.to_string(),
                status,
                summary: summary.map(String::from),
            });
        }
    }
}
