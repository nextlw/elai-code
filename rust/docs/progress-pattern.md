# TUI-safe Progress Pattern

Comandos longos (init, verify, dream, commit-push-pr, plugin install, etc.)
**não podem** usar `eprintln!`, `println!` ou crates como `indicatif` enquanto
o TUI ratatui está ativo — ANSI control codes corrompem o alternate screen.

## Convenção

Toda função pública que pode rodar tanto em CLI puro quanto dentro do TUI
DEVE aceitar um parâmetro `&dyn runtime::ProgressReporter` (ou
`impl runtime::ProgressReporter`):

```rust
pub fn run_long_op(
    input: &Input,
    reporter: &dyn runtime::ProgressReporter,
) -> Result<Output, Error> {
    reporter.report("Starting...");
    // ...
}
```

Mantenha um wrapper sem o param para compat com call sites legados:

```rust
pub fn run_long_op_default(input: &Input) -> Result<Output, Error> {
    run_long_op(input, &runtime::EprintlnReporter::new())
}
```

## Reporters disponíveis

| Reporter | Quando usar |
|---|---|
| `EprintlnReporter` | CLI puro (sem TUI). Saída em stderr. |
| `NoopReporter` | CI/batch/quiet mode. |
| `CollectingReporter` | Testes ou render adiado (acumula em memória). |
| Closure `\|s: &str\| { ... }` | Inline rápido. Blanket impl no trait. |
| `ChannelReporter` (em elai-cli) | TUI: envia `TuiMsg::SystemNote` via `mpsc`. |

## Visualização: progress_bar

Use `runtime::progress_bar(pct, width)` ou `runtime::progress_bar_labeled(label, current, total, width)`
para gerar barras como `[██████░░░░░░░░] 30%`. **Não** use `indicatif` ou
qualquer crate que escreva direto em stderr.

## Pattern para CLI binary

```rust
fn cli_command(args: &Args) -> Result<()> {
    let reporter = runtime::EprintlnReporter::new();
    let result = library::run_long_op(args, &reporter)?;
    println!("{}", result.render());
    Ok(())
}
```

## Pattern para TUI (`elai-cli`)

```rust
SlashCommand::SomeLongOp => {
    let tx = msg_tx.clone();
    std::thread::spawn(move || {
        let send = |s: &str| {
            let _ = tx.send(tui::TuiMsg::SystemNote(s.to_string()));
        };
        match library::run_long_op(&args, &send) {
            Ok(r) => { let _ = tx.send(tui::TuiMsg::SystemNote(r.render())); }
            Err(e) => { let _ = tx.send(tui::TuiMsg::Error(format!("{e}"))); }
        }
    });
}
```

## Anti-patterns

- `eprintln!("...")` em código que roda dentro do TUI.
- `indicatif::ProgressBar` em qualquer lugar (pode rodar no TUI futuramente).
- `print!("\r{}", ...)` ou cursor manipulation manual.
- Hard-coding `EprintlnReporter` na assinatura — sempre aceitar trait.

## Quando esse pattern NÃO se aplica

- Comandos one-shot que rodam fora do TUI por design (ex: `elai update` que
  baixa binário e sai). Esses podem usar `eprintln!`/`indicatif` à vontade.
