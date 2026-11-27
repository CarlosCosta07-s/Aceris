# Benchmark de desempenho com títulos fornecidos pelo usuário

Este guia descreve como medir o throughput do núcleo Aceris com uma imagem de jogo/homebrew que o usuário possui legalmente. O arquivo e quaisquer chaves permanecem no computador do usuário e não devem ser enviados ao repositório, CI, issues ou releases.

## Estado e objetivo

O exemplo `crates/aceris-core/examples/benchmark.rs` executa o mesmo `System::load_executable_file`/caminho de sessão ARM11 do núcleo, aquece a sessão e mede uma quantidade configurável de chamadas `run_frame_with_budget`.

O resultado inclui:
- tempo total de processamento no host e throughput de quadros limitados por orçamento;
- média, p50, p95, p99, mínimo e máximo do tempo por chamada;
- instruções ARM11 executadas e throughput de instruções;
- command lists GSP/PICA submetidas e operações de scanout LCD;
- motivo de parada da CPU, estado final PC/CPSR e quadros sem instruções.

**Limite importante:** isto mede throughput do código Aceris no host, não FPS real nem velocidade de relógio do Nintendo 3DS. O orçamento de instruções é um limite determinístico por chamada, e o tempo virtual do frame é nominal. O caminho de renderização PICA200 ainda tem etapas incompletas; contar command lists ou scanouts não prova que todos os shaders, texturas, efeitos, áudio, controles ou gameplay de um título estejam funcionando.

## Executar no PC

Use um build release e o caminho da sua própria imagem:

```sh
cargo run --release -p aceris-core --example benchmark -- "/caminho/para/seu-jogo.cia" --warmup 30 --frames 300 --dump-lcd ./aceris-pc-lcd
```

Também são aceitos arquivos 3DSX e imagens NCSD `.3ds`/`.cci`, desde que o caminho de carregamento atual consiga preparar o conteúdo. Parâmetros:
- `--warmup N`: chamadas descartadas antes da medição (padrão 15);
- `--frames N`: chamadas medidas (padrão 120);
- `--instruction-budget N`: limite de instruções por chamada (padrão igual a `DEFAULT_FRAME_INSTRUCTION_BUDGET`).
- `--dump-lcd DIR`: salva `top_left.png`, `top_right.png` e `bottom.png` depois da medição, sem incluir a gravação no tempo reportado.

No Windows PowerShell:

```powershell
cargo run --release -p aceris-core --example benchmark -- "D:\Jogos\seu-jogo.cia" --warmup 30 --frames 300 --dump-lcd ./aceris-pc-lcd
```

A janela gráfica não é necessária para esse exemplo. O benchmark é um utilitário de linha de comando desktop.

## Reproduzir uma sessão real com entradas determinísticas

O benchmark também aceita toques programados no touchscreen inferior e pode exportar um trace CSV por quadro:

- `--tap X,Y@FRAME[+HOLD]`: injeta um toque nas coordenadas inferiores 320×240 no quadro indicado. Pode ser repetido. O tempo de HOLD é medido em quadros e o padrão é 3.
- `--frame-trace ARQUIVO.csv`: grava, depois da medição, uma linha por quadro medido com tempo de host, instruções executadas, command lists GSP/PICA, scanouts LCD, entregas de interrupção, motivo de parada, PC e CPSR.
- O número de quadro dos toques começa em zero no primeiro quadro guest, incluindo os quadros de aquecimento. Use `--warmup 0` para manter os números iguais aos da sequência de entrada.

Exemplo de sequência repetível para o homebrew Cookie Clicker que já foi exercitado manualmente no Aceris:

```powershell
cargo run --release -p aceris-core --example benchmark -- "D:\Jogos\Cookie Clicker.cia" --warmup 0 --frames 1000 --instruction-budget 4000000 --tap 160,80@600 --tap 160,120@700 --tap 160,120@720 --tap 160,120@740 --tap 160,120@760 --tap 160,120@780 --tap 160,120@800 --tap 160,120@820 --tap 160,120@840 --frame-trace .\cookie-clicker-trace.csv --dump-lcd .\cookie-clicker-profile
```

