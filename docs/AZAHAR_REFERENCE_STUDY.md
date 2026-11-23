# Estudo de referência técnica — Azahar e interrupções GSP

**Data da consulta:** 23/11/2026  
**Escopo:** estudo comportamental de GSP/GPU e comparação com o modelo GIC do Aceris.  
**Política:** nenhuma implementação, teste, fixture, shader, patch, asset ou estrutura de código do Azahar foi incorporada. A alteração do Aceris foi desenhada independentemente em Rust.

## Fontes consultadas

### Azahar

- Repositório: https://github.com/azahar-emu/azahar
- Revisão observada durante a consulta: `4a147c509571a3d16a06abe679fe1339dbbdbfd7` (branch `master`).
- Licença do repositório: GNU General Public License v3.0 (GPL-3.0).
- Área de comportamento GSP consultada: `src/core/hle/service/gsp/gsp_gpu.cpp` e `src/core/hle/service/gsp/gsp_gpu.h`.
- PR #2095: https://github.com/azahar-emu/azahar/pull/2095 — fila de interrupções GPU, eventos de conclusão e temporização opcional.
- PR #2273: https://github.com/azahar-emu/azahar/pull/2273 — ciclo de vida GSP, framebuffer/VRAM compartilhados e ordenação de eventos.

### Referências independentes de hardware

- ARM, ARM11 MPCore TRM, DDI0360 r2p0: https://support.arm.com/documentation/ddi0360/f/mpcore-distributed-interrupt-controller/interrupt-distributor
- 3dbrew, ARM11 Interrupts: https://3dbrew.org/wiki/ARM11_Interrupts

## Perguntas técnicas

1. Como distinguir interrupções de VBlank das interrupções de conclusão de comandos GPU?
2. Como o modelo GSP trata eventos que chegam enquanto a fila relay está cheia?
3. Qual semântica é necessária para uma fonte GIC configurada como level-sensitive?
4. O que não deve ser aproximado sem medição independente de hardware?

## Observações

- O comportamento estudado no Azahar separa PDC0/PDC1, associados ao VBlank já agendado, de outras conclusões GSP. Dependendo da opção de temporização, conclusões não-VBlank podem receber atrasos modelados antes de o evento ser entregue.
- O código consultado contém valores de referência de 300.000 ns para o atraso síncrono e 1.000.000 ns no caso especial de conclusão PPF associado a cópia de textura atrasada. Esses valores são próprios do modelo de compatibilidade daquele projeto, não uma especificação do hardware.
- A discussão do PR #2095 explica que o backend GPU síncrono pode fazer uma operação parecer instantânea ao guest; temporização opcional foi introduzida para aproximar resultados observados em hardware. A discussão do PR #2273 destaca ordenação de eventos e comportamento de framebuffer/VRAM como aspectos importantes para compatibilidade.
- A fila relay do Aceris já representa o ring de 0x34 slots, o threshold PDC de 0x20, contadores de PDC perdidos, ignore-PDC e erro de fila cheia. O estudo não justificou duplicar esse código nem substituir suas regras.
- A tabela pública de interrupções ARM11 lista PSC0/PSC1/PDC0/PDC1/PPF/P3D nos IDs 0x28–0x2D e os descreve como level-sensitive. A configuração efetiva deve continuar a ser confrontada com o caminho de assert/clear de cada dispositivo.
- O TRM distingue fontes edge-triggered e level-sensitive: na modalidade level, a linha da fonte precisa ser desassertada pelo dispositivo; limpar apenas o estado pending do controlador não substitui a limpeza da causa no periférico.

## Mudança original aplicada no Aceris

Commit: `40238cef5728a579e2205be3b8f38356ec66b435`  
Arquivo: `crates/aceris-core/src/cpu/mpcore_gic.rs`

O modelo anteriormente guardava somente o nível da entrada do timer privado. A alteração generalizou isso para um vetor de níveis de entrada com 128 posições e introduziu `set_interrupt_input_level(interrupt_id, asserted)`.

O contrato agora é:
- IDs fora de 0..127 são rejeitados.
- Uma entrada edge-triggered registra uma transição low-to-high; a queda posterior não apaga o pending já latched.
- Uma entrada level-triggered permanece pending enquanto a linha está assertada e não está active.
- Ao reconhecer a interrupção, o controlador move pending para active.
- Após EOI, uma fonte level-triggered ainda assertada volta a pending; se o dispositivo já limpou a linha, ela não é reapresentada.
- O timer PPI 29 continua usando a mesma API especializada, agora delegada ao mecanismo genérico.

Foram adicionados quatro testes sintéticos para: reassert após EOI enquanto PSC0 continua alto; limpeza de P3D antes de acknowledge; pulso edge-triggered de PSC1; e rejeição do ID 128.

## Limites e próximas tarefas

- Esta mudança fornece o contrato genérico do GIC; **não conecta ainda** as fontes físicas GSP ao distribuidor. Antes disso, cada caminho de conclusão GPU precisa definir quando a linha correspondente é assertada e quando é limpa, sem usar o evento relay como substituto automático do estado elétrico do dispositivo.
- Não foram copiados os atrasos de 300 us ou 1 ms do Azahar. O Aceris ainda precisa de um modelo de eventos/relógio virtual e evidência independente que justifique latências por operação.
- Próxima integração: ligar, em separado, cada conclusão PICA200/GSP à respectiva entrada GIC; limpar a fonte no ponto correto do ciclo de vida; e testar pending/active, EOI, relay queue, event handle e wake-up guest em conjunto.
- Multi-core/afinidade, FIQ e a temporização ciclo-a-ciclo continuam fora do escopo desta alteração.

## Registro de autoria e independência

A consulta ao Azahar serviu para formular perguntas de compatibilidade e identificar comportamentos observáveis. Nenhum trecho de código foi reutilizado ou traduzido. O código Aceris tem API, estado, implementação e testes próprios em Rust. A evidência observada em outro emulador não é tratada como prova de comportamento físico; as referências independentes de ARM e 3dbrew e os testes futuros continuam necessários.
