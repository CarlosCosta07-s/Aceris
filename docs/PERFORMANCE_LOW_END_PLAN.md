# Plano de desempenho para PCs e dispositivos de entrada

**Revisão:** 27/11/2026  
**Escopo:** execução ARM11, memória, recompilação, PICA200, apresentação, Android e builds desktop.  
**Objetivo:** fazer o Aceris alcançar velocidade de emulação sustentável em hardware modesto, mantendo uma experiência de maior qualidade em máquinas mais fortes.

Este plano complementa [PERFORMANCE_REFERENCE_STUDY.md](PERFORMANCE_REFERENCE_STUDY.md) e [PERFORMANCE_TESTING.md](PERFORMANCE_TESTING.md). As referências externas servem para estudar princípios e resultados publicados. Toda implementação do Aceris permanece original, em Rust, sem copiar/adaptar código, shaders, testes, assets ou conteúdo de outros projetos.

## 1. Objetivo realista

A meta de engenharia é atingir **100% da velocidade emulada**, com frame pacing estável, em uma seleção crescente de jogos de referência, usando resolução original do 3DS e sem exigir GPU dedicada. A meta será medida separadamente em:

- PCs x86-64 de entrada, incluindo gráficos integrados;
- Android ARM64 de entrada, com memória e orçamento térmico limitados;
- máquinas intermediárias e avançadas, que poderão habilitar melhorias visuais.

Não será prometido que todo jogo 3DS funcionará em todo celular. A carga varia muito por título: CPU, GPU, serviços HLE, áudio e sincronização podem ser gargalos diferentes. Os aparelhos mínimos serão definidos com medições reais, não por comparação de especificações isoladas.

**Velocidade emulada não é sinônimo de FPS apresentado.** A CPU guest, timers, áudio e serviços devem continuar no tempo correto. Quando necessário, o Aceris pode reduzir trabalho visual opcional ou deixar de apresentar um quadro repetido; não deve simplesmente deixar de executar lógica do jogo, adiantar timers ou alterar o clock guest para fabricar um número de FPS.

## 2. O que aprendemos com as referências

