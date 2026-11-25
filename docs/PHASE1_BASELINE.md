# Fase 1 — linha de base de desempenho e diagnóstico

**Estado:** instrumentação inicial adicionada; resultados quantitativos multiplataforma ainda não registrados.  
**Escopo:** CPU ARM11, memória, rasterização de referência e decodificação IPC.  
**Regra:** medir antes de otimizar. Nenhuma rotina do caminho de emulação é alterada por este perfil.

## 1. Perfil isolado dos componentes

O exemplo `crates/aceris-core/examples/benchmark.rs` aceita `--profile-components` como primeiro argumento. O perfil funciona sem ROM, firmware ou chaves:

```sh
cargo run --release -p aceris-core --example benchmark -- --profile-components
```

Quando combinado com uma imagem fornecida pelo usuário, executa primeiro os microbenchmarks e depois o benchmark integrado:

```sh
cargo run --release -p aceris-core --example benchmark -- "/caminho/para/seu-jogo.cia" --profile-components --warmup 30 --frames 300
```

O perfil independente executa workloads determinísticos com contagem fixa:

| Domínio | Operação medida | Indicadores |
|---|---|---|
| CPU | Interpretador A32 com uma sequência curta de MOV, ADD e CMP | instruções/s e ns/instrução |
| Memória | Leitura e escrita de 32 bits em `FlatMemory` | operações/s, ns/op e MiB/s |
| Memória | Leitura e escrita de 32 bits no backing físico esparso da GPU | operações/s, ns/op e MiB/s |
| GPU | Triângulos preenchidos pelo `SoftwareRasterizer` de referência em 400×240 | triângulos/s e pixels/s |
| IPC | Decodificação de mensagem e descritor de buffer mapeado | mensagens/s e ns/mensagem |

A sequência de CPU e as regiões de memória são sintéticas. A rasterização exercita somente o rasterizador de referência; não executa o pipeline PICA200 completo. O teste IPC mede parser e descritores; não inclui `srv:GetServiceHandle`, ACL, handles do kernel, escalonamento ou execução do serviço.

Esses limites são intencionais: o perfil fornece linhas de base de componentes sem alterar ou instrumentar o caminho quente do emulador. Ele não deve ser apresentado como FPS de jogo ou tempo total de emulação.

## 2. Benchmark integrado de jogo

Para medir o caminho real do Aceris com um título que o usuário possui, use o mesmo executável sem `--profile-components`:

```sh
cargo run --release -p aceris-core --example benchmark -- "/caminho/para/seu-jogo.cia" --warmup 30 --frames 300
```

Esse relatório já mede duração de processamento por quadro (média, p50, p95 e p99), instruções ARM11, throughput do guest, listas GSP, operações de scanout LCD, motivos de parada e diagnóstico de memória do espaço de processo.

O orçamento de instruções é um limite determinístico, não um relógio ARM11 nem um quadro de hardware. As contagens de listas GSP e scanouts não provam renderização, áudio ou gameplay corretos. Para comparar builds, preserve a mesma imagem, opções, versão release, aquecimento e condições térmicas.

## 3. Execução no Android ARM64

O workflow CI publica o binário `aceris-android-benchmark-arm64-v8a`. Depois de baixar o artefato aprovado:

```sh
adb push aceris-benchmark /data/local/tmp/aceris-benchmark
adb shell chmod 700 /data/local/tmp/aceris-benchmark
adb shell /data/local/tmp/aceris-benchmark --profile-components
```

Para o benchmark integrado, copie também a imagem do próprio usuário e execute-a pelo mesmo binário. Nenhum arquivo de jogo ou chave deve ser enviado ao repositório, CI ou artefatos.

## 4. Protocolo para registrar resultados

Registre cada execução com:

- commit SHA e estado da CI;
- sistema operacional, arquitetura, CPU, RAM e, no Android, SoC/GPU;
- modo release, opções e número de repetições;
- alimentação, modo de energia, estado térmico e aplicações em segundo plano;
- saída completa do perfil e, separadamente, saída do benchmark integrado.

Faça pelo menos três execuções independentes por plataforma e compare a mediana. Não misture valores de microbenchmarks sintéticos com o tempo por quadro do jogo.

### Planilha de resultados

| Plataforma | CPU A32 (instr/s) | FlatMemory R/W (MiB/s) | GPU backing R/W (MiB/s) | Rasterizer (pixels/s) | IPC (msg/s) | Commit |
|---|---:|---:|---:|---:|---:|---|
| Windows x86-64 (CI) | 47.78 M | 1,119.2 / 8,838.5 | 68.8 / 176.8 | 32.39 M | 15.98 M | `a07936e` |
| Linux x86-64 (CI) | 55.05 M | 2,691.8 / 7,602.3 | 102.1 / 365.4 | 42.90 M | 43.73 M | `a07936e` |
| macOS ARM64 (CI) | 29.24 M | 946.4 / 5,379.1 | 74.0 / 202.1 | 63.06 M | 32.49 M | `a07936e` |
| Android ARM64 físico | Pendente | Pendente | Pendente | Pendente | Pendente | — |

