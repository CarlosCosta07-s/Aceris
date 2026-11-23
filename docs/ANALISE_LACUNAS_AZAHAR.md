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

O `Arm11InterruptController` de sessão continua representando a API guest de `BindInterrupt` para eventos/semafóros. Agora há também um `MpcoreInterruptController` separado, que modela o distribuidor e a interface da CPU para um único CPU de aplicação, e um timer privado no barramento MPCore. A integração coberta hoje é somente o timer PPI 29; isso **não** é suporte SMP nem um GIC completo para todas as fontes de hardware.

Implementação necessária / estado:
1. **Parcial — IDs e registradores:** o modelo cobre IDs 0x00–0x7F, SGI/PPI privados e SPI compartilhados, prioridade, target, trigger, enable, pending, active set/clear e registradores da CPU interface. Só CPU0 é executado; entrega multi-core/afinidade e SGI não estão conectadas ao scheduler.
2. **Parcial — timer privado:** Load, Counter, Control, Interrupt Status, prescaler, one-shot/auto-reload e W1C são modelados em `0x17E00600`, com alias CPU0 em `0x17E00700`. O status aciona PPI 29 e passa pelo distribuidor antes de chegar ao IRQ input da CPU. O modelo usa 134.055.928 Hz como frequência inicial Old 3DS, derivada da metade do clock nominal ARM11 268.111.856 Hz; a frequência efetiva do timer ainda requer confirmação em hardware/medição independente.
3. **Pendente — watchdog:** PPI 30 não foi implementada, nem o caminho de reset do console.
4. **Parcial — fontes compartilhadas:** GSP já envia PSC0/PSC1/PDC0/PDC1/PPF/P3D aos relay queues e eventos HLE, e `BindInterrupt` pode acordar objetos associados. Essas fontes ainda não alimentam o GIC porque estão configuradas como level-sensitive e o estado do dispositivo não é limpo no caminho de acknowledge/EOI.
5. **Pendente — FIQ/SGI e ciclo de CPU:** FIQ continua sendo linha externa separada; falta contexto pendente físico para múltiplos CPUs, preempção/quantum e clock sincronizado a ciclos de instrução.

O timer/distribuidor usa exclusivamente Rust original e documentação pública ARM/3DS. Nenhum código de outro emulador foi usado.

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

O parser de service access control agora lê os 32 slots regulares e os dois slots estendidos do ACI. O espaço de endereçamento NCCH preserva essa ACL, o `srv:GetServiceHandle` verifica o nome exato antes de criar um endpoint HLE e o `srv:RegisterClient` confere o PID virtual traduzido pelo kernel (o bootstrap usa PID 6, fora da faixa privilegiada 1–5). Foi adicionado um teste sintético para permitir `gsp::Gpu` e negar `ssl:C`. **Essa implementação ainda não foi compilada nem executada nesta sessão**: o ambiente não tem `cargo`/`rustc` e o status de checks do commit está vazio. A validação do AccessDesc contra o ACI, o registro/wait de serviços e o ciclo de vida de servidores continuam pendentes.

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
