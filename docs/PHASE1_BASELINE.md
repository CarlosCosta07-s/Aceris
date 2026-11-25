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

## 5.1 Linha de base repetida — CI do commit 7b01668 (25/11/2026)

Workflow: [45649636486](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45649636486). Todos os quatro jobs concluíram com sucesso: Windows x86-64, Linux x86-64, macOS ARM64 e build Android ARM64. Os três runners desktop executaram um aquecimento descartado e cinco repetições medidas. Os valores abaixo são medianas das cinco repetições extraídas dos logs; não misturar com a fotografia de amostra única da seção anterior.

| Métrica | Windows x86-64 | Linux x86-64 | macOS ARM64 |
|---|---:|---:|---:|
| Interpretador ARM11 A32 (instr/s) | 50,34 M | 57,97 M | 40,15 M |
| Interpretador ARM11 Thumb (instr/s) | 73,59 M | 82,43 M | 66,91 M |
| Despacho de listas PICA (listas/s; 4 writes/lista) | 15,21 M | 17,61 M | 16,37 M |
| FlatMemory write32 (MiB/s efetivos) | 1.260,0 | 2.761,9 | 1.489,0 |
| FlatMemory read32 (MiB/s efetivos) | 6.150,8 | 10.532,6 | 8.361,7 |
| Backing GPU write32 (MiB/s efetivos) | 74,4 | 99,3 | 67,6 |
| Backing GPU read32 (MiB/s efetivos) | 178,9 | 381,3 | 188,7 |
| Rasterizador de referência (pixels/s) | 26,48 M | 42,70 M | 51,97 M |
| Decodificação IPC (mensagens/s) | 16,82 M | 37,11 M | 24,81 M |

**Escopo dos resultados adicionais:**
- A32 e Thumb executam sequências curtas e determinísticas diretamente em `Arm11Core::step`; não incluem MMU, D-cache, scheduler ou serviços.
- PICA mede listas de quatro escritas de registradores no decoder `Pica200::execute_command_list`; não executa vertex/fragment shaders nem o restante do pipeline gráfico.
- O backing GPU e o FlatMemory usam estruturas e sincronização diferentes. Os MiB/s indicam somente quatro bytes por operação no workload de 64 KiB; não representam largura de banda DRAM.
- Rasterização é o rasterizador de referência, não a etapa fragment do pipeline PICA200.
- IPC mede `IpcMessage::decode` e descritores; não inclui lookup SRV, ACL, handles, scheduler ou execução HLE.

**Leitura inicial para orientar a próxima investigação:**
1. O backing GPU esparso permanece claramente mais caro por operação que o `FlatMemory` neste workload. A razão entre os valores medianos de escrita é aproximadamente 17×–28×; para leitura, aproximadamente 28×–44×. Isso justifica um perfil dedicado de acesso por página, padrão sequencial/aleatório, cruzamento de página e contenção. Não justifica trocar `BTreeMap`, locks ou o modelo de páginas antes de medir esses casos.
2. O throughput A32/Thumb não é o gargalo global do emulador: o custo do guest real inclui MMU, D-cache, exceções, scheduler e dispositivos. O benchmark integrado continua necessário para separar custo por instrução de throughput por quadro.
3. O runner macOS apresentou dispersão maior em algumas métricas (por exemplo, A32 variou aproximadamente de 35,2 M a 56,5 M instr/s; listas PICA, de 11,7 M a 19,0 M listas/s). Tratar a mediana como referência daquele runner, não como propriedade fixa da arquitetura ARM64.
4. O Android ARM64 foi compilado com sucesso, mas a CI não executa o binário em aparelho físico. A linha de base Android continua pendente.

## 5.2 Perfil de acesso ao backing físico GPU — 25/11/2026

O novo exemplo `crates/aceris-core/examples/memory_profile.rs` compara padrões de acesso usando a API pública `GpuPhysicalMemory`, sem modificar a implementação de memória:

