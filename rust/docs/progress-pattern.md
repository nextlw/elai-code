# TUI-safe Progress Pattern

Comandos longos (`/init`, `/verify`, `/dream`, `/commit-push-pr`, plugin install,
download de modelo de embedding, …) **não podem** usar `eprintln!`, `println!`
ou crates como `indicatif` enquanto o TUI ratatui está ativo — ANSI control
codes corrompem o alternate screen.

A solução é o módulo `runtime::tasks::progress` (+ sinks em
`runtime::tasks::sinks`). Toda operação longa **registra uma task** no
`TaskRegistry` global e roteia updates por um `ProgressSink` que sabe desenhar
no destino correto.

## A regra

Toda função pública que pode rodar tanto em CLI puro quanto dentro do TUI deve
aceitar um `&dyn runtime::ProgressReporter` (ou rodar dentro de
[`with_task`] / [`with_task_default`]):

```rust
pub fn run_long_op(
    input: &Input,
    reporter: &dyn runtime::ProgressReporter,
) -> Result<Output, Error> {
    reporter.report("Starting…");
    // …
}
```

`TaskProgressReporter` implementa o trait `ProgressReporter` — call sites que já
recebem `&dyn ProgressReporter` continuam funcionando sem mudança.

## Pattern obrigatório: `with_task` / `with_task_default`

Em vez de instanciar um reporter manualmente, embrulhe o trabalho com
`with_task_default` (que resolve o sink default do processo):

```rust
use runtime::{with_task_default, TaskType};

with_task_default(
    TaskType::LocalWorkflow,
    "elai init",          // descrição registrada no TaskRegistry
    "Indexing",           // label exibido na linha viva
    None,                 // parent_id (Some(id) para sub-task)
    |reporter| -> Result<(), MyError> {
        reporter.report("Walking files…");
        // … long work …
        reporter.report("Done.");
        Ok(())
    },
)?;
```

`with_task_default` cuida de:

1. Gerar um `task_id` e registrar o `TaskState` no `task_registry()`.
2. Marcar a task como `Running`.
3. Entregar um `TaskProgressReporter` para a closure.
4. Finalizar como `Completed` / `Failed` / `Killed` (panic é re-propagado).
5. Chamar `sink.finalize` com o status final.

Use `with_task` (sem `_default`) quando precisar passar um sink específico
(testes, sub-tasks com sink alternativo).

### Sub-tasks

Passe `Some(parent_id)` em `with_task` para registrar a task como filha de
outra. `TaskState::new_with_parent` e `registry.list_children` permitem
inspecionar a hierarquia (usado, por exemplo, pelo download do modelo BGE
durante `elai init`).

## Sinks disponíveis

Vivem em `runtime::tasks::sinks`. Implementam o trait:

```rust
pub trait ProgressSink: Send + Sync {
    fn emit(&self, task_id: &str, label: &str, msg: &str);
    fn finalize(&self, task_id: &str, label: &str,
                status: TaskStatus, summary: Option<&str>);
}
```

| Sink | Quando usar |
|---|---|
| `LiveStderrSink` | CLI em TTY. Repinta a linha com `\r\x1b[2K`, throttle de 80 ms, respeita `$COLUMNS`. |
| `PlainStderrSink` | CLI piping / CI / não-TTY. Append-only, emite uma linha a cada delta de 5 %. |
| `NoopSink` | Modo quiet / batch — descarta tudo. |
| `CollectingSink` | Testes / render adiado — acumula `(task_id, label, msg, kind)` em memória. |
| `ChannelSink` (em `crates/elai-cli/src/tui_sink.rs`) | TUI — encaminha para `TuiMsg::TaskProgress{,End}` via `mpsc`. |

## Sink default por processo

`runtime::tasks::progress::default_sink()` retorna o sink global. O slot é
inicializado por `build_default_sink()`:

- TTY em stderr → `LiveStderrSink`.
- Não-TTY → `PlainStderrSink`.

O TUI chama `set_default_sink(Arc::new(ChannelSink::new(tx)))` no startup,
substituindo o slot por um sink que despacha `TuiMsg::TaskProgress`.

```rust
use std::sync::Arc;
use runtime::set_default_sink;
use elai_cli::tui_sink::ChannelSink;

set_default_sink(Arc::new(ChannelSink::new(msg_tx.clone())));
```

## Visualização: `progress_bar`

Use `runtime::progress_bar(pct, width)` ou
`runtime::progress_bar_labeled(label, current, total, width)` para gerar barras
como `[██████░░░░░░░░] 30 %`. **Não** use `indicatif` ou qualquer crate que
escreva direto em stderr — quebra o TUI e ignora o sink.

## Pattern para CLI binary

```rust
fn cli_command(args: &Args) -> Result<()> {
    runtime::with_task_default(
        runtime::TaskType::LocalWorkflow,
        "elai some-op",
        "Running",
        None,
        |reporter| library::run_long_op(args, reporter),
    )
}
```

## Pattern para TUI (`elai-cli`)

```rust
SlashCommand::SomeLongOp => {
    let tx = msg_tx.clone();
    std::thread::spawn(move || {
        let res = runtime::with_task_default(
            runtime::TaskType::LocalWorkflow,
            "elai some-op",
            "Running",
            None,
            |reporter| library::run_long_op(&args, reporter),
        );
        match res {
            Ok(r) => { let _ = tx.send(tui::TuiMsg::SystemNote(r.render())); }
            Err(e) => { let _ = tx.send(tui::TuiMsg::Error(format!("{e}"))); }
        }
    });
}
```

`ChannelSink` (já instalado no startup do TUI) traduz cada `reporter.report`
em `TuiMsg::TaskProgress { task_id, label, msg }`, que o renderer desenha numa
linha viva acima da composer.

## Anti-patterns

- `eprintln!("…")` ou `println!("…")` em código que pode rodar dentro do TUI.
- `indicatif::ProgressBar` em qualquer lugar (pode rodar no TUI futuramente).
- `print!("\r{}", …)` ou cursor manipulation manual.
- Hard-coding `LiveStderrSink` na assinatura — sempre aceitar trait e/ou usar
  `with_task_default`.
- Operação longa fora de `with_task` — significa que ela some do `/status`,
  do `task_registry`, e do widget de tasks do TUI.

## Quando o pattern NÃO se aplica

Comandos one-shot que rodam fora do TUI por design (ex.: `elai self-update`,
que baixa o binário e sai antes do TUI subir). Esses podem usar
`eprintln!` / `indicatif` à vontade — `build_default_sink()` vai escolher
`PlainStderrSink` automaticamente.
