# Roadmap do Aceris

> Estado revisado em 13/11/2026, com base na implementação de código até o commit `120f2d21d077fc4181c28f01ba1699e4c8d39720`.
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
- [x] README, arquitetura e roadmap sincronizados com o estado observado no commit `6e7d0e81d6d43a03d734fd24526737072b637db3`, sem anunciar compatibilidade não testada

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
- [x] SRS ARMv6: grava LR/SPSR em SP bancado selecionado, com os quatro modos de endereçamento, writeback, validação de modo e endianness; SP desalinhado é alinhado quando SCTLR.A/U estão claros e gera fault quando qualquer um está habilitado; testes aprovados no workflow [43231651724](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43231651724)
- [~] Validar todas as combinações de exceção, vetores altos, endianness, máscaras, abortos durante transferências múltiplas e retornos em sequências longas
- [x] Testes dedicados de SVC HLE e de entrada/retorno SVC de hardware, além de IRQ/FIQ com prioridade, máscaras A/I/F e restauração do CPSR; matriz aprovada no workflow [43318212186](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43318212186)
- [ ] Integrar um controlador de interrupções e temporizadores de dispositivos ao modelo de IRQ/FIQ; as linhas externas são atualmente acionadas pelo host

### Instruções ARM e Thumb

- [x] Instruções ARM básicas de processamento de dados, desvios, transferências, blocos e multiplicações comuns
- [x] A32 `MUL`/`MLA` e `UMULL`/`UMLAL`/`SMULL`/`SMLAL`: validação de `r15`, campo SBZ de `MUL` e distinção entre `RdHi`/`RdLo`; aliases entre fonte e destino permitidos no ARMv6 preservados por leitura prévia dos operandos, com testes de regressão
- [x] Famílias ARMv6 de exclusivas A32: LDREX/STREX, LDREXB/STREXB, LDREXH/STREXH e LDREXD/STREXD
- [x] Monitor exclusivo local com granularidade de reserva de 8 bytes integrado ao caminho MMU; teste verifica LDREX/STREX por aliases virtuais distintos do mesmo endereço físico. Escritas de DMA/GPU/LCD no backing físico compartilhado incrementam a geração de reserva e invalidam o monitor (mapeamentos lineares); ainda não há outros dispositivos mapeados que escrevam na memória do processo
- [~] Famílias ARMv6 de mídia/DSP, saturação, operações paralelas, extensão de sinal/zero e multiplicações de meia palavra: há implementação e testes para várias famílias, mas a cobertura integral ainda não foi demonstrada
- [x] A32 ARM1136 `SSAT16`/`USAT16`: saturação independente dos dois halfwords assinados, `CPSR.Q` sticky e rejeição de PC; testes aprovados no workflow [43317800061](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43317800061)
- [x] A32 ARM1136 `NOP32`: verdadeiro no-op que ignora a condição; `YIELD` e hints reservados continuam sem efeito no perfil modelado. Testes EQ, AL e NV aprovados no workflow [43318212186](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43318212186)
- [~] ARMv6K A32 `WFE`/`WFI`/`SEV`: registrador de evento, wake-up por IRQ/FIQ conforme máscaras, estado de espera e eventos de sessão implementados e testados. `SEV` é reportado como `SendEvent`; eventos externos entram por `signal_cpu_event`. Retornos de exceção A32 também levantam um latch consultável pelo host (`take_exception_return_wakeup`), convertido pela sessão em `ExceptionReturn` para que o scheduler sinalize os outros MP11 CPUs. Broadcast SMP automático e wake-up por debug ainda não estão integrados. CI [43329839504](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43329839504) aprovada em todas as plataformas.
- [x] A32 ARMv6 `LDRT`/`LDRBT`/`STRT`/`STRBT`: o MMU reavalia permissões User por transação sem alterar CPSR nem o privilégio global; testes de acesso permitido, rejeição de página privilegiada, TLB em cache e writeback aprovados no workflow [43321797092](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43321797092)
- [~] Interpretador Thumb de 16 bits: formatos clássicos e extensões ARMv6 (ADC/SBC, CMN/CMP, BL/BLX/BX, transferências, pilha, CPS/CPY/SETEND/BKPT, REV/SXTx/UXTx); 91 testes declarados no módulo. Inclui validação de registradores altos, PC+4 preservando bit[1], SP como operando alto e alvo de BX/BLX, BKPT com preservação integral do CPSR, ADD PC,Rm sem interworking, LDR literal/ADR com PC alinhado, branches condicionais e incondicionais com PC+4 preservando bit[1], limites de extensão de sinal dos offsets B/BNE/BL/BLX, e pares BL/BLX com alinhamento específico do destino BLX; permanecem cobertas as restrições de CMP/BX/BLX e rejeição determinística de codificações UNPREDICTABLE. A32 e Thumb compartilham os helpers ARMv6 de alinhamento de meia palavra; Thumb também usa o helper de palavra comum nos loads/stores simples: U=0/A=0 mantém rotação/alinhamento legado de palavra e rejeita meia palavra ímpar como UNPREDICTABLE; U=1/A=0 permite acessos desalinhados de palavra/meia palavra em memória Normal com endianness CPSR; A=1 gera fault de alinhamento. LDMIA/STMIA e PUSH/POP preservam Word[Align(Addr)] no legado, mantêm writeback baseado na base original e geram fault quando SCTLR.U ou SCTLR.A exige alinhamento. Testes integrados verificam Data Abort, SPSR/LR, DFAR/DFSR e ausência de alterações parciais para acessos simples e multiword; workflow [43357433947](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43357433947) aprovado em Linux/Windows/macOS e Android ARM64. BFxx/IT de NOP Thumb v6T2 e hints Thumb v7 permanecem rejeitados no perfil ARMv6K. A cobertura completa da ISA e de todos os casos UNPREDICTABLE/UNDEFINED continua pendente.
- [x] Thumb BKPT, CPS e SETEND implementados; BKPT é encaminhado como evento de breakpoint para a camada de processo
- [ ] Inventariar codificações A32 ARMv6/ARMv6K ainda não suportadas ou decodificadas incorretamente
- [~] Completar a matriz de validação Thumb aplicável ao ARM11: BFxx/IT e NOP ARMv6T2, além de hints ARMv7, são rejeitados no perfil ARMv6K; listas vazias PUSH/POP/LDM/STM são rejeitadas deterministicamente. Ainda faltam testes independentes por codificação e auditoria completa de estados UNPREDICTABLE/UNDEFINED. Não tratar Thumb-2 como requisito sem evidência de que a arquitetura-alvo o suporta
- [ ] Auditar alinhamento, endianness, flags, estados UNPREDICTABLE/UNDEFINED e efeitos colaterais de todas as famílias
- [ ] Criar testes de conformidade independentes por instrução, além dos testes unitários internos

