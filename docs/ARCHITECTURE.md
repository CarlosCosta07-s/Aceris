# Arquitetura

## Visão geral

```
frontends/desktop ─┐
                   ├──► aceris-core ──► aceris-common
frontends/android ─┘
```

O núcleo concentra a emulação e não depende de janelas, áudio de saída ou interface gráfica. Os frontends são responsáveis por hospedar o núcleo, receber entrada, apresentar vídeo, reproduzir áudio e administrar o ciclo de vida da aplicação.

O fluxo de execução experimental é:

```
Frontend
  └── System
        ├── sessão/processo ARM11
        │     ├── CPU A32/Thumb
        │     ├── CP15 / MMU / TLB
        │     ├── VFPv2
        │     └── kernel HLE / SVC
        ├── memória física compartilhada
        ├── fila de comandos PICA200 / GX
        └── LCD / scanout das telas
```

`System::run_frame_with_budget` executa um orçamento determinístico de instruções, avança o relógio virtual, processa command buffers PICA200 pendentes e faz o scanout LCD. O orçamento não é ciclo-a-ciclo e o caminho gráfico ainda não executa uma cena completa de um jogo.

## Crates

| Crate | Responsabilidade |
|---|---|
| `aceris-common` | Tipos de erro e resultado compartilhados. |
| `aceris-core` | CPU ARM11, CP15/MMU, VFP, memória, loader, kernel inicial, GPU PICA200, LCD e sistema. |
| `aceris-desktop` | Binário de diagnóstico e autoteste por linha de comando; ainda não é um frontend gráfico jogável. |
| `aceris-android` | Biblioteca nativa Rust `cdylib` para Android; ainda não constitui um aplicativo Android completo. |
| `aceris-icongen` | Ferramenta Rust interna para gerar os ícones originais do projeto. |

O produto permanece implementado em Rust e usa somente crates internos do workspace, conforme as regras do repositório.

## CPU ARM11

O núcleo usa um interpretador para estados A32 e Thumb. O estado da CPU inclui CPSR, bancos de registradores para User/System, FIQ, IRQ, Supervisor, Abort e Undefined, além de SPSR nos modos de exceção.

A implementação inclui processamento de dados, desvios, transferências de memória, operações de bloco, multiplicações, instruções ARMv6 de mídia/DSP, exclusivas, CPS, SETEND, BKPT, SRS/RFE e sinais IRQ/FIQ. SRS usa o SP bancado do modo-alvo e respeita SCTLR.A/U para alinhamento; a entrada de exceções permanece opcional. O suporte é amplo, mas ainda não constitui uma implementação certificada de todas as codificações ARMv6/ARMv6K.

As falhas de instrução não implementada ou acesso inválido são reportadas explicitamente; não devem ser interpretadas como compatibilidade.

## CP15, MMU e TLB

O CP15 mantém MIDR, CTR, TLBTR, registradores de feature ARM11 PFR/DFR/MMFR/ISAR, registradores de controle, TTBR0/TTBR1, TTBCR, DACR, FCSE PID, CONTEXTIDR, registradores de fault e identificadores de thread.

A MMU implementa descritores curtos ARMv6 com seções, superseções e coarse page tables. No formato XP=1, aplica AP/APX/XN em páginas grandes e pequenas e rejeita descritores L2 type 0b11 como reservados. No formato legado XP=0, aplica um AP para seções, AP0–AP3 para subpáginas de páginas grandes/pequenas, regras S/R para AP=00 e extended-small-page de 4 KiB em coarse tables. O FCSE converte VAs abaixo de 32 MiB em MVAs antes do page walk e do TLB. Faults de tradução, domínio e permissão são encaminhados aos registradores de fault CP15.

O caminho de tradução usa um TLB unificado de software com correspondência por ASID e entradas globais. O CP15 c10 controla oito slots de lockdown por P/Victim; TLBIALL preserva esses slots, enquanto invalidações seletivas aplicam suas regras próprias. As operações CP15 c8 de invalidação global, por MVA+ASID, por ASID e por MVA ignorando ASID são aplicadas antes do próximo acesso da CPU.

A região normal do TLB tem 64 entradas de software com substituição round-robin; os oito slots de lockdown são separados e explícitos. A substituição normal ainda não reproduz a associatividade de duas vias nem o algoritmo do hardware. Fine page tables e tiny pages são obsoletas no VMSAv6 e não fazem parte do alvo; atributos TEX, faults de alinhamento para acessos de dados comuns, caches ARM e coerência de manutenção entre CPU e dispositivos continuam pendentes.

