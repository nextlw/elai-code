//! TUI-safe progress reporting pattern.
//!
//! # Por que existe
//!
//! Comandos que rodam dentro do TUI (alternate screen do ratatui) **não podem**
//! escrever direto em `stdout`/`stderr` durante a execução: ANSI control codes
//! (`\r`, cursor-up, clear-line) usados por `eprintln!`/`println!`/`indicatif`
//! corrompem o render do TUI. Dois exemplos reais que aconteceram aqui:
//!
//! 1. `fastembed` com `with_show_download_progress(true)` derrubou o layout
//!    com 5 progress bars empilhadas no meio do chat.
//! 2. `eprintln!("Indexando...")` em `init.rs` saiu fora de posição entre
//!    frames do ratatui.
//!
//! # Como usar
//!
//! Comandos longos aceitam `&dyn ProgressReporter` (ou `impl ProgressReporter`).
//! Cada caller decide o destino:
//!
//! - **CLI puro** (`elai init`, `elai verify`, ...): use [`EprintlnReporter`].
//!   `eprintln!` é seguro porque não há TUI ativo.
//! - **TUI** (`/init` no chat): use um `ChannelReporter` (em `elai-cli` ou
//!   similar) que envia [`TuiMsg::SystemNote`] via `mpsc::Sender`. O TUI
//!   drena no tick e renderiza como `ChatEntry::SystemNote`.
//! - **Quiet/CI**: use [`NoopReporter`].
//!
//! # Pattern para novas funções
//!
//! ```ignore
//! pub fn long_running_op(
//!     input: &Input,
//!     reporter: &dyn ProgressReporter,
//! ) -> Result<Output, Error> {
//!     reporter.report("Starting...");
//!     // ... work ...
//!     for (i, item) in items.iter().enumerate() {
//!         reporter.report(&progress_bar_labeled("Processing", i, items.len(), 20));
//!         process(item)?;
//!     }
//!     reporter.report("Done.");
//!     Ok(output)
//! }
//! ```
//!
//! Closures (`Fn(&str)`) também funcionam graças à blanket impl abaixo, mas o
//! trait é preferível para call sites novos pois aceita estado (counters,
//! filtros, prefixos).

use std::sync::Mutex;

pub use code_index::progress::{progress_bar, progress_bar_labeled};

/// Receptor de mensagens de progresso de comandos longos.
///
/// Implementações DEVEM ser thread-safe (`Send + Sync`) e tolerar mensagens
/// arbitrariamente curtas/longas. Mensagens NÃO devem conter ANSI control
/// codes — apenas Unicode estático (incluindo barras `█`/`░` de
/// [`progress_bar`]).
pub trait ProgressReporter: Send + Sync {
    fn report(&self, msg: &str);
}

/// Blanket impl: qualquer `Fn(&str) + Send + Sync` é um `ProgressReporter`.
/// Permite passar closures como reporters sem boilerplate.
impl<F: Fn(&str) + Send + Sync> ProgressReporter for F {
    fn report(&self, msg: &str) {
        self(msg);
    }
}

/// Reporter que escreve em stderr via `eprintln!`. Use em CLI puro (sem TUI).
/// Cada mensagem é prefixada com 2 espaços para alinhamento visual.
pub struct EprintlnReporter {
    prefix: &'static str,
}

impl EprintlnReporter {
    #[must_use]
    pub const fn new() -> Self {
        Self { prefix: "  " }
    }

    #[must_use]
    pub const fn with_prefix(prefix: &'static str) -> Self {
        Self { prefix }
    }
}

impl Default for EprintlnReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressReporter for EprintlnReporter {
    fn report(&self, msg: &str) {
        eprintln!("{}{msg}", self.prefix);
    }
}

/// Reporter silencioso. Use em CI, batch scripts, ou quando o caller não quer
/// nenhum output durante a operação.
pub struct NoopReporter;

impl ProgressReporter for NoopReporter {
    fn report(&self, _msg: &str) {}
}

/// Reporter que acumula mensagens em memória. Útil para testes ou para
/// caller que quer renderizar tudo de uma vez no final.
pub struct CollectingReporter {
    inner: Mutex<Vec<String>>,
}

impl CollectingReporter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn drain(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Default for CollectingReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressReporter for CollectingReporter {
    fn report(&self, msg: &str) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(msg.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closure_implements_reporter_via_blanket() {
        let collected = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let c = collected.clone();
        let reporter = move |msg: &str| {
            c.lock().unwrap().push(msg.to_string());
        };
        // Borrows the closure as &dyn ProgressReporter.
        let dyn_ref: &dyn ProgressReporter = &reporter;
        dyn_ref.report("hello");
        dyn_ref.report("world");
        assert_eq!(*collected.lock().unwrap(), vec!["hello", "world"]);
    }

    #[test]
    fn noop_reporter_drops_messages() {
        let r = NoopReporter;
        r.report("ignored"); // no panic, no observable effect
    }

    #[test]
    fn collecting_reporter_accumulates_messages() {
        let r = CollectingReporter::new();
        r.report("a");
        r.report("b");
        r.report("c");
        assert_eq!(r.snapshot(), vec!["a", "b", "c"]);
        assert_eq!(r.drain(), vec!["a", "b", "c"]);
        assert!(r.snapshot().is_empty(), "drain should empty the buffer");
    }

    #[test]
    fn collecting_reporter_is_thread_safe() {
        use std::thread;
        let r = std::sync::Arc::new(CollectingReporter::new());
        let mut handles = Vec::new();
        for i in 0..10 {
            let r = r.clone();
            handles.push(thread::spawn(move || {
                r.report(&format!("msg-{i}"));
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(r.snapshot().len(), 10);
    }

    #[test]
    fn eprintln_reporter_does_not_panic() {
        let r = EprintlnReporter::new();
        r.report("test message");
        let r2 = EprintlnReporter::with_prefix(">>> ");
        r2.report("test 2");
    }

    #[test]
    fn progress_bar_helpers_are_re_exported() {
        let bar = progress_bar(50.0, 10);
        assert!(bar.contains("█████"));
        let labeled = progress_bar_labeled("Doing", 5, 10, 10);
        assert!(labeled.starts_with("Doing"));
    }
}