### Coprocessadores e MMU

- [~] Estado CP15 ARM11: MIDR/CTR/TLBTR, PFR/DFR/MMFR/ISAR, SCTLR/ACTLR/CPACR, TTBR0/1, TTBCR, DACR, DFSR/IFSR/DFAR/IFAR, FCSE PID, CONTEXTIDR, TPIDR*, PRRR/NMRR e PAR; acessos privilegiados e TPIDRURO User testados. PAR permite leitura/escrita (`c7,c4,0`) e recebe o resultado das consultas VA→PA (`c7,c8,op2=0..3`).
- [x] CP15 `PRRR` (`c10,c2,0`), `NMRR` (`c10,c2,1`) e `SCTLR.TRE` têm estado, reset, máscaras e regras de privilégio; são consumidos na resolução dos atributos de memória de novos page walks. Alterações em PRRR/NMRR preservam atributos já cacheados até uma invalidação de TLB, coberta por teste.
- [x] CP15 WFI (`MCR p15,0,Rt,c7,c0,4`) suspende o core e interrompe o orçamento de execução; IRQ/FIQ acordam WFI mesmo com CPSR.I/F mascarado, e somente uma interrupção desmascarada entra no vetor. A sessão expõe linhas IRQ/FIQ externas ao host; não substitui um controlador de interrupções de dispositivos.
- [~] Operações de manutenção do TLB CP15 c8 estão ligadas ao TLB unificado de software; manutenção de cache, coerência entre agentes e algumas barreiras ainda não têm efeitos arquiteturais modelados
- [~] MMU ARMv6 de descritores curtos: TTBR0/TTBR1 e TTBCR.N; seções/superseções e coarse tables; XP=1 com AP/APX/XN e páginas grandes/pequenas; XP=0 com AP0–AP3 por subpágina, regras AP=00/S/R e extended-small-page de 4 KiB em coarse tables; FCSE PID e domínios; preserva TEX/C/B/S nos formatos suportados e resolve tipo/shareability/atributos inner/outer com PRRR/NMRR quando TRE está ativo. Atributos resolvidos permanecem no TLB até invalidação.
- [~] Faults de tradução, domínio e permissão são reportados e registrados em CP15; VA→PA codifica sucesso e o campo FSR de falha atualmente modelado no PAR. Fine tables/tiny pages permanecem fora do perfil VMSAv6; estados adicionais de FSR no PAR, faults de alinhamento para acessos de dados comuns e semântica completa de caches/barreiras ainda precisam de implementação/validação.
- [~] VFPv2: registradores S/D, FPSCR/FPEXC/FPSID, transferências ARM/VFP, loads/stores múltiplos, operações aritméticas, comparação e conversões; operações escalares consultam FPSCR.RMode e registram IXC/UFC conforme o caminho implementado
- [~] Short vectors VFPv2 implementados para VADD/VSUB/VMUL/VNMUL/VDIV, VMLA/VMLS/VNMLA/VNMLS e VMOV/VABS/VNEG/VSQRT; inclui quatro bancos circulares, wrap, Fm escalar em bank 0, destino escalar em bank 0 e validação de LEN/STRIDE. Comparações/conversões permanecem escalares; traps VFP, FLDMX/FSTMX e casos extremos ainda precisam de validação/implementação
- [x] TLB unificado de software integrado ao caminho real de tradução: cacheia seções, superseções, páginas grandes e pequenas; aplica ASID/global, oito slots de lockdown controlados por CP15 c10 e TLBIALL/TLBIMVA/TLBIASID/TLBIMVAA. A região normal ainda usa capacidade de 64 entradas e substituição round-robin simplificada, sem equivalência ao replacement set-associative físico
- [ ] Implementar caches ARM observáveis, invalidação/manutenção de caches e barreiras com efeitos coerentes entre CPU e dispositivos
- [x] Integrar o monitor exclusivo ao caminho MMU e validar reservas perante escritas da CPU por aliases virtuais distintos do mesmo endereço físico; escritas de DMA/GPU/LCD no backing físico compartilhado também invalidam reservas em mapeamentos lineares (teste de processo e de memória física)
- [ ] Executar testes de conformidade ARM11 em ARM, Thumb, CP15, VFP e exceções

