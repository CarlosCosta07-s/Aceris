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