Essa sequência toca no menu no quadro 600 e depois toca oito vezes na área do cookie. O arquivo CIA continua sendo fornecido pelo usuário e deve permanecer fora do Git, da CI e dos artefatos. Use sempre o mesmo arquivo, versão release, orçamento, sequência de toques e condições da máquina ao comparar duas builds.

O tempo registrado para cada linha cobre a chamada completa a `System::run_frame_with_budget`. A aplicação dos eventos de toque pelo benchmark ocorre antes do cronômetro; o processamento guest de HID permanece dentro da chamada medida. O trace correlaciona tempo e atividade por quadro, mas não divide internamente o tempo entre ARM11, scheduler, MMU, DSP, PICA200 e LCD. Para atribuir custo a funções específicas, faça uma segunda execução com um profiler de amostragem do host usando exatamente a mesma sequência de entrada. Não adicione temporizadores por instrução ao caminho quente.


## Exemplo de diagnóstico com Super Mario 64 3DS Port (Luigi Version)

Em 25/11/2026, o CIA NoCrypto fornecido pelo mantenedor foi carregado pelo benchmark release. O arquivo permaneceu local; não foi enviado ao repositório nem à CI. Para reproduzir os primeiros cinco quadros e obter o CSV:

```powershell
cargo run --release -p aceris-core --example benchmark -- "D:\Jogos\Super Mario 64 Luigi.cia" --warmup 0 --frames 5 --instruction-budget 4000000 --frame-trace .\sm64-luigi-first5.csv --dump-lcd .\sm64-luigi-first5-lcd
```

Resultado desta execução no host Linux x86-64:

| Quadro | Tempo host | Instruções ARM11 | Stop reason | GSP lists | LCD scanouts |
|---:|---:|---:|---|---:|---:|
| 1 | 202,166 ms | 2.348.557 | SVC 0x38 | 0 | 0 |
| 2 | 0,004 ms | 15 | SVC 0x39 | 0 | 0 |
| 3 | 0,001 ms | 7 | SVC 0x3A | 0 | 0 |
| 4 | 0,005 ms | 49 | SVC 0x3C | 0 | 0 |
| 5 | 0,002 ms | 24 | SVC 0x3C | 0 | 0 |

Esta janela **não mede FPS de jogo**. O primeiro quadro gasta quase todo o tempo no bootstrap; os seguintes param em chamadas SVC não tratadas. Não houve submissão de listas gráficas nem scanout, e as três capturas LCD ficaram pretas.

Para reproduzir a falha no sexto quadro e imprimir o contexto ARM11, use seis quadros de aquecimento:

```powershell
$env:ACERIS_TRACE_SVC = "1"
cargo run --release -p aceris-core --example benchmark -- "D:\Jogos\Super Mario 64 Luigi.cia" --warmup 6 --frames 1 --instruction-budget 4000000
Remove-Item Env:ACERIS_TRACE_SVC
```

