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
cargo run --release -p aceris-core --example benchmark -- "/caminho/para/seu-jogo.cia" --warmup 30 --frames 300
```

Também são aceitos arquivos 3DSX e imagens NCSD `.3ds`/`.cci`, desde que o caminho de carregamento atual consiga preparar o conteúdo. Parâmetros:
- `--warmup N`: chamadas descartadas antes da medição (padrão 15);
- `--frames N`: chamadas medidas (padrão 120);
- `--instruction-budget N`: limite de instruções por chamada (padrão igual a `DEFAULT_FRAME_INSTRUCTION_BUDGET`).

No Windows PowerShell:

```powershell
cargo run --release -p aceris-core --example benchmark -- "D:\Jogos\seu-jogo.cia" --warmup 30 --frames 300
```

A janela gráfica não é necessária para esse exemplo. O benchmark é um utilitário de linha de comando desktop.

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

Este exemplo mede o build desktop. O crate Android atual é uma biblioteca nativa Rust, não um aplicativo instalável com Activity/superfície/ciclo de vida completos. A build Android ARM64 no CI não é uma medição de desempenho em celular. Um teste físico Android exige primeiro o host Android instalável e o envio de frames/entrada conectados ao mesmo núcleo.
