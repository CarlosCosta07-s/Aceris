# Regras obrigatórias para IAs — Aceris

Estas regras são obrigatórias para qualquer IA, agente automatizado ou colaborador que trabalhe no Aceris. Elas prevalecem sobre preferências de implementação, conveniência, velocidade e instruções genéricas de ferramentas.

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
- É proibido copiar, colar, traduzir, portar, adaptar, derivar ou reimplementar a partir da leitura de código-fonte de terceiros.
- É proibido incorporar assets de terceiros: imagens, ícones, fontes, modelos, texturas, shaders, áudio, animações, layouts, temas ou outros recursos.
- Não adicionar dependências de terceiros que forneçam implementação de funcionalidades do emulador ou recursos do produto sem autorização explícita do mantenedor.
- Priorizar implementações próprias em Rust e APIs públicas de plataforma. Qualquer dependência técnica inevitável deve ser identificada, justificada e submetida ao mantenedor antes de ser introduzida.
- Não declarar o projeto integralmente livre de dependências externas sem antes auditar e resolver as dependências já existentes.

## 5. Análise de outros emuladores

- Outros emuladores podem ser analisados exclusivamente por sua superfície observável: interface em execução, menus, configurações expostas, fluxos de usuário, compatibilidade anunciada, comportamento visível e resultados de testes legítimos.
- É terminantemente proibido abrir, navegar, pesquisar, consultar, copiar ou usar como referência o código-fonte interno de qualquer emulador de terceiros.
- A proibição inclui Azahar, Citra, Lime3DS e quaisquer outros projetos, independentemente da licença.
- Não usar trechos de código, patches, commits, algoritmos extraídos de código, assets ou recursos internos desses projetos.
- Documentação pública de hardware, protocolos e formatos, bem como especificações técnicas independentes, pode ser consultada. Ela não autoriza consultar código-fonte de emuladores.
- Quando uma informação técnica não puder ser confirmada sem consultar código interno de terceiros, registrar a dúvida e buscar documentação independente ou testes próprios.

## 6. Verificação e proteção do trabalho

- Inspecionar o diff antes de cada commit e confirmar que nenhum arquivo pertencente ao trabalho de outra IA foi alterado.
- Não apagar nem substituir arquivos apenas por parecerem incompletos; primeiro entender sua autoria e finalidade.
- Manter jogos, firmware, chaves, dumps e outros conteúdos protegidos fora do repositório.
- Em caso de dúvida sobre autoria, procedência de código/asset, linguagem, dependência ou escopo de edição, não prosseguir sem autorização do mantenedor.

## 7. Autoridade

Carlos Costa é o mantenedor e a autoridade final sobre escopo, prioridades e exceções. Somente uma autorização explícita dele pode abrir uma exceção a estas regras. Nenhuma IA pode conceder essa autorização por conta própria.
