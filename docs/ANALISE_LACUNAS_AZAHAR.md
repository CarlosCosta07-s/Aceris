# Análise de lacunas do Aceris usando Azahar como referência de comportamento

**Data:** 23/11/2026  
**Branch:** `main`  
**HEAD consultado:** `fcdf202016b08a77d5ddcdcaeb854c777523f631`  
**Objetivo do projeto:** executar jogos Nintendo 3DS fornecidos legalmente pelo usuário em builds Rust para desktop e Android, sem incorporar código, assets ou dependências de implementação de outros emuladores.

## 1. Método e limites da comparação

Azahar é usado como referência de resultado observável: um emulador 3DS disponível para PC e Android, com lista pública de compatibilidade e documentação de instalação. Esta análise **não** tenta deduzir detalhes internos não publicados do Azahar e não usa seu código-fonte, testes, shaders, fixtures, assets ou dependências.

A implementação do Aceris deve ser derivada de especificações ARM/Nintendo 3DS publicamente documentadas, testes próprios e traces de programas/jogos que o usuário tenha direito de executar. O comportamento visível no Azahar pode orientar os critérios de aceitação, mas não é uma especificação de hardware.

Referências:
- Azahar — repositório e README oficiais: https://github.com/azahar-emu/azahar
- Azahar — site oficial: https://www.azahar-emu.org/
- 3dbrew — ARM11 Interrupts: https://3dbrew.org/wiki/ARM11_Interrupts
- 3dbrew — Services API: https://3dbrew.org/wiki/Services_API
- 3dbrew — IPC: https://3dbrew.org/wiki/IPC
- 3dbrew — Kernel: https://3dbrew.org/wiki/Kernel
- ARM — ARM11 MPCore TRM DDI0360: https://support.arm.com/documentation/ddi0360

## 2. Estado real do Aceris em 23/11/2026

O Aceris não está começando do zero. Já existem:
- interpretador ARM/Thumb ARM11, estado CP15, VFP e exceções;
- MMU ARMv6 short-descriptor, FCSE, TLB unificado, ASID/global e operações TLBI;
- D-cache/write buffer modelados e memória compartilhada/aliases;
- threads e objetos de sincronização com escalonamento cooperativo;
- SVCs iniciais, sessões de serviço, parser IPC e tradução parcial de handles/buffers;
- integração guest → srv → GSP, fila GX, GPU/PICA200, LCD e serviços HLE;
- serviços parciais `hid:USER`, `cfg:u`, `apt:U`, `fs:USER`, `ptm:u` e `dsp::DSP`.

**Limite de compatibilidade comprovado:** o registro de transferência de 22/11 informa que o Cookie Clicker via Lua Player Plus executou mais de 300 quadros sem o erro anterior, enviando comandos gráficos e SetBufferSwap, mas as telas capturadas permaneciam pretas. A taxa medida era de aproximadamente 2–3 quadros por segundo, ainda muito abaixo do tempo real. Isso é progresso de execução de homebrew, não prova de jogo comercial jogável.

## 3. Lacunas prioritárias

### P0 — Desempenho e primeiro quadro visível

A baixa velocidade é um bloqueio imediato. A otimização recente do caminho de D-cache reduziu uma medição de 69 s para 19,6 s em 60 quadros, aproximadamente 3,5 vezes mais rápido, mas não atingiu tempo real.

Próximas tarefas:
1. manter um benchmark reproduzível com o mesmo programa, orçamento, PC final e saída de tela;
2. perfilar separadamente fetch/decodificação ARM, tradução MMU, busca de regiões de memória, dispatch IPC e rasterização;
3. otimizar apenas o caminho medido como dominante, preservando a semântica de cache/MMU;
4. transformar a primeira imagem não preta em um teste de regressão reproduzível.

### P1 — IPC real entre processos e ciclo de vida do kernel

Este é o maior bloqueio arquitetural para serviços do sistema.

O Aceris já traduz descritores de handles e substitui descritores de ProcessId pelo PID virtual do remetente. Porém, o caminho atual usa a mesma tabela de objetos do processo da aplicação. O próprio `ipc_handles.rs` registra que namespaces independentes de handles ainda não existem. O `System` mantém uma única `ncsd_session`.

Implementação necessária:
1. criar um gerenciador de processos com PID único, estado de execução/saída, recursos e memória próprios;
2. separar tabelas de handles, TLS, threads e espaços de endereçamento por processo;
3. modelar portas e sessões com identidade de cliente/servidor e direitos de acesso;
4. implementar o ciclo request/reply/receive, incluindo bloqueio e despertar das threads servidoras;
5. transportar handles por COPY/MOVE entre tabelas diferentes, com validação e rollback atômico;
6. concluir buffers estáticos, buffers mapeados e tabelas de chunks físicos no transporte real, não apenas em helpers host-side;
7. cobrir encerramento de processo, fechamento de portas/sessões e liberação de objetos pendentes.

Critério de conclusão: dois processos sintéticos independentes comunicam-se por uma porta; cada um mantém PID, TLS, memória e handles próprios; COPY e MOVE mantêm identidade compartilhada e ciclo de vida corretos.

### P1 — Controlador MPCore, IRQ/FIQ e scheduler

O `Arm11InterruptController` atual representa registros guest de `BindInterrupt` para eventos/semafóros. Ele **não** é o controlador de interrupções de hardware MPCore.

O mapeamento atual da região privada MPCore expõe o subconjunto SCU nos primeiros 0x100 bytes. O temporizador privado, watchdog, interfaces de CPU e distribuidor de interrupções ainda não compõem um caminho de hardware integrado.

