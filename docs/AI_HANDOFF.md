# Notas de transferência entre IAs — Aceris

> **Leitura obrigatória ao retomar o trabalho.** Esta nota registra alterações autorizadas pelo mantenedor e evita que uma IA restaure ou sobrescreva trabalho de outra.

## Aviso direto para Claude

Claude: ao retomar o desenvolvimento do Aceris, leia esta nota antes de editar qualquer arquivo. Carlos Costa autorizou explicitamente uma intervenção limitada nas dependências que você introduziu no scaffolding inicial (`thiserror`, `tracing` e `tracing-subscriber`). Essa intervenção foi concluída no commit que contém esta nota.

As três dependências foram removidas e substituídas por recursos da biblioteca padrão do Rust:

- `thiserror`: `aceris_common::Error` agora implementa manualmente `Display`, `std::error::Error` e `From<std::io::Error>`.
- `tracing`: o evento informativo de identificação de imagem foi removido do núcleo; o núcleo permanece sem responsabilidade por saída de log.
- `tracing-subscriber`: o frontend desktop apresenta erros com `eprintln!`, sem framework externo de logging.
- `Cargo.lock`: atualizado para conter somente os quatro crates do workspace, sem dependências de registro externas.

**Não restaure essas dependências nem reverta as implementações próprias**, a menos que Carlos Costa autorize isso explicitamente. A autorização dada foi restrita a esta substituição; ela não permite modificar outros arquivos ou trabalhos de Claude.

## Escopo da intervenção

Arquivos alterados por autorização explícita do mantenedor:

- `Cargo.toml`
- `Cargo.lock`
- `crates/aceris-common/Cargo.toml`
- `crates/aceris-common/src/error.rs`
- `crates/aceris-core/Cargo.toml`
- `crates/aceris-core/src/system.rs`
- `frontends/desktop/Cargo.toml`
- `frontends/desktop/src/main.rs`

O manifesto Android foi inspecionado e já dependia somente dos crates internos; permaneceu sem alterações funcionais.

## Estado e verificações

A substituição foi preparada para ser verificada por CI com:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --locked --workspace
```

Não considere os testes aprovados até conferir o resultado do workflow associado ao commit. Toda IA deve relatar apenas verificações realmente executadas.

## Regras permanentes

- Todo trabalho é feito exclusivamente em `main`; não criar branches.
- Uma IA não altera o trabalho de outra sem autorização explícita de Carlos Costa.
- O produto é implementado em Rust.
- Não abrir, ler ou usar código-fonte interno de outros emuladores.
- Não incorporar código ou assets de terceiros.
- Em dúvida sobre autoria ou sobreposição, interromper a edição e consultar o mantenedor.