## 2. Memória, endereçamento e carregamento de títulos

### Memória

- [x] Trait Bus little-endian e FlatMemory para testes e bring-up, incluindo monitor exclusivo
- [x] Espaço de endereços do processo com regiões de código, dados, BSS, pilha, heap e TLS, validação de intervalos e permissões
- [x] TLS por thread com slots zerados na reutilização e endereço exposto pelo TPIDRURO
- [x] Memória física esparsa inicial para 6 MiB de VRAM e 128 MiB de FCRAM do modelo original
- [~] Memória física esparsa compartilhada por handles clonados: CPU, GPU DMA e LCD observam o mesmo backing VRAM/FCRAM; isto não simula caches ARM nem substitui flush/invalidate de hardware
- [~] Alocador de FCRAM contíguo por páginas, com reserva automática ou em endereço físico solicitado, rejeição de sobreposição, leitura inicial zerada e liberação exata que limpa as páginas reservadas
- [~] Bus do processo mapeia aliases lineares antigos e novos diretamente para a mesma FCRAM física; leituras/escritas CPU são visíveis pelos dispositivos, QueryMemory reconhece a região como Private/RW e o unmap invalida reservas exclusivas
- [~] Memória física atual é limitada às regiões VRAM e aos 128 MiB FCRAM do modelo original; o bloco adicional de FCRAM do New 3DS e o mapa físico completo do console ainda não existem
- [~] Tradução virtual ARM via MMU quando SCTLR.M está habilitado, com TLB unificado de software e invalidação CP15 c8; atributos de memória, cache/coerência e barramentos de todos os dispositivos ainda não estão integrados
- [ ] Mapa completo de memória física e barramentos mapeados para CPU, GPU, DSP, LCD, serviços e periféricos
- [~] Regiões lineares de FCRAM acessíveis ao processo e ao DMA GPU: `ControlMemory(MEMOP_ALLOC | MEMOP_LINEAR)` e `MEMOP_FREE | MEMOP_LINEAR` reservam/liberam backing físico, honram hints não nulos nas janelas antigas e novas e registram/removem automaticamente o mapa DMA; rollback é aplicado quando uma etapa falha
- [ ] Memória compartilhada entre processos/GSP, sincronização de agentes, semântica de cache ARM/GSP flush/invalidate e coerência observável
- [ ] Suporte aos 128 MiB adicionais de FCRAM do New 3DS e demais diferenças de mapa físico entre modelos

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
- [~] SVCs com handlers iniciais: ControlMemory, QueryMemory, CreateMemoryBlock (bloco anônimo somente para SYSTEM/BASE; addr não nulo pode compartilhar uma subfaixa de uma alocação existente do heap regular; W-only é rejeitado), MapMemoryBlock, UnmapMemoryBlock, ExitProcess, CreateThread, ExitThread, SleepThread, Get/SetThreadPriority, GetProcessorId, mutexes, semáforos, eventos, timers, WaitSynchronization1 e CloseHandle
- [ ] Completar ABI e semântica de todos os SVCs; em CreateMemoryBlock ainda faltam tipos de memória regionais completos, regiões de origem além do heap regular modelado, alias que atravesse várias alocações e integração com processos/handles distintos
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
- [~] Integração inicial no ciclo de frame: captura descritores dos dois canais PICA200, lê command buffers do backing físico FCRAM/VRAM e aplica as escritas aos registradores; os pedidos GSP via IPC e eventos continuam pendentes
- [~] Parser do GX command queue em shared memory: cabeçalho de 8 bytes, ring de 15 entradas de 32 bytes, wrap do índice, contagem/status/resultado, flags halt-after e fail-if-busy
- [~] Dispatcher inicial para GX `ProcessCommandList` (ID 0x01): valida slot/client, tamanho e alinhamento, resolve intervalos de VRAM ou DMA linear, submete a lista ao parser PICA200, consome entradas, atualiza o cabeçalho e registra erro de endereço; a API exige que o bloco de shared memory já esteja mapeado no processo
- [~] Flags flush-buffer, update-GAS e fail-if-busy são reconhecidas e expostas no relatório; cache flush real, GAS e arbitragem de busy não são simulados
- [~] `GpuDmaAddressSpace` compartilha seus registros com o processo e traduz intervalos lineares virtuais para FCRAM físico: janela antiga `0x14000000–0x1C000000` com deslocamento `+0x0C000000`, e janela nova `0x30000000–0x38000000` relativa ao início de FCRAM; intervalos são registrados automaticamente pelo `ControlMemory` e validados por alinhamento, sobreposição, overflow e limite físico
- [~] `ControlMemory` aloca e libera linear FCRAM nos aliases antigo e novo; hints não nulos são honrados por reserva física exata, e ponteiros CPU mapeados usam o mesmo backing observado pelo DMA/GPU/LCD
- [~] `System::submit_pica_command_list_from_linear_memory` já resolve o intervalo completo pelo mapa DMA automático e encaminha a lista ao parser PICA200; descritores internos do command buffer PICA são tratados separadamente como endereços físicos
- [x] Rasterizador de referência próprio em CPU para triângulos já transformados: interpolação de cor/profundidade, teste de profundidade e regra top-left; testes de cobertura, orientação, profundidade e entradas inválidas
- [x] Estágio geométrico de referência independente: matriz 4x4 column-major, transformação homogênea, clipping contra os seis planos canônicos, proteção de W positivo, triangulação em leque e viewport com Y descendente
- [x] Rasterizador de referência com interpolação de cor perspective-correct por reciprocal W, interpolação de profundidade e teste de depth
- [~] O estágio geométrico e o rasterizador podem ser usados juntos por `GeometryPipeline::draw_triangle`, mas ainda são genéricos e independentes dos registradores, shaders e comandos reais do PICA200
- [~] Snapshot do bloco de entrada de vértices diretamente do banco PICA200: base bruta 0x200, formatos globais 0x201–0x202 e doze triplets offset/config1/config2 em 0x203–0x226
- [~] Decoder de layout: até doze atributos/arrays; cada nibble decodifica tipo escalar e largura de 8/16/24/32 bits; a quantidade de componentes é contada pelas referências nos seletores dos arrays; alinhamento, padding e stride validados
- [~] Leitura de atributos brutos por vértice de VRAM/FCRAM e conversão numérica para `f32`: extensão de sinal conforme largura, conversão de inteiros sem normalização implícita, float32 IEEE e float24 PICA 1.7.16; larguras float8/16 são rejeitadas explicitamente
- [~] Snapshot dos registradores de índice/contagem/offset: 0x227, 0x228 e 0x22A; base e offset permanecem valores brutos de registrador
- [~] Leitor de índices U8/U16 em little-endian, com limite defensivo e validação física; há rota alternativa de fetch DMA linear com range-check, mas ainda não aplica vertex offset nem é acionada por DRAW-elements
- [~] `Pica200::read_vertex_at_physical_base`, `read_numeric_vertex_at_physical_base` e `read_indices_at_physical_base` conectam snapshots ao fetch físico usando base já resolvida; rotas separadas `read_numeric_vertex_via_dma`/`read_indices_via_dma` usam ponteiro linear de CPU explicitamente retido pelo chamador e o mapa registrado; nenhuma rota dispara draw ou aplica vertex offset
- [~] Snapshot de `GPUREG_VSH_INPUTBUFFER_CONFIG` (0x2B9) e permutações (0x2BB/0x2BC); associação explícita dos atributos numéricos a `v0–v15`, sem execução do vertex shader
- [~] Armazenamento dos doze atributos fixos via `GPUREG_FIXEDATTRIB_INDEX` (0x232) e `DATA0–2` (0x233–0x235), incluindo writes por command list e canal imediato 0xF
- [~] Upload de instruções do vertex shader por `GPUREG_VSH_CODETRANSFER_INDEX/DATA0–7` (0x2CB–0x2D3), com avanço automático do índice de 12 bits, armazenamento do espaço de transferência endereçável e sinalização de `CODETRANSFER_END`; o flag indica apenas fim do upload, não validação/execução do programa
- [~] Upload de operand descriptors por `GPUREG_VSH_OPDESCS_INDEX/DATA0–2` (0x2D5–0x2D8), com avanço automático do offset; a capacidade armazenada corresponde ao espaço endereçável de 12 bits, não a uma afirmação sobre quantidade máxima efetiva de descritores executáveis
- [~] Decodificação de uniforms vetoriais c0–c95 pelo FIFO `GPUREG_VSH_FLOATUNIFORM_INDEX/DATA0–7`: float24 1.7.16 em três palavras empacotadas na ordem reversa e float32 IEEE em quatro palavras também na ordem reversa; índices, modo de transferência, agrupamento e limites são testados
- [~] Desempacotamento XYZW float1.7.16 (float24) e aplicação de atributos fixos selecionados pelo mask; valores fornecidos por arrays têm precedência; saída marcada como resolvida para o shader
- [~] Fetch de vértices e índices dispõe de rotas DMA que validam intervalos completos; o caminho DRAW real, a semântica do base address PICA, cache flush/invalidate e coerência ARM/GPU ainda não estão integrados
- [ ] Decodificar as codificações de instrução e operand descriptors do vertex shader em uma representação tipada; o material de ISA disponível é reverse-engineered e exige testes independentes por formato/opcode antes de habilitar execução
- [ ] Implementar executor do subconjunto de opcodes PICA200 verificado, com registradores v/o/r/c, swizzles, máscaras, conversões e controle de fluxo; não tratar o armazenamento/transferência atual como execução
- [ ] Integrar o fetch de atributos/índices ao executor do vertex shader PICA200 e ao estágio genérico de transformação, clipping e rasterizador
- [ ] Decodificar e executar os triggers DRAWARRAYS/DRAWELEMENTS, primitiva, culling e montagem com semântica confirmada por documentação/testes próprios
- [~] Modelo inicial da fila GX e despacho síncrono de `ProcessCommandList`; RequestDMA, MemoryFill, DisplayTransfer, TextureCopy, FlushCacheRegions, estados busy e sincronização assíncrona ainda não são despachados
- [~] Modelo da GSP Interrupt Relay Queue por cliente: bloco de 0x40 bytes com ring de 52 eventos, PDC0/PDC1 limitados a 0x20 com contadores de perdas, `ignore_pdc` e endereçamento de quatro clientes; o registro GSP e a entrega de eventos já usam shared memory mapeada e handles de evento. O kernel oferece CreateMemoryBlock anônimo para SYSTEM/BASE e criação de backing sobre heap regular já mapeado, além de Map/Unmap local; o despacho IPC de registro e a resposta estendida ainda não estão integrados
- [~] Existem primitivas host-side para importar (`Arm11ProcessSession::import_memory_block_handle`, COPY-like), mover (`move_memory_block_handle`) e aplicar descritores IPC type-0 a um único ou a um array de memory blocks (`translate_memory_block_handle_from_ipc` / `translate_memory_block_handles_from_ipc`): COPY mantém as origens, MOVE fecha as origens somente após importar os destinos, handles zero permanecem nulos, MOVE repetido é rejeitado e todos os objetos são pré-validados como memory blocks. `session::ipc::IpcCommandHeader` e `IpcMessage` agora leem o cabeçalho TLS, validam a contagem de words e separam parâmetros normais da seção de tradução; `decode_translation_section` reconhece estruturalmente type 0 (handles), type 1 (static buffer) e types 5/6/7 (mapeamento temporário R/W/RW). `copy_static_buffer_translation_from_ipc` aplica type 1 usando a tabela de 16 entradas do TLS da thread receptora, verifica capacidade e permissões, e prepara todas as fontes antes de escrever qualquer destino; types 5/6/7 continuam apenas decodificados, sem mapeamento efetivo. Types 2/3 já são decodificados e materializam tabelas de pares endereço físico/tamanho em static buffers do receptor, limitados a ranges FCRAM linear explicitamente mapeados; buffers privados e shared sem endereço físico são recusados. O decoder GSP consome a visão comum, e `translate_memory_block_translation_section_from_ipc` exige exatamente um grupo type-0 de handles para a transferência host-side de arrays homogêneos. O serializador de resposta estendida de `RegisterInterruptRelayQueue` modela resultado, thread ID e handle COPY. Ainda faltam o mapeamento efetivo de buffers temporários types 5/6/7, a tradução IPC geral para outros tipos de kernel object e arrays heterogêneos de handles, integração ao transporte/dispatcher real, criação do backing pelo serviço, `TriggerCmdReqQueue` pelo caminho IPC e handoff que suspenda/reative threads IPC. O `System` ainda não mantém nem agenda múltiplas sessões de processo ARM11.
- [ ] Implementar comandos GX adicionais e semântica de busy/erro/async com registros GPU e devices correspondentes
- [~] Transformação matricial e clipping genéricos implementados; clipping, montagem de primitivas, culling e convenções exatas do PICA200 ainda pendentes
- [ ] Fragment pipeline, texturas, filtragem, mipmaps e formatos de superfície
- [ ] Depth/stencil, blending, máscaras e operações completas de framebuffer
- [ ] Integração com memória física, DMA, cache, sincronização e interrupções
- [ ] Cenas determinísticas próprias, testes de regressão e comparação visual

