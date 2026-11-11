# Contribuindo com o Aceris

## Regras obrigatórias

Antes de qualquer contribuição, leia [REGRAS_PARA_IA.md](docs/REGRAS_PARA_IA.md). As regras de isolamento entre IAs, uso exclusivo de `main`, Rust em todo o produto e autoria integral de código e assets são obrigatórias.

### Política de sala limpa

- Não abra, leia, pesquise ou consulte o código-fonte interno de outros emuladores, incluindo Azahar, Citra, Lime3DS e derivados.
- Outros emuladores só podem ser analisados pela superfície observável: interface em execução, menus, configurações, fluxos de usuário e comportamento visível.
- Não copie, adapte, traduza ou derive código, interface, texto ou assets de terceiros.
- Documentação pública independente de hardware e formatos pode ser consultada; isso não autoriza consultar código-fonte de emuladores.
- Todo o produto, inclusive interface e frontends, deve ser implementado em Rust.
- Não adicione dependências que forneçam implementação ou recursos do produto sem autorização explícita do mantenedor.
- Não inclua jogos, firmware, BIOS, chaves ou material protegido no repositório.

### Isolamento de agentes

Cada IA deve respeitar os arquivos e tarefas atribuídos às demais. É proibido alterar trabalho de outra IA sem autorização explícita de Carlos Costa. Se houver dúvida sobre autoria ou sobreposição, interrompa a edição e peça orientação.

## Fluxo de trabalho

1. Trabalhe exclusivamente na branch `main`. É proibido criar branches ou usar pull requests como fluxo de desenvolvimento.
2. Confira o HEAD e o diff antes de editar ou publicar alterações. Nunca sobrescreva commits ou alterações que chegaram depois da sua leitura.
3. Execute, quando aplicável:
   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
4. Informe exatamente quais verificações foram executadas e seus resultados. Não afirme que compilou ou testou o que não foi executado.

## Estilo

- Código e comentários em inglês; documentação do repositório em português.
- Prefira testes pequenos e determinísticos.
- Evite `unsafe`; quando necessário, documente o invariante com `// SAFETY:`.
