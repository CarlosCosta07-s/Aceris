# Regras obrigatórias para IAs — Aceris

Estas regras são obrigatórias para qualquer IA, agente automatizado ou colaborador que trabalhe no Aceris. Elas prevalecem sobre preferências de implementação, conveniência, velocidade e instruções genéricas de ferramentas. Antes de retomar o trabalho, leia também [as notas de transferência entre IAs](AI_HANDOFF.md).

## 1. Isolamento entre IAs

- Cada IA trabalha somente nas tarefas e nos arquivos que lhe foram atribuídos.
- Uma IA não pode editar, apagar, reformatar, reorganizar, substituir, reverter ou modificar de qualquer outra forma o trabalho produzido por outra IA.
- Antes de editar arquivos, verifique o estado atual de `main` e identifique alterações recentes e áreas atribuídas a outros agentes.
- Se houver conflito, sobreposição ou dúvida sobre a autoria de uma alteração, pare e peça orientação ao mantenedor.
- Só é permitido interferir no trabalho de outra IA quando o mantenedor, Carlos Costa, autorizar explicitamente essa intervenção.
- Uma autorização vale apenas para o escopo solicitado; não representa permissão geral para modificar outros trabalhos.

## 2. Repositório e fluxo de trabalho

- Todo trabalho é realizado e salvo exclusivamente na branch `main`.
- É proibido criar ou utilizar branches de trabalho, desenvolvimento, teste, correção ou integração.
- Não abrir pull requests como mecanismo de desenvolvimento.
- Antes de publicar um commit, confirmar que o HEAD de `main` continua sendo o pai esperado. Se outra alteração tiver chegado, interromper e reavaliar; nunca sobrescrever trabalho alheio.
- Commits devem ser pequenos, descritivos e acompanhados de testes proporcionais à mudança.
- Nunca declarar que algo foi compilado, testado ou executado se isso não tiver sido realmente verificado.

## 3. Rust em todo o projeto

- Toda implementação do Aceris deve ser escrita em Rust, incluindo núcleo, lógica de aplicação, interface e frontends para desktop e Android.
- Não introduzir Kotlin, Java, C, C++, C#, JavaScript, Python ou outra linguagem como parte da implementação do produto.
- Ferramentas auxiliares externas ao produto só podem ser usadas quando necessárias ao processo de build/teste e não podem conter a implementação de funcionalidades do emulador.
- Não introduzir uma camada de aplicação em outra linguagem para contornar esta regra.

## 4. Código e recursos inteiramente próprios

- Todo código-fonte de implementação deve ser escrito especificamente para o Aceris.
- É proibido copiar, colar, traduzir, portar ou adaptar código-fonte de terceiros para o Aceris. A consulta de código-fonte como referência técnica é permitida somente nos limites da seção 5. Toda implementação deve ser escrita independentemente em Rust, com estrutura própria; não faça tradução linha a linha, reprodução estrutural ou adaptação de trechos.
- É proibido incorporar assets de terceiros: imagens, ícones, fontes, modelos, texturas, shaders, áudio, animações, layouts, temas ou outros recursos.
- Não adicionar dependências de terceiros que forneçam implementação de funcionalidades do emulador ou recursos do produto sem autorização explícita do mantenedor.
- Priorizar implementações próprias em Rust e APIs públicas de plataforma. Qualquer dependência técnica inevitável deve ser identificada, justificada e submetida ao mantenedor antes de ser introduzida.
- Não declarar o projeto integralmente livre de dependências externas sem antes auditar e resolver as dependências já existentes.

## 5. Referências técnicas de outros emuladores

- É proibido consultar código-fonte interno de outros emuladores, inclusive Azahar, Citra, Lime3DS e semelhantes. Não abrir, pesquisar, baixar, copiar ou usar arquivos-fonte, testes, fixtures, shaders, patches ou organização interna.
- A análise permitida limita-se à superfície observável do produto e à documentação pública independente de hardware, formatos, APIs e protocolos.
- Não copiar, traduzir, portar, adaptar ou reproduzir código, assets, interfaces ou organização interna de terceiros. Toda implementação do Aceris deve ser original e em Rust.
- Se uma fonte misturar documentação independente com código interno de emulador, não consultar a parte de código. Em caso de dúvida, interromper e pedir orientação ao mantenedor.
- Registrar fontes documentais independentes, URLs, datas, perguntas técnicas, nível de confiança e testes próprios. Comportamento observado em outro emulador não prova o comportamento físico do console.
- Assets e recursos visuais/sonoros de terceiros continuam proibidos. Não incorporar dependências que forneçam implementação do emulador sem autorização explícita.
- Todas as demais regras deste documento permanecem inalteradas: Rust em todo o produto, autoria própria, somente `main`, isolamento entre agentes e proteção de conteúdo.

## 6. Verificação e proteção do trabalho

- Inspecionar o diff antes de cada commit e confirmar que nenhum arquivo pertencente ao trabalho de outra IA foi alterado.
- Não apagar nem substituir arquivos apenas por parecerem incompletos; primeiro entender sua autoria e finalidade.
- Manter jogos, firmware, chaves, dumps e outros conteúdos protegidos fora do repositório.
- Em caso de dúvida sobre autoria, procedência de código/asset, linguagem, dependência ou escopo de edição, não prosseguir sem autorização do mantenedor.

## 7. Autoridade

Carlos Costa é o mantenedor e a autoridade final sobre escopo, prioridades e exceções. Somente uma autorização explícita dele pode abrir uma exceção a estas regras. Nenhuma IA pode conceder essa autorização por conta própria.


## 8. Estado verificado — 22/11/2026

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.

Esta atualização foi autorizada pelo mantenedor e limita-se aos arquivos Markdown. Nenhum arquivo de implementação foi alterado.
