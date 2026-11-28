# Matriz de capacidades reais do Aceris

> Auditoria de produto e código — 27/11/2026. Complementa o [plano de implementação](PRODUCT_IMPLEMENTATION_PLAN.md) e a [especificação de experiência](USER_EXPERIENCE.md). Retrata o código observado nesta revisão e deve ser atualizada junto às funcionalidades.

**Regra de leitura:** suporte no core, capacidade na ABI e opção disponível na interface são três níveis diferentes. Um tipo, registrador, parser, sink de áudio, helper ou função exportada não significa por si só que o usuário tenha uma opção funcional.

## 1. Legenda

- **Disponível:** fluxo ligado à aplicação e com comportamento observável no escopo indicado.
- **Parcial:** existe parte do fluxo, mas há restrições, ausência de integração completa ou validação pendente.
- **Não disponível:** não há ação funcional exposta ao usuário.
- **Não validado em hardware:** pode compilar ou ter teste automatizado, mas falta confirmação em dispositivo ou driver físico.

## 2. Biblioteca e navegação

| Capacidade | Estado | Comportamento observado e limite |
|---|---|---|
| Página inicial | Disponível | Mostra retomada do título recente, contador de favoritos, coleção homebrew e indicação de mídia externa; ainda sem capas reais. |
| Jogos, Favoritos, Recentes e Homebrew | Disponível | Seções próprias na biblioteca compartilhada em Rust. |
| Pesquisa | Disponível | Pesquisa local pelo título derivado do nome e pelo caminho; não consulta Title ID nem catálogo externo. |
| Favoritos | Disponível | Persistidos em .aceris/favorites.txt, identificados por caminho completo com prefixo versionado. |
| Recentes | Disponível | Persistidos em .aceris/recent.txt; mantém até 24 identificadores e a ordem de execução. |
| Grade e lista | Parcial | Alternância presente na biblioteca horizontal; retrato usa fallback responsivo. Grade/Lista e ordenação A–Z/Z–A são persistidas em `.aceris/library_preferences_v1.txt`. |
| Ordenação A–Z/Z–A | Disponível | Alterna a ordem alfabética da coleção atual. Recentes preserva a ordem de execução. Não há ordenação por tamanho, data de adição ou tempo de jogo. |
| Filtro por extensão | Disponível | Filtra Todos, .3DS, .CCI, .CIA e .3DSX sem diferenciar caixa. É uma classificação pelo nome do arquivo, não valida conteúdo, instalação ou compatibilidade. |
| Atualização manual | Disponível | Reexecuta a varredura da biblioteca em memória. |
| Várias pastas | Parcial | Modelo persistente de várias raízes e opção de recursão; a configuração atual usa caminhos de filesystem. |
| Recursão | Disponível | Pode ser ativada/desativada; limitada em profundidade e sem seguir links simbólicos durante a travessia. |
| Adicionar pasta | Parcial | Usuário digita um caminho existente. Ainda não há seletor nativo de diretório no desktop nem SAF Android. |
| Remover pasta | Parcial | Remove raiz configurada e revarre; a última raiz não pode ser removida. A interface lista até três caminhos diretamente e informa quantos adicionais existem. |
| Progresso/cancelamento da varredura | Não disponível | A varredura é síncrona no fluxo atual; não há progresso detalhado nem cancelamento pelo usuário. |
| Varredura incremental/monitoramento automático | Não disponível | Há varredura ao iniciar e atualização manual; não há watcher de filesystem nem cache incremental de índice. |
| Duplicatas | Parcial | O scanner deduplica caminhos e raízes sobrepostas. Não identifica cópias byte-a-byte do mesmo jogo em caminhos diferentes. |
| Seleção múltipla/ações em lote | Não disponível | Não há seleção múltipla da biblioteca. |
| Capas e metadados de jogos | Não disponível | A grade usa cartões genéricos; não há busca ou cadastro de capa/Title ID pela interface. |
| Página detalhada do jogo | Não disponível | Não há painel de propriedades do título. |

**Evidência principal:** crates/aceris-core/src/video/library.rs — LibraryPage, LibraryFormatFilter, LibraryScanConfig, scan_games_from_directories, Library::from_scan_settings, toggle_view_mode, toggle_sort_order, filtered_entries, render_landscape e click_responsive.

