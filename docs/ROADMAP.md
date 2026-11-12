# Roadmap do Aceris

> Estado revisado em 12/11/2026, com base na implementação no commit `8aedeb346014917935985cd5fe913ee9606e18cc`.
>
> **Legenda:** `[x]` implementado no escopo indicado; `[~]` implementação parcial ou infraestrutura inicial; `[ ]` pendente. Uma caixa marcada não comprova compatibilidade completa com o hardware nem execução de jogos comerciais.

Este documento registra o estado observado no código. Não marque um item como concluído apenas porque existe um tipo, parser, stub, registrador ou interface. Cada marco exige testes e integração no caminho real de execução.

## 0. Fundação, regras e validação

- [x] Workspace Rust com crates compartilhados, núcleo e frontends iniciais para desktop e Android
- [x] Branch principal de desenvolvimento definida como `main`; nenhuma branch de trabalho deve ser criada
- [x] Regras de isolamento entre agentes, autoria própria e proteção de conteúdo protegido documentadas
- [x] Política de não incluir jogos, firmware, chaves, dumps ou conteúdo protegido no repositório
- [x] CI configurado para Linux, Windows e macOS com Rustfmt, Clippy e testes; build Android ARM64 separado
- [x] Workflow completo do commit `849e5a33933ea9444122f2160daf867e2d4e60d0` terminou com sucesso em 12/11/2026, incluindo a etapa Android ARM64
- [x] Workflow do commit `cadeb4381de1fe3227ec1d152a91c2b86ae61808` concluído com sucesso: Rustfmt, Clippy e testes em Linux, Windows e macOS; build Android `arm64-v8a` também aprovado
- [x] Executar `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` no CI para Linux, Windows e macOS no commit `cadeb4381de1fe3227ec1d152a91c2b86ae61808`
- [ ] Executar e registrar `cargo build --locked --workspace` explicitamente no CI
- [ ] Manter README, arquitetura e roadmap sincronizados com o código, sem anunciar compatibilidade não testada

## 1. CPU ARM11 / ARMv6 e Thumb

### Estado arquitetural e exceções

- [x] Interpretador A32 e Thumb com estado CPSR e avaliação das condições ARM
- [x] Bancos de registradores para User/System, FIQ, IRQ, Supervisor, Abort e Undefined, com SPSR por modo de exceção
- [x] Acesso MRS/MSR ao CPSR e SPSR com validação de privilégio
- [x] Troca de modo privilegiado e instrução ARMv6 CPS
- [x] Entrada de exceção SVC e retorno por restauração de estado
- [x] Sinais IRQ/FIQ externos com prioridade de FIQ e respeito às máscaras CPSR
- [x] Entrada opcional em Undefined Instruction, Prefetch Abort e Data Abort; falhas de emulação continuam sendo reportadas como erros
- [x] Retorno de exceção A32 por instrução de processamento de dados e RFE
- [~] Validar todas as combinações de exceção, vetores altos, endianness, máscaras, abortos durante transferências múltiplas e retornos em sequências longas
- [ ] Integrar um controlador de interrupções e temporizadores do sistema ao modelo de IRQ/FIQ

### Instruções ARM e Thumb

- [x] Instruções ARM básicas de processamento de dados, desvios, transferências, blocos e multiplicações comuns
- [x] Famílias ARMv6 de exclusivas A32: LDREX/STREX, LDREXB/STREXB, LDREXH/STREXH e LDREXD/STREXD
- [~] Monitor exclusivo local com geração de reserva de 8 bytes em FlatMemory e no espaço de endereços do processo; a integração com todos os adaptadores de barramento, especialmente o caminho MMU, ainda precisa ser completada
- [~] Famílias ARMv6 de mídia/DSP, saturação, operações paralelas, extensão de sinal/zero e multiplicações de meia palavra: há implementação e testes para várias famílias, mas a cobertura integral ainda não foi demonstrada
- [~] Conjunto Thumb-1 principal com desvios, acessos, pilha, operações aritméticas/lógicas e extensões ARMv6 REV/SXT/UXT
- [x] Thumb BKPT, CPS e SETEND implementados; BKPT é encaminhado como evento de breakpoint para a camada de processo
- [ ] Inventariar codificações A32 ARMv6/ARMv6K ainda não suportadas ou decodificadas incorretamente
- [ ] Completar e validar todas as codificações Thumb aplicáveis ao ARM11; não tratar Thumb-2 como requisito sem evidência de que a arquitetura-alvo o suporta
- [ ] Auditar alinhamento, endianness, flags, estados UNPREDICTABLE/UNDEFINED e efeitos colaterais de todas as famílias
- [ ] Criar testes de conformidade independentes por instrução, além dos testes unitários internos

