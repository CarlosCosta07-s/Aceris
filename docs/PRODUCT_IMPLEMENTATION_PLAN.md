# Plano de implementação do produto Aceris

> Plano executivo derivado de `docs/USER_EXPERIENCE.md`. Revisão: 27/11/2026. Este documento ordena o trabalho por dependência e define critérios objetivos de conclusão. Os itens marcados como pendentes são trabalho futuro; não representam recursos já disponíveis. A especificação complementar de menu rápido e configurações está em [`QUICK_MENU_SETTINGS_SPEC.md`](QUICK_MENU_SETTINGS_SPEC.md).

## 1. Objetivo e regras de execução

Entregar uma experiência coerente entre PC e Android, com identidade própria, interface simples para uso casual e configurações avançadas quando necessárias. O núcleo e a lógica compartilhada continuam em Rust; diferenças de plataforma ficam restritas a adaptadores de entrada, janela, armazenamento, áudio e ciclo de vida.

Regras obrigatórias:

- Desenvolver somente na branch `main`; não criar branches.
- Preservar a autoria e os limites de trabalho documentados em `docs/AI_HANDOFF.md` e `docs/REGRAS_PARA_IA.md`. A autorização geral mais recente do mantenedor deve prevalecer sobre notas históricas mais restritas.
- Não copiar código, recursos visuais, textos, testes ou organização interna de outros emuladores. PPSSPP, PCSX2, Azahar e Citra são referências de requisitos e experiência, não modelos para reprodução.
- Não adicionar jogos, firmware, chaves, saves reais ou conteúdo protegido ao repositório, CI, issues ou releases.
- Não apresentar como funcional uma opção que só tenha componente visual, stub ou caminho parcial.
- Não apagar arquivos originais, saves ou configurações sem ação explícita e confirmação compreensível.
- Cada mudança deve incluir testes apropriados e atualizar `docs/ROADMAP.md`, `docs/FRONTENDS.md` ou `docs/USER_EXPERIENCE.md` quando alterar o estado descrito.
- Relatar builds, CI, testes físicos e ganhos de desempenho separadamente. Build aprovado não é prova de compatibilidade ou de funcionamento em hardware real.

## 2. Estado inicial considerado

O estado abaixo foi consolidado a partir de `docs/USER_EXPERIENCE.md`, `docs/FRONTENDS.md` e `docs/ROADMAP.md` consultados em 27/11/2026. Antes de cada marco, revalidar o código e os documentos, pois a implementação pode avançar em paralelo.

### Já disponível, com limites documentados

- Biblioteca com páginas Jogos, Favoritos, Recentes, Homebrew e Configurações.
- Pesquisa local por título, nome de arquivo e caminho.
- Grade/lista nos contextos suportados pelo layout atual; preferência e ordenação já são persistidas no arquivo versionado da biblioteca.
- Favoritos e Recentes persistidos.
- Filtro de extensão para formatos listados pelo scanner (.3DS, .CCI, .CIA e .3DSX); listar CIA não significa instalar títulos.
- Gerenciamento básico de raízes de biblioteca, recursão opcional e persistência da configuração do scanner.
- Atualização manual da biblioteca, ordenação e plano de fundo personalizado.
- Apresentação das duas telas e entrada básica por teclado/toque, com uma camada inicial de controles virtuais.
- Modo Desempenho opcional, baseado em fast memory, com ressalva de possíveis diferenças de coerência de cache.
- Estrutura de dados de usuário por jogo nos caminhos HLE atualmente implementados.

### Lacunas que condicionam o plano