## 3. Formatos, carregamento e identidade dos jogos

| Capacidade | Estado | Comportamento observado e limite |
|---|---|---|
| Scanner .3DS/.CCI | Parcial | Extensões são listadas. O carregamento NCSD comum depende das condições NoCrypto suportadas pelo loader; a extensão sozinha não valida o arquivo. |
| Carregar .3DSX | Parcial | Caminho de homebrew existe no loader. A presença no catálogo não garante que toda aplicação homebrew funcione. |
| Carregar .CIA | Parcial | Lançamento direto apenas quando o conteúdo de boot está em claro e o NCCH é NoCrypto. Não existe instalação persistente ou gestão de títulos/atualizações pela biblioteca. CIA criptografado é recusado pelo caminho normal. |
| Conteúdo criptografado | Não disponível no lançamento normal | O caminho usado pela biblioteca recusa NCSD/CIA criptografados e aceita somente NCCH NoCrypto e conteúdo CIA em claro. Rotinas criptográficas de baixo nível permanecem para testes internos, mas não estão integradas à experiência de lançamento. Aceris não descobre, incorpora nem persiste chaves. |
| Identidade de título por Title ID | Parcial | System::game_identity expõe o program ID de NCCH em NCSD/CCI e o Title ID do TMD em CIA após conferir o NCCH. 3DSX permanece sem identidade; região, produto, versão/revisão e metadados na biblioteca ainda faltam. SourceId da biblioteca continua baseado no caminho. |
| Relatório de compatibilidade por título | Não disponível | Há mensagens de erro e diagnóstico de execução, mas não há classificação de compatibilidade ou histórico por jogo. |

**Evidência principal:** crates/aceris-core/src/video/library.rs, crates/aceris-core/src/loader.rs, frontends/android/src/lib.rs, frontends/desktop/src/main.rs e docs/FRONTENDS.md.

## 4. Jogo, telas e entrada

| Capacidade | Estado | Comportamento observado e limite |
|---|---|---|
| Apresentar as duas telas | Disponível | Composição das telas LCD existentes para a superfície do host, mantendo proporção. |
| Modos de tela básicos | Parcial | Menu oferece lado a lado, vertical, superior ampliada, inferior ampliada e somente superior/inferior; Android segue rotação e desktop redimensiona a janela. Perfil global persistido separadamente para retrato/paisagem; override por jogo e teste físico pendentes. |
| Trocar tela principal/secundária | Parcial | A ação Trocar telas troca a posição dos LCDs sem perder a identidade física; o toque continua ligado ao LCD inferior. Falta validação em aparelho e nos backends desktop. |
| Layout personalizado/ajuste fino/janela separada | Não disponível | Não há editor de posição, tamanho ou espaçamento, nem janela independente por tela. |
| Touchscreen da tela inferior | Parcial | O mapeamento usa o retângulo atual do LCD inferior nos seis modos e após a troca; ainda falta validar toque físico em aparelho e gestos complexos. |
| Botões digitais | Parcial | O core define bits para D-pad, A/B/X/Y, L/R, Start/Select e ZL/ZR; hosts encaminham um subconjunto pelos caminhos atuais. |
| Circle Pad analógico | Parcial | Android encaminha o eixo primário X/Y de eventos joystick USB/Bluetooth ao Circle Pad (±156), com deadzone padrão de 16% e inversão do eixo Y. Ainda faltam perfis/calibração, ajustes de deadzone/sensibilidade, eixos adicionais, gamepad físico no desktop e validação em aparelho real. |
| Perfis de controle/remapeamento | Parcial | Controles virtuais Android têm layout global e override por Title ID; o remapeamento de teclado/gamepad físico e perfis por dispositivo ainda não estão disponíveis. |
| Deadzone/sensibilidade/inversão | Não disponível | Não existem controles de ajuste expostos para dispositivos físicos; a escala/posição dos controles virtuais é uma função separada. |
| Controles virtuais Android | Parcial | Editor funcional para visibilidade, opacidade em níveis, escala, presets, posições por arraste, tamanho por pinça, layouts por orientação e herança/overrides por Title ID. Falta validação em aparelho físico e melhor integração com controles físicos. |
| Multi-touch | Parcial | NativeActivity coleta ponteiros e o app mantém estado multitoque; falta validar em aparelho e todas as combinações de stylus + botões. |
| Gamepad físico Android | Parcial | Eventos joystick encaminham o eixo primário X/Y para o Circle Pad com deadzone de 16%; eventos de tecla mapeiam D-pad, A/B/X/Y, L1/R1, L2/R2 para L/R/ZL/ZR e Start/Select. Sem remapeamento, seleção de dispositivo, configuração de deadzone, eixos adicionais ou validação em controle físico. |
| Menu rápido/pausa | Parcial | Android tem botão MENU e Voltar; PC usa Esc. Continuar, Reiniciar e Voltar têm confirmação preventiva; as filas host de áudio são limpas ao pausar. O menu oferece seis ações, seis layouts, troca de telas e restauração do padrão global. Atalho configurável, ajuste fino e validação física pendentes. |
| Atalhos configuráveis | Não disponível | Existem teclas/eventos específicos do host, mas não há editor de atalhos. |
| Screenshot durante a partida | Parcial | Ação Capturar tela no menu rápido salva a composição atual em PNG e mostra confirmação no painel. Android grava em `.aceris/screenshots` dentro dos dados do app; desktop grava em `screenshots` na pasta de configurações e imprime o caminho. Escolha de destino por diálogo e validação física ainda pendentes. |

