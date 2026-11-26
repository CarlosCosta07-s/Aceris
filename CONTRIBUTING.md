# Contribuindo com o Aceris

## Regras obrigatórias

Antes de qualquer contribuição, leia [REGRAS_PARA_IA.md](docs/REGRAS_PARA_IA.md). As regras de isolamento entre IAs, uso exclusivo de `main`, Rust em todo o produto e autoria integral de código e assets são obrigatórias.

### Política de implementação original e referências técnicas

- É permitido consultar código-fonte de emuladores efetivamente open source (Azahar, Citra, Lime3DS e semelhantes) como referência técnica, nos limites da [seção 5 das regras](docs/REGRAS_PARA_IA.md#5-referências-de-código-aberto-de-outros-emuladores): verificar antes a licença do projeto e do arquivo, e nunca incorporar o código consultado.
- Não copiar, adaptar, traduzir, portar ou reproduzir código, interfaces, estruturas, testes, fixtures, shaders, patches, assets ou organização interna de terceiros.
- Não faça tradução linha a linha nem reproduza estrutura expressiva de qualquer material de terceiros. Toda implementação do Aceris deve ser concebida independentemente e escrita em Rust, com arquitetura, interfaces, nomes e testes próprios.
- Registre cada fonte consultada (documentação pública, observação externa ou código open source): título ou projeto, URL, arquivo/área, revisão, licença, data, pergunta técnica, nível de confiança e testes próprios que sustentam a conclusão.
- Se houver dúvida sobre a independência/procedência de uma fonte ou risco de obra derivada, interrompa a pesquisa e peça orientação a Carlos Costa.
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


## Estado do projeto

Este arquivo não registra estado de desenvolvimento, para não ficar desatualizado. Consulte o [README](README.md), o [roadmap](docs/ROADMAP.md) e as [notas de transferência](docs/AI_HANDOFF.md).