- Seleção de pastas por diálogo nativo no desktop e seleção de diretórios via Android Storage Access Framework (SAF), com permissões persistentes.
- Progresso, cancelamento, resumo e recuperação de erros do processo de importação/varredura.
- Persistência ampliada de metadados da biblioteca e página detalhada de jogo.
- Mapeamento completo de teclado/gamepad, detecção/reconexão, perfis e editor de controles virtuais.
- Menu rápido básico implementado, incluindo limpeza explícita da fila host de áudio, confirmação preventiva antes de Reiniciar/Voltar à biblioteca, seis layouts de tela e troca de LCDs. Atalhos configuráveis, espaçamento/posição personalizados e perfis globais/por jogo continuam pendentes. O host não detecta automaticamente progresso não salvo.
- Configurações globais com overrides por jogo e migrações seguras.
- Save states versionados, slots e recuperação — distintos dos saves gravados pelo próprio jogo.
- Validação física do frontend Android e dos backends desktop; saída de áudio do sistema emulado ainda não foi confirmada como funcional.
- Temas completos, acessibilidade, ajuda contextual e ferramentas de relatório em nível de produto.

## 3. Sequência de marcos

### M0 — Contratos, inventário e qualidade da experiência

**Prioridade:** bloqueador de produto.  
**Objetivo:** garantir que os próximos marcos não dupliquem funções nem dependam de pressupostos falsos.

- [x] Criar um inventário de opções reais do core e dos hosts, com origem da configuração, efeito e disponibilidade por plataforma — ver [CAPABILITY_MATRIX.md](CAPABILITY_MATRIX.md), revisão de 27/11/2026.
- [ ] Separar visualmente estados: Disponível, Experimental, Indisponível e Planejado. O estado deve vir de uma capacidade real, não de um texto fixo que possa ficar desatualizado.
- [ ] Definir contratos compartilhados para identificador de jogo, origem de arquivo, configuração e ações de menu.
- [ ] Registrar quais arquivos/módulos pertencem a cada agente antes de editar áreas compartilhadas.
- [ ] Criar uma matriz de regressão PC/Android para os fluxos principais: abrir biblioteca, encontrar jogo, iniciar, voltar à biblioteca e preservar dados.

**Critério de aceite:** nenhuma tela mostra uma ação não implementada como se estivesse disponível; cada recurso do plano tem dependências e dono de implementação identificáveis.

### M1 — Entrada de jogos e biblioteca confiável

**Prioridade:** P0 — primeira entrega de experiência.  
**Objetivo:** o usuário adicionar seus próprios jogos sem precisar digitar caminhos manualmente nem entender detalhes do sistema de arquivos.

**M1.1 — Acesso às pastas**
- [ ] Desktop: seletor nativo de pasta, múltiplas raízes, permissões/erros claros e persistência da escolha.
- [ ] Android: fluxo SAF para selecionar uma pasta, manter o URI e persistir a permissão concedida pelo usuário.
- [ ] Introduzir uma abstração de origem/leitura de arquivos para que o scanner não presuma que toda origem Android seja um caminho POSIX comum.
- [ ] Permitir remover o acesso a uma origem sem apagar o conteúdo armazenado nela.
- [ ] Mostrar quando uma pasta escolhida deixou de estar disponível ou perdeu permissão.

**M1.2 — Varredura**
- [ ] Mostrar progresso, pasta atual, arquivos examinados e resumo final.
- [ ] Permitir cancelar sem deixar a biblioteca em estado parcialmente corrompido.
- [ ] Diferenciar encontrado, formato reconhecido, duplicado, inacessível e rejeitado; explicar o motivo da rejeição quando conhecido.
- [ ] Tratar pastas sobrepostas, links simbólicos, arquivos temporários e falhas de leitura sem travar a interface.
- [ ] Implementar varredura incremental quando houver metadados suficientes para detectar entradas inalteradas; manter atualização completa disponível para recuperação.
- [ ] Evitar recalcular hashes completos de arquivos grandes em cada inicialização; definir uma estratégia de identidade e invalidação baseada em metadados, com hash apenas quando necessário.