**Evidência principal:** crates/aceris-core/src/input.rs (InputState, InputSnapshot, pad e touch_from_surface_point), crates/aceris-core/src/video/frontend.rs, frontends/android/src/app.rs (apply_game_input, hit_virtual_button e step_game_into) e frontends/android/src/native_activity.rs.

## 5. Configurações, preferências e dados de usuário

| Capacidade | Estado | Comportamento observado e limite |
|---|---|---|
| Modo Desempenho/Fast Memory | Disponível, avançado | Opt-in persistido em .aceris/performance_mode.txt e aplicado antes do próximo jogo. Desvia do modelo preciso de D-cache e pode causar incompatibilidade; não é ganho garantido. |
| Log de desempenho desktop | Parcial | Opt-in por ACERIS_PERFORMANCE_LOG=1 ou arquivo .aceris/desktop_performance_log_v1.txt; imprime relatórios locais de tempo host. |
| Overlay de diagnóstico em jogo | Parcial | A ação Diagnóstico do menu rápido ativa um painel em desktop e Android com PC ARM11, quantidade/primeira requisição IPC não tratada e resumo opcional de tempos/FPS apresentados pelo host. Não calcula velocidade guest. Ainda faltam métricas guest reais, refinamento do painel e validação física. |
| Métricas de desempenho Android | Parcial | Monitor registra tentativas, apresentação, percentis e uso do presenter; relatórios são diagnósticos locais, não overlay configurável de usuário. |
| Fast Forward/velocidade alternativa | Não disponível | Não há controle de velocidade de emulação na interface; não confundir FPS host com velocidade guest. |
| FPS/velocidade guest em overlay | Parcial | Overlay apresenta apenas métricas host claramente identificadas (FPS apresentado e tempos host, quando a coleta opt-in está ativa). FPS/velocidade guest ainda não é medido nem exibido. |
| Mídia externa | Parcial | O host Android pode detectar mídia externa e informar estado à biblioteca; não é oferecido por todas as plataformas e não identifica o conteúdo tocado. |
| Preferências globais completas | Não disponível | Ainda não há painel de idioma, tema, escala de interface, alto contraste ou conjunto amplo de preferências. |
| Configurações por jogo | Parcial | Overrides de layout por Title ID, com herança global, orientação independente e restauração por título. Categorias gerais de configurações por jogo ainda não existem; 3DSX sem identidade herda o perfil global. |
| Backup/exportação de configurações | Não disponível | Não há fluxo de exportação/importação. |
| Save data/extdata do jogo | Parcial | Diretórios de armazenamento por jogo existem nos caminhos HLE atuais. Isso não equivale a suporte completo a todos os serviços de armazenamento do console. |
| Save states | Infraestrutura parcial; recurso de jogo indisponível | Core possui envelope e tabela de seções versionados, checksum, identidade por hash exato do jogo e armazenamento de slots por gerações imutáveis com fallback. Ainda não existe captura/restauração completa do `System`, integração nos frontends ou slots utilizáveis durante a partida. |
| Auto-save/rotação de backups | Infraestrutura parcial | O armazenamento de save states retém gerações anteriores e recupera uma geração válida; ainda não há snapshot completo, auto-save ou Rewind expostos ao usuário. |

