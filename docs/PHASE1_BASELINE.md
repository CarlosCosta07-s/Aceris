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
| Windows x86-64 | Pendente | Pendente | Pendente | Pendente | Pendente | — |
| Linux x86-64 | Pendente | Pendente | Pendente | Pendente | Pendente | — |
| Android ARM64 | Pendente | Pendente | Pendente | Pendente | Pendente | — |

## 5. O que esta etapa ainda não mede

1. CPU ARM11 com scheduler, MMU e D-cache ativos em uma sessão guest completa. Use o benchmark integrado e perfis externos para esse caminho.
2. Tempos separados dos estágios PICA200 (vertex, clipping, fragment, textura, TEV e render target). O perfil atual mede apenas o rasterizador de referência.
3. Latência de IPC/HLE de ponta a ponta, incluindo resolução de serviço, ACL, alocação/tradução de handles, despacho e resposta.
4. Contenção/latência concorrente de memória entre CPU, GPU, DSP e LCD.
5. RSS do processo host e consumo térmico/energético do dispositivo.

Esses itens permanecem no backlog da Fase 1. Não adicionar temporizadores por instrução nem locks de profiling aos caminhos quentes sem uma medição que justifique o custo. A próxima instrumentação integrada deve ser agregada por lote/quadro e desligada por padrão.

## 6. Critério de saída da Fase 1

A Fase 1 só estará concluída quando houver:

- perfil independente compilando e executando em Windows, Linux e Android ARM64;
- pelo menos três resultados comparáveis por plataforma;
- benchmark integrado de jogo repetido com a mesma imagem e orçamento;
- identificação de um gargalo por evidência quantitativa, não apenas por inspeção estática;
- nenhuma regressão na suíte de testes ou na CI.