**M1.3 — Biblioteca**
- [x] Persistir preferência Grade/Lista e ordenação em `.aceris/library_preferences_v1.txt`, com parser versionado, fallback seguro e testes de recuperação após reabrir a biblioteca.
- [ ] Adicionar ordenação por data de adição, última execução e tamanho somente quando esses dados forem persistidos com precisão.
- [ ] Melhorar seleção múltipla e operações em lote sem tocar nos arquivos originais.
- [ ] Identificar duplicatas visualmente e permitir ao usuário escolher a entrada preferida, sem remover arquivos automaticamente.
- [ ] Distinguir "remover da biblioteca" de "excluir arquivo".
- [ ] Adicionar filtros por formato e estado de carregamento real; não inferir compatibilidade pelo nome/extensão.
- [ ] Mostrar claramente que CIA não possui instalação persistente enquanto esse fluxo não existir.

**Critério de aceite:** o usuário consegue conceder acesso a uma pasta em cada plataforma, cancelar/repetir a varredura, entender os resultados e iniciar um arquivo listado sem perder a biblioteca ou os dados existentes.

### M2 — Identidade, metadados e página de jogo

**Prioridade:** P0.  
**Dependência:** M1 fornece origens de arquivo confiáveis.

Projeto da fronteira de leitura e seleção de pastas: [FILE_SOURCE_ARCHITECTURE.md](FILE_SOURCE_ARCHITECTURE.md).

- [ ] Criar identidade separada para o título e para cada arquivo/origem encontrada. Um mesmo título pode ter cópias ou revisões distintas.
- [ ] Extrair Title ID, região, produto, versão/revisão e tipo de conteúdo somente quando o cabeçalho realmente fornecer esses dados e a validação for bem-sucedida.
- [ ] Manter fallback estável para homebrew ou arquivos sem metadados completos.
- [ ] Não associar configurações por jogo usando apenas o nome do arquivo.
- [ ] Implementar página de detalhes com título, formato, caminho/URI, identificadores conhecidos, estado de validação, ações e mensagens.
- [ ] Permitir arte/capa local escolhida pelo usuário e fallback sem capa.
- [ ] Manter capa e metadados como dados auxiliares: sua ausência nunca deve impedir a inicialização.
- [ ] Registrar última execução e tempo de jogo somente quando a medição for confiável; separar tempo de sessão de tempo estimado.
- [ ] Oferecer relatório de diagnóstico por título com prévia e sanitização.

**Critério de aceite:** arquivos com nomes iguais não são confundidos; revisões/regiões diferentes não compartilham perfil por engano; detalhes e arte não são requisito para executar o jogo.

### M3 — Sistema de entrada e perfis de controle

**Prioridade:** P0 para ergonomia de jogo.  
**Objetivo:** permitir jogar por touchscreen, teclado e controles físicos de forma consistente.

**M3.1 — Camada comum**
- [ ] Definir modelo comum de ação lógica (botões digitais, eixos analógicos, touchscreen/stylus e hotkeys).
- [ ] Separar dispositivo físico de mapeamento lógico; um perfil pode mapear vários dispositivos.
- [ ] Persistir perfis globais, por dispositivo e por jogo com herança explícita.
- [ ] Tratar conexão, desconexão, reconexão e troca de dispositivo.
- [ ] Implementar teste visual de botões, eixos, deadzone, sensibilidade e inversão.
- [ ] Detectar conflitos de atalhos e permitir redefinir o padrão.

**M3.2 — Desktop**
- [ ] Enumerar gamepads e apresentar nomes dos dispositivos detectados.
- [ ] Mapeamento automático somente para layouts conhecidos e confirmados; manter remapeamento manual.
- [ ] Permitir mapear teclado, mouse quando aplicável, botões, gatilhos e eixos.
- [ ] Atalhos globais configuráveis para pausar, retomar, menu, screenshot, troca de telas e ações de progresso quando existirem.