| Referência | Técnica observada | Aplicação ao Aceris | Limite |
|---|---|---|---|
| [Black for Android v1.8.1](https://github.com/RanggaCasper/Black-Android/releases) / [Black-2006-Recomp](https://github.com/portingpete/Black-2006-Recomp) | Recompilação estática antecipada do XBE x86 de BLACK para C++/ARM64; Vulkan; SDL3; escala de resolução configurável (20–200%, 67% padrão no port), V-Sync/FPS cap, contador de FPS, filtragem e cache de shaders persistente | Separação entre runtime guest e host; opções visuais independentes da execução; perfil por dispositivo; apresentação limitada e cache persistente para shaders/pipelines quando houver renderer PICA em GPU | É recompilação específica de um jogo, não um emulador geral. O README declara Android 10+, ARM64, Vulkan 1.1 e 4 GB de RAM; não é prova de suporte a celulares abaixo dessa classe. Os valores de escala/FPS não devem ser copiados sem benchmark Aceris |
| [PPSSPP — desenvolvimento](https://www.ppsspp.org/docs/development/getting-started/) | JIT/dynarec traduz instruções guest diretamente para x86/x64, ARM/ARM64 e outras arquiteturas | Principal referência para um tradutor dinâmico ARM11/Thumb, cache de blocos e execução nativa | PSP e 3DS têm arquiteturas e periféricos diferentes; não copiar o desenho interno nem assumir ganhos idênticos |
| [PPSSPP — configurações de baixo desempenho](https://www.ppsspp.org/docs/settings/recommended/) | Reduz resolução de saída, desliga pós-processamento e deixa frameskip/readback hacks como último recurso | Preset econômico, sem melhorias visuais caras por padrão; pular somente a apresentação quando for seguro | As opções de skinning e readback do PSP não podem ser transplantadas como regras para PICA200 |
| [Azahar/Citra](https://github.com/azahar-emu/azahar) e [Dynarmic](https://github.com/azahar-emu/dynarmic) | JIT ARM, fast interpreter, shader JIT, shaders de hardware e cache em disco | Referência mais próxima do alvo ARM11; arquitetura de fallback, invalidação e aceleração de shaders | O Aceris implementará seu próprio tradutor, memória, HLE e renderizador |
| [Dolphin](https://dolphin-emu.org/docs/guides/performance-guide/) | Cache de shaders especializados, compilação híbrida/assíncrona e configurações por jogo | Evitar travamentos de compilação e preservar um caminho temporário correto enquanto o shader é compilado | Shaders genéricos/ubershaders podem custar demais em GPUs fracas; backend e driver precisam ser medidos |
| [PCSX2](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/Docs/PCSX2_FAQ.md) | Divide trabalho entre threads quando existem unidades independentes; MTVU usa outro core para VU1 | Paralelizar renderização, compilação e tarefas independentes com filas limitadas | Paralelismo indiscriminado cria corrida e dessincronização. Nunca presumir que mais threads significam mais velocidade |
| [Android ADPF](https://developer.android.com/games/optimize/adpf) | Observa estado térmico/capacidade sustentada e ajusta qualidade de forma gradual | Modo adaptativo que evita throttling prolongado e reduz opções visuais antes do aparelho superaquecer | Não acelera o código por si só; não pode alterar arbitrariamente o tempo guest |

### Observação específica sobre Black

O port Android de BLACK recompila antecipadamente o XBE x86 conhecido para código C++ e o compila para ARM64. É uma referência forte de **recompilação estática por título**. O Aceris, porém, precisa executar vários executáveis ARM11 desconhecidos, lidar com memória virtual, auto-modificação de código, exceções, SVC, interrupções, cache e periféricos. Portanto, a técnica aplicável ao nosso emulador é a **recompilação dinâmica de blocos ARM11**, não criar ports estáticos separados de cada jogo.

O mesmo README lista Android 10+, ARM64, driver Vulkan 1.1, 4 GB de RAM e 2 GB de armazenamento como requisitos do port. Isso é uma pista de viabilidade de um port nativo específico, não uma meta suficiente para o Aceris, cujo objetivo inclui dispositivos mais fracos. A release mais recente visível nesta revisão é a v1.8.1, publicada em 1/11/2026; as notas dessa release corrigem uma falha da página de opções gerais, não anunciam um novo ganho de desempenho.

## 3. Diagnóstico do Aceris hoje

A referência de perfilamento adicionada no commit `3e4c99a3` percorre o caminho completo da sessão ARM11 com um laço sintético de operações ALU/load/store. No contêiner de nuvem foram observados aproximadamente 19–23 milhões de instruções guest por segundo, ou cerca de cinco chamadas de quadro por segundo com orçamento de 4 milhões.

O perfil Callgrind dessa sonda registrou:

- `Arm11Core::step_instruction`: aproximadamente 72% inclusivo;
- loads e stores pelo modelo de D-cache: aproximadamente 45% inclusivo;
- `Arm11ProcessSession::step`: aproximadamente 13% próprio;
- publicação de ACTLR.SMP: aproximadamente 5%;
- `locate`: aproximadamente 5%.

Esses percentuais são dados da sonda e não devem ser somados como custos independentes, pois há custos inclusivos. Também não são FPS de jogo nem benchmark de Android físico. A sonda confirma que a CPU guest e o caminho de memória merecem prioridade antes de micro-otimizações de apresentação.

O projeto já possui benchmark release com aquecimento, amostras por quadro, média, p50/p95/p99, instruções executadas, atividade GSP/LCD e CSV de trace. O próximo passo é ampliar a atribuição por estágio sem inserir temporizadores a cada instrução no caminho quente.

## 4. Estratégia técnica por camada

### 4.1 CPU ARM11: interpreter rápido antes do JIT

1. **Cache de instruções decodificadas por bloco:** decodificar A32/Thumb uma vez e reutilizar a representação enquanto a página de código continuar válida. O bloco deve terminar em branch, SVC, exceção, operação CP15 relevante, evento, acesso MMIO ou outro ponto de sincronização necessário.
2. **Fast path de memória por página:** resolver rapidamente páginas comuns de RAM/FCRAM, mantendo verificação de privilégio/permissão, alinhamento, endianness, TLB, atributos, D-cache e fallback para MMIO/fault. A otimização não pode transformar toda a memória em um ponteiro plano.
3. **Menos trabalho por instrução:** remover alocações e consultas repetidas apenas quando o perfil demonstrar que estão no caminho quente; manter scratch buffers, tabelas de decodificação e estruturas de tamanho fixo quando forem comprovadamente mais eficientes.
4. **Escalonador por deadlines/eventos:** executar blocos maiores entre pontos de verificação quando for seguro, mas continuar respeitando orçamento de instruções, IRQ/FIQ, timers, WFI/WFE, IPC e conclusão GSP. O tamanho do bloco deve ser limitado por eventos pendentes.
5. **Reduzir custos de contexto:** evitar cópias e reconstruções de estado de thread por instrução. Preservar a semântica de prioridade, espera, wake-up e troca de contexto.

### 4.2 IR e JIT nativo multiplataforma

Depois de estabilizar o cache de blocos interpretados:

1. Especificar uma IR própria que represente operações A32/Thumb, flags NZCV, leituras/escritas de registradores, memória, exceções e saídas de bloco.
2. Executar a IR em um executor rápido e comparar deterministicamente o estado final com o interpretador.
3. Criar compilação JIT por blocos para **AArch64** (Android e ARM64 desktop) e **x86-64** (PC).
4. Manter o interpretador como fallback obrigatório, modo de diagnóstico e oráculo de testes.
5. Adicionar cache de código nativo limitado por memória, compilação preguiçosa e, depois, cache persistente versionado. O primeiro arranque/cold cache será medido separadamente do cache quente.

O JIT precisa de limites de bloco nos pontos de interrupção/SVC/sincronização e invalidação quando código guest for escrito ou remapeado. Escritas em páginas executáveis, ControlMemory, mudança de mapeamento/MMU e alterações de permissões não podem deixar código obsoleto em execução. A geração de código deverá respeitar W^X e permissões de memória do host.

Não incluir uma dependência de JIT de terceiros automaticamente. Qualquer nova dependência exige análise de licença, portabilidade e autorização conforme as regras do repositório. A implementação do produto permanece em Rust.

### 4.3 GPU PICA200 e caminho de apresentação

O presenter atual com wgpu apresenta quadros LCD já compostos; ele não transforma, por si só, a rasterização PICA200 em trabalho de GPU. Portanto, a prioridade gráfica de desempenho deve vir depois de o caminho guest-driven estar correto.

- Implementar e medir um backend de renderização PICA200 acelerado por GPU, preservando o rasterizador CPU como referência e fallback.
- Traduzir os estágios PICA para shaders host próprios e cachear shaders/pipelines com chaves explícitas de estado, formatos e driver.
- Reutilizar buffers, descritores e recursos compatíveis; evitar recriação de pipeline, alocação e cópias por draw.
- Compilar shaders em segundo plano e guardar cache em disco. Até a compilação terminar, usar um caminho temporário correto; não remover objetos nem ignorar draws silenciosamente para esconder stutter.
- Evitar readback GPU→CPU e transferências redundantes. Quando a leitura guest de framebuffer/depth for obrigatória, manter o comportamento correto e medir o custo.
- Começar com resolução nativa do 3DS, sem MSAA, filtros ou pós-processamento. A escala da janela/tela no host não deve mudar a resolução guest nem a semântica do framebuffer.
- Medir Vulkan e OpenGL/OpenGL ES nos dispositivos reais. Vulkan será preferido somente quando o driver/adapter demonstrar vantagem; manter seleção automática e fallback.

### 4.4 Paralelismo sem perder sincronização

Priorizar tarefas que podem ser realmente independentes: compilação de shaders, carregamento/decompressão, preparação de recursos, apresentação e trabalho de áudio em buffers. Usar filas limitadas e quantidade de workers adequada ao número de cores disponíveis.

Não colocar locks, canais ou alocações por instrução guest. Não dividir a CPU guest ou os dispositivos em threads independentes antes de mapear precisamente a ordem de memória, interrupções, DMA, filas GSP e pontos de sincronização. PCSX2/Dolphin demonstram benefícios de paralelismo especializado, mas também documentam limites de sincronização e stutter de shaders.

### 4.5 Android e aparelhos que aquecem

- Medir em aparelho físico com Perfetto, Android Studio Profiler e ferramentas de GPU quando disponíveis.
- Usar ADPF/thermal signals para observar headroom e ajustar gradualmente **somente a qualidade host opcional**: filtragem, efeitos, escala de apresentação e carga de tarefas em segundo plano.
- Preservar clock, timers, áudio e número de instruções guest. Não desacelerar o console emulado automaticamente para mascarar superaquecimento.
- Medir sessões sustentadas, não apenas benchmarks curtos: pelo menos 15 minutos, registrando temperatura/estado térmico e queda de desempenho.
- Reduzir workers e buffers em dispositivos com pouca RAM/cores; manter caches de shaders/JIT limitados e com política de descarte.

## 5. Perfis de qualidade para o usuário

| Perfil | Padrão |
|---|---|
| **Compatibilidade** | Precisão prioritária, resolução original, sem speed hacks que alterem temporização; fallback interpretado disponível |
| **Baixo consumo** | Resolução original, sem efeitos/AA adicionais, cache limitado, apresentação econômica e menos tarefas em segundo plano |
| **Desempenho** | JIT ativado quando validado, backend GPU medido, compilação/cache de shaders e uso de workers independentes |
| **Personalizado** | Controles por jogo, com aviso claro para opções experimentais que possam afetar compatibilidade |

Frameskip, skip-readback ou mudanças no clock guest não serão ativados como solução universal. O PPSSPP documenta que esses recursos podem melhorar alguns casos e causar falhas visuais; o Dolphin também documenta speedhacks que podem quebrar jogos. Só poderão existir como opções específicas, com impacto e compatibilidade descritos e testes por jogo.

## 6. Build e especialização de CPU

O workspace já usa `opt-level = 3`, Thin LTO e um codegen unit no perfil release. A próxima etapa de build deve ser medida, sem assumir que flags adicionais sempre ajudam:

- avaliar Profile-Guided Optimization (PGO) com workloads representativos de CPU, memória, GPU e biblioteca;
- manter binários genéricos compatíveis com PCs antigos; não compilar todo o produto com `target-cpu=native`;
- usar caminhos SIMD especializados por feature de CPU somente quando o ganho for medido e houver fallback portátil;
- manter ARM64 como alvo prioritário para Android e x86-64 para desktops; não sacrificar uma arquitetura para beneficiar a outra.

## 7. Ordem de implementação e critérios de aceite

| Fase | Entrega | Critério para avançar |
|---|---|---|
| P0 — Medição | Benchmarks repetíveis e perfis por estágio no caminho guest real | Mesmo título/entrada/configuração; p50/p95/p99; resultados PC e Android separados |
| P1 — Fast interpreter | Cache de instruções decodificadas e fast path de memória | Ganho release demonstrado; estado CPU, faults, MMU, interrupções e testes permanecem idênticos |
| P2 — IR | IR própria e executor de referência | Diferencial contra interpretador em corpus ARM/Thumb; cobertura de flags e exceções |
| P3 — JIT | Backends AArch64 e x86-64 com cache e fallback | Ganho sustentado em workloads reais; invalidação, SVC, IRQ/FIQ, auto-modificação e memória corretas |
| P4 — PICA200 GPU | Renderizador acelerado, cache de pipelines e compilação assíncrona | Comparação determinística de pixels; sem regressão de efeitos/readback obrigatórios |
| P5 — Concorrência | Workers e filas de tarefas independentes | Ganho em hardware de 2/4 cores sem deadlocks, stutter novo ou dessincronização |
| P6 — Mobile sustentado | ADPF, qualidade adaptativa e limites de memória | Sessões de 15+ min com desempenho/temperatura registrados em aparelhos de entrada |

### Regras obrigatórias para cada otimização

- Medir antes e depois em build release com o mesmo commit-base, mesmo título local, mesma sequência de entrada, orçamento e aquecimento.
- Executar no mínimo três rodadas e registrar mediana, p95/p99 e dispersão; não aceitar melhoria baseada apenas em uma rodada.
- Comparar saída guest: PC/CPSR, registradores, flags, memória, interrupções, contadores, imagens LCD e áudio quando aplicável.
- Separar cache frio/quente, tempo de compilação JIT, tempo de execução e tempo de apresentação.
- Testar CPU-bound e GPU-bound; não otimizar o subsistema errado.
- Reverter qualquer melhoria que produza pixels incorretos, falhas de serviço, drift de áudio, perda de entrada ou divergência de estado.
- Não declarar sucesso para Android com base apenas em cross-compilation ou APK gerado; executar o benchmark no aparelho.
- Registrar SoC/CPU, GPU, RAM, driver, SO, temperatura aproximada, perfil de energia, revisão Git e configuração.

## 8. Próximo trabalho concreto

A primeira mudança de código orientada por este plano deve ser **um cache de blocos ARM/Thumb decodificados, alimentado pelo perfil atual**, acompanhado de um fast path seguro para páginas de memória comuns. Não começar pelo JIT completo nem por micro-otimizações aleatórias. O novo caminho será comparado com o interpretador atual em testes diferenciais e no benchmark de sessão.

Depois dessa etapa, o projeto poderá avançar para IR/JIT. Paralelamente, a renderização PICA200 acelerada deve ser planejada como uma fase própria: a GPU host atual ainda é presenter de LCD, não substituto do rasterizador PICA200.

## 9. Referência Android aplicada: Black for Android v1.8.1 (27/11/2026)

Fontes de leitura: [releases do Black for Android](https://github.com/RanggaCasper/Black-Android/releases) e [README](https://github.com/RanggaCasper/Black-Android). A v1.8.1 descreve um port nativo do jogo BLACK de Xbox: recompila antecipadamente o XBE x86 conhecido para C++/ARM64, desenha por Vulkan e usa SDL3 para janela, entrada e áudio. O app mantém configurações, saves, logs e shader cache no armazenamento privado. As opções documentadas incluem escala de renderização de 20% a 200% (67% padrão), V-Sync, limite de FPS, contador, filtragem anisotrópica e draw distance 2×/4×/8×. Os requisitos declarados são Android 10+, ARM64, Vulkan 1.1, 4 GB de RAM e 2 GB de armazenamento.

### O que será aproveitado no Aceris

1. **Perfis Android realmente separados:** persistir as opções host em configuração estruturada e versionada (perfil, backend, apresentação, escala host, filtros e limites de cache), sem misturar isso com estado guest, saves ou memória do jogo. A atual opção `performance_mode.txt` permanece opt-in e controla apenas a memória rápida; não deve virar um interruptor genérico para todas as opções.
2. **Qualidade host escalável:** estudar um controle de escala da composição/apresentação Android. Ele deve redimensionar apenas a imagem final exibida no telefone; jamais alterar as dimensões, stride, formatos, endereços, sincronização ou resolução dos framebuffers guest 3DS. A escala de 67% do Black não será adotada como padrão: as duas telas pequenas do 3DS e o presenter do Aceris têm características diferentes. Só implementar depois de medir 100%, 75% e 50% no aparelho.
3. **Apresentação com pacing estável:** manter o relógio, timers, áudio, IRQs e instruções guest no tempo correto. O Black deixa V-Sync desligado e FPS ilimitado por padrão para um único jogo; o Aceris não deve copiar esse comportamento. O host pode limitar a apresentação à taxa suportada pelo display, evitar apresentar quadros repetidos e expor FPS/frametime, mas não pode acelerar ou desacelerar o console guest para alcançar um número de FPS.
4. **Cache persistente de pipeline/shader:** quando o renderer PICA200 host existir, usar cache limitado, versionado e invalidado por versão do Aceris, GPU, driver, backend e chave de pipeline. Guardar o cache no armazenamento privado do app Android, separar cache frio/quente e permitir limpeza. O presenter atual possui apenas shaders simples de composição de LCD e não justifica um subsistema de cache de shaders PICA antes de existir um rasterizador PICA em GPU.
5. **Seleção Vulkan com fallback real:** continuar testando Vulkan e OpenGL ES por device/driver, registrando adapter e backend escolhidos e mantendo fallback. O README do Black exige Vulkan 1.1; isso não autoriza tornar Vulkan requisito absoluto para todos os telefones Aceris.
6. **Ajuste térmico gradual:** inspirar-se na ideia de controlar qualidade visual e trabalho host, mas validar ADPF/thermal headroom em aparelhos reais. Pode reduzir escala da composição, filtragem opcional, efeitos host e workers não essenciais; não pode alterar temporização guest nem desligar passos de emulação necessários.
7. **Diagnóstico integrado:** oferecer FPS apresentado, frametime p50/p95, resolução host, backend/adapter, estado de cache e perfil ativo. As métricas de apresentação devem ser separadas das instruções guest/s e dos quadros guest processados.

### Limites do port e estado do Aceris

- A recompilação estática do Black só é possível porque o código executável de um jogo específico é conhecido no build. O Aceris carrega executáveis ARM11/Thumb diferentes fornecidos pelo usuário; a direção correta continua sendo interpreter rápido, cache de decodificação, IR e JIT dinâmico genérico, com fallback preciso.
- O Aceris já possui um presenter host `wgpu` para Vulkan/GL (Metal em macOS), fallback CPU e upload direto RGBA/BGRX. A configuração atual solicita `PowerPreference::HighPerformance` e usa `FIFO` quando disponível; isso precisa ser medido em Android físico antes de mudar para `LowPower`, outro present mode ou limite fixo.
- O presenter recebe LCD já composto. Ele não reduz o custo do rasterizador PICA200 em CPU. Reduzir apenas a escala host pode diminuir tráfego/composição, mas não deve ser anunciado como aceleração do CPU guest.
- O renderer PICA em GPU e seu cache de pipelines são um marco futuro. O cache de shader do Black não é um atalho que possa ser ligado ao presenter LCD atual para acelerar o guest.
- O Black declara mínimo de 4 GB de RAM e Vulkan 1.1. O Aceris continuará buscando compatibilidade abaixo dessa referência, com fallback OpenGL ES/CPU e medição real; cross-compilation ARM64 não comprova desempenho em telefone.

### Experimentos e aceite específicos para Android

- Medir no mesmo aparelho e na mesma sequência: Vulkan versus OpenGL ES; perfil `Compatibilidade` versus `Baixo consumo`; apresentação em escala 100%, 75% e 50% quando suportada; FIFO versus qualquer alternativa só se o driver apresentar modo seguro.
- Três ou mais amostras release por configuração; registrar instruções guest/s, quadros guest, FPS apresentado, frametime p50/p95, upload CPU→GPU, memória, temperaturas/estado térmico e bateria.
- Fazer ensaio sustentado de pelo menos 15 minutos, separando cache frio e quente. Nenhum modo será promovido a padrão por um benchmark curto ou por resultado de runner em nuvem.
- Aceite exige imagens LCD guest idênticas no mesmo estado de execução, temporização guest preservada, nenhuma perda de áudio/entrada e melhora mensurável no consumo, frametime ou estabilidade.

### Telemetria host Android (27/11/2026)

- Implementado monitor opt-in no render thread Android, habilitado quando `games/.aceris/android_performance_log_v1.txt` contém `1`.
- A cada cinco segundos reporta FPS de apresentações concluídas, duração do render host (média/p50/p95), p95 de `App::step_into`, p95 da apresentação, intervalo p50/p95 entre apresentações, proporção de frames pelo presenter GPU e tentativas não apresentadas.
- Usa uma janela fixa de até 300 amostras e só chama `Instant` quando a coleta está habilitada. Não muda orçamento guest, relógio, timers, resolução LCD, layout, áudio ou ritmo do sistema guest.
- `App::step_into` inclui execução do jogo e composição do frame; não deve ser interpretado como CPU guest pura. A telemetria ainda não mede temperatura/bateria/ADPF nem custo isolado de cada estágio PICA.
- Guia de ativação por ADB em `docs/ANDROID_DEVICE_TRIAL.md`. Próxima validação: CI e coleta de um aparelho físico em pelo menos 15 minutos, com identificação do SoC/GPU/driver e comparação Vulkan versus OpenGL ES.
### Telemetria host desktop (27/11/2026)

- Implementado monitor opt-in em `frontends/desktop/src/performance.rs`, habilitado por `ACERIS_PERFORMANCE_LOG=1` ou por `games/.aceris/desktop_performance_log_v1.txt` com conteúdo `1`.
- Relatórios a cada cinco segundos, janela de até 300 amostras e modos separados `intro`, `library` e `game`.
- Mede FPS apresentado, frametime host média/p50/p95, p95 da execução de um frame do sistema, composição/escala, processamento de áudio, apresentação host, intervalo entre apresentações e proporção do presenter GPU.
- Na partida, `emulation_ms_p95` cronometra `run_frame_with_diagnostics`; não é custo isolado por instrução. `composition_ms_p95` mede apenas a composição final CPU dos LCDs. O presenter atual continua sendo saída host, não rasterização PICA200.
- A coleta é desativada por padrão e usa `Instant` adicional somente quando habilitada. Não modifica relógio guest, timers, orçamento de instruções, áudio, frame pacing, resolução ou conteúdo das LCDs.
- Procedimento e interpretação em `docs/PERFORMANCE_TESTING.md`. Próxima validação: build/teste desktop Windows/Linux/macOS e comparação release em hardware físico, Vulkan versus OpenGL e presenter GPU versus CPU sem mudar outras variáveis.