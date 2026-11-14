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

Para o primeiro bring-up de uma imagem real, desktop e Android usam o mesmo contrato do núcleo: ambos chamam `System::load_ncsd_no_crypto_file` para preparar o processo ARM11 de contêineres NCSD sem criptografia, incluindo arquivos `.3ds` e `.cci`. O desktop registra PC, CPSR e o motivo de parada do primeiro quadro; o ABI Android expõe PC/CPSR e permite recuperar a mensagem original de falhas de carregamento ou execução. Imagens criptografadas continuam exigindo chaves fornecidas explicitamente pelo host; o Aceris não as procura nem as armazena.

Da mesma forma, o ciclo de execução e o dispatcher SVC/IPC/GSP são implementados no `aceris-core`; os frontends PC e Android devem integrar e validar essas mesmas funções em paralelo, sem manter caminhos funcionais diferentes.

## CPU ARM11

O núcleo usa um interpretador para estados A32 e Thumb. O estado da CPU inclui CPSR, bancos de registradores para User/System, FIQ, IRQ, Supervisor, Abort e Undefined, além de SPSR nos modos de exceção.

A implementação inclui processamento de dados, desvios, transferências de memória, operações de bloco, multiplicações, instruções ARMv6 de mídia/DSP, exclusivas, CPS, SETEND, BKPT, SRS/RFE, WFI, BXJ no perfil Jazelle trivial/desabilitado e sinais IRQ/FIQ. Há testes diferenciais independentes para as 36 variantes de operações aritméticas paralelas e para as famílias DSP de meia palavra, produtos duplos, multiplicação por meia palavra, resultados altos e USAD8/USADA8. SRS usa o SP bancado do modo-alvo e respeita SCTLR.A/U para alinhamento; a entrada de exceções permanece opcional. CP15 WFI suspende a busca de instruções até IRQ/FIQ e o despertar respeita as máscaras do CPSR apenas para a tomada da exceção. O suporte é amplo, mas ainda não constitui uma implementação certificada de todas as codificações ARMv6/ARMv6K.

As falhas de instrução não implementada ou acesso inválido são reportadas explicitamente; não devem ser interpretadas como compatibilidade.

## CP15, MMU e TLB

O CP15 mantém MIDR, CTR, TLBTR, registradores de feature ARM11 PFR/DFR/MMFR/ISAR, registradores de controle, TTBR0/TTBR1, TTBCR, DACR, FCSE PID, CONTEXTIDR, registradores de fault e identificadores de thread.

A MMU implementa descritores curtos ARMv6 com seções, superseções e coarse page tables. No formato XP=1, aplica AP/APX/XN em páginas grandes e pequenas e rejeita descritores L2 type 0b11 como reservados. No formato legado XP=0, aplica um AP para seções, AP0–AP3 para subpáginas de páginas grandes/pequenas, regras S/R para AP=00 e extended-small-page de 4 KiB em coarse tables. O FCSE converte VAs abaixo de 32 MiB em MVAs antes do page walk e do TLB. Faults de tradução, domínio e permissão são encaminhados aos registradores de fault CP15.

O caminho de tradução usa um TLB unificado de software com correspondência por ASID e entradas globais. O CP15 c10 controla oito slots de lockdown por P/Victim; TLBIALL preserva esses slots, enquanto invalidações seletivas aplicam suas regras próprias. As operações CP15 c8 de invalidação global, por MVA+ASID, por ASID e por MVA ignorando ASID são aplicadas antes do próximo acesso da CPU. CP15 WFI (`c7,c0,4`) é reconhecido em MCR e suspende a execução; uma linha IRQ/FIQ acorda o core mesmo quando a interrupção está mascarada, mas a exceção só é tomada quando a máscara correspondente está limpa.

A região normal do TLB tem 64 entradas de software com substituição round-robin; os oito slots de lockdown são separados e explícitos. A substituição normal ainda não reproduz a associatividade de duas vias nem o algoritmo do hardware. Fine page tables e tiny pages são obsoletas no VMSAv6 e não fazem parte do alvo; atributos TEX, faults de alinhamento para acessos de dados comuns, caches ARM e coerência de manutenção entre CPU e dispositivos continuam pendentes.