**M3.3 — Android**
- [ ] Mapear gamepads USB/Bluetooth e teclas externas.
- [ ] Editor dos controles virtuais: mover, redimensionar, ocultar, opacidade e área de toque.
- [ ] Presets padrão, compacto, canhoto, destro e personalizado.
- [ ] Salvar layouts separados para retrato e paisagem, mas só expor orientação depois de validar rotação e composição de telas.
- [ ] Ajustar Circle Pad e área de stylus, sem sobrepor indevidamente a superfície de toque da tela inferior.
- [ ] Feedback tátil configurável; não prometer rumble se o hardware/host não suportar.
- [ ] Ocultar os controles virtuais quando um controle físico conectar, com opção de manter o overlay.

**Critério de aceite:** o usuário consegue mapear e testar entrada sem editar arquivos; perfis por jogo não alteram o mapeamento global; toque e entradas físicas coexistem sem eventos presos após perda de foco.

### M4 — Menu rápido, pausa e layouts das duas telas

**Prioridade:** P0.  
**Dependências:** M3 para atalhos/ações consistentes e um mecanismo seguro de pausar o agendamento guest.

**M4.1 — Pausa e menu**
- [x] Pausar a execução guest no limite entre quadros nos loops desktop e Android; enquanto o painel está aberto, o host continua processando eventos e redesenhando o último scanout, sem executar o próximo quadro guest.
- [x] Abrir o menu sem bloquear a thread de UI. Entradas mantidas são liberadas ao abrir/retomar, e o toque no botão MENU Android é separado da entrada stylus.
- [x] Expor Continuar, Reiniciar e Voltar à biblioteca. Reiniciar recria a sessão do mesmo arquivo.
- [x] Disponibilizar botão visível e tecla Voltar no Android e Esc no desktop. O mapeamento de teclas ainda não é configurável.
- [x] Manter o menu compacto no celular e painel/overlay no PC.
- [x] Limpar explicitamente as filas host ao abrir o menu: Android pausa, aguarda e limpa AAudio, retomando com `requestStart`; ALSA usa `snd_pcm_drop`/`snd_pcm_prepare`, WinMM usa `waveOutReset` e AudioQueue usa parada imediata seguida de reinício. O resampler desktop é reiniciado para descartar seu histórico.
- [x] Pedir confirmação antes de Reiniciar ou Voltar à biblioteca, com aviso de que progresso não salvo pelo próprio jogo pode ser perdido. A confirmação é preventiva; não há detecção automática de estado sujo.
- [ ] Validar pausa, limpeza/retomada de áudio e confirmação de ações em hardware físico no Android e nos backends desktop.

**M4.2 — Layout**
- [x] Implementar modos vertical, lado a lado, superior ampliada, inferior ampliada e tela única (superior/inferior).
- [x] Adicionar ação para trocar a posição física das telas principal/secundária.
- [x] Preservar proporções dos LCDs e encaixar cada modo dentro da surface; o desktop redimensiona a janela para o canvas natural do modo.
- [x] Mapear toque/stylus para o LCD inferior em todos os modos, inclusive após a troca física de posições.
- [ ] Permitir ajustar espaçamento e posição dentro de limites seguros.
- [ ] Criar modo personalizado com restaurar padrão.
- [ ] Persistir perfil global e override por jogo; manter preferências retrato/paisagem separadas quando a plataforma suportar ambos.
- [ ] Janela separada de telas no desktop fica para etapa posterior, após validar o gerenciamento de múltiplas janelas.
- [ ] Validar rotação, redimensionamento, troca, hit-test do stylus e renderização em Android físico e Windows/macOS/Linux.

**Critério de aceite:** o usuário pausa e retoma sem reiniciar a sessão, troca a disposição das telas durante o jogo e o toque continua atingindo a coordenada correta da tela inferior.

### M5 — Configurações globais e por jogo

**Prioridade:** P0/P1.  
**Dependência:** M2 para identidade estável do título.

