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

## 5. Referências de código aberto de outros emuladores

- É permitido consultar código-fonte de emuladores e ferramentas que sejam efetivamente open source, inclusive Azahar, Citra, Lime3DS e semelhantes, para investigar como resolvem problemas técnicos do Aceris.
- Antes de consultar cada repositório e arquivo, verificar a licença do projeto, a licença específica do arquivo e eventuais avisos de terceiros. Um repositório público no GitHub não é automaticamente open source; arquivos distintos podem ter cabeçalhos/licenças diferentes.
- A consulta pode abranger algoritmos, estados internos, protocolos, casos-limite, testes e estratégias de compatibilidade. Usar essa informação para compreender o problema e elaborar uma especificação técnica própria para o Aceris.
- Toda implementação do produto deve continuar sendo escrita em Rust, com arquitetura, estruturas, APIs, testes e organização próprios. Não copiar trechos, traduzir linha a linha, portar ou reproduzir estruturalmente uma implementação de terceiros como forma de contornar a licença.
- A inspiração técnica e a compreensão de um algoritmo não autorizam incorporar código protegido. Se uma solução exigir reutilização ou adaptação direta de código, parar e pedir autorização explícita ao mantenedor; antes de qualquer uso, avaliar compatibilidade da licença com a licença do Aceris, preservar avisos e cumprir todas as obrigações aplicáveis. Nunca presumir que código GPL possa ser incorporado a um projeto MIT sem consequências.
- Não copiar testes, fixtures, shaders, patches, assets, interfaces ou documentação proprietária. Cenários de teste podem inspirar testes novos, desde que os dados e a implementação sejam criados independentemente e não contenham material protegido.
- Registrar a fonte consultada (repositório, revisão, caminho, licença e data), a pergunta técnica, o entendimento obtido, o que foi implementado de forma independente e o nível de confiança. Comportamento de outro emulador não prova, por si só, comportamento do hardware.
- Assets e recursos visuais/sonoros de terceiros continuam proibidos. Não introduzir dependências que forneçam implementação do emulador sem autorização explícita.
- Todas as demais regras permanecem: Rust em todo o produto, autoria própria, somente `main`, isolamento entre agentes e proteção de conteúdo.

## 6. Verificação e proteção do trabalho

- Inspecionar o diff antes de cada commit e confirmar que nenhum arquivo pertencente ao trabalho de outra IA foi alterado.
- Não apagar nem substituir arquivos apenas por parecerem incompletos; primeiro entender sua autoria e finalidade.
- Manter jogos, firmware, chaves, dumps e outros conteúdos protegidos fora do repositório.
- Em caso de dúvida sobre autoria, procedência de código/asset, linguagem, dependência ou escopo de edição, não prosseguir sem autorização do mantenedor.

## 7. Autoridade

Carlos Costa é o mantenedor e a autoridade final sobre escopo, prioridades e exceções. Somente uma autorização explícita dele pode abrir uma exceção a estas regras. Nenhuma IA pode conceder essa autorização por conta própria.


### 7.1 Autorizações vigentes

- **26/11/2026 — autorização total ao Claude.** Carlos Costa autorizou Claude a desenvolver qualquer parte do código do Aceris, independentemente da IA que a tenha produzido, para executar a lista de trabalho: (1) consistência das regras, (2) kernel ARM11, (3) estágios pendentes da PICA200, (4) CSND e áudio, (5) desempenho, (6) validação em Android físico e (7) testes com outros homebrews. É uma exceção explícita à seção 1 dentro desse escopo. Todas as demais regras continuam valendo: somente `main`, produto em Rust, autoria própria, consulta a código open source apenas como referência (seção 5) e proteção de conteúdo.

## 8. Precedência e estado do projeto

- Este arquivo é a fonte normativa. `CONTRIBUTING.md`, `docs/INDEPENDENT_RESEARCH.md` e `docs/AI_HANDOFF.md` devem apenas repeti-lo ou detalhá-lo; em caso de divergência, vale este arquivo.
- Registros datados em `docs/AI_HANDOFF.md` que digam “não consultar código-fonte de outros emuladores” descrevem a política da época (anterior a 17/11/2026) e não estão mais em vigor.
- Este arquivo não guarda estado de desenvolvimento. O estado atual fica no [README](../README.md), no [roadmap](ROADMAP.md) e nas [notas de transferência](AI_HANDOFF.md).
