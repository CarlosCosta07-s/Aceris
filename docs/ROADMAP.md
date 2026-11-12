# Roadmap do Aceris

> Estado revisado em 12/11/2026, com base no código da branch `main` no commit `8b252ac43425cbc4679732fb806b1d5a4f8b9f5c`.
>
> **Legenda:** `[x]` implementado no escopo indicado; `[~]` implementação parcial/infraestrutura inicial; `[ ]` pendente. Uma caixa marcada não significa, por si só, compatibilidade completa com o hardware ou com jogos.

Este roadmap descreve o estado observado no código, não metas já alcançadas. Atualize os itens somente após inspecionar a implementação e os testes correspondentes. Não marque uma funcionalidade como concluída apenas porque existe um tipo, parser, stub ou interface para ela.

## 0. Fundação, regras e validação

- [x] Workspace Rust com crates compartilhados, núcleo e frontends iniciais para desktop e Android
- [x] Branch principal de desenvolvimento definida como `main`
- [x] Regras de isolamento entre agentes, autoria própria e proteção de conteúdo protegido documentadas
- [x] Política de não incluir jogos, firmware, chaves, dumps ou conteúdo protegido no repositório
- [~] CI configurado para formatação, Clippy, testes multiplataforma e build Android
- [ ] Restaurar o CI verde: no workflow associado ao commit de referência, `cargo fmt --all --check` falhou em Linux, Windows e macOS; Clippy e testes foram ignorados nessas três plataformas
- [x] Build Android `arm64-v8a` concluído com sucesso no workflow associado ao commit de referência
- [ ] Executar e registrar localmente, ou por CI, a sequência completa: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` e `cargo build --locked --workspace`
- [ ] Manter este roadmap sincronizado com o código após cada marco relevante

## 1. CPU ARM11 / ARMv6 e Thumb

### Modelo do processador e estado arquitetural

- [x] Execução básica de instruções ARM em modo usuário
- [x] Registradores de estado CPSR e condições de execução
- [x] Modos privilegiados e bancos de registradores implementados inicialmente
- [x] Acesso MRS/MSR ao CPSR e ao SPSR, com verificações de privilégio
- [x] Entrada inicial na exceção SVC e retorno de exceção por instruções de processamento de dados
- [x] Retorno de exceção A32 por RFE
- [~] Vetores, prioridades, máscaras e ciclo completo de exceções/interrupções
- [ ] Modelo completo de exceções ARM11, incluindo IRQ, FIQ, abortos e comportamento preciso de retorno

### Instruções ARM e Thumb

- [x] Conjunto ARM básico de processamento de dados, desvios e transferências
- [x] Instruções Thumb clássicas e formas de desvio BL/BLX já cobertas pelo núcleo
- [x] Thumb REV, extensões de sinal e zero (SXT/UXT)
- [x] Thumb BKPT, CPS e SETEND
- [~] Instruções ARMv6 de saturação e DSP/multiplicação: há famílias implementadas, mas ainda existem codificações não suportadas
- [~] Instruções exclusivas A32 LDREX/STREX, LDREXB/STREXB, LDREXH/STREXH e LDREXD/STREXD
- [~] Monitor exclusivo local e controle por geração na memória FlatMemory; suporte ainda precisa ser integrado e validado em todos os barramentos/mapeamentos
- [ ] Completar todas as instruções ARMv6/ARMv6K exigidas pelo ARM11, após inventário de codificações ausentes
- [ ] Completar instruções Thumb-1/Thumb-2 aplicáveis ao processador-alvo e testar interações ARM/Thumb
- [ ] Revisar alinhamento, endianness, flags, comportamento indefinido/UNPREDICTABLE e efeitos colaterais de cada família

### Coprocessadores e conformidade

- [~] CP15 com registradores de identificação, controle, tradução e falhas, além de verificações iniciais de privilégio
- [~] Despacho inicial de instruções de coprocessador MRC/MCR
- [ ] Implementar MMU e tradução de endereços; o CP15 atual ainda não fornece essa funcionalidade
- [ ] Implementar TLB, manutenção de TLB/cache e barreiras com efeitos arquiteturais reais; operações atualmente aceitas como no-op não contam como implementação
- [ ] Implementar VFP e demais recursos de coprocessador exigidos pelo ARM11
- [ ] Criar testes de conformidade por instrução, incluindo casos positivos, limites, exceções e regressões

## 2. Memória, barramentos e carregamento de títulos

### Memória e MMU

- [~] Barramento e memória plana com verificações de acesso e suporte inicial ao monitor exclusivo
- [ ] Mapa de memória física do Nintendo 3DS
- [ ] Barramentos e regiões mapeadas para CPU, GPU, DSP, LCD e periféricos
- [ ] MMU ARM11: caminhamento de tabelas, domínios, permissões e faults
- [ ] TLB e invalidações coerentes com CP15
- [ ] Memória virtual por processo, regiões, alocação, proteção e compartilhamento
- [ ] Coerência e sincronização de acessos entre CPU e dispositivos

### Formatos de entrada

- [x] Parser estrutural NCSD para imagens `.3DS` e `.CCI`, com validação de entradas e limites de partições
- [~] Parser CIA: estruturas iniciais de cabeçalho, conteúdo, TMD e tickets; integração completa de instalação/carregamento ainda pendente
- [~] Parser NCCH e leitura de estruturas de cabeçalho
- [~] Leitura de ExeFS e RomFS em desenvolvimento
- [~] Rotinas próprias de SHA-256, AES-CBC/AES-CTR e descompressão BLZ presentes; integração e validação ponta a ponta pendentes
- [ ] Validar conteúdo, versões, tamanhos, alinhamentos, regiões e erros sem confiar em metadados não verificados
- [ ] Completar tratamento de conteúdo criptografado usando somente chaves e arquivos fornecidos legalmente pelo usuário, mantidos fora do repositório
- [ ] Suporte robusto a partições e conteúdos múltiplos, sem assumir que reconhecer um formato equivale a executar o título

### Primeiro carregamento executável

- [~] Estruturas iniciais de processo e sessão ARM11
- [ ] Definir e documentar o primeiro subconjunto de homebrew suportado
- [ ] Preparar imagem executável, segmentos, permissões, pilha, argumentos e estado inicial da CPU
- [ ] Carregar e mapear dependências necessárias ao título
- [ ] Executar um homebrew simples até um resultado observável, com diagnóstico de falhas
- [ ] Testar o fluxo completo de arquivo até execução, sem depender de firmware, chaves ou jogos distribuídos pelo projeto

## 3. Kernel ARM11 e serviços do sistema

### Processos, threads e objetos

- [~] Contextos independentes de threads e escalonador cooperativo
- [~] Criação/encerramento de threads e manipulação inicial de prioridades
- [~] Estruturas iniciais de processo, handles e objetos do kernel
- [~] Mutexes, semáforos, eventos e timers com operações básicas
- [~] SVCs iniciais: ControlMemory, QueryMemory, CreateThread, ExitThread, SleepThread, prioridades, mutex, semáforo, evento, timer, espera de sincronização e CloseHandle
- [ ] Completar ABI, códigos de retorno, validações de parâmetros e permissões dos SVCs existentes
- [ ] Implementar espera múltipla, timeout, despertar, cancelamento e escalonamento preemptivo conforme necessário
- [ ] Completar ciclo de vida de processos, handles, sincronização e limpeza de recursos

### IPC e serviços do sistema

- [ ] IPC entre processos e sessões
- [ ] Infraestrutura de serviços do sistema e registro/descoberta de serviços
- [ ] Serviços essenciais para inicialização e execução de homebrew
- [ ] Serviços adicionais necessários à compatibilidade de jogos, priorizados por evidência de execução
- [ ] Interrupções, timers globais e comunicação entre dispositivos
- [ ] Testes de integração de kernel e SVC com programas ARM11 reais e casos sintéticos

## 4. GPU PICA200, vídeo e LCD

### GPU

- [~] Banco inicial de registradores PICA200
- [~] Processamento inicial de listas de comandos, canais e máscaras de escrita
- [ ] Modelo completo de command processor, filas, sincronização e interrupções
- [ ] Unidade de geometria e transformação de vértices
- [ ] Processamento de atributos, uniformes e programas de shader
- [ ] Montagem e clipping de primitivas
- [ ] Rasterização de triângulos e linhas
- [ ] Texturas, formatos, filtragem e mipmaps
- [ ] Depth/stencil, blending, testes e operações de framebuffer
- [ ] Resolução de formatos e conversões de superfície usados pelo hardware
- [ ] Integração de acessos DMA/memória, cache e sincronização com a CPU
- [ ] Testes gráficos determinísticos e cenas de referência próprias

### LCD e apresentação

- [~] Estruturas iniciais para LCD superior e inferior, buffers e leitura de framebuffer
- [~] Infraestrutura inicial de apresentação de superfícies no núcleo/frontend Android
- [ ] Ligar a saída do renderizador PICA200 aos framebuffers efetivamente apresentados
- [ ] Implementar resolução, escala, orientação, layout das duas telas e proporções corretas
- [ ] Apresentar as duas telas no desktop e Android com ciclo de vida e redimensionamento corretos
- [ ] Validar atualização, sincronização e ausência de frames pretos/falsos positivos

## 5. Áudio e dispositivos de entrada

- [ ] Modelo de áudio e processamento DSP
- [ ] Saída de áudio no desktop e Android
- [ ] Entrada por teclado e gamepad
- [ ] Mapeamento configurável de botões e analógicos
- [ ] Touchscreen e coordenadas da tela inferior
- [ ] Microfone e demais dispositivos conforme necessidade de compatibilidade
- [ ] Sincronização de entrada, áudio, vídeo e temporização do sistema

## 6. Frontends e experiência do usuário

### Desktop

- [~] Frontend Rust inicial, atualmente voltado a diagnóstico/execução básica
- [ ] Janela e superfície gráfica funcionais
- [ ] Biblioteca de jogos e importação de arquivos `.3DS`, `.CCI` e `.CIA`
- [ ] Configurações de vídeo, áudio, controles e caminhos de usuário
- [ ] Logs, relatórios de falha e diagnóstico de compatibilidade
- [ ] Salvamento, carregamento e gerenciamento de arquivos de usuário

### Android

- [~] Biblioteca Rust nativa `aceris-android` inicial
- [ ] Aplicativo Android completo e empacotamento instalável
- [ ] Integração de superfície, ciclo de vida, entrada e áudio
- [ ] Biblioteca de jogos, configurações e permissões de armazenamento
- [ ] Testes em ARM64 e dispositivos de desempenho baixo/médio
- [ ] Diagnósticos e recuperação de falhas sem travamentos silenciosos

### Regra de implementação

- [x] Implementação do produto exclusivamente em Rust, conforme as regras do projeto
- [ ] Manter qualquer integração de plataforma estritamente como ponte/host; não deslocar lógica do emulador para Kotlin, Java, C, C++ ou outra linguagem

## 7. Testes, compatibilidade e desempenho

- [ ] Suíte de testes unitários por subsistema
- [ ] Testes de integração CPU + memória + kernel + carregador
- [ ] Testes de conformidade ARMv6/Thumb com corpus independente e legalmente utilizável
- [ ] Testes de parser com arquivos sintéticos, truncados, inválidos e limites adversariais
- [ ] Testes de execução de homebrew com resultados reproduzíveis
- [ ] Testes de GPU com cenas e resultados de referência próprios
- [ ] Testes de regressão em Linux, Windows, macOS e Android ARM64
- [ ] Medir desempenho antes de otimizar; preservar precisão e compatibilidade
- [ ] Perfilamento e otimizações de CPU, GPU, memória e renderização
- [ ] Matriz pública de compatibilidade baseada somente em testes realmente executados

## 8. Marcos de entrega

1. **M0 — Base estável:** CI verde, documentação coerente e testes reproduzíveis.
2. **M1 — CPU utilizável:** conjunto ARMv6/Thumb necessário ao primeiro alvo, exceções e memória básica cobertos.
3. **M2 — Primeiro homebrew:** arquivo legalmente fornecido pelo usuário carregado e executado até produzir saída observável.
4. **M3 — Kernel funcional:** SVC, threads, memória virtual e IPC necessários ao alvo inicial.
5. **M4 — Primeira imagem 3DS:** GPU PICA200 mínima ligada aos dois LCDs, com apresentação real.
6. **M5 — Experiência jogável inicial:** entrada, vídeo, áudio e ciclo de execução integrados em um título de teste.
7. **M6 — Compatibilidade ampliada:** mais serviços, recursos gráficos, testes e otimizações, sempre guiados por evidências.

## Critérios para marcar um marco como concluído

- O código correspondente existe e está integrado ao caminho real de execução.
- Há testes automatizados ou um procedimento manual reproduzível adequado ao recurso.
- As verificações aplicáveis foram realmente executadas e seus resultados registrados.
- Não há dependência de conteúdo protegido incluído no repositório.
- A documentação descreve limitações restantes; stubs, no-ops e parsers isolados não contam como funcionalidade completa.
