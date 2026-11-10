# Contribuindo com o Aceris

## Política de sala limpa

O Aceris é uma implementação **independente**. Para manter isso verdadeiro:

- **Não copie, traduza ou adapte código** de outros emuladores (incluindo Azahar,
  Citra, Lime3DS e derivados), nem de interfaces, textos, ícones ou recursos
  visuais deles.
- **Não cole trechos de código sob licenças incompatíveis** (por exemplo GPL) no projeto.
- Ser *inspirado* por ideias gerais é aceitável (por exemplo, "emuladores
  costumam separar núcleo e frontend"). Reproduzir a implementação não é.
- Documentação pública de hardware e de formatos de arquivo pode ser consultada
  como referência. Escreva o código com suas próprias palavras e estrutura, e
  cite a fonte nos comentários quando o comportamento for pouco óbvio.
- Se você já leu o código-fonte de outro emulador para a mesma peça que vai
  implementar, avise no PR para decidirmos juntos como proceder.
- **Nunca** envie jogos, firmware, BIOS, chaves ou qualquer material protegido por
  direitos autorais. O `.gitignore` bloqueia as extensões mais comuns.

## Fluxo de trabalho

1. Crie um branch a partir de `main`.
2. Antes de abrir o PR, rode:
   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
3. Abra o PR com uma descrição do que mudou e de onde vieram as informações usadas
   (documentação, testes próprios, etc.).

## Estilo

- Código e comentários em inglês; documentação do repositório em português.
- Prefira testes pequenos que descrevam o comportamento de hardware
  esperado (veja `cpu/condition.rs`).
- Evite `unsafe`; quando necessário, comente o invariante com `// SAFETY:`.