### Vídeo e LCD

- [x] Superfícies de saída RGBA8 para LCD superior esquerdo, superior direito (estéreo) e inferior
- [x] Modelo inicial de registradores do controlador LCD/PDC e seleção de framebuffer
- [x] Scanout básico de framebuffers a partir da memória física para as três superfícies
- [~] Scanout e apresentação no núcleo são chamados pelo ciclo de frame; o frontend desktop Linux apresenta uma composição monoscópica da tela superior esquerda e da inferior em janela X11. A tela superior direita/estéreo permanece apenas como superfície separada, ainda sem seleção/apresentação no frontend.
- [ ] Integrar a saída efetiva do PICA200 às áreas de framebuffer apresentadas
- [ ] Validar resoluções, formatos, stride, escala, orientação, alternância de buffers e sincronização
- [~] `aceris <JOGO> --dump-lcd <DIR>` grava as três superfícies como PNG RGBA8 (codificador próprio, sem dependências, validado com decodificadores independentes); é saída para inspeção, não uma janela
- [~] Apresentação monoscópica das superfícies superior esquerda + inferior em janela X11 no desktop Linux; faltam seleção/alternância estéreo, redimensionamento, ciclo de vida, backends Win32/Cocoa e superfície Android.

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

- [~] Binário Rust de diagnóstico/linha de comando e autoteste; `--dump-lcd` exporta as telas como PNG após o quadro. A carga de `.3DS`/`.CCI` sem criptografia agora prepara o processo ARM11 pelo mesmo método do Android; o primeiro quadro registra instruções, motivo de parada, PC/CPSR e atividade GPU/LCD.
- [~] Janela X11 (Linux) sem crates: Xlib carregada por `dlopen` e FFI própria; `aceris <JOGO> --window [--scale N]` compõe as duas LCDs (400x480, escala inteira) e roda quadros a ~60 Hz até fechar a janela; validada sob Xvfb com captura de tela. Windows (Win32) e macOS (Cocoa) ainda não têm backend; sem entrada de teclado/toque, áudio nem redimensionamento
- [ ] Biblioteca/importação de jogos para `.3DS`, `.CCI` e `.CIA`
- [ ] Configurações de vídeo, áudio, controles, caminhos e armazenamento
- [~] Diagnóstico inicial no CLI: mensagens de erro do loader e resumo da primeira execução; logs contínuos, relatórios persistentes e diagnóstico de compatibilidade continuam pendentes.
- [ ] Save data e gerenciamento de arquivos de usuário