## VFPv2

O estado VFP inclui registradores S0–S31 (com pares D), FPSCR, FPEXC e FPSID. Há transferências entre registradores ARM e VFP, loads/stores escalares e múltiplos, operações aritméticas escalares, comparação, raiz quadrada e conversões.

As operações escalares consultam FPSCR.RMode e registram flags como IXC e UFC nos caminhos implementados. Um subconjunto de short vectors também está integrado: operações aritméticas de três operandos (VMLA/VMLS/VNMLA/VNMLS/VMUL/VNMUL/VADD/VSUB/VDIV) e operações de dois operandos (VMOV/VABS/VNEG/VSQRT) percorrem os quatro bancos circulares, aplicam wrap, suportam Fm escalar em bank 0 e respeitam a seleção escalar quando Fd está em bank 0. LEN/STRIDE que repetiriam um registrador são rejeitados como imprevisíveis. Comparações e conversões permanecem escalares. A implementação ainda é parcial: exceções VFP habilitadas/trap, formas legadas FLDMX/FSTMX e casos extremos de precisão/subnormais precisam de trabalho e validação independente. Não se deve inferir compatibilidade integral do VFPv2 a partir dos testes existentes.

## Memória e barramentos

A interface `Bus` define os acessos usados pela CPU. `FlatMemory` atende testes e bring-up. O modelo físico esparso atual inclui regiões iniciais de VRAM e os 128 MiB de FCRAM do modelo original.

O espaço de endereços do processo mapeia regiões de código, dados, BSS, heap, pilha e TLS. Aliases lineares de FCRAM podem compartilhar o mesmo backing físico observado pela CPU e pelo caminho DMA/GPU/LCD.

Esse compartilhamento de backing não simula caches de CPU nem garante coerência de hardware. O mapa físico completo, memória compartilhada entre processos e manutenção/flush/invalidate observável ainda precisam ser implementados.

## Loader e formatos

O loader identifica e valida estruturalmente imagens NCSD, incluindo as partições usadas em contêineres `.3DS` e `.CCI`. Existem parsers e leitores parciais para NCCH, ExeFS, RomFS e CIA, além de rotinas de hash, AES e BLZ.

A presença de parser não representa suporte completo a um título. Assinaturas, todos os caminhos criptográficos, seleção/preparação de conteúdo e execução ponta a ponta ainda precisam ser validados. O projeto não inclui jogos, firmware ou chaves.

## Kernel e processos

A infraestrutura atual contém contextos por thread, escalonamento cooperativo, TLS, objetos iniciais de sincronização e um conjunto parcial de SVCs. O dispatch de serviços do sistema, IPC completo, temporização precisa, interrupções de dispositivos e ciclo de vida completo de processos/handles continuam pendentes.

## GPU PICA200 e LCD

A GPU possui banco inicial de registradores, parser de command lists, canais de command buffer, fila GX inicial e armazenamento de transferências de código/descriptors/uniforms de vertex shader. Há fetch inicial de atributos e índices, transformação geométrica genérica, clipping e rasterizador de referência em CPU.

Ainda faltam a execução real dos vertex shaders PICA200, decodificação e execução integrada de DRAWARRAYS/DRAWELEMENTS, fragment pipeline, texturas, blending, sincronização GSP/IPC e coerência de cache.

O LCD possui superfícies de saída e scanout básico dos framebuffers para as telas. O resultado PICA200 ainda não está integrado a uma pipeline gráfica completa e os frontends não apresentam as duas telas em uma janela gráfica funcional.

## Frontends e validação

- **Desktop:** atualmente oferece diagnóstico/autoteste por linha de comando; janela gráfica, apresentação de telas, áudio, controles e biblioteca de títulos estão pendentes.
- **Android:** o crate Rust nativo compila para `arm64-v8a`; aplicativo instalável completo, interface, ciclo de vida, entrada, áudio e testes em dispositivo real ainda estão pendentes.
- **CI:** Rustfmt, Clippy e testes do workspace são executados para Linux, Windows e macOS; o build nativo Android ARM64 é validado em workflow separado.

Comandos locais de verificação:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --locked --workspace
```

O último comando deve ser executado e registrado separadamente; não está implícito nos três anteriores.