- [ ] Definir schema versionado para preferências globais e overrides por jogo.
- [ ] Implementar herança: o jogo herda o padrão global até o usuário alterar uma opção.
- [ ] Diferenciar na interface "Usando padrão global" de "Personalizado para este jogo".
- [ ] Criar categorias Geral, Sistema, Gráficos, Layout, Áudio, Controles e Diagnóstico, exibindo apenas opções que existem.
- [ ] Armazenar overrides usando identidade estável de conteúdo/título, com fallback seguro.
- [ ] Implementar migração de versões e preservação de campos desconhecidos quando possível.
- [ ] Botão "Restaurar padrão deste jogo" remove somente overrides daquele jogo.
- [ ] Mostrar impacto conhecido de cada opção em desempenho, memória, bateria e precisão.
- [ ] Não permitir que ajuste de compatibilidade de um título altere silenciosamente a configuração global.

**Critério de aceite:** alternar entre dois jogos restaura corretamente os respectivos perfis; redefinir um perfil não altera os demais; reiniciar o Aceris mantém configurações e migrações não descartam dados silenciosamente.

### M6 — Progresso: save states, slots e proteção de dados

**Prioridade:** P1 — recurso de alto risco técnico.  
**Dependência:** sessão pausável e arquitetura de snapshot completa.

Antes de implementar a interface, demonstrar que o estado serializado inclui todos os dados determinantes para retomar a execução de forma coerente: CPU e modos bancados, MMU/TLB/cache quando aplicável, memória, GPU/PICA e filas pendentes, serviços/HLE, timers, interrupções, DSP/áudio, touchscreen/HID e sincronização de threads/eventos. Recursos do host (janelas, ponteiros, dispositivos e handles nativos) devem ser religados, não serializados como ponteiros.

- [ ] Inventariar o estado completo do emulador e classificar cada campo como serializável, reconstruível ou não persistente.
- [ ] Definir formato versionado, assinatura de integridade e identificação do jogo.
- [ ] Criar captura e restauração atômicas, com validação antes de alterar a sessão ativa.
- [ ] Salvar em arquivo temporário e trocar pelo destino somente após gravação completa.
- [ ] Implementar slots manuais nomeados, quick save/load e metadados de data/hora.
- [ ] Adicionar screenshot de prévia apenas depois de a captura visual estar sincronizada com o snapshot.
- [ ] Confirmar sobrescrita e carregamento de estados; nunca descartar arquivo inválido.
- [ ] Criar backups rotativos e recuperação após falha de gravação.
- [ ] Distinguir claramente save do jogo e save state em toda a interface.
- [ ] Auto-save ao sair e Rewind ficam desativados por padrão e só entram após testes de consistência e medição de consumo de armazenamento/memória.

**Critério de aceite:** salvar, fechar o processo, reiniciar e carregar restaura uma sessão determinística em testes de regressão; arquivos corrompidos ou de versão incompatível são rejeitados sem destruir outros slots ou saves normais.

### M7 — Gráficos, desempenho e áudio apresentados com honestidade

**Prioridade:** P1.  
**Dependência:** opções reais do core e evidência por backend.

- [ ] Criar perfis Compatibilidade, Equilibrado e Desempenho usando apenas opções já implementadas e testadas.
- [ ] Identificar claramente o modo Fast Memory como experimental ou potencialmente incompatível enquanto a coerência de cache não estiver garantida.
- [ ] Expor VSync, limitador, resolução interna, filtros e antialiasing somente quando cada recurso tiver efeito real e validação.
- [ ] Diferenciar resolução interna de ampliação/escala do framebuffer LCD.
- [ ] Não exibir filtros ou multiplicadores de resolução que só ampliem a imagem final.
- [ ] Apresentar FPS, velocidade relativa e frametime com método de medição consistente; métricas de CPU/GPU/bateria somente se disponíveis pelo host.
- [ ] Manter telemetria local e desativada por padrão; sem upload automático.
- [ ] Resolver e validar a inicialização/saída real do áudio antes de apresentar controles de áudio como prontos.
- [ ] Adicionar volume, mute, dispositivo e latência somente quando controlarem de fato o caminho de áudio.
- [ ] Testar suspensão, troca de dispositivo, foco e retomada sem ruído, bloqueio ou áudio preso.