```sh
cargo run --release -p aceris-core --example memory_profile
```

O teste pré-materializa 4 MiB (1.024 páginas de 4 KiB), gera os endereços antes da região cronometrada e executa um aquecimento descartado mais cinco repetições. Cada caso faz um milhão de leituras ou escritas de 32 bits.

| Padrão | Windows leitura/escrita | Linux leitura/escrita | macOS leitura/escrita |
|---|---:|---:|---:|
| Página quente de 4 KiB | 287,4 / 109,1 MiB/s | 332,5 / 172,1 MiB/s | 176,5 / 80,0 MiB/s |
| Sequencial em 4 MiB | 218,8 / 72,6 MiB/s | 249,5 / 92,5 MiB/s | 173,5 / 59,2 MiB/s |
| Pseudoaleatório em 4 MiB | 70,9 / 18,1 MiB/s | 75,0 / 20,9 MiB/s | 49,7 / 9,0 MiB/s |

**Validação desktop:** workflow [45651300462](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45651300462), concluído com sucesso em Windows, Linux e macOS. Formatação, build, Clippy, suíte completa de testes, perfil de componentes e perfil de memória passaram nos três runners. O job Android compilou o app e o benchmark de componentes; a compilação do novo exemplo Android foi adicionada ao workflow seguinte.

**Interpretação:**
- O padrão pseudoaleatório foi aproximadamente 3,5×–4,4× mais lento na leitura e 6,0×–8,9× mais lento na escrita que o padrão concentrado na página quente.
- O padrão sequencial ficou entre os dois, indicando que a localidade de página influencia bastante o custo observado.
- A escrita inclui atualização do mapa de versões do monitor de exclusividade para os granules tocados. A leitura inclui validação de intervalo, aquisição do `RwLock` e lookup na estrutura esparsa.
- A diferença é compatível com custo de lookup/localidade das páginas e do mapa de versões; este teste não isola individualmente esses componentes. Não concluir que o `BTreeMap` ou o lock, isoladamente, é responsável pelo resultado.
- O perfil é single-threaded e exclui alocação inicial de páginas. Ainda não mede contenção real entre CPU, GPU, DSP e LCD, nem acesso pelo caminho completo da MMU/D-cache.

### Execução no Android ARM64

O workflow também compila o exemplo como executável Android em `target/aarch64-linux-android/release/examples/memory_profile`. Depois de baixar o artefato da CI:

```sh
adb push memory_profile /data/local/tmp/aceris-memory-profile
adb shell chmod 700 /data/local/tmp/aceris-memory-profile
adb shell /data/local/tmp/aceris-memory-profile
```

A CI apenas compila o executável Android; a medição ARM64 física continua pendente de execução em aparelho real. Não comparar resultados de runners CI com resultados de telefone sem considerar CPU/SoC, temperatura, energia e carga de fundo.

## 5.3 Execução de homebrew real — Super Mario 64 3DS Port (Luigi Version) — 25/11/2026

**Arquivo de teste:** CIA fornecido localmente pelo mantenedor, Title ID `0x000400000FAA5C00`, NCCH NoCrypto. O CIA e o código extraído permaneceram fora do repositório e da CI.

### Resultado do caminho de execução

O benchmark release oficial carregou o CIA e preparou o ARM11 em `PC=0x00100000`, `CPSR=0x00000010`. Nos cinco primeiros quadros:

| Quadro | Tempo host | Instruções guest | Stop reason | GSP lists | LCD scanouts |
|---:|---:|---:|---|---:|---:|
| 1 | 202,166 ms | 2.348.557 | SVC 0x38 | 0 | 0 |
| 2 | 0,004 ms | 15 | SVC 0x39 | 0 | 0 |
| 3 | 0,001 ms | 7 | SVC 0x3A | 0 | 0 |
| 4 | 0,005 ms | 49 | SVC 0x3C | 0 | 0 |
| 5 | 0,002 ms | 24 | SVC 0x3C | 0 | 0 |