**Evidência principal:** crates/aceris-core/src/video/library.rs (performance_mode e set_performance_mode), frontends/desktop/src/performance.rs, frontends/android/src/performance.rs, frontends/desktop/src/main.rs e frontends/android/src/app.rs.

## 6. Gráficos, áudio e integrações

| Capacidade | Estado | Comportamento observado e limite |
|---|---|---|
| Presenter de GPU do host | Parcial | aceris-graphics usa wgpu para upload/apresentação do framebuffer LCD já composto; não substitui o rasterizador PICA200 guest. |
| Fallback CPU de apresentação | Disponível | O desktop mantém caminho CPU quando o presenter não pode ser usado. |
| Resolução interna ampliada | Não disponível | Não há opção funcional de resolução interna ampliada para PICA200. |
| Filtros de textura/AA/shader cache | Não disponível como opção de usuário | Não expor controles que não tenham efeito real e testes de regressão. |
| Substituição de texturas | Não disponível | Não existe carregamento de pacotes locais, associação por Title ID/hash ou cache de texturas substituídas. |
| VSync/limitador de quadros configuráveis | Não disponível como opção de usuário | Ritmo de renderização dos hosts não equivale a um painel de configuração de sincronização. |
| Sink de áudio desktop/Android | Parcial | Existem saídas de host ALSA/WinMM/AudioQueue/AAudio, mas o caminho de áudio guest continua em desenvolvimento; docs/FRONTENDS.md registra falha em Sound.init no homebrew de ensaio e ausência de áudio audível confirmado. |
| Volume/dispositivo/latência em UI | Não disponível | Não há controle completo na interface. |
| Relatórios de diagnóstico exportáveis | Não disponível | Há logs locais e mensagens de falha; não há prévia sanitizada e exportação controlada pelo usuário. |
| Cheats | Não disponível | Sem gerenciador de códigos por título. |
| Substituição de textura | Não disponível | Sem pipeline de substituição apresentado ao usuário. |
| Câmera, microfone, sensores, NFC e multiplayer | Não disponível | Requerem HLE, integração da plataforma, permissões e validação específica. |
| HLE `ndm:u` | Parcial | Modela estado exclusivo, bloqueio, suspensão aninhada dos quatro daemons e intervalos; não executa networking nem daemons online. Comandos não suportados são registrados e retornam erro, sem sucesso genérico. |
| HLE `ptm:sysm` | Parcial | Responde consultas de estado da bateria, tampa, passos, modelo O3DS e flag de encerramento com valores determinísticos; operações de energia e LED permanecem não implementadas e são diagnosticadas. |
| HLE `gsp::Lcd` | Não disponível | O endpoint pode ser aberto, mas seus comandos ainda não têm implementação; eles deixam diagnóstico em `unhandled_requests()` e não retornam falso sucesso. |
| Sincronização em nuvem/conquistas | Não disponível | Sem integração online habilitada na experiência do usuário. |

**Evidência principal:** crates/aceris-graphics/src/lib.rs, frontends/desktop/src/audio.rs, frontends/android/src/aaudio.rs, crates/aceris-core/src/csnd.rs, crates/aceris-core/src/dsp.rs e docs/FRONTENDS.md.

## 7. Plataforma, acessibilidade e confiabilidade