### Android

- [~] Crate Rust `cdylib` para ARM64; mantém o carregamento NCSD sem criptografia compartilhado com o desktop. O ABI expõe PC/CPSR e permite consultar a mensagem completa de falhas de load/frame; o workflow compila `arm64-v8a`.
- [ ] Aplicativo Android instalável, com interface implementada em Rust
- [ ] Integração de superfície, ciclo de vida, entrada, áudio e permissões
- [ ] Biblioteca de jogos, configurações e armazenamento
- [ ] Testes em dispositivos ARM64 de baixo e médio desempenho
- [~] Diagnóstico básico via ABI para consultar falhas de carregamento/execução e estado PC/CPSR; integração visual desses dados no host Android e recuperação completa continuam pendentes.

### Regra de implementação

- [x] A implementação do produto permanece exclusivamente em Rust
- [x] Desktop e Android chamam o mesmo `aceris-core` e usam o contrato comum `System::load_ncsd_no_crypto_file` para preparar NCSD sem criptografia (`.3DS`/`.CCI`)
- [ ] Cada novo marco funcional deve avançar PC e Android em paralelo; nenhum marco é considerado concluído até a validação correspondente nas duas plataformas
- [ ] Toda integração de plataforma deve continuar como ponte/host, sem deslocar lógica do emulador para Kotlin, Java, C, C++ ou outra linguagem