**Critério de aceite:** cada opção muda o comportamento documentado; as medições têm definição clara; perfis podem ser revertidos; PC e Android têm testes próprios e nenhuma melhoria é alegada sem medição repetível.

### M8 — Acessibilidade, navegação, ajuda e acabamento

**Prioridade:** P1, executada transversalmente desde M1 e concluída aqui.

- [ ] Navegação integral por teclado, D-pad/controle e toque.
- [ ] Foco visível, escala de UI e tamanho de texto independentes da escala do jogo.
- [ ] Alto contraste e redução/desativação de animações.
- [ ] Não usar apenas cor como sinal de erro, seleção ou compatibilidade.
- [ ] Testar alvos de toque, safe areas, barras do sistema e diferentes densidades de tela.
- [ ] Ajuda contextual pesquisável, com explicação simples e detalhes técnicos opcionais.
- [ ] Mensagens de erro com causa, arquivo afetado e ação de recuperação.
- [ ] Histórico local de avisos com opção de limpar.
- [ ] Temas claro/escuro e preferência de idioma, com PT-BR inicial e estrutura de localização.
- [ ] Revisar textos para evitar jargão, ambiguidade e opções sem explicação.

**Critério de aceite:** fluxos essenciais podem ser concluídos sem mouse e sem depender de cor; erros comuns indicam um próximo passo; textos não se sobrepõem ou são cortados nas escalas suportadas.

### M9 — Ferramentas e integração avançada

**Prioridade:** P2 — não bloqueia a primeira experiência completa.

- [ ] Gerenciador de cheats locais por jogo, com origem, descrição e desativar tudo.
- [ ] Screenshot durante a partida, com escolha de destino e confirmação visual.
- [ ] Exportação de relatórios sanitizados, com prévia dos campos compartilhados.
- [ ] Importação/exportação de perfis, controles e layouts em formato versionado.
- [ ] Texturas personalizadas locais quando o pipeline de substituição estiver validado.
- [ ] Câmera, microfone, sensores, NFC/Amiibo e multiplayer somente após HLE, permissões e testes funcionais.
- [ ] Integrações online opcionais (como conquistas) só depois de revisar compatibilidade, autenticação, privacidade e regras do serviço.
- [ ] Sincronização em nuvem é posterior; nunca incluir jogos, firmware, chaves ou envio de saves sem escolha explícita do usuário.

**Critério de aceite:** cada ferramenta tem origem clara dos dados, permissões explícitas, opção de desligar e testes de falha; recursos indisponíveis não aparecem como ativos.

## 4. Ordem crítica e dependências

A ordem principal é:

```text
M0 Contratos e inventário
  └─ M1 Fontes de arquivos + biblioteca
      └─ M2 Identidade e página de jogo
          └─ M5 Configuração por jogo
              └─ M6 Save states

M3 Entrada e perfis
  └─ M4 Menu de pausa e layouts
      └─ M5 Configuração por jogo (integração de perfis)

M7 Gráficos/áudio depende das capacidades reais do core e dos hosts.
M8 Acessibilidade atravessa todos os marcos.
M9 Ferramentas avançadas depende dos respectivos subsistemas.
```

M1, M3 e a estabilidade técnica do núcleo podem avançar em paralelo, desde que os contratos M0 estejam definidos e não exista edição concorrente de arquivos com autoria reservada.

## 5. Pacote de testes obrigatório por marco

Todo marco deve cobrir os cenários aplicáveis abaixo:

