# Validação manual de compatibilidade — Aceris

## Regra de validação

Todos os testes que carregam ou executam homebrew ou jogos comerciais são realizados manualmente pelo mantenedor, diretamente no Aceris. A CI não deve baixar, armazenar, executar ou exigir ROMs, CIAs/CCIs/3DSX, firmware, chaves ou dados extraídos de jogos.

A automação cobre somente código e contratos sintéticos/autocontidos: formatação, compilação, lint, testes unitários, testes de protocolos com fixtures próprias, benchmark de componentes e empacotamento do aplicativo. Uma suíte automatizada verde não declara compatibilidade de título.

## Procedimento no dispositivo

1. Instale a versão do Aceris gerada pelo workflow de APK ou o build desktop correspondente.
2. Abra a biblioteca e selecione um arquivo local obtido legalmente. O arquivo e quaisquer dados protegidos permanecem apenas no dispositivo.
3. Registre se o contêiner foi reconhecido, se os metadados foram identificados e se a sessão chegou à primeira imagem.
4. Teste a interação com os controles e a transição entre telas/menu. Não use apenas a abertura do arquivo como critério de jogabilidade.
5. Teste áudio separadamente: inicialização, saída audível, continuidade e sincronização.
6. Observe estabilidade por alguns minutos, pausas, retorno à biblioteca e encerramento/reabertura.
7. Quando ocorrer uma falha, preserve a mensagem e os logs do Aceris e anote a etapa exata em que ocorreu.

## Ficha de resultado

Copie e preencha uma ficha por título. Não envie o jogo, firmware, chaves, saves ou arquivos extraídos.

| Campo | Resultado |
|---|---|
| Identificador do teste | |
| Tipo | Homebrew / Comercial |
| Título e versão/região (se identificáveis) | |
| Formato do arquivo | 3DS / CCI / CIA / 3DSX / outro |
| Sistema | Windows / Linux / macOS / Android |
| CPU, GPU e RAM do dispositivo | |
| Commit/build do Aceris | |
| Arquivo reconhecido | Sim / Não |
| Sessão iniciada | Sim / Não |
| Primeira imagem | Sim / Não |
| Gráficos | Correto / Parcial / Falhou |
| Controles | Correto / Parcial / Falhou |
| Áudio | Correto / Parcial / Ausente / Falhou |
| Estabilidade | Estável / Lento / Travou / Fechou |
| FPS/frametime observado (se disponível) | |
| Mensagem ou etapa da falha | |
| Logs/diagnóstico relevante | |

## Critérios de classificação

- **Reconhecido:** o formato e os metadados foram identificados.
- **Inicializa:** o processo guest começa e os serviços iniciais respondem.
- **Primeiro frame:** a imagem apresentada é produzida pelo estado do guest, não por uma tela de demonstração do host.
- **Executa:** há progresso observável além da tela inicial.
- **Jogável:** controles, áudio e renderização permitem jogar sem bloqueios críticos.
- **Compatível:** o mantenedor repetiu a sequência de teste e confirmou estabilidade suficiente para o escopo declarado.

Esses níveis são distintos. Nunca classificar como jogável ou compatível somente porque o arquivo abriu ou a primeira imagem apareceu.

## Proteção do repositório

- Nenhum jogo, imagem de disco, firmware, chave, save, dump, shader extraído ou outro dado protegido vai para o Git, CI, issues ou releases.
- Não anexar arquivos de jogo a logs/relatórios. Compartilhe somente metadados, mensagens e trechos de diagnóstico que não contenham conteúdo protegido.
- A ausência de um título nos testes automatizados é intencional: a execução do título pertence ao mantenedor, em ambiente local.