| Capacidade | Estado | Comportamento observado e limite |
|---|---|---|
| SVC GetResourceLimit (0x38–0x3A) | Disponível para os 10 tipos ABI | Consulta limites do perfil APPLICATION 64 MiB e uso atual de COMMIT, threads, eventos, mutexes, semáforos, timers, shared memory e address arbiters. Priority e CPUTime têm valores de perfil explícitos. Isso não significa que todas as quotas já sejam impostas por cada caminho de alocação. |
| ControlMemory ALLOC/MAP/UNMAP/PROTECT | Parcial, heap regular e aliases | ALLOC aceita endereço fixo ou `addr0 = 0` com first-fit page-aligned. MAP/UNMAP cria e remove aliases de heap; as quatro combinações R/W são válidas. PROTECT altera permissões de uma alocação inteira de heap ou de um alias compartilhado e invalida caches. Proteção de subfaixas, memória linear/estática, `MEMOP_RESERVE`, regiões especiais e validação em hardware continuam pendentes. |

| Capacidade | Estado | Comportamento observado e limite |
|---|---|---|
| UI compartilhada em Rust | Disponível | Biblioteca e modelo de UI compartilhados; hosts recebem eventos e apresentam o frame. |
| Seletor nativo de diretórios desktop | Não disponível | Configuração atual pede caminho digitado. |
| Android SAF e permissões persistentes | Não disponível | A biblioteca trabalha com caminhos de filesystem; URIs de documentos não são tratadas como origem do scanner. |
| Ciclo de vida Android | Parcial/não validado em hardware | NativeActivity trata callbacks de janela, foco, pause/resume e fila de input; FRONTENDS.md registra ausência de execução em aparelho físico. |
| Temas claro/escuro | Não disponível | Paleta da interface estática. |
| Localização selecionável | Não disponível | Textos atuais são essencialmente PT-BR; não há seletor nem sistema completo de tradução. |
| Escala de UI/alto contraste/tamanho de texto | Não disponível | Não há ajustes de acessibilidade na interface. |
| Navegação completa sem mouse | Parcial | A biblioteca recebe alguns comandos direcionais/confirmar; não existe cobertura de teclado/controle para todos os menus e páginas. |
| Mensagens de erro | Parcial | Há status na biblioteca e diagnóstico de falhas de execução/carregamento; não existe centro de ajuda, histórico estruturado ou ação contextual abrangente. |
| Atualizador integrado | Não disponível | Não há fluxo integrado de atualização/canais/changelog. |
| Hardware Android real | Não validado | Build APK/ARM64 na CI não comprova instalação, renderização, toque, áudio ou desempenho em aparelho. |
| Hardware desktop real | Parcial/depende de matriz manual | Backends e builds por SO constam da documentação; drivers GPU, áudio e controles precisam validação em equipamentos reais. |

**Evidência principal:** frontends/android/src/native_activity.rs, frontends/android/src/app.rs, frontends/desktop/src/main.rs, crates/aceris-core/src/video/library.rs e docs/FRONTENDS.md.

## 8. Regras de exposição na interface

1. Não apresentar .CIA como "Instalar" enquanto não existir instalação persistente com validação, atualização e remoção.
2. Não chamar a escala de apresentação de resolução interna ampliada.
3. Não chamar save data/extdata de save state.
4. Não apresentar contagem de frames apresentados pelo host como FPS guest.
5. Não apresentar Circle Pad no modelo de entrada como editor de gamepad pronto.
6. Não apresentar detecção de mídia externa como configuração universal de áudio.
7. Não expor gráficos avançados, cheats, Rewind, filtros, texturas, sensores, câmera ou serviços online antes de existir implementação integrada e testada.
8. Indicar quando a função estiver limitada a uma plataforma, backend, formato ou classe de arquivo.
9. Mostrar claramente quando uma opção pode causar incompatibilidade, especialmente Fast Memory.
10. Atualizar esta matriz junto ao código sempre que uma funcionalidade mudar de estado.

## 9. Próximas lacunas bloqueadoras

A sequência deve continuar alinhada a docs/PRODUCT_IMPLEMENTATION_PLAN.md:

1. Origem de arquivos multiplataforma que suporte caminhos desktop e URIs SAF no Android.
2. Seleção nativa de pasta e persistência das permissões concedidas.
3. Varredura com progresso, cancelamento e relatório de rejeições.
4. Identidade de título/arquivo que permita metadados e perfis por jogo sem colisões.
5. Modelo comum de ações de entrada e perfis antes do editor visual.
6. Menu de pausa real, com coordenação segura da sessão, antes de expor ações durante a partida.

A matriz é um retrato conservador do código, não uma promessa de compatibilidade de jogos.
