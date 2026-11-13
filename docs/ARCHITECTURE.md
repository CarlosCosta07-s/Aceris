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

## CPU ARM11

O núcleo usa um interpretador para estados A32 e Thumb. O estado da CPU inclui CPSR, bancos de registradores para User/System, FIQ, IRQ, Supervisor, Abort e Undefined, além de SPSR nos modos de exceção.

A implementação inclui processamento de dados, desvios, transferências de memória, operações de bloco, multiplicações, instruções ARMv6 de mídia/DSP, exclusivas, CPS, SETEND, BKPT, SRS/RFE, WFI e sinais IRQ/FIQ. SRS usa o SP bancado do modo-alvo e respeita SCTLR.A/U para alinhamento; a entrada de exceções permanece opcional. CP15 WFI suspende a busca de instruções até IRQ/FIQ e o despertar respeita as máscaras do CPSR apenas para a tomada da exceção. O suporte é amplo, mas ainda não constitui uma implementação certificada de todas as codificações ARMv6/ARMv6K.

As falhas de instrução não implementada ou acesso inválido são reportadas explicitamente; não devem ser interpretadas como compatibilidade.

## CP15, MMU e TLB

O CP15 mantém MIDR, CTR, TLBTR, registradores de feature ARM11 PFR/DFR/MMFR/ISAR, registradores de controle, TTBR0/TTBR1, TTBCR, DACR, FCSE PID, CONTEXTIDR, registradores de fault e identificadores de thread.

A MMU implementa descritores curtos ARMv6 com seções, superseções e coarse page tables. No formato XP=1, aplica AP/APX/XN em páginas grandes e pequenas e rejeita descritores L2 type 0b11 como reservados. No formato legado XP=0, aplica um AP para seções, AP0–AP3 para subpáginas de páginas grandes/pequenas, regras S/R para AP=00 e extended-small-page de 4 KiB em coarse tables. O FCSE converte VAs abaixo de 32 MiB em MVAs antes do page walk e do TLB. Faults de tradução, domínio e permissão são encaminhados aos registradores de fault CP15.

O caminho de tradução usa um TLB unificado de software com correspondência por ASID e entradas globais. O CP15 c10 controla oito slots de lockdown por P/Victim; TLBIALL preserva esses slots, enquanto invalidações seletivas aplicam suas regras próprias. As operações CP15 c8 de invalidação global, por MVA+ASID, por ASID e por MVA ignorando ASID são aplicadas antes do próximo acesso da CPU. CP15 WFI (`c7,c0,4`) é reconhecido em MCR e suspende a execução; uma linha IRQ/FIQ acorda o core mesmo quando a interrupção está mascarada, mas a exceção só é tomada quando a máscara correspondente está limpa.

A região normal do TLB tem 64 entradas de software com substituição round-robin; os oito slots de lockdown são separados e explícitos. A substituição normal ainda não reproduz a associatividade de duas vias nem o algoritmo do hardware. Fine page tables e tiny pages são obsoletas no VMSAv6 e não fazem parte do alvo; atributos TEX, faults de alinhamento para acessos de dados comuns, caches ARM e coerência de manutenção entre CPU e dispositivos continuam pendentes.

## VFPv2

O estado VFP inclui registradores S0–S31 (com pares D), FPSCR, FPEXC e FPSID. Há transferências entre registradores ARM e VFP, loads/stores escalares e múltiplos, operações aritméticas escalares, comparação, raiz quadrada e conversões.

As operações escalares consultam FPSCR.RMode e registram flags como IXC e UFC nos caminhos implementados. Um subconjunto de short vectors também está integrado: operações aritméticas de três operandos (VMLA/VMLS/VNMLA/VNMLS/VMUL/VNMUL/VADD/VSUB/VDIV) e operações de dois operandos (VMOV/VABS/VNEG/VSQRT) percorrem os quatro bancos circulares, aplicam wrap, suportam Fm escalar em bank 0 e respeitam a seleção escalar quando Fd está em bank 0. Combinações LEN/STRIDE fora da tabela do VFP11 são rejeitadas como imprevisíveis antes da seleção escalar/vetorial; comparações, conversões inteiras e conversões de precisão permanecem escalares independentemente do LEN/STRIDE. A implementação ainda é parcial: exceções VFP habilitadas/trap, formas legadas FLDMX/FSTMX e casos extremos de precisão/subnormais precisam de trabalho e validação independente. Não se deve inferir compatibilidade integral do VFPv2 a partir dos testes existentes.

## Memória e barramentos

A interface `Bus` define os acessos usados pela CPU. `FlatMemory` atende testes e bring-up. O modelo físico esparso atual inclui regiões iniciais de VRAM e os 128 MiB de FCRAM do modelo original.

O espaço de endereços do processo mapeia regiões de código, dados, BSS, heap, pilha e TLS. Aliases lineares de FCRAM podem compartilhar o mesmo backing físico observado pela CPU e pelo caminho DMA/GPU/LCD.

Esse compartilhamento de backing não simula caches de CPU nem garante coerência de hardware. O mapa físico completo, memória compartilhada entre processos e manutenção/flush/invalidate observável ainda precisam ser implementados.

## Loader e formatos

O loader identifica e valida estruturalmente imagens NCSD, incluindo as partições usadas em contêineres `.3DS` e `.CCI`. Existem parsers e leitores parciais para NCCH, ExeFS, RomFS e CIA, além de rotinas de hash, AES e BLZ.