Implementação necessária:
1. modelar os IDs privados por CPU (0x00–0x1F) e fontes compartilhadas (0x20–0x7F), respeitando prioridade e afinidade documentadas;
2. implementar registradores e estados do controlador distribuído e das interfaces de CPU de acordo com o ARM11 MPCore, sem transplantar um modelo de outra geração ARM;
3. modelar temporizador privado e watchdog, com contagem baseada no relógio virtual e os IRQs correspondentes;
4. ligar fontes reais do Aceris (GSP/PDC, DSP, HID e DMA quando a conclusão estiver modelada) às linhas corretas;
5. manter IRQ/FIQ pendentes no contexto do processador físico, não somente no objeto de thread que estiver ativo;
6. integrar preempção/quantum e wake-up do scheduler com eventos e temporizadores, mantendo a execução determinística nos testes.

Referências independentes: 3dbrew descreve IDs privados 0x00–0x1F, IRQs compartilhadas 0x20–0x7F e o uso dos IDs 0x1D/0x1E para timer/watchdog. O TRM ARM11 MPCore descreve o distribuidor, prioridades, alvos e interfaces por CPU.

Critério de conclusão: testes guest-side configuram registradores, habilitam uma fonte, recebem IRQ no core correto, acordam uma thread em espera e confirmam prioridade, acknowledge/EOI e limpeza do pending sem gerar interrupções duplicadas.

### P1 — Inicialização e permissões dos serviços do sistema

Os serviços existentes são HLE parciais; não representam a inicialização completa do ambiente 3DS. `am:u` ainda tem sessão sem comandos de gerenciamento de títulos, e diversos comandos/serviços não são modelados. `cfg:i` e `cfg:s` foram conectados ao conjunto mínimo de `cfg:u`, mas não possuem toda a superfície de comandos das variantes reais.

Implementação necessária:
1. inicializar o registro de serviços e as sessões com regras explícitas de ciclo de vida;
2. integrar a lista de permissões de serviços do Access Control Info do exheader ao `srv:GetServiceHandle`;
3. tratar nome inexistente, serviço não permitido, serviço não inicializado e handle inválido com resultados distintos e testados;
4. completar comandos por demanda a partir de traces reais, começando pelos que impedem o primeiro quadro e o áudio;
5. priorizar `fs:USER`, `apt:U`, `cfg:*`, `am:*`, `dsp::DSP`, `hid:USER`, `gsp::Lcd`, `ndm:u`, `frd:u` e demais serviços conforme evidência do guest;
6. substituir respostas neutras/fixas por estado coerente quando o título depender desses valores.

O parser de service access control foi exposto no último commit consultado, mas sua integração ao registro/runtime de `srv:` e sua validação no CI ainda precisam ser confirmadas.

Critério de conclusão: um título de teste recebe somente serviços permitidos por seu exheader, inicializa cada sessão com estado próprio e consegue fechar/reabrir serviços sem handles obsoletos.

### P2 — Fechamento da MMU e do TLB

A base já cobre uma parte ampla do ARMv6: seções/superseções, coarse tables, páginas de 4 KiB/64 KiB, XP=0/XP=1, FCSE, domínios, AP/APX/XN, ASID/global, lockdown e operações TLBI. Os testes recentes já cobrem invalidação por ASID, preservação de entradas globais/lockdown, FCSE e páginas pequenas/grandes.

Não reiniciar esse subsistema. Continuar por uma matriz de conformidade:
1. testar combinações de TTBR0/TTBR1 e TTBCR.N em limites de endereço;
2. cobrir faults de alinhamento, domínio, permissão, XN e atualização de DFSR/IFSR/DFAR/IFAR;
3. auditar interações de lockdown, substituição, ASID/global e invalidações em todas as operações CP15 c8 implementadas;
4. conferir atributos TEX/C/B/S, PRRR/NMRR e TRE em acessos CPU e DMA;
5. documentar diferenças intencionais do modelo de cache, outer cache, snooping e temporização;
6. usar cenários independentes e resultados de hardware/documentação para evitar testes que apenas reproduzam a própria implementação.

Critério de conclusão: matriz de testes independente por classe de descritor, permissão, ASID, invalidação e fault, sem regressões entre desktop e Android.

## 4. Ordem de execução recomendada

1. **Destravar o primeiro quadro visível e acelerar o caminho quente** — sem isso, cada novo serviço torna a validação mais lenta.
2. **Separar processos e finalizar o transporte IPC** — requisito estrutural para serviços reais e para handles/memória compartilhada entre cliente e servidor.
3. **Implementar MPCore IRQ/timer e integrar o scheduler** — requisito para bloqueio, notificações e sincronização assíncrona confiáveis.
4. **Conectar ACL do exheader ao SRV e completar serviços guiados por traces** — não criar stubs sem evidência.
5. **Fechar a matriz MMU/TLB e expandir a validação** — preservar a base já implementada.
6. **Testar títulos comerciais fornecidos pelo usuário, um por vez**, mantendo ROMs, firmware, chaves, saves e dumps fora do repositório, CI e releases.

## 5. Regras de implementação

- Tudo em Rust, inclusive frontends e integração de dispositivos.
- Código, testes e arquitetura próprios; não copiar, adaptar ou traduzir implementação de emulador de terceiros.
- Nenhum asset, firmware, chave, ROM, save ou dado extraído de jogo no repositório.
- Trabalhar somente na branch `main`; não criar branches ou PRs.
- Não considerar um teste aprovado sem resultado observável de execução local ou CI.
- Não declarar compatibilidade jogável com base somente em carregamento, execução de alguns quadros ou saída de comandos GPU.