## 7. Testes, compatibilidade e desempenho

- [x] Testes unitários para CPU, memória, loaders, processo, kernel inicial, PICA200 command lists e LCD
- [~] Testes de integração sintéticos para NCSD/CCI, preparação de processo, instruções ARM, command buffers e scanout de duas telas; teste real com uma imagem fornecida pelo usuário ainda pendente
- [~] Testes ABI Android para leitura de PC/CPSR e recuperação da mensagem original de erro; validação de carga/execução em dispositivo ainda pendente
- [x] Testes do rasterizador, interpolação perspective-correct e estágio geométrico aprovados no workflow multiplataforma do commit `cadeb4381de1fe3227ec1d152a91c2b86ae61808`
- [x] Decoder de arrays, leitura de atributos brutos, leitura de índices e snapshots de registradores PICA200 aprovados no workflow do commit `629c3a6c5eb808724bc8e74bb8ed4af82fd959a7`
- [x] Bridge de fetch físico, formatos de 8/16/24/32 bits, conversão numérica, float24, armazenamento/aplicação de atributos fixos e mapeamento de entradas `v0–v15` aprovados em Rustfmt, Clippy e testes Linux/Windows/macOS; build Android `arm64-v8a` aprovado no workflow [43177475915](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43177475915), código no commit `b7c4b92cfc1356ddda9ba40ba96573c812443a56`
- [x] Rustfmt, Clippy e testes em Linux, Windows e macOS aprovados no commit `629c3a6c5eb808724bc8e74bb8ed4af82fd959a7`
- [x] TLB ARMv6, invalidações CP15 c8, exclusivas pelo caminho MMU, arredondamento escalar VFPv2 e regressões Thumb: Rustfmt, Clippy e `cargo test --workspace` aprovados em Linux, Windows e macOS; build Android `arm64-v8a` aprovado no workflow [43227584938](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43227584938)
- [x] Lockdown TLB c10, bit P/Victim, registradores CTR/TLBTR e regressões de invalidação: Rustfmt, Clippy e testes aprovados em Linux/Windows/macOS e build Android ARM64 no workflow [43229169229](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43229169229)
- [x] CP15 ARM11 PFR/DFR/MMFR/ISAR, FCSE PID no caminho MMU, FCSE/TLB isolation tests e TPIDRURO write rules: Rustfmt, Clippy e `cargo test --workspace` aprovados em Linux/Windows/macOS; build Android `arm64-v8a` aprovado no workflow [43230750417](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43230750417)
- [x] SRS ARMv6, incluindo modos inválidos, alinhamento condicionado por SCTLR.A/U e SP desalinhado com SPSR válido, aprovado em Linux/Windows/macOS e Android ARM64 no workflow [43231651724](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43231651724)
- [x] VFPv2 short vectors: bancos circulares single/double, wrap, broadcast scalar Fm, stride 1/2, unary sqrt, rejeição de combinações LEN/STRIDE fora da tabela VFP11 e regressões para VCMP/VCVT sempre escalares; Rustfmt, Clippy e `cargo test --workspace` aprovados em Linux/Windows/macOS e build Android `arm64-v8a` aprovado no workflow [43235400622](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43235400622)
- [x] MMU legacy XP=0: permissões AP0–AP3 de seções/páginas, regras S/R de AP=00 e extended-small-page em coarse tables; XP=1 rejeita L2 type 0b11 reservado. Rustfmt, Clippy, testes em Linux/Windows/macOS e build Android ARM64 aprovados no workflow [43234327123](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43234327123)
- [x] CP15 WFI e despertar por IRQ/FIQ: espera sem consumo de instruções, PC preservado, despertar com interrupção mascarada sem exceção e vetor IRQ quando desmascarada; Rustfmt, Clippy, testes Linux/Windows/macOS e build Android `arm64-v8a` aprovados no workflow [43236859913](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43236859913)
- [x] GSP Interrupt Relay Queue: bloco 0x40 por cliente, ring de 52 eventos, PDC overflow/ignore, integração de leitura e gravação no espaço de memória do processo e relatório `event_signal_required`; Rustfmt, Clippy e testes em Linux/Windows/macOS e build Android `arm64-v8a` aprovados no workflow [43238985073](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43238985073)
- [x] Testes adicionais de arredondamento VFP para multiplicação, divisão, raiz quadrada, underflow e flush-to-zero aprovados em Linux, Windows e macOS; build Android `arm64-v8a` aprovado no workflow [43227896584](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43227896584)
- [x] Build Android `arm64-v8a` aprovado no mesmo workflow; testes de execução no dispositivo Android ainda pendentes
- [x] Alocação/liberação linear FCRAM, aliases CPU→físico, backing compartilhado e integração ControlMemory passaram Rustfmt, Clippy e testes em Linux/Windows/macOS; build Android ARM64 também aprovado no workflow [43183171033](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43183171033), código no commit `a7cd047b17db9e1a1441f19f540941259445770a`
- [x] Bancos de upload de vertex shader (instruções e operand descriptors), conclusão de transferência e uniforms float24/float32 passaram Rustfmt, Clippy e testes Linux/Windows/macOS; build Android `arm64-v8a` aprovado no workflow [43185644119](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43185644119), código no commit `d559ef7b14fddac6cf758613f72fa75350cf96a5`; execução de shader continua pendente
- [x] Testes para VA→PA/PAR e atributos TEX/C/B/S: consultas privilegiadas/User, fallback com MMU desligada, MCR durante execução MMU traduzida, PRRR/NMRR/TRE e revalidação de atributos após TLBI; matriz aprovada no workflow [43337932833](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43337932833)
- [x] Teste guest-side das quatro variantes `MCR c7,c8,op2=0..3` (leitura/escrita privilegiada e User) e teste guest-side de FCSE com VA baixa; Rustfmt, Clippy e testes em Linux/Windows/macOS e build Android ARM64 aprovados no workflow [43338851227](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43338851227)
- [x] Thumb ARMv6 U/A e faults de alinhamento: loads/stores simples de palavra e meia palavra (incluindo LDRH/LDRSH/STRH), LDMIA/STMIA e PUSH/POP têm testes de little-endian/big-endian, alinhamento legado, acessos U=1, faults A=1 e entrada Data Abort com DFAR/DFSR; 82 testes declarados no módulo Thumb. A32 e Thumb compartilham helpers de alinhamento para meia palavra. Rustfmt, Clippy e `cargo test --workspace` aprovados em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado no workflow [43357747003](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43357747003), código no commit `32fa96d3299be5bde6e319a81dacdcbc1cc5ce78`.
- [x] Auditoria adicional Thumb T16: regressões para MOV/ADD/CMP com SP, BX/BLX SP, BKPT (imm8, StepEvent::Breakpoint, PC e CPSR), ADD PC,Rm sem interworking, LDR literal/ADR com PC alinhado, branches condicionais/incondicionais com PC+4, limites positivo/negativo de extensão de sinal e pares BL/BLX com alinhamento específico; módulo com 91 testes declarados. Código consolidado no commit `3e54357f11525448037111a7200d2ea72c35bb31`; workflow [43361987199](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43361987199) concluído com sucesso em Linux, Windows e macOS, além do build release Android `arm64-v8a`. O encaminhamento de BKPT pela sessão já tinha teste integrado em `thumb_breakpoint_stops_the_session_with_its_immediate`.
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