A presença de parser não representa suporte completo a um título. Assinaturas, todos os caminhos criptográficos, seleção/preparação de conteúdo e execução ponta a ponta ainda precisam ser validados. O projeto não inclui jogos, firmware ou chaves.

## Kernel e processos

A infraestrutura atual contém contextos por thread, escalonamento cooperativo, TLS, objetos iniciais de sincronização e um conjunto parcial de SVCs. O exheader agora fornece o tipo de memória ARM11 a partir do descritor OtherCapabilities (ACI, offset 0x170); descritor ausente ou valor não reconhecido é tratado conservadoramente como APPLICATION. `CreateMemoryBlock` (0x1E) cria um bloco anônimo zerado somente para processos SYSTEM/BASE; APPLICATION e tipos desconhecidos recebem `0xD92007EA`. Com `addr` não nulo, o bootstrap aceita uma faixa alinhada por página dentro de uma única alocação existente do heap regular criada pelo `ControlMemory` modelado, e promove o backing sem copiar os bytes. A implementação ainda não aceita faixas que cruzem alocações, segmentos estáticos, TLS, aliases shared já mapeados ou memória linear; o backing do heap permanece um `Vec` do host promovido a `Arc<Mutex<_>>`, não um alocador físico FCRAM completo. `MapMemoryBlock` (0x1F) e `UnmapMemoryBlock` (0x20) mapeiam e removem aliases locais com validação de permissões; R=1, W=2, RW=3, mas W isolado é rejeitado. Há primitivas host-side para importar (`Arm11ProcessSession::import_memory_block_handle`) e mover (`move_memory_block_handle`) memory-block handles entre tabelas de duas sessões, mantendo o mesmo backing e invertendo permissões owner/peer; a operação MOVE fecha o handle de origem após importar e fecha o handle de destino se o fechamento da origem falhar. `IpcHandleTranslationDescriptor` decodifica o descritor IPC type-0 (quantidade, MOVE e ProcessId), e `Arm11ProcessSession::translate_memory_block_handle_from_ipc` aplica COPY/MOVE para um memory block, e `translate_memory_block_handles_from_ipc` trata arrays homogêneos: valida todos os handles antes de criar destinos, mantém nulos, rejeita tipos incompatíveis e handles repetidos em MOVE, e fecha as origens somente depois de importar os destinos. `session::ipc::IpcCommandHeader` e `IpcMessage` agora oferecem a leitura comum do cabeçalho TLS, validam o total de words declarado e separam parâmetros normais da seção de tradução; `IpcMessage::translation_parameters` e `decode_translation_section` reconhecem type 0 (handles), type 1 (static buffer) e types 5/6/7 (mapeamentos temporários com permissões). `copy_static_buffer_translation_from_ipc` aplica as cópias type-1 na tabela TLS da thread receptora, validando os 16 slots, capacidade e permissões de escrita; todos os bytes de origem são preparados antes de modificar os destinos. Types 5/6/7 continuam sem mapeamento efetivo. Types 2/3 são decodificados e a sessão pode materializar tabelas físicas de chunks em static buffers de destino, mas somente para ranges respaldados por mapeamentos lineares FCRAM explícitos; nenhum endereço físico é inferido de `Vec` privado ou shared backing. O decoder GSP usa a visão comum em vez de indexar o buffer TLS diretamente. `translate_memory_block_translation_section_from_ipc` aceita exatamente um grupo type-0 e entrega seu array homogêneo à transferência host-side; rejeita descritores adicionais ou de outra família. Isso ainda não constitui o transporte IPC geral: mapeamento temporário types 5/6/7, tipos 2/3, outros objetos de kernel, arrays heterogêneos, sessões/portas, filas de requisições e associação de sessões por processo permanecem pendentes. `run_instructions` retorna `WaitForInterrupt` ao executar WFI ou enquanto o core permanece suspenso, sem contar chamadas ociosas como instruções. A sessão permite ao host acionar as linhas IRQ/FIQ; o controlador de interrupções de dispositivos, eventos GSP ligados às linhas, IPC completo, temporização precisa e ciclo de vida completo de processos/handles continuam pendentes.

## GPU PICA200 e LCD

A GPU possui banco inicial de registradores, parser de command lists, canais de command buffer, fila GX e modelo da GSP Interrupt Relay Queue. A fila de interrupções mantém o layout de 0x40 bytes por cliente, ring de eventos, contadores de PDC perdidos e flag ignore-PDC. O serviço GSP já pode associar a fila a shared memory mapeada e a um event handle, e a entrega de interrupções sinaliza esse evento. O kernel oferece criação de blocos anônimos zerados para SYSTEM/BASE, criação de backing compartilhável sobre heap regular existente e os SVCs locais de map/unmap; o serializador da resposta estendida de `RegisterInterruptRelayQueue` já codifica resultado, thread ID e descritor/handle COPY em cinco words, e existem primitivas host-side para importar/mover memory blocks entre tabelas de sessão. A leitura tipada de descritores IPC para type 0, type 1 e types 5/6/7, além da transferência host-side de um ou vários memory blocks, já está modelada, mas não é ainda o transporte de handles do kernel nem executa cópia ou mapeamento de buffers. Ainda faltam a tradução de outros tipos de objeto e arrays heterogêneos, a criação do backing pelo serviço, o dispatcher IPC real, sessões/portas e a integração com o gerenciamento de processos; `System` ainda não gerencia múltiplos processos ARM11 independentes. Há fetch inicial de atributos e índices, transformação geométrica genérica, clipping e rasterizador de referência em CPU.

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