## 5. Fotografia inicial dos runners CI — 25/11/2026

Workflow: [45646435525](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45646435525), commit `a07936ec797d5769d36fbdd193985fa5e99b2162`. Formatação, build, Clippy, testes completos, build release do benchmark e execução dos perfis passaram em Windows x86-64, Linux x86-64 e macOS ARM64. O job Android ARM64 aprovou os builds release do aplicativo e do executável benchmark.

Cada runner executou uma única amostra de 250.000 instruções/operações e 32 triângulos, sem repetição estatística nem aquecimento separado:

| Métrica | Windows x86-64 | Linux x86-64 | macOS ARM64 |
|---|---:|---:|---:|
| Interpretador ARM11 A32 (ops/s) | 47.784.701 | 55.050.461 | 29.240.481 |
| FlatMemory write32 (MiB/s efetivos) | 1.119,2 | 2.691,8 | 946,4 |
| FlatMemory read32 (MiB/s efetivos) | 8.838,5 | 7.602,3 | 5.379,1 |
| GPU backing write32 (MiB/s efetivos) | 68,8 | 102,1 | 74,0 |
| GPU backing read32 (MiB/s efetivos) | 176,8 | 365,4 | 202,1 |
| Rasterizador de referência (pixels/s) | 32.387.283 | 42.899.890 | 63.062.048 |
| Decodificação IPC (mensagens/s) | 15.975.564 | 43.734.675 | 32.487.046 |

Os números de memória representam quatro bytes processados por operação no conjunto de dados deste microbenchmark. Não são medição de largura de banda DRAM do aparelho: o FlatMemory usa uma região de 64 KiB e o backing GPU usa páginas esparsas/lookup sincronizado. Os resultados variam com CPU, cache, compilador, sistema operacional, runner e carga de fundo. O valor do macOS é ARM64, não x86-64.

**Leitura inicial, não conclusão de gargalo:**
- O backing físico esparso tem custo por operação bem acima do `FlatMemory` neste cenário sintético. Vale um benchmark mais específico de acesso por página, cruzamento de página, contenção e lock antes de mudar sua estrutura.
- A rasterização de referência mostra diferenças expressivas entre runners, mas não mede o fragment rasterizer PICA200 completo. Não autoriza otimizar a GPU guest com base isolada nesse número.
- O throughput do interpretador A32 é somente o custo de `Arm11Core::step` com uma pequena sequência sintética; não inclui MMU, D-cache, scheduler, serviços, nem espera de dispositivos.
- IPC mede a decodificação de mensagens/descritores com alocação de resultado. A parte de resolução SRV/ACL/handles/HLE ainda exige profiling ponta a ponta.

Não calcular uma classificação de plataformas a partir desses valores: runners CI podem usar CPUs, caches e limites de carga diferentes. O passo seguinte é coletar pelo menos três execuções em cada máquina local, além do benchmark integrado com um mesmo homebrew e orçamento.

## 6. O que esta etapa ainda não mede

1. CPU ARM11 com scheduler, MMU e D-cache ativos em uma sessão guest completa. Use o benchmark integrado e perfis externos para esse caminho.
2. Tempos separados dos estágios PICA200 (vertex, clipping, fragment, textura, TEV e render target). O perfil atual mede apenas o rasterizador de referência.
3. Latência de IPC/HLE de ponta a ponta, incluindo resolução de serviço, ACL, alocação/tradução de handles, despacho e resposta.
4. Contenção/latência concorrente de memória entre CPU, GPU, DSP e LCD.
5. RSS do processo host e consumo térmico/energético do dispositivo.

Esses itens permanecem no backlog da Fase 1. Não adicionar temporizadores por instrução nem locks de profiling aos caminhos quentes sem uma medição que justifique o custo. A próxima instrumentação integrada deve ser agregada por lote/quadro e desligada por padrão.

## 7. Critério de saída da Fase 1

A Fase 1 só estará concluída quando houver:

- perfil independente compilando e executando em Windows, Linux e Android ARM64;
- pelo menos três resultados comparáveis por plataforma;
- benchmark integrado de jogo repetido com a mesma imagem e orçamento;
- identificação de um gargalo por evidência quantitativa, não apenas por inspeção estática;
- nenhuma regressão na suíte de testes ou na CI.