## VFPv2

O estado VFP inclui registradores S0–S31 (com pares D), FPSCR, FPEXC, FPSID, MVFR0/MVFR1 e os registradores de contexto FPINST/FPINST2. MVFR0/MVFR1 são somente-leitura e refletem o perfil VFP11 modelado; FPEXC preserva os campos definidos EX/EN/FP2V/VECITR/INV/UFC/OFC/IOC, enquanto os campos SBZ são lidos como zero. FPINST e FPINST2 são privilegiados; FPINST inicia em 0xEE000A00 e FPINST2 usa zero determinístico no modelo, embora seu reset arquitetural seja UNPREDICTABLE. Há transferências entre registradores ARM e VFP, loads/stores escalares e múltiplos, operações aritméticas escalares, comparação, raiz quadrada e conversões.

As operações escalares consultam FPSCR.RMode e registram flags como IXC e UFC nos caminhos implementados. Um subconjunto de short vectors também está integrado: operações aritméticas de três operandos (VMLA/VMLS/VNMLA/VNMLS/VMUL/VNMUL/VADD/VSUB/VDIV) e operações de dois operandos (VMOV/VABS/VNEG/VSQRT) percorrem os quatro bancos circulares, aplicam wrap, suportam Fm escalar em bank 0 e respeitam a seleção escalar quando Fd está em bank 0. Combinações LEN/STRIDE fora da tabela do VFP11 são rejeitadas como imprevisíveis antes da seleção escalar/vetorial; comparações, conversões inteiras e conversões de precisão permanecem escalares independentemente do LEN/STRIDE. As formas legadas FLDMX/FSTMX para transferências de contexto double com contagem ímpar de words são aceitas e possuem teste de round-trip. A implementação ainda é parcial: o estado e os registradores de contexto necessários ao suporte a exceções estão modelados, mas o hardware não gera ainda o estado de bounce nem integra o tratamento de traps ao scheduler/pipeline. Casos extremos de precisão/subnormais também precisam de trabalho e validação independente. Não se deve inferir compatibilidade integral do VFPv2 a partir dos testes existentes.

## Memória e barramentos

A interface `Bus` define os acessos usados pela CPU. `FlatMemory` atende testes e bring-up. O modelo físico esparso atual inclui regiões iniciais de VRAM e os 128 MiB de FCRAM do modelo original.

O espaço de endereços do processo mapeia regiões de código, dados, BSS, heap, pilha e TLS. Aliases lineares de FCRAM podem compartilhar o mesmo backing físico observado pela CPU e pelo caminho DMA/GPU/LCD.

Esse compartilhamento de backing não simula caches de CPU nem garante coerência de hardware. O mapa físico completo, memória compartilhada entre processos e manutenção/flush/invalidate observável ainda precisam ser implementados.

## Loader e formatos

O loader identifica e valida estruturalmente imagens NCSD, incluindo as partições usadas em contêineres `.3DS` e `.CCI`. Existem parsers e leitores parciais para NCCH, ExeFS, RomFS e CIA, além de rotinas de hash, AES e BLZ.

A presença de parser não representa suporte completo a um título. Assinaturas, todos os caminhos criptográficos, seleção/preparação de conteúdo e execução ponta a ponta ainda precisam ser validados. O projeto não inclui jogos, firmware ou chaves.

## Kernel e processos

A infraestrutura atual contém contextos ARM11 por thread, escalonamento cooperativo, TLS, objetos de sincronização e um subconjunto de SVCs. `CreateMemoryBlock` (0x1E) cria blocos anônimos zerados para SYSTEM/BASE ou backing compartilhado sobre uma faixa elegível do heap regular; `MapMemoryBlock` e `UnmapMemoryBlock` validam tipo de handle, permissões e mapeamento. Isso ainda não equivale à gestão completa de processos, regiões físicas ou objetos de kernel.

O transporte IPC inicial já entra pelo caminho real do convidado: `SVC ConnectToPort` aceita `srv:`, `SVC SendSyncRequest` entrega o command buffer da TLS (`+0x80`) ao dispatcher comum do `System`, e a resposta volta à TLS e à thread que originou a chamada. O gerenciador exige `RegisterClient` antes de `GetServiceHandle("gsp::Gpu")`. Essa implementação cobre somente esse serviço e o fluxo mínimo necessário para registrar e remover uma fila GSP; não é um registry IPC geral, ainda não suporta sessões de processos independentes e não modela o ProcessId fornecido pelo kernel.