As capturas top-left, top-right e bottom ficaram pretas. O throughput agregado aparente de 24,73 quadros/s nesta amostra **não é FPS de jogo**: quase todo o tempo foi gasto no primeiro trecho de bootstrap e os quadros seguintes apenas pararam em chamadas de supervisor não tratadas. Não houve submissão de listas gráficas nem scanout.

### Falha reproduzível no bootstrap

Com seis quadros de aquecimento e orçamento de 4.000.000 instruções por quadro, a execução falha no quadro 6:

- erro: `invalid memory access at 0x08007FF8`;
- thread: `ThreadId(1)`;
- PC: `0x001D8C1C`;
- SP (r13): `0x08008000`;
- tamanho de stack principal declarado: `0x40000`;
- heap de aplicação mapeado no relatório: **0 KiB**.

A instrução em `0x001D8C1C` é `push {r4, lr}`; por isso ela tenta escrever os oito bytes em `SP - 8 = 0x08007FF8`. A falha é consistente com uma stack secundária de 32 KiB posicionada no início da janela de heap, mas a região não foi mapeada.

### Rastreamento SVC e interpretação

O trace mostra o guest chamando `0x38`, `0x39` e `0x3A` — GetResourceLimit, GetResourceLimitLimitValues e GetResourceLimitCurrentValues — e em seguida `0x23` (CloseHandle). Esses três SVCs de ResourceLimit não possuem handlers no despacho atual do Aceris e caem no caminho de SVC não tratado. A tabela pública de SVC documenta essas assinaturas e identifica `0x3C` como Break. [Referência SVC — 3dbrew](https://www.3dbrew.org/wiki/SVC).

Depois das consultas, o guest pede `ControlMemory` (`0x01`) com operação de alocação, endereço `0x08000000` e tamanho zero. O Aceris responde `0xD8E007F1`; em seguida o guest emite `Break` (`0x3C`) e continua até usar `SP=0x08008000`. O trace também mostra uma tentativa de alocação linear `0x10003` com tamanho zero. A documentação pública descreve `0x08000000) como a janela de heap mapeada por `ControlMemory`. [Layout de memória — 3dbrew](https://www.3dbrew.org/wiki/Memory_layout) e [Memory Management — 3dbrew](https://www.3dbrew.org/wiki/Memory_Management).

**Conclusão sustentada pelos dados:** o primeiro bloqueio observado deste título é de inicialização de memória/serviços do kernel guest, não um gargalo de throughput de CPU ou rasterização. A sequência aponta fortemente para a ausência de SVCs ResourceLimit e para uma alocação de heap com tamanho zero; ainda é necessário conferir o contrato de inicialização usado por este título antes de definir valores ou política de alocação. Não mascarar o problema mapeando uma página arbitrária nem relaxar permissões.

### Próximas tarefas verificáveis

1. Auditar, com o responsável pelo módulo de sessão/SVC, a semântica de `0x38`–`0x3A` e o encerramento/retorno de `0x3C`.
2. Rastrear a origem do tamanho zero entregue a `ControlMemory` e validar o ABI completo `r0)–`r4` para a alocação regular e linear.
3. Corrigir o mapeamento de heap/stack somente após determinar a alocação guest pretendida; cobrir base, tamanho, alinhamento, permissões e falhas com testes.
4. Repetir o mesmo CIA e orçamento. O critério de avanço é chegar a uma primeira apresentação LCD não preta e atividade GSP/PICA observável antes de iniciar qualquer medição de FPS ou otimização por perfil.

A coleta foi feita com o benchmark do commit `9228a03201d135b13f78b2b9a2605ce09e1824b4`; o helper de falha imprime thread, PC/CPSR, registradores e resumo de memória. Nenhum módulo de CPU, sessão, SVC, memória, GPU ou frontend foi alterado nesta análise.

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