A falha observada foi `invalid memory access at 0x08007FF8`, no PC `0x001D8C1C`, com SP `0x08008000`. A instrução nesse PC é `push {r4, lr}`, que tenta gravar em SP−8. O relatório registrou 0 KiB de heap de aplicação mapeado. Antes disso, o guest chama SVCs `0x38`–`0x3A` (consultas ResourceLimit sem handler no despacho atual), tenta `ControlMemory` em `0x08000000` com tamanho zero e recebe `0xD8E007F1`. A tabela pública documenta os SVCs ResourceLimit e Break; o mapa público descreve `0x08000000` como heap mapeado por ControlMemory: [SVC](https://www.3dbrew.org/wiki/SVC), [Memory layout](https://www.3dbrew.org/wiki/Memory_layout), [Memory Management](https://www.3dbrew.org/wiki/Memory_Management).
A leitura estática do bootstrap em `0x001D7608` fechou a ligação: os dois buffers de saída de 64 bits são inicializados com zero; o guest chama `0x39` e `0x3A` para o mesmo tipo de recurso, mas como não há handlers os buffers não são escritos. O código então subtrai uso atual de limite e obtém zero. O `0x38` também deixa `r1=0xFFFF8001` inalterado, de modo que as consultas e o `CloseHandle` seguintes recebem o pseudo-handle do processo, não um ResourceLimit handle válido. Isso explica por que o `ControlMemory` observado recebe tamanho zero; os valores corretos de ResourceLimit ainda precisam ser implementados conforme o contrato guest/kernel, não substituídos por constantes arbitrárias.

**Interpretação:** isto identifica primeiro um bloqueio de compatibilidade na inicialização de heap/stack e nos SVCs do kernel guest, não um gargalo de CPU/GPU já mensurável. Não otimize o rasterizador, o decoder ou a memória com base nesses cinco quadros. Primeiro é necessário confirmar o contrato dos SVCs e a origem do tamanho zero, mapear corretamente a região pretendida e alcançar uma primeira apresentação LCD não preta.

## Imagens criptografadas: chaves fornecidas pelo usuário

Sem variáveis de chave, o benchmark tenta os caminhos sem criptografia já suportados pelo carregador. Para uma imagem NCSD criptografada, o usuário pode fornecer as duas chaves exigidas pelo caminho existente:

Linux/macOS:
```sh
export ACERIS_MENU_INFO_KEY="<32 caracteres hexadecimais>"
export ACERIS_CONTENT_KEY="<32 caracteres hexadecimais>"
cargo run --release -p aceris-core --example benchmark -- "/caminho/para/seu-jogo.3ds"
unset ACERIS_MENU_INFO_KEY ACERIS_CONTENT_KEY
```

PowerShell:
```powershell
$env:ACERIS_MENU_INFO_KEY = "<32 caracteres hexadecimais>"
$env:ACERIS_CONTENT_KEY = "<32 caracteres hexadecimais>"
cargo run --release -p aceris-core --example benchmark -- "D:\Jogos\seu-jogo.3ds"
Remove-Item Env:ACERIS_MENU_INFO_KEY, Env:ACERIS_CONTENT_KEY
```

Para CIA criptografado, o carregador precisa também do material de chave comum indicado pelo ticket. Configure:
- `ACERIS_COMMON_KEY_INDEX`: índice de 0 a 5 declarado pelo ticket;
- e **uma** das opções: `ACERIS_NORMAL_COMMON_KEY`, ou o par `ACERIS_COMMON_KEY_X` + `ACERIS_COMMON_KEY_Y`;
- além de `ACERIS_MENU_INFO_KEY` e `ACERIS_CONTENT_KEY`.

Cada chave deve ter exatamente 32 caracteres hexadecimais (16 bytes); o prefixo opcional `0x` é aceito. Os valores são lidos pelo processo, nunca impressos pelo benchmark e nunca gravados pelo exemplo. Variáveis de ambiente não são um cofre de segredos: evite compartilhá-las, incluí-las em logs ou deixá-las configuradas depois do teste. O Aceris não descobre nem fornece chaves.

O suporte criptográfico continua sujeito às limitações documentadas no roadmap: a disponibilidade do caminho de descriptografia não implica validação completa das assinaturas RSA/ECDSA nem compatibilidade com todos os arranjos de conteúdo.

## Como interpretar os resultados

1. Confirme que o programa realmente avançou: instruções por frame não podem ser zero continuamente; examine PC/CPSR e motivo de parada.
2. Confira se há trabalho gráfico: command lists submetidas e LCD scanouts são sinais de atividade, não prova de renderização correta.
3. Repita a medição pelo menos três vezes com as mesmas opções e compare mediana/p95, não apenas o melhor resultado.
4. Mantenha a máquina ligada à tomada, mesmo modo de energia, temperatura estabilizada e nenhum processo pesado em segundo plano.
5. Compare builds release com o mesmo arquivo e orçamento. Não compare um benchmark com janela/limitação de 60 Hz a uma execução sem apresentação.
6. Guarde o relatório de texto sem anexar a imagem de jogo nem qualquer dado de chave.

## Primeiro teste real já preparado no repositório

O guia [REAL_HOMEBREW_TESTING.md](REAL_HOMEBREW_TESTING.md) descreve um teste manual para uma amostra CIA NoCrypto fornecida anteriormente pelo mantenedor. Ele valida a estrutura e executa um frame limitado. O teste exige que o arquivo continue no ambiente local:

Linux/macOS:
```sh
export ACERIS_REAL_CIA="/caminho/local/para/Super Mario 64 Luigi.cia"
cargo test -p aceris-core --test cia_real_homebrew -- --ignored --nocapture
unset ACERIS_REAL_CIA
```

PowerShell:
```powershell
$env:ACERIS_REAL_CIA = "D:\Jogos\Super Mario 64 Luigi.cia"
cargo test -p aceris-core --test cia_real_homebrew -- --ignored --nocapture
Remove-Item Env:ACERIS_REAL_CIA
```

O teste acima e o benchmark são passos complementares: o primeiro verifica a preparação/execução inicial da imagem de referência; o segundo mede uma sequência de chamadas limitadas. Nenhum dos dois substitui uma sessão prolongada com imagem, áudio, controles e gameplay validados.

## PC versus Android

O exemplo desktop e o executável Android usam a mesma implementação do benchmark e o mesmo `aceris-core`. A diferença é somente a plataforma na qual o processo do host é executado.

### PC

Também serão publicados executáveis prontos pela CI:
- Windows: artefato `aceris-pc-benchmark-windows-latest`;
- Linux: artefato `aceris-pc-benchmark-ubuntu-latest`.

Baixe-os em [GitHub Actions](https://github.com/CarlosCosta07-s/Aceris/actions/workflows/ci.yml), na execução aprovada mais recente, e execute no diretório do binário:

Linux:
```sh
chmod +x ./benchmark
./benchmark "/caminho/para/seu-jogo.cia" --warmup 30 --frames 300 --dump-lcd ./aceris-pc-lcd
```

Windows PowerShell:
```powershell
.\\benchmark.exe "D:\\Jogos\\seu-jogo.cia" --warmup 30 --frames 300 --dump-lcd .\\aceris-pc-lcd
```

Também é possível compilar e rodar pelo Rust:

```sh
cargo run --release -p aceris-core --example benchmark -- "/caminho/para/seu-jogo.cia" --warmup 30 --frames 300 --dump-lcd ./aceris-pc-lcd
```

### Android ARM64 físico por ADB

A CI Android compila o executável `aceris-benchmark` para `arm64-v8a` e publica o arquivo como artefato `aceris-android-benchmark-arm64-v8a` no workflow. Baixe o artefato da execução mais recente aprovada em [GitHub Actions](https://github.com/CarlosCosta07-s/Aceris/actions/workflows/ci.yml).

Com a depuração USB habilitada e `adb` disponível no PC, copie o executável e a sua própria imagem de jogo para o aparelho. Exemplo:

```sh
adb devices
adb shell getprop ro.product.cpu.abi
adb push aceris-benchmark /data/local/tmp/aceris-benchmark
adb push "/caminho/para/seu-jogo.cia" /data/local/tmp/seu-jogo.cia
adb shell chmod 700 /data/local/tmp/aceris-benchmark
adb shell /data/local/tmp/aceris-benchmark /data/local/tmp/seu-jogo.cia --warmup 30 --frames 300 --dump-lcd /data/local/tmp/aceris-android-lcd > aceris-android-report.txt 2>&1
```

O valor de `ro.product.cpu.abi` deve incluir `arm64-v8a`. Para salvar o relatório diretamente no telefone e copiá-lo depois:

```sh
adb shell '/data/local/tmp/aceris-benchmark /data/local/tmp/seu-jogo.cia --warmup 30 --frames 300 --dump-lcd /data/local/tmp/aceris-android-lcd > /data/local/tmp/aceris-report.txt 2>&1'
adb pull /data/local/tmp/aceris-report.txt .
adb pull /data/local/tmp/aceris-android-lcd ./aceris-android-lcd
```

Para imagens criptografadas, entre no shell do aparelho e configure localmente as variáveis exigidas pelo caminho de carga. Não coloque valores reais no repositório, em relatórios ou em mensagens compartilhadas.

NCSD criptografado:
```sh
adb shell
export ACERIS_MENU_INFO_KEY="<32 caracteres hexadecimais>"
export ACERIS_CONTENT_KEY="<32 caracteres hexadecimais>"
/data/local/tmp/aceris-benchmark /data/local/tmp/seu-jogo.3ds --warmup 30 --frames 300 --dump-lcd /data/local/tmp/aceris-android-lcd
unset ACERIS_MENU_INFO_KEY ACERIS_CONTENT_KEY
exit
```

CIA criptografado também exige `ACERIS_COMMON_KEY_INDEX` e uma das opções documentadas acima: `ACERIS_NORMAL_COMMON_KEY` ou o par `ACERIS_COMMON_KEY_X` + `ACERIS_COMMON_KEY_Y`. O benchmark não descobre, imprime nem persiste esse material.

O programa executa no processador ARM64 do telefone, sem emulação de CPU do host e sem depender de uma Activity Android. O arquivo do jogo permanece no aparelho. Para repetir com o mesmo título no PC, use exatamente o mesmo arquivo e as mesmas opções. As três capturas PNG são gravadas somente depois da janela medida; o custo de codificação/gravação não entra nos tempos reportados.

Alguns firmwares podem impedir a execução direta de binários em `/data/local/tmp`. Nesse caso, a medição por ADB não está disponível nesse aparelho sem um host Android instalável; não interprete uma falha de permissão do Android como falha do núcleo Aceris.

### O que este teste Android entrega — e o que não entrega

- Entrega medição do tempo de processamento de frames limitados, throughput de instruções ARM11, GSP e operações LCD executadas pelo mesmo núcleo no processador do celular.
- Não apresenta as telas do jogo na interface do Android, não mede custo de composição da superfície, entrada touchscreen ou áudio.
- Não é um APK de emulador instalável. É um executável de diagnóstico para teste de desempenho via ADB.
- A build Android ARM64 aprovada no CI comprova compilação para a ABI; somente uma execução no aparelho físico comprova o comportamento naquele telefone.

Para uma comparação minimamente útil, mantenha o mesmo arquivo, o mesmo número de frames de aquecimento/medição, o mesmo orçamento de instruções e builds `--release`. Execute três rodadas em cada plataforma, sem outros processos pesados, e compare p50/p95, instruções por segundo e motivos de parada. Anote modelo do aparelho, versão do Android, arquitetura, temperatura aproximada e modo de energia.

**Limite de compatibilidade:** o benchmark mede o caminho de execução que o carregador consegue preparar. Se o jogo parar em SVC/serviço não implementado, se não executar instruções em frames sucessivos ou se a renderização estiver incompleta, o relatório deve ser tratado como diagnóstico de progresso, não como FPS jogável. O executável Android não transforma sozinho o Aceris em um aplicativo gráfico completo.


## Resultado real mais recente — 22/11/2026

O teste manual do Lua Player Plus Cookie Clicker (CIA NoCrypto fornecido pelo mantenedor) executou 300 quadros headless com orçamento de 4.000.000 instruções por quadro e levou aproximadamente 100 segundos de CPU do host. O runtime carregou RomFS, publicou framebuffers e desenhou a tela de erro do próprio aplicativo. Isso é diagnóstico de progresso, não medição de FPS jogável; o desempenho está muito abaixo de tempo real.

O commit 0dc68f9a adicionou tracing opcional de resultados SVC e HLE mínimo csnd:SND; csndInit passa, mas Sound.init ainda falha antes de uma inicialização DSP observável. Não afirmar que o áudio funciona. A CI ARM64 é compilação, não medição de gameplay em Android físico.

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.


## Sonda do caminho completo da sessão ARM11 (26/11/2026, Claude)

- Teste ignorado `system::tests::session_loop_throughput`: laço ARM de load/store/ALU executado pelo caminho real da sessão, incluindo modelo de D-cache, monitor exclusivo e scheduler. **Esta sonda não habilita SCTLR.M**: o núcleo parte do reset com MMU desligada e o programa do teste não a liga; portanto, ela não mede a tradução MMU. Execução: `cargo test --release -p aceris-core --lib session_loop_throughput -- --ignored --nocapture` (`ACERIS_PROBE_FRAMES` define o número de quadros).
- Resultado no contêiner de nuvem: 19–23 M instruções guest/s (≈5 quadros/s com orçamento de 4M). O tempo de relógio varia ±15% entre execuções nesta máquina; para comparar mudanças, use contagem determinística de instruções do host com `valgrind --tool=callgrind` sobre 1 quadro (linha de base: 1,865 bilhão de instruções do host por 4M instruções guest, ≈466 por instrução guest).
- Distribuição (callgrind, inclusivo): `Arm11Core::step_instruction` 72%; loads e stores pelo modelo de D-cache ≈45% (store ≈390 e load ≈280 instruções do host); `Arm11ProcessSession::step` 13% próprio; publicação de ACTLR.SMP 5%; `locate` 5%.
- Tentativas descartadas por medição: trocar os `BTreeMap` do D-cache e do monitor exclusivo por `HashMap` com hasher multiplicativo ficou mais lento (o conjunto residente típico é pequeno); tornar inline a publicação SMP rendeu só 1,5%, e apenas sem SCU.
- Conclusão: não há ganho grande e seguro só com micro-otimização. O próximo passo de alto retorno é um cache de instruções decodificadas por bloco, com caminho rápido de memória por página para acessos que acertam linhas já residentes, e depois um JIT. Cada etapa precisa preservar o modelo de cache e manter a suíte de testes passando.

### Memória rápida e laço de despacho rápido (27/11/2026, Claude)

- **Memória rápida (opcional, desligada por padrão):** loads e stores do ARM11 vão direto ao armazenamento, sem passar pelo modelo de D-cache e write buffer. É a abordagem do Citra/Azahar e do PPSSPP ("fastmem"), em que todos os agentes veem uma memória coerente. Programas corretos não mudam de comportamento: as operações de flush e invalidate viram no-ops efetivos. Fica desligada por padrão para seguir `docs/PERFORMANCE_LOW_END_PLAN.md` (preservar o D-cache e não ativar hacks de precisão globalmente); destina-se ao preset "Desempenho" via `System::set_fast_memory(true)`. Os 12 testes de incoerência de cache fixam o modo preciso explicitamente. Ao ativar a memória rápida, as linhas sujas são escritas de volta e o write buffer é drenado, para não perder escritas.
- **Laço de despacho rápido:** `Arm11Core::run_fast` executa instruções em sequência sem repetir as verificações globais por instrução de `step`. Com MMU desligada, usa o laço direto; com `SCTLR.M` ligada, encaminha para `run_fast_translated`, que mantém um `MmuBus` e a TLB durante o lote, mas ainda traduz cada fetch/acesso e valida permissões. O lote retorna ao passo preciso antes de CP15, quando ocorre um evento ou quando o barramento sinaliza acesso a registradores de interrupção/timer/SCU, ou qualquer condição de elegibilidade muda. A contagem e o estado arquitetural são comparados nos testes diferenciais.
- **Medição (callgrind, 1 quadro da sonda; linha de base 1,865 bilhão de instruções do host):** padrão (modelo preciso + despacho rápido) 1,270 bilhão (−32%), ~36 M instruções guest/s; com memória rápida (`ACERIS_PROBE_FAST_MEMORY=1`) 1,139 bilhão (−39%), ~41 M instruções guest/s.
- **Próximos gargalos medidos:** laço/fetch 23%, `data_processing` 18% (~100 instruções do host por instrução ALU), decodificação em `execute_arm` 16%, `locate` 9%. Próximo passo: cache de instruções pré-decodificadas para o segmento de texto (imutável), no estilo do interpretador IR do PPSSPP.
- **Referências consultadas (somente leitura):** PPSSPP (GPLv2+), `Core/MIPS/IR/IRInterpreter.cpp` e `IRJit.cpp`, para o conceito de despacho por blocos; Azahar (GPLv3), modelo de memória sem cache de dados. O port do Black para Android (XboxXBE-Recomp) usa recompilação estática mais cache de shaders e escala de resolução; essas ideias ficam registradas para a GPU e para a interface. Nenhum código foi copiado.


## Despacho em lote com MMU ativa (27/11/2026, ChatGPT)

- `Arm11Core::run_fast` agora encaminha sessões com `SCTLR.M` ativa para `run_fast_translated` em `cpu/mmu.rs`.
- O lote mantém uma instância de `MmuBus` e a TLB do núcleo durante várias instruções, evitando reconstruir o adaptador e retirar/recolocar a TLB em cada passo. A tradução de endereços virtuais, permissões, atributos de memória e política de cache continuam sendo verificadas para cada fetch e acesso de dados.
- O estado de privilégio é atualizado após alterações de modo. Instruções CP15 e eventos saem para o despachante preciso; acessos observáveis aos registradores GIC, timer ou SCU fazem o lote ceder ao caminho preciso.
- Testes diferenciais comparam ARM e Thumb contra `step` preciso; o caso ARM também faz `STR/LDR` através da MMU. Testes separados verificam o retorno em SVC e o handoff de instrução CP15.
- Os valores Callgrind de 1,865B → 1,270B → 1,139B instruções do host pertencem à sonda sintética anterior, que executa com a MMU guest desligada. Não devem ser atribuídos ao novo lote traduzido.
- A sonda `translated_fast_dispatch_throughput` agora mede o caminho MMU ativo em build release no runner Ubuntu do GitHub Actions. Na execução de 27/11/2026 (1M de instruções por rodada), o passo preciso marcou 6,14 M instruções guest/s nas três rodadas; o lote marcou 16,71, 16,89 e 16,90 M instruções guest/s, com speedup de 2,72×, 2,75× e 2,75×. Mediana observada: 2,75×.
- **Limite da medição:** o teste usa `FlatMemory` e isola CPU/MMU/dispatch. Não mede `Arm11ProcessSession`, D-cache do processo, scheduler, periféricos, GPU, apresentação, gameplay ou Android físico. É evidência de redução do custo do núcleo neste microcenário, não um ganho de FPS equivalente.
- Próxima medição necessária: uma sonda de sessão completa com MMU ativa e, separadamente, execução em Android ARM64 físico; manter contagem guest e programa idênticos entre passo preciso e lote.

## Sonda isolada do despacho com MMU ativa (27/11/2026)

- O teste ignorado `cpu::mmu::tests::translated_fast_dispatch_throughput` compara o passo preciso (`Arm11Core::step`) com o lote (`Arm11Core::run_fast`) usando `SCTLR.M=1`, uma seção traduzida e o mesmo laço ARM de ALU + `STR/LDR`. Ambos executam exatamente o mesmo orçamento e verificam registradores, CPSR, PC e memória antes de imprimir taxa e speedup.
- Execução release: `cargo test --release -p aceris-core --lib translated_fast_dispatch_throughput -- --ignored --nocapture`.
- Padrões: 1.000.000 instruções por rodada e 3 rodadas. `ACERIS_MMU_PROBE_INSTRUCTIONS` e `ACERIS_MMU_PROBE_ROUNDS` permitem ajustar duração e amostras.
- Esta é uma microsonda de CPU/MMU com `FlatMemory`; não inclui scheduler/process session, o D-cache completo do processo, periféricos, GPU ou apresentação. Ela isola o custo da tradução e do despacho e não deve ser descrita como FPS de jogo.
- A suíte normal compila o teste, mas não o executa porque está marcado `#[ignore]`. O workflow CI tem uma etapa adicional Ubuntu-only que o executa explicitamente em release, com 1M de instruções × 3 rodadas; a saída aparece no log do job. A CI de 27/11/2026 passou em Ubuntu, Windows, macOS e Android arm64-v8a.

### Cache de instruções A32 decodificadas (27/11/2026, Claude)

- `crates/aceris-core/src/cpu/decode_cache.rs`: cache de mapeamento direto com 4096 entradas, indexado pela **codificação** da instrução, não pelo endereço. Como a decodificação depende só dos 32 bits, o cache nunca precisa de invalidação, seja por código reescrito, remapeado, CRO ou MMU. Formas decodificadas: data processing com imediato ou registrador com deslocamento imediato; LDR/STR/LDRB/STRB com offset imediato; B/BL. Qualquer forma com R15, codificação rejeitada pelo caminho genérico, LDRT/STRT ou condição 0xF cai em `execute_arm`.
- Usado apenas pelo laço rápido (`run_fast`); o passo preciso continua usando `execute_arm`.
- Teste diferencial `decoded_execution_matches_the_generic_interpreter`: 60 mil instruções aleatórias, com estado e memória aleatórios, comparando resultado, registradores, CPSR, branch e memória entre os dois caminhos.
- Medição (callgrind, sonda, modo padrão): de 1,270 para 1,025 bilhão de instruções do host por quadro (−19%). Acumulado desde a linha de base de 1,865 bilhão: −45%.
- Ideia inspirada no "decodificar uma vez, despachar muitas" do interpretador IR do PPSSPP (consulta somente leitura, GPLv2+); código próprio.
- Próximos gargalos: laço/fetch (~44% próprio de `run_instructions`), escrita pelo modelo de D-cache (13%) e `locate` (10%).

### `locate` com dica de segmento e laço sem reavaliar elegibilidade (27/11/2026, Claude)

- `locate` tenta primeiro o segmento estático que resolveu o último endereço (`static_segment_hint`). Os segmentos são disjuntos e o layout não muda depois da construção, então o resultado é idêntico à varredura em ordem. Medição: de 1,025 para 0,957 bilhão de instruções do host por quadro.
- `run_fast` deixou de reavaliar `fast_path_eligible()` a cada instrução: nenhuma das condições pode mudar dentro do laço sem um evento, um passo preciso ou uma mudança de estado de interrupção já sinalizada pelo barramento. Um `debug_assert!` mantém a verificação nas builds de teste. Medição: de 0,957 para 0,909 bilhão.
- Acumulado no modo padrão (modelo preciso de D-cache): de 1,865 para 0,909 bilhão de instruções do host por quadro, −51%, ou seja, cerca de 2× a velocidade guest da linha de base.

### Opção "Desempenho" nos frontends (27/11/2026)

A memória rápida pode ser ativada pelo usuário, nunca por padrão. No desktop e
no Android há um botão em Configurações (`performance_mode.txt`). No desktop
também funciona `ACERIS_FAST_MEMORY=1`. No benchmark, use
`cargo run --release -p aceris-core --example benchmark -- <JOGO> --fast-memory`.