1. Implementar o serviço GSP IPC em torno do GX queue: mapping de shared memory, rendering rights, `TriggerCmdReqQueue`, relay queue/eventos e callbacks; conectar cache flush/invalidate somente quando houver um modelo de cache ARM observável.
2. Implementar decodificadores tipados para os formatos de instrução e operand descriptors de vertex shader, priorizando formatos 1/1i/1u/1c e opcodes aritméticos básicos documentados; adicionar testes de campo e rejeitar codificações desconhecidas. O armazenamento de código/descriptors e uniforms já existe, sem executor.
3. Decodificar DRAWARRAYS/DRAWELEMENTS, primitive config, índices e vertex offset, com testes próprios para draw arrays e draw elements.
4. Estender a invalidação de reservas LDREX/STREX a outras regiões compartilhadas e dispositivos que venham a escrever na memória do processo (DMA/GPU/LCD em FCRAM linear já invalidam).
5. Completar associatividade e replacement do TLB normal e implementar efeitos observáveis de manutenção de cache, IMB e barreiras CP15; atributos TEX/C/B/S e remapeamento PRRR/NMRR já estão parcialmente integrados e testados.
6. Evoluir a janela X11 Linux para seleção das três superfícies, redimensionamento e entrada; implementar backends nativos Win32/Cocoa e depois a superfície Android com ciclo de vida/entrada.

## Critérios para concluir um item

- O código está integrado ao caminho real de execução.
- Existem testes automatizados ou um procedimento manual reproduzível adequado ao recurso.
- As verificações aplicáveis foram executadas e seus resultados registrados.
- Nenhum jogo, firmware, chave ou dump protegido está incluído no repositório.
- Stubs, no-ops, parsers isolados e fixtures sintéticas não são apresentados como compatibilidade completa.
