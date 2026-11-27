# Plano de desempenho para PCs e dispositivos de entrada

**Revisão:** 26/11/2026  
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
| [Black for Android](https://github.com/RanggaCasper/Black-Android) / [Black-2006-Recomp](https://github.com/portingpete/Black-2006-Recomp) | Recompilação estática do código x86 específico de BLACK para C++ e build ARM64; Vulkan no desenho Android; SDL3 para janela, entrada e áudio | Separação clara entre código traduzido, runtime e host; geração nativa por arquitetura; cache de shaders; opções de resolução e limite de quadros | É um port de um título conhecido, não um emulador geral. O README Android declara ARM64, Vulkan 1.1 e 4 GB de RAM; não é prova de que rode em aparelhos abaixo dessa classe |
| [PPSSPP — desenvolvimento](https://www.ppsspp.org/docs/development/getting-started/) | JIT/dynarec traduz instruções guest diretamente para x86/x64, ARM/ARM64 e outras arquiteturas | Principal referência para um tradutor dinâmico ARM11/Thumb, cache de blocos e execução nativa | PSP e 3DS têm arquiteturas e periféricos diferentes; não copiar o desenho interno nem assumir ganhos idênticos |
| [PPSSPP — configurações de baixo desempenho](https://www.ppsspp.org/docs/settings/recommended/) | Reduz resolução de saída, desliga pós-processamento e deixa frameskip/readback hacks como último recurso | Preset econômico, sem melhorias visuais caras por padrão; pular somente a apresentação quando for seguro | As opções de skinning e readback do PSP não podem ser transplantadas como regras para PICA200 |
| [Azahar/Citra](https://github.com/azahar-emu/azahar) e [Dynarmic](https://github.com/azahar-emu/dynarmic) | JIT ARM, fast interpreter, shader JIT, shaders de hardware e cache em disco | Referência mais próxima do alvo ARM11; arquitetura de fallback, invalidação e aceleração de shaders | O Aceris implementará seu próprio tradutor, memória, HLE e renderizador |
| [Dolphin](https://dolphin-emu.org/docs/guides/performance-guide/) | Cache de shaders especializados, compilação híbrida/assíncrona e configurações por jogo | Evitar travamentos de compilação e preservar um caminho temporário correto enquanto o shader é compilado | Shaders genéricos/ubershaders podem custar demais em GPUs fracas; backend e driver precisam ser medidos |
| [PCSX2](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/Docs/PCSX2_FAQ.md) | Divide trabalho entre threads quando existem unidades independentes; MTVU usa outro core para VU1 | Paralelizar renderização, compilação e tarefas independentes com filas limitadas | Paralelismo indiscriminado cria corrida e dessincronização. Nunca presumir que mais threads significam mais velocidade |
| [Android ADPF](https://developer.android.com/games/optimize/adpf) | Observa estado térmico/capacidade sustentada e ajusta qualidade de forma gradual | Modo adaptativo que evita throttling prolongado e reduz opções visuais antes do aparelho superaquecer | Não acelera o código por si só; não pode alterar arbitrariamente o tempo guest |

### Observação específica sobre Black

O port Android de BLACK recompila antecipadamente o XBE x86 conhecido para código C++ e o compila para ARM64. É uma referência forte de **recompilação estática por título**. O Aceris, porém, precisa executar vários executáveis ARM11 desconhecidos, lidar com memória virtual, auto-modificação de código, exceções, SVC, interrupções, cache e periféricos. Portanto, a técnica aplicável ao nosso emulador é a **recompilação dinâmica de blocos ARM11**, não criar ports estáticos separados de cada jogo.

O mesmo README lista Vulkan 1.1 e 4 GB de memória como requisitos do port Android. Isso é uma pista de viabilidade de port nativo, não uma meta suficiente para o Aceris, cujo objetivo inclui dispositivos mais fracos.

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