### Coprocessadores e MMU

- [x] Arquivo de estado CP15 com registradores de identificação, controle, CPACR, TTBR0/1, TTBCR, DACR, fault status/address, CONTEXTIDR e TPIDR*
- [~] Operações de manutenção de cache/TLB e algumas barreiras ainda são aceitas como no-op; isso não equivale a modelagem arquitetural
- [~] MMU ARMv6 de descritores curtos: TTBR0/TTBR1 e TTBCR.N, seções, superseções, coarse tables, páginas grandes/pequenas, domínios, permissões AP/APX e XN
- [~] Faults de tradução, domínio e permissão são reportados e registrados em CP15; TLB, atributos TEX, permissões de subpágina no formato antigo e faults de alinhamento ainda não estão modelados
- [~] VFPv2 inicial: registradores S/D, FPSCR/FPEXC/FPSID, transferências ARM/VFP, loads/stores múltiplos, operações aritméticas, comparação e conversões
- [ ] Completar semântica VFP pendente: vetores curtos, exceções habilitadas, arredondamentos adicionais, flags de precisão/underflow e instruções de transferência legadas não suportadas
- [ ] Implementar TLB, invalidação e manutenção de caches com efeitos observáveis
- [ ] Integrar o monitor exclusivo ao caminho MMU e validar reservas perante escritas de outros agentes/dispositivos
- [ ] Executar testes de conformidade ARM11 em ARM, Thumb, CP15, VFP e exceções

## 2. Memória, endereçamento e carregamento de títulos

### Memória

- [x] Trait Bus little-endian e FlatMemory para testes e bring-up, incluindo monitor exclusivo
- [x] Espaço de endereços do processo com regiões de código, dados, BSS, pilha, heap e TLS, validação de intervalos e permissões
- [x] TLS por thread com slots zerados na reutilização e endereço exposto pelo TPIDRURO
- [x] Memória física esparsa inicial para 6 MiB de VRAM e 128 MiB de FCRAM do modelo original
- [~] Memória física atual é limitada às regiões VRAM/FCRAM usadas por GPU/LCD; ainda não existe o mapa físico completo do console
- [~] Tradução virtual ARM via MMU quando SCTLR.M está habilitado; ainda sem TLB e sem integração completa de dispositivos
- [ ] Mapa completo de memória física e barramentos mapeados para CPU, GPU, DSP, LCD, serviços e periféricos
- [ ] Regiões de memória compartilhada, DMA, cache/coerência e sincronização CPU-dispositivos
- [ ] Suporte às diferenças de memória dos modelos New 3DS, quando necessário para os alvos de compatibilidade

### Formatos e conteúdo

- [x] Parser estrutural NCSD para imagens `.3DS` e `.CCI`, com tabela de até oito partições, limites e validação de sobreposição
- [~] Parser NCCH com leitura de cabeçalho, regiões declaradas e leitores limitados por intervalo
- [~] ExeFS: leitura das entradas, extração do código e verificação de hashes implementadas em partes; assinatura RSA e todos os cenários de crypto ainda precisam de validação ponta a ponta
- [~] RomFS: cabeçalho IVFC, leitor de Level 3, cadeia SHA-256 e verificação de integridade presentes; descriptografia e todos os caminhos de instalação/uso ainda exigem validação integrada
- [~] CIA: parsers de cabeçalho, seções, ticket/TMD e leitores de conteúdo presentes; fluxo completo de instalação, seleção de conteúdo e preparação de executável ainda não é um caminho de compatibilidade concluído
- [~] Rotinas próprias de AES e SHA-256, AES-CBC/AES-CTR e descompressão BLZ presentes; a criptografia usa material fornecido explicitamente pelo usuário e não inclui chaves no repositório
- [ ] Validar assinaturas, hashes, tamanhos, alinhamentos, regiões, múltiplos conteúdos e arquivos malformados em todas as combinações relevantes
- [ ] Completar o caminho de conteúdo criptografado com chaves do usuário, sem descoberta, armazenamento ou distribuição de chaves pelo projeto

### Preparação e execução de processos

