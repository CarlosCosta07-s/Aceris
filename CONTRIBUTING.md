# Contribuindo com o Aceris

## Regras obrigatórias

Antes de qualquer contribuição, leia [REGRAS_PARA_IA.md](docs/REGRAS_PARA_IA.md). As regras de isolamento entre IAs, uso exclusivo de `main`, Rust em todo o produto e autoria integral de código e assets são obrigatórias.

### Política de implementação original e referências técnicas

- Não consultar código-fonte interno de outros emuladores, incluindo Azahar, Citra, Lime3DS ou semelhantes. A referência permitida limita-se à superfície observável do produto e à documentação pública independente de hardware, formatos e protocolos.
- Não copiar, adaptar, traduzir, portar ou reproduzir código, interfaces, estruturas, testes, fixtures, shaders, patches, assets ou organização interna de terceiros.
- Não faça tradução linha a linha nem reproduza a estrutura expressiva do código consultado. Toda implementação do Aceris deve ser concebida independentemente e escrita em Rust, com arquitetura, interfaces, nomes e testes próprios.
- Registre projeto, URL, arquivo/área consultada, commit/revisão, licença, data e pergunta técnica investigada. Converta o aprendizado em especificação independente e valide-o com documentação e testes próprios.
- Se houver dúvida sobre compatibilidade de licença ou risco de obra derivada, interrompa o uso daquela referência na implementação e peça orientação a Carlos Costa.
- Assets e recursos visuais/sonoros de terceiros continuam proibidos. Não adicione dependências que forneçam implementação ou recursos do produto sem autorização explícita do mantenedor.
- Todo o produto, inclusive núcleo, interface e frontends, deve ser implementado em Rust.
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


## Estado de referência — 22/11/2026

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.

Esta revisão altera somente documentação; não autoriza editar arquivos atribuídos a outro agente. Somente main, sem branches ou PRs, permanece obrigatório.
