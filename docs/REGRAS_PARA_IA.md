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

- Outros emuladores podem ser estudados como referências técnicas, inclusive por consulta ao código-fonte publicamente disponível, para compreender abordagens, arquitetura, tratamento de casos-limite e hipóteses de comportamento.
- Essa permissão é exclusivamente de pesquisa. Não autoriza copiar, colar, traduzir, portar, adaptar ou reproduzir trechos, funções, estruturas, testes, fixtures, shaders, patches, assets ou organização interna.
- Não faça tradução linha a linha nem uma reimplementação que reproduza a estrutura expressiva do código consultado. O código do Aceris deve ser concebido e escrito independentemente em Rust, com arquitetura, interfaces, nomes e testes próprios.
- A permissão inclui, entre outros, Azahar, Citra, Lime3DS e outros projetos, independentemente da licença. A licença de cada referência deve ser registrada; se houver dúvida sobre compatibilidade de licença ou risco de obra derivada, interrompa o uso daquela referência na implementação e peça orientação ao mantenedor.
- Para cada consulta de código, registre projeto, URL, arquivo ou área consultada, commit/revisão, licença, data e a pergunta técnica investigada. Registre conclusões conceituais em uma especificação independente, sem colar trechos de código.
- Confirme conclusões relevantes com documentação de hardware/protocolos, testes originais do Aceris e, quando possível, observações em hardware legítimo. Código de outro emulador não é, sozinho, prova de comportamento fiel ao console.
- Assets e recursos visuais/sonoros de terceiros continuam proibidos. Não incorporar dependências que forneçam implementação do emulador sem autorização explícita.
- Todas as demais regras deste documento permanecem inalteradas: Rust em todo o produto, autoria própria, somente `main`, isolamento entre agentes e proteção de conteúdo.

## 6. Verificação e proteção do trabalho

- Inspecionar o diff antes de cada commit e confirmar que nenhum arquivo pertencente ao trabalho de outra IA foi alterado.
- Não apagar nem substituir arquivos apenas por parecerem incompletos; primeiro entender sua autoria e finalidade.
- Manter jogos, firmware, chaves, dumps e outros conteúdos protegidos fora do repositório.
- Em caso de dúvida sobre autoria, procedência de código/asset, linguagem, dependência ou escopo de edição, não prosseguir sem autorização do mantenedor.

## 7. Autoridade

Carlos Costa é o mantenedor e a autoridade final sobre escopo, prioridades e exceções. Somente uma autorização explícita dele pode abrir uma exceção a estas regras. Nenhuma IA pode conceder essa autorização por conta própria.