- [x] Validação do layout ARM11 a partir de metadados de executável
- [x] Mapeamento inicial de código, BSS, pilha, memória dinâmica e TLS do processo
- [x] Sessão ARM11 preparada com contexto inicial, threads, execução de instruções e dispatch de SVCs suportados
- [x] Testes sintéticos carregam NCSD/NCCH sem crypto, iniciam um processo de teste e executam instruções ARM
- [~] A execução demonstrada atualmente usa fixtures sintéticas e caminhos controlados; isso não comprova inicialização de homebrew comercialmente distribuído nem de jogos
- [ ] Definir um alvo homebrew mínimo, legalmente fornecido pelo usuário, e documentar exatamente o subconjunto de sistema necessário
- [ ] Concluir carregamento de executáveis, argumentos, dependências, permissões, serviços e preparação do processo para esse alvo
- [ ] Executar um homebrew real até produzir resultado observável, com logs e diagnóstico de falhas

## 3. Kernel ARM11 e serviços HLE

### Processos, threads e objetos

- [x] Contextos independentes por thread, escalonador cooperativo, criação e encerramento de threads
- [x] Alocação e liberação de TLS por thread
- [~] Prioridades, sleep/yield, espera com timeout e despertar existem em escopo inicial; preempção e temporização precisa ainda faltam
- [~] Objetos de mutex, semáforo, evento e timer com operações iniciais
- [~] SVCs com handlers iniciais: ControlMemory, QueryMemory, ExitProcess, CreateThread, ExitThread, SleepThread, Get/SetThreadPriority, GetProcessorId, mutexes, semáforos, eventos, timers, WaitSynchronization1 e CloseHandle
- [ ] Completar ABI, códigos de retorno, validações de parâmetros, direitos de acesso e comportamento de erro de todos os SVCs presentes
- [ ] Completar espera múltipla, timeout, cancelamento, herança de prioridade e ciclo de vida dos objetos
- [ ] Completar ciclo de vida de processos, handles, compartilhamento e limpeza de recursos

### IPC e serviços do sistema

- [ ] IPC entre processos e sessões
- [ ] Infraestrutura de registro, descoberta e despacho dos serviços do sistema
- [ ] Serviços mínimos necessários para inicializar homebrew
- [ ] Serviços adicionais priorizados pela evidência dos testes de compatibilidade
- [ ] Controlador de interrupções, timers de hardware e notificações entre dispositivos
- [ ] Testes de integração de SVC, threads e objetos com binários de teste independentes

## 4. GPU PICA200, rasterização e LCD

### GPU PICA200

- [x] Banco inicial de 0x400 registradores PICA200
- [x] Decodificador de command lists com parâmetros, cabeçalhos, máscaras de byte, escrita consecutiva e validação de limites
- [x] Descritores e acknowledgement dos canais de command buffer 0 e 1
- [~] Integração inicial no ciclo de frame: captura command buffers da memória visível ao processo e aplica as escritas aos registradores
- [~] O bridge atual trata o endereço decodificado do command buffer como endereço visível ao processo; tradução DMA/endereçamento físico do hardware ainda precisa ser implementada
- [x] Rasterizador de referência próprio em CPU para triângulos já transformados: interpolação de cor/profundidade, teste de profundidade e regra top-left; testes de cobertura, orientação, profundidade e entradas inválidas
- [x] Estágio geométrico de referência independente: matriz 4x4 column-major, transformação homogênea, clipping contra os seis planos canônicos, proteção de W positivo, triangulação em leque e viewport com Y descendente
- [x] Rasterizador de referência com interpolação de cor perspective-correct por reciprocal W, interpolação de profundidade e teste de depth
- [~] O estágio geométrico e o rasterizador podem ser usados juntos por `GeometryPipeline::draw_triangle`, mas ainda são genéricos e independentes dos registradores, shaders e comandos reais do PICA200
- [~] Snapshot do bloco de entrada de vértices diretamente do banco PICA200: base bruta 0x200, formatos globais 0x201–0x202 e doze triplets offset/config1/config2 em 0x203–0x226
- [~] Decoder de layout: até doze atributos/arrays; cada nibble de formato decodifica tipo escalar e quantidade de 1–4 componentes; largura do escalar é 1 byte (byte/ubyte), 2 (short) ou 4 (float), com alinhamento e padding
- [~] Leitura de atributos brutos por vértice de VRAM/FCRAM e conversão numérica para `f32`: extensão de sinal para byte/short assinados, conversão sem sinal para ubyte e `from_bits` para float32; sem normalização implícita e sem execução de shader
- [~] Snapshot dos registradores de índice/contagem/offset: 0x227, 0x228 e 0x22A; base e offset permanecem valores brutos de registrador
- [~] Leitor de índices U8/U16 em little-endian, com limite defensivo e validação de faixa física; ainda não aplica vertex offset nem resolve o endereço DMA
- [~] `Pica200::read_vertex_at_physical_base`, `read_numeric_vertex_at_physical_base` e `read_indices_at_physical_base` conectam snapshots ao fetch físico usando base já resolvida pelo chamador; não fazem tradução DMA, aplicação de vertex offset nem dispatch de draw
- [~] Snapshot do `GPUREG_VSH_INPUTBUFFER_CONFIG` (0x2B9) e dos registradores de permutação (0x2BB/0x2BC); mapeamento explícito dos atributos numéricos para registradores de entrada `v0–v15`, sem executar o vertex shader ou resolver valores fixos
- [ ] Resolver `GPUREG_ATTRIBBUFFERS_LOC` e endereços de arrays/índices pelo mapa físico e pela rota DMA real antes de acessar memória
- [ ] Converter componentes brutos para entradas numéricas do shader com a normalização, extensão de sinal e tratamento float corretos
- [ ] Integrar o fetch de atributos/índices ao vertex shader PICA200 e ao estágio genérico de transformação, clipping e rasterizador
- [ ] Decodificar e executar os triggers DRAWARRAYS/DRAWELEMENTS, primitiva, culling e montagem com semântica confirmada por documentação/testes próprios
- [ ] Modelo completo de command processor, filas, sincronização, interrupções e efeitos dos registradores
- [~] Transformação matricial e clipping genéricos implementados; clipping, montagem de primitivas, culling e convenções exatas do PICA200 ainda pendentes
- [ ] Fragment pipeline, texturas, filtragem, mipmaps e formatos de superfície
- [ ] Depth/stencil, blending, máscaras e operações completas de framebuffer
- [ ] Integração com memória física, DMA, cache, sincronização e interrupções
- [ ] Cenas determinísticas próprias, testes de regressão e comparação visual

