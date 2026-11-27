# Estudo de referências externas — Aceris

> Revisão em 25/11/2026. Este documento transforma referências externas em perguntas e critérios para uma implementação independente. Não autoriza copiar, adaptar, traduzir ou portar código, shaders, testes, assets ou organização expressiva de terceiros.

## Regras que não mudam

- O produto Aceris permanece integralmente em Rust: núcleo, frontends e lógica da aplicação.
- Todo trabalho continua exclusivamente em `main`; não criar branches nem usar pull requests como fluxo de desenvolvimento.
- Nenhuma IA pode modificar trabalho de outra IA sem autorização específica do mantenedor. Esta pesquisa não altera módulos de CPU, MMU, loader, PICA, frontend Android ou presenter.
- Antes de consultar código de terceiros, identificar a licença do projeto e do arquivo específico. A existência de um repositório público não torna todo o conteúdo reutilizável.
- Não incluir jogos, ROMs, CIA/CCI/3DS, firmware, chaves, saves ou dados extraídos no Git, CI, issues, releases ou artefatos.
- As referências produzem hipóteses e especificações independentes; a confirmação exige testes próprios e, quando possível, hardware legítimo.

## Matriz de referências

| Projeto | O que estudar | Limite de uso |
|---|---|---|
| [Azahar](https://github.com/azahar-emu/azahar) | Compatibilidade 3DS, PICA200, GSP, serviços HLE, sincronização, renderizadores e Android | GPL-3.0; estudo técnico e comportamental. Nenhum código, shader, teste ou asset deve ser incorporado |
| [PPSSPP](https://github.com/hrydgard/ppsspp) | JIT/dynarec ARM64, intérprete rápido, caches de blocos, precisão versus velocidade, renderização e perfis móveis | GPL-2.0-or-later; somente referência. Implementação do Aceris deve ser original em Rust |
| [DraStic](https://drasticemulator.com/frequently-asked-questions-drastic-ds-emulator/) | Experiência Android, duas telas, controles táteis, suspensão/retomada, desempenho e configurações | Projeto fechado; não há código-fonte público para consulta. Apenas comportamento observável e documentação oficial |
| [Eden](https://github.com/eden-emulator/mirror) | Vulkan móvel, seleção de GPU/driver, perfis por dispositivo e diagnóstico gráfico | GPL-3.0-or-later; Switch é hardware diferente de 3DS. Usar só como referência de backend e operação Android |
| [PCSX2](https://github.com/PCSX2/pcsx2) | Separação entre CPU e GS, intérpretes/recompiladores, OpenGL/Vulkan, sincronização e testes de renderizador | GPL-3.0; projeto desktop/PS2. Não transportar código ou pressupor semântica PICA200 |
| [Black for Android](https://github.com/RanggaCasper/Black-Android) | Fluxo de importação, verificação local, pacote ARM64, Vulkan, controles na tela, logs e configurações por dispositivo | Licença MIT no código próprio do repositório; conteúdo de jogo e arquivos de terceiros mantêm direitos/licenças próprios. A recompilação de Xbox não é emulação 3DS |
| [Dragon Rage / Tenkaichi3DecompAndroid](https://github.com/dfdx047/Tenkaichi3DecompAndroid) | Empacotamento Android, importação local, suporte a GPU, configuração, controles táteis e distribuição de APK | Port/recompilação de jogo PS2, não emulador. O repositório declara AGPL-3.0 para o port e MIT para partes do app; código do jogo continua pertencendo aos detentores dos direitos |
| [Gen1Recomp](https://github.com/bryanthaboi/gen1recomp) | Validação de arquivo fornecido pelo usuário, importação, cache privado versionado, progresso, saves e recuperação de falhas | É uma recriação em Lua/LÖVE, não um emulador Game Boy nem um recompilador estático. Não foi encontrada licença na raiz consultada; não usar seu código até esclarecer a licença |

## Lições técnicas que se aplicam ao Aceris

### CPU ARM11 e desempenho

1. Separar custo de interpretação, MMU/TLB, scheduler, serviços HLE, memória e rasterização por quadro.
2. Preservar uma implementação de referência precisa antes de criar caminhos rápidos.
3. Investigar dispatch de instruções, redução de alocações, acesso rápido a regiões de código imutáveis e caches de tradução.
4. Considerar um dynarec ARMv6/ARMv6K → AArch64 escrito em Rust apenas depois de ter contratos de exceção, MMU, self-modifying code, invalidação e sincronização bem testados. Um JIT não deve ser iniciado como um atalho sem esses invariantes.
5. Comparar throughput com a mesma sequência de quadros e entradas. Não comparar apenas tempos de boot ou quadros ociosos.

### PICA200 e backend gráfico

1. Manter o rasterizador PICA em CPU como oráculo de compatibilidade.
2. Definir um pacote de draw imutável, independente de Vulkan/OpenGL, contendo estado de vértices, shaders, texturas, TEV, framebuffer, depth/stencil, blend e scissor.
3. Introduzir aceleração por subconjuntos de funcionalidades, com fallback explícito para CPU quando uma operação não estiver suportada.
4. Preservar semântica de memória guest, DMA, render targets, readback, ordenação e sincronização. Uma swapchain que mostra um frame já composto não prova aceleração PICA.
5. Comparar imagens determinísticas CPU versus backend host com entradas próprias e registrar diferenças por pixel.

### Android e entrega testável

1. Produzir APK ARM64 repetível a partir da branch `main`, sem incluir conteúdo de jogo.
2. Validar assinatura, ABI, manifesto, dependências nativas e instalação antes de testar gameplay.
3. No aparelho, registrar SoC, GPU/driver, Android, memória disponível, backend selecionado, tempo por quadro, temperatura e logs.
4. Testar primeiro inicialização, duas telas, toque, pausa/retomada e retorno ao launcher; depois carregar homebrew local sem criptografia externa.
5. Não declarar compatibilidade jogável a partir de compilação, tela estática ou teste headless.

## Estado observado no Aceris em 25/11/2026

- A branch `main` estava no commit `c1b39086ccd86d6753af33b1335672ca1bb701e9` quando este documento foi preparado.
- O histórico de trabalho registra Cookie Clicker executando gameplay headless com injeção de toques e HUD em 8 cookies.
- Um perfil anterior registrou cerca de 3,5 quadros/s no desktop; isso é evidência de gargalo de desempenho, não uma medição Android.
- O presenter `aceris-graphics` apresenta frames LCD já compostos; ainda não rasteriza diretamente os draws PICA200.
- Android ARM64 compila na CI, mas o projeto ainda não tinha APK instalado ou validado em aparelho físico.
- A CI global recente falhou no Clippy do `aceris-core` com ocorrências de `manual_is_multiple_of`. A correção desses módulos deve ser coordenada com o agente responsável; esta etapa não os modifica.

## Próximos experimentos próprios

1. Obter um APK de teste assinado automaticamente para instalar a aplicação sem depender de consertar primeiro o Clippy global.
2. Fazer um smoke test no aparelho sem jogo e confirmar ciclo de vida, superfície, toque e logs.
3. Carregar um homebrew 3DS legal do usuário pela pasta local e guardar diagnóstico de serviços HLE.
4. Repetir no Android o benchmark por quadro com a mesma sequência de toque usada no desktop.
5. Só então priorizar o maior gargalo observado: CPU/interpreter, rasterização PICA, GPU host, áudio ou acesso a arquivos.

## Referências públicas consultadas

- Azahar README e changelog: https://github.com/azahar-emu/azahar e https://github.com/azahar-emu/azahar/releases
- PPSSPP introdução de desenvolvimento e licença: https://www.ppsspp.org/docs/development/getting-started/
- DraStic FAQ (código fechado): https://drasticemulator.com/frequently-asked-questions-drastic-ds-emulator/
- Eden repository e handbook: https://github.com/eden-emulator/mirror e https://github.com/eden-emulator/mirror/tree/master/docs/user
- PCSX2 repository e GS renderer: https://github.com/PCSX2/pcsx2 e https://github.com/PCSX2/pcsx2/blob/master/pcsx2/GS/GS.cpp
- Black for Android: https://github.com/RanggaCasper/Black-Android
- Dragon Rage: https://github.com/dfdx047/Tenkaichi3DecompAndroid
- Gen1Recomp: https://github.com/bryanthaboi/gen1recomp


## Revisão comparativa aplicada ao Aceris — 27/11/2026

Esta revisão foi feita sobre as revisões públicas abaixo. O objetivo é extrair contratos técnicos e métodos de validação; não incorporar código, testes, fixtures, shaders, nomes internos ou organização expressiva dos projetos consultados.

| Projeto | Revisão pública examinada | Licença identificada | Uso permitido neste estudo |
|---|---|---|---|
| Azahar | `c5177f309d4f4efac669623dddeb08e2e5262da1` (27/11/2026) | GPL-3.0 | Comportamento do 3DS, scheduler, DSP, GSP, PICA200 e changelog |
| PPSSPP | `1cd06ff11dd7010123e0b5ada244f76597c42ea5` (27/11/2026) | GPL-2.0-or-later | Separação interpretador/IR/JIT, validação automatizada e profiling |
| PCSX2 | `51e3036bdb7cad7ae7e25ad8caa438b06f538c4e` (26/11/2026) | GPL-3.0 | Recompilação, estado virtual da máquina, diagnóstico por subsistema e regressões |

O Aceris permanece MIT e integralmente em Rust. As licenças GPL dessas referências não são uma autorização para copiar nem incorporar código, testes, assets ou estrutura. Nenhum arquivo desses projetos foi adicionado ao workspace, Cargo.lock, CI ou produto.

### 1. Azahar — contratos específicos do 3DS

**Core 1 e ResourceLimit.** A revisão 2127.0 corrigiu a validação do limite de tempo do core 1. O valor 10000 é tratado no Azahar como exceção à preempção; não deve ser copiado como constante universal nem confundido com um orçamento de CPU. O Aceris ainda modela uma sessão ARM11 de um único core e não possui o scheduler do core 1. Portanto, a correção não pode ser transplantada como um simples retorno fixo de `CPU_TIME` em `GetResourceLimitCurrentValues`.

Experimento próprio necessário antes de implementar essa área:

1. Interpretar os descritores de limite de recursos da ACI do extended header, em particular o máximo de CPU time, sem presumir categoria ou valor.
2. Especificar separadamente limite máximo, uso atual, política de preempção e o caso em que o core 1/scheduler ainda não existe.
3. Cobrir SVC, valores inválidos, limites e mudanças de processo com testes de ABI independentes.
4. Somente depois adicionar um scheduler de core adicional e comparar o comportamento com traces de homebrew.

**DSP e tempo de áudio.** A revisão 2127.0 também ajustou a conclusão de sleep do DSP para o limite de dois frames de áudio. Isso é uma pista útil para modelar conclusão e despertar por relógio de áudio virtual, em vez de temporizadores host arbitrários. Ainda não prova que essa seja a causa da falha `Sound.init` no Cookie Clicker: o trace Aceris anterior não mostrou uma requisição DSP, então é necessário identificar primeiro até onde a inicialização guest chega (Service Manager, obtenção do handle, primeiro IPC e primeira submissão DSP).

**GPU/GSP.** A referência reforça a necessidade de validar a cadeia completa guest → serviço GSP → fila GX/registradores PICA200 → framebuffer → LCD. Um triângulo preparado pelo host ou um framebuffer apresentado por `wgpu` não demonstra que os comandos DRAWARRAYS/DRAWELEMENTS e os shaders do guest foram executados corretamente. O renderer de software Aceris deve continuar como oráculo e fallback.

### 2. PPSSPP — arquitetura de execução e desempenho

A documentação do PPSSPP descreve a progressão conceitual interpretador → interpretador IR → JIT baseado em IR, com blocos de instruções e passes de otimização. O guest do PPSSPP é MIPS/PSP, não ARM11/3DS; instruções, ABI, temporização e modelo de memória não são transferíveis.

Aplicação ao Aceris:

- Preservar o interpretador A32/Thumb preciso como referência e fallback.
- Medir o cache de decodificação e o cache de blocos separadamente.
- Ampliar o IR somente quando operações, flags CPSR, condições, branches, faults, acessos à memória e efeitos colaterais tiverem equivalência diferencial.
- Criar perfis release repetíveis para desktop e Android; não inferir ganho apenas pelo número de instruções que foram pré-decodificadas.
- Iniciar JIT só depois de o IR e a invalidação de blocos terem contrato estável. Cada backend terá W^X, sincronização de cache de instruções, limites de memória, descarte de código e fallback explícitos.

A suíte de testes deve comparar estado de registradores, CPSR, PC, memória e eventos entre o caminho preciso e o caminho IR; falhas precisam identificar a instrução guest e a primeira diferença observável.

### 3. PCSX2 — disciplina de validação

O PS2 tem arquitetura distinta, com CPU MIPS, unidades vetoriais e Graphics Synthesizer próprios. PCSX2 não é uma especificação do 3DS nem uma fonte para implementar PICA200, ARM11 ou serviços Nintendo.

O que é útil como método:

- Isolar o subsistema responsável antes de alterar código (CPU, scheduler, memória, DSP, GSP, PICA200, apresentação).
- Alterar uma variável de emulação por experimento e comparar a mesma sequência de entrada.
- Manter caminho interpretado/de referência para detectar regressões de recompilação.
- Registrar jogo, região/versão, configuração, backend, driver, resultado esperado, saída real e logs.
- Tratar cada correção de compatibilidade como regressão reproduzível, sem fazer refactors amplos junto com a correção de hardware.

### 4. Estado Aceris e bloqueios após a correção da CI

Na revisão de 27/11, o projeto já possui cache de decodificação A32, cache de blocos restrito a texto executável imutável e uma primeira IR tipada para ALU, transferências, halfword, blocos, multiplicações e branches. O JIT ainda não existe. A GPU host apresenta LCD já composto; a rasterização PICA200 do guest permanece no core.

O caminho Cookie Clicker já demonstrou imagem guest-driven e oito toques injetados resultando em oito cookies em ensaio headless, mas não representa velocidade em tempo real nem compatibilidade comercial. `Sound.init` segue sem causa raiz confirmada, áudio audível não foi validado e Android físico ainda não foi testado. O loader também permanece parcial para títulos criptografados e para recursos de sistema que dependem de dados locais legítimos.

Correções publicadas durante esta revisão:

- `23637be` / `45a495e`: compartilhamento dos helpers da MMU entre módulos de teste e ordenação de imports; o código funcional da MMU não foi alterado.
- `fd2282b`: converte teclas Android do espaço de bits do frontend para os bits físicos antes da navegação dos menus; o estado enviado ao guest continua usando a conversão própria já existente.
- `b3a7e04` / `61f3878`: ajusta as expectativas do teste IR e corrige o bit de codificação ausente em fixtures DSP ARMv6.
- `b096b2a`: garante que o teste de snapshot tenha um registro de página real antes de corromper o índice.
- `5103d5b` / `de258c4`: mantém os centros dos controles virtuais estáveis ao redimensionar e reabre a página Configurações nos testes de persistência.

A matriz CI precisa ser confirmada no HEAD resultante antes de marcar o ciclo como concluído. Nenhum resultado de compilação ou teste automatizado equivale a compatibilidade de jogo comercial.

### 5. Próximos testes de compatibilidade

1. Recuperar CI verde em Linux, Windows, macOS e Android ARM64, incluindo testes completos, Clippy e benchmark release.
2. Reexecutar o homebrew local e recolher trace de `Sound.init`, `srv:GetServiceHandle`, primeiro IPC DSP e temporização.
3. Cobrir ACI/ResourceLimit e CPU time apenas com valores fundamentados no extended header e no modelo de scheduler efetivamente implementado.
4. Validar DSP por frames de áudio virtuais e testes determinísticos de sleep/wakeup.
5. Fechar cenas PICA200 guest-driven próprias e comparar buffers, shaders e scanout; manter os testes sem dumps protegidos.
6. Instalar o APK num aparelho ARM64 e validar ciclo de vida, toque, áudio e frametime.
7. Só então iniciar uma matriz de jogos comerciais com dumps legalmente obtidos e fornecidos localmente pelo usuário, mantendo imagens, firmware e chaves fora do repositório e da CI.

**Critério de declaração:** “abre”, “chega ao primeiro frame”, “renderiza uma cena”, “jogável” e “compatível” são níveis distintos. Cada título precisa ter versão/região identificada, sequência de teste, resultado de áudio/entrada/gráficos, estabilidade e desempenho registrados antes de receber um status de compatibilidade.