O GSP cria o bloco compartilhado do cliente, associa o event handle, e trata `RegisterInterruptRelayQueue`, `TriggerCmdReqQueue`, `UnregisterInterruptRelayQueue`, `TryAcquireRight`, `AcquireRight` e `ReleaseRight`. `AcquireRight` suspende threads ARM11 que aguardam; o handoff FIFO grava a resposta em cada TLS pendente e recoloca as threads no escalonador. A fila GX processa parcialmente `ProcessCommandList` e `MemoryFill`; neste último, os buffers de VRAM são preenchidos e a conclusão gera `PSC0`/`PSC1` na relay queue, com sinalização do event handle. Ainda faltam IPC genérico, multi-processo, outros serviços, tradução efetiva dos demais tipos de buffers/handles e o conjunto restante de comandos GSP.

## GPU PICA200 e LCD

A GPU possui banco inicial de registradores, parser PICA200, canais de command buffer, memória física esparsa e um modelo da GSP Interrupt Relay Queue. O dispatcher conectado ao IPC convidado processa `ProcessCommandList` (ID 0x01) e `MemoryFill` (ID 0x02); `MemoryFill` suporta padrões de 16/24/32 bits em faixas de VRAM validadas e notifica a conclusão pela relay queue e pelo event handle do cliente. A implementação de `ProcessCommandList` lê e valida os comandos na memória física/DMA e submete as escritas ao parser PICA200. Não há ainda executor de vertex shader ou pipeline PICA200 completa.

Os comandos GX `RequestDMA`, `DisplayTransfer` e `TextureCopy`, cache flush/invalidate, arbitragem completa de busy, estados externos de hardware, coerência de cache ARM/GPU e semântica assíncrona completa continuam pendentes. A infraestrutura atual também não transforma a presença dos parsers em compatibilidade de jogos.

Ainda faltam a execução real dos vertex shaders PICA200, decodificação e execução integrada de DRAWARRAYS/DRAWELEMENTS, fragment pipeline, texturas, blending, sincronização GSP/IPC e coerência de cache.

O LCD possui superfícies de saída e scanout básico dos framebuffers para as telas. O resultado PICA200 ainda não está integrado a uma pipeline gráfica completa e os frontends não apresentam as duas telas em uma janela gráfica funcional.

## Frontends e validação

- **Desktop:** o binário mantém diagnóstico/autoteste por linha de comando e possui uma janela experimental X11 no Linux. A janela reproduz a abertura original, apresenta a biblioteca compartilhada com movimento ambiente a 60 Hz e aceita sequências pessoais de fundos RGBA por `--background-dir`/`--background-fps`. A apresentação dos LCDs permanece experimental; não representa compatibilidade de jogos. Backends Win32/Cocoa, áudio e controles de jogo ainda estão pendentes.
- **Android:** o crate Rust compila como `arm64-v8a` e expõe C ABI para abertura/biblioteca, fundos personalizados (com quadros separados para vertical e horizontal), preferência de orientação e geometria das duas telas, além de desenho/hit-test de controles virtuais. Ainda não há Activity/host instalável que apresente essas superfícies, escolha mídias, solicite rotação do sistema ou encaminhe toques ao dispositivo HID do núcleo. Não considerar os controles jogáveis até essa última integração.
- **Contrato visual:** a lógica RGBA da biblioteca e as geometrias de tela/controles ficam em `aceris-core`; os frontends hospedam essas funções e preservam a identidade definida em `docs/IDENTIDADE_VISUAL.md`. Especificações e formatos de fundos estão em `docs/FRONTENDS.md`.
- **CI:** Rustfmt, Clippy e testes do workspace são executados para Linux, Windows e macOS; o build nativo Android ARM64 é validado em workflow separado.

Comandos locais de verificação:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --locked --workspace
```

O último comando deve ser executado e registrado separadamente; não está implícito nos três anteriores.