### Vídeo e LCD

- [x] Superfícies de saída RGBA8 para LCD superior esquerdo, superior direito (estéreo) e inferior
- [x] Modelo inicial de registradores do controlador LCD/PDC e seleção de framebuffer
- [x] Scanout básico de framebuffers a partir da memória física para as três superfícies
- [~] Scanout e apresentação no núcleo já são chamados pelo ciclo experimental de frame; o frontend ainda não exibe essas superfícies em uma janela gráfica funcional
- [ ] Integrar a saída efetiva do PICA200 às áreas de framebuffer apresentadas
- [ ] Validar resoluções, formatos, stride, escala, orientação, alternância de buffers e sincronização
- [ ] Apresentar as duas telas no desktop e Android com redimensionamento e ciclo de vida corretos

## 5. Ciclo de execução e dispositivos

- [x] `System::run_frame_with_budget` executa um orçamento de instruções ARM11, avança o relógio virtual, processa command buffers pendentes, faz scanout LCD e incrementa o contador de frame
- [~] O orçamento de instruções é determinístico, mas não é temporização ciclo-a-ciclo; o renderizador ainda não executa a pipeline gráfica PICA200
- [ ] Integrar relógios e interrupções dos dispositivos ao escalonador e à CPU
- [ ] Modelo de áudio e DSP
- [ ] Saída de áudio desktop e Android
- [ ] Entrada por teclado, gamepad, analógicos e touchscreen
- [ ] Microfone e demais dispositivos conforme necessidade de compatibilidade
- [ ] Sincronizar entrada, áudio, vídeo e temporização do sistema

## 6. Frontends e experiência do usuário

### Desktop

- [~] Binário Rust de diagnóstico/linha de comando e autoteste
- [ ] Janela e superfície gráfica funcionais
- [ ] Biblioteca/importação de jogos para `.3DS`, `.CCI` e `.CIA`
- [ ] Configurações de vídeo, áudio, controles, caminhos e armazenamento
- [ ] Logs, relatórios de falha e diagnóstico de compatibilidade
- [ ] Save data e gerenciamento de arquivos de usuário

### Android

- [~] Crate Rust `cdylib` inicial para ARM64; o workflow compila o alvo `arm64-v8a`
- [ ] Aplicativo Android instalável, com interface implementada em Rust
- [ ] Integração de superfície, ciclo de vida, entrada, áudio e permissões
- [ ] Biblioteca de jogos, configurações e armazenamento
- [ ] Testes em dispositivos ARM64 de baixo e médio desempenho
- [ ] Diagnósticos e recuperação de falhas sem travamentos silenciosos

### Regra de implementação

- [x] A implementação do produto permanece exclusivamente em Rust
- [ ] Toda integração de plataforma deve continuar como ponte/host, sem deslocar lógica do emulador para Kotlin, Java, C, C++ ou outra linguagem