| Área | Testes mínimos |
|---|---|
| Biblioteca | diretório vazio, muitos arquivos, duplicatas, extensão em caixa diferente, caminho removido, falta de permissão, erro de leitura, cancelamento e nova varredura |
| Identidade | nomes iguais, caminhos diferentes, título/revisão/região diferentes, arquivo sem metadados e arquivo alterado após indexação |
| Controles | mapeamento, conflito de teclas, deadzone, eixo invertido, desconexão/reconexão, multi-touch, perda de foco e retorno |
| Menu/layout | pausar/retomar repetidamente, mudar layout durante jogo, rotação, proporção, troca de tela e hit-test do stylus |
| Configuração | herança global, override, alternância entre jogos, reset por jogo, migração e arquivo inválido |
| Save states | múltiplos slots, interrupção durante escrita, corrupção, versão antiga/nova, jogo incompatível e restauração repetida |
| Gráficos/áudio | opção ligada/desligada, backend alternativo, foco perdido, troca de dispositivo e teste em hardware quando possível |
| Acessibilidade | teclado sem mouse, D-pad, leitor/escala quando suportado, alto contraste, texto ampliado e alvos de toque |

### Matriz de plataformas

- **Linux/Windows/macOS:** CI para formatação, Clippy, testes e builds; testes manuais de janela, controles, áudio e GPU em cada SO conforme hardware disponível.
- **Android ARM64:** CI para build/APK; instalação e teste físico para SAF, navegação, multi-touch, controles externos, ciclo de vida, áudio, surface e desempenho.
- **Core compartilhado:** testes unitários e de integração independentes de plataforma sempre que possível; não considerar um frontend validado porque outro passou.

A CI deve executar comandos do workspace definidos pelo projeto. Resultado vazio, indisponível ou não consultável não equivale a sucesso. Testes físicos devem ser registrados separadamente.

## 6. Definição de concluído

Um item só pode ser marcado como concluído quando:

1. O fluxo está integrado ao caminho real da aplicação, não apenas desenhado.
2. A configuração persiste e sobrevive a reinício quando esse comportamento é prometido.
3. Erros, cancelamento e recuperação foram testados.
4. Há testes automatizados para lógica compartilhada e teste de host para comportamento específico de plataforma.
5. Não houve regressão nos fluxos já existentes.
6. A documentação descreve exatamente o comportamento observado e limitações restantes.
7. A CI do commit foi verificada quando disponível; qualquer limitação da verificação é declarada.
8. O usuário não perde jogos, saves, perfis ou arquivos por uma operação implícita.

## 7. Primeira fila concreta de desenvolvimento

Após este plano, a sequência recomendada para o próximo ciclo é:

1. **M0.1:** inventário de capacidades reais e opções configuráveis no core/hosts.
2. **M1.1:** desenhar a abstração de origem de arquivo que acomoda caminhos desktop e URIs SAF Android.
3. **M1.2:** implementar primeiro o seletor de pastas e a persistência de permissões, sem remover o suporte atual por caminho.
4. **M1.3:** adicionar estados de progresso/cancelamento/resultado da varredura.
5. **M2.1:** definir identidade de jogo versus identidade de arquivo, antes dos perfis por jogo.
6. **M3.1:** definir o modelo lógico de input e o formato de perfil, antes de construir o editor visual.
7. **M4:** só então ligar o menu de pausa às ações e layouts efetivamente disponíveis.

Não iniciar save states, Rewind, filtros gráficos fictícios, instalação persistente de CIA ou integrações online antes de suas dependências técnicas estarem concluídas.

## 8. Relação com os demais documentos

- Inventário auditado do que existe no código e do que chega à interface: [CAPABILITY_MATRIX.md](CAPABILITY_MATRIX.md).
- Requisitos funcionais, experiência e referência de produto: [USER_EXPERIENCE.md](USER_EXPERIENCE.md).
- Estado técnico real do núcleo e marcos de emulação: [ROADMAP.md](ROADMAP.md).
- Diferenças e validação dos hosts PC/Android: [FRONTENDS.md](FRONTENDS.md).
- Autoria, regras entre agentes e limites de edição: [AI_HANDOFF.md](AI_HANDOFF.md) e [REGRAS_PARA_IA.md](REGRAS_PARA_IA.md).