## 7. Testes, compatibilidade e desempenho

- [x] Testes unitários para CPU, memória, loaders, processo, kernel inicial, PICA200 command lists e LCD
- [~] Testes de integração sintéticos para NCSD/CCI, preparação de processo, instruções ARM, command buffers e scanout de duas telas
- [x] Testes do rasterizador, interpolação perspective-correct e estágio geométrico aprovados no workflow multiplataforma do commit `cadeb4381de1fe3227ec1d152a91c2b86ae61808`
- [x] Decoder de arrays, leitura de atributos brutos, leitura de índices e snapshots de registradores PICA200 aprovados no workflow do commit `629c3a6c5eb808724bc8e74bb8ed4af82fd959a7`
- [x] Bridge de fetch físico, conversão escalar PICA200 e mapeamento de atributos para entradas `v0–v15` aprovados em Rustfmt, Clippy e testes Linux/Windows/macOS; build Android `arm64-v8a` aprovado no workflow [43176284651](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43176284651), código no commit `8aedeb346014917935985cd5fe913ee9606e18cc`
- [x] Rustfmt, Clippy e testes em Linux, Windows e macOS aprovados no commit `629c3a6c5eb808724bc8e74bb8ed4af82fd959a7`
- [x] Build Android `arm64-v8a` aprovado no mesmo workflow; testes de execução no dispositivo Android ainda pendentes
- [ ] Testes de conformidade ARMv6/Thumb/VFP com corpus independente e legalmente utilizável
- [ ] Testes adversariais de parser para CIA/NCSD/NCCH/ExeFS/RomFS
- [ ] Testes de execução de homebrew com resultados reproduzíveis
- [ ] Testes gráficos com cenas próprias e resultados de referência
- [x] CI de regressão em Linux, Windows e macOS; build cruzado Android ARM64
- [ ] Testes de execução e ciclo de vida em dispositivo Android ARM64
- [ ] Medir desempenho antes de otimizar; preservar precisão e compatibilidade
- [ ] Perfilamento e otimizações de CPU, GPU, memória e renderização
- [ ] Matriz pública de compatibilidade baseada somente em testes realmente executados

## 8. Marcos de entrega

1. **M0 — Base estável:** CI verde no HEAD, documentação coerente e testes reproduzíveis.
2. **M1 — CPU utilizável:** ARMv6/Thumb necessários ao alvo inicial, exceções, VFP e memória integrados e validados.
3. **M2 — Primeiro homebrew:** arquivo fornecido legalmente pelo usuário preparado e executado até produzir saída observável.
4. **M3 — Kernel funcional:** SVC, threads, memória virtual, sincronização e IPC necessários ao alvo inicial.
5. **M4 — Primeira imagem 3DS:** vertex pipeline PICA200, rasterização e framebuffers integrados aos dois LCDs.
6. **M5 — Experiência jogável inicial:** entrada, vídeo, áudio e ciclo de execução integrados em um alvo de teste.
7. **M6 — Compatibilidade ampliada:** mais serviços, recursos gráficos, testes e otimizações orientados por evidências.

## Próximas tarefas prioritárias

1. Corrigir o bridge de command buffers e resolver a base dos arrays/índices para o mapa físico e DMA real; não tratar valores brutos dos registradores como endereços físicos.
2. Decodificar os valores fixos dos atributos em `GPUREG_FIXEDATTRIB_DATA0–2` (float24) e combiná-los com o mask, sem sobrescrever atributos fornecidos por arrays.
3. Implementar conversão float24, programa/uniforms e execução do vertex shader PICA200; só então conectar as saídas ao estágio geométrico e rasterizador.
4. Decodificar DRAWARRAYS/DRAWELEMENTS, primitive config, índices e vertex offset, com testes próprios para draw arrays e draw elements.
5. Completar o encaminhamento de LDREX/STREX pelo barramento MMU e validar invalidação de reservas entre agentes.
6. Adicionar TLB e semântica real de manutenção de cache/barreiras ao CP15.
7. Expor as superfícies LCD no frontend desktop e depois Android.

## Critérios para concluir um item

- O código está integrado ao caminho real de execução.
- Existem testes automatizados ou um procedimento manual reproduzível adequado ao recurso.
- As verificações aplicáveis foram executadas e seus resultados registrados.
- Nenhum jogo, firmware, chave ou dump protegido está incluído no repositório.
- Stubs, no-ops, parsers isolados e fixtures sintéticas não são apresentados como compatibilidade completa.
