# Estudo de referências para desempenho — Aceris

**Revisão:** 25/11/2026  
**Escopo:** CPU ARM11, execução de código, memória, pipeline gráfico e desempenho multiplataforma.  
**Regra de implementação:** toda melhoria do Aceris continua sendo desenvolvida independentemente, em Rust, sem copiar, traduzir ou portar código, shaders, testes ou assets de terceiros. Este documento registra princípios técnicos e hipóteses para validar no próprio projeto.

## Resumo executivo

As referências apontam para três linhas complementares:

1. **Tradução de código:** emuladores como PPSSPP e a família Citra/Azahar aceleram a execução por meio de um caminho intermediário e recompilação dinâmica (JIT). BLACK demonstra recompilação estática para um título específico, inclusive gerando builds nativos para ARM64.
2. **Renderização medida por estágio:** Dragon Rage mostra que contar FPS não basta. É preciso separar tempo de CPU, GPU e apresentação, medir passes/cópias e comparar a imagem antes e depois de cada otimização.
3. **Especialização por plataforma:** Black Android e Dragon Rage mantêm o código de jogo/execução separado da camada de plataforma. Dragon Rage também registra resultados distintos por GPU, sem presumir que uma configuração ideal para Mali será ideal para Adreno.

Nenhuma dessas soluções pode ser transplantada diretamente para o Aceris. Um port recompilado de um único jogo não precisa reproduzir todo o hardware de um console; o Aceris precisa manter a semântica geral do ARM11, MMU, sistema operacional, PICA200, áudio e entrada de diferentes títulos.

## 1. BLACK — recompilação estática para PC e Android

**Referências consultadas**

- [Black Android — README](https://github.com/RanggaCasper/Black-Android/blob/main/README.md)
- [Black Android — CMakeLists](https://github.com/RanggaCasper/Black-Android/blob/main/CMakeLists.txt)
- [BLACK-XBERecomp](https://github.com/TGP482/BLACK-XBERecomp)
- Revisões observadas: Black Android `8e9c6fc7d570503ea68d039cff16c09339ee6d30` (01/11/2026); BLACK-XBERecomp `10b8464fa06ebd6e54e814b250e9e3554a48ef1c` (29/10/2026).
- Licença verificada: o repositório Black Android declara MIT em seu [LICENSE](https://github.com/RanggaCasper/Black-Android/blob/main/LICENSE). A licença do código da ferramenta/repositório BLACK-XBERecomp deve ser verificada individualmente antes de qualquer estudo de implementação; não foi utilizada como base de código.

**O que o projeto faz**

O executável Xbox XBE é recompilado estaticamente no computador de desenvolvimento. O código gerado é então compilado para a arquitetura de destino; o port Android descrito no README produz uma biblioteca ARM64, usa Vulkan para desenho e SDL3 para janela, entrada e áudio. O CMake fixa uma revisão específica do runtime de recompilação e separa os arquivos gerados do código de integração Android.

**O que é útil para o Aceris**

- Separar claramente o código traduzido do runtime que implementa memória, serviços, gráficos e integração com o sistema operacional.
- Tratar a geração/compilação de código como uma etapa cacheável, em vez de repetir trabalho a cada inicialização.
- Manter o mesmo comportamento de jogo e runtime com backends específicos para x86-64 e ARM64.
- Medir o custo do primeiro arranque separadamente do custo de execução subsequente.

**O que não se aplica diretamente**

BLACK é um port de um único programa x86 conhecido. O Aceris precisa aceitar aplicações ARM11 diferentes e executar instruções dinamicamente, com exceções, permissões, cache e efeitos observáveis por dispositivos. Recompilar estaticamente um jogo 3DS não substitui um núcleo de emulação geral. A analogia correta para o Aceris é um futuro tradutor dinâmico de blocos ARM11, não a cópia do pipeline XBE.

## 2. Dragon Rage / Tenkaichi3Decomp — port nativo e laboratório de GPU

**Referências consultadas**

- [Tenkaichi3Decomp — PC port](https://github.com/z3xox/Tenkaichi3Decomp)
- [Dragon Rage — Android README](https://github.com/dfdx047/Tenkaichi3DecompAndroid/blob/main/README.md)
- [Device lab: RG Cube e Retroid Pocket 5](https://github.com/dfdx047/Tenkaichi3DecompAndroid/blob/main/docs/device-lab-2026-10.md)
- [Dragon Rage Lite — notas de trabalho](https://github.com/dfdx047/Tenkaichi3DecompAndroid/blob/main/docs/lite-notes.md)
- Revisões observadas: Android `2caa07a69ed4576704730e27a286df7eb4c2c3a2` (11/11/2026); PC `48d66c1f66fdcaf71428a06c9fa9eb036e994e2b` (02/11/2026).
- Licença verificada: os repositórios atuais do port indicam AGPL-3.0 para o código próprio coberto por essa licença. O código do jogo descompilado é uma categoria distinta e continua pertencendo aos titulares de direitos. Nenhum código foi reutilizado.

**O que é o projeto**

Tenkaichi3Decomp é um port nativo construído sobre uma descompilação específica do jogo. Dragon Rage leva esse motor para Android ARM64. Não é uma emulação genérica de PlayStation 2: o código de um jogo conhecido é compilado para a plataforma de destino, com uma camada própria de plataforma, renderização, áudio e entrada.

**Lições de medição que interessam ao Aceris**

O laboratório do projeto mede separadamente velocidade da lógica do jogo, imagens por segundo, trabalho médio por quadro, pior quadro, tempo de apresentação, número de passes e cópias. Também faz execuções limitadas pela CPU e pela GPU para separar gargalos.

Alguns resultados registrados no Mali-G57 MC4 ilustram a importância de otimizar o trabalho real, e não apenas reduzir chamadas:

- Desabilitar load/store de profundidade nos passes que não a utilizam elevou a velocidade de 73,3% para 86,8% no cenário Vulkan 2x. A comparação de pixels indicada no relatório ficou idêntica.
- Remover cópias de teste de alfa produziu resultados mais rápidos, mas imagens incorretas; portanto, não é uma otimização aceitável.
- No Adreno 650, manter buffers persistentes tornou viável o caminho OpenGL 2x que antes levava aproximadamente quatro segundos por quadro.
- O relatório também registra testes sem ganho mensurável, como algumas quantidades de quadros em voo e modos de apresentação. Isso evita adotar ajustes por intuição.

As notas Lite registram ainda que reaproveitar conjuntos de descritores Vulkan em layouts compatíveis elevou uma medição de 4x4 de 74% para 86% no Adreno. Esse é um exemplo de cache de estado de renderização; qualquer cache equivalente no Aceris precisa ser validado contra alterações de textura, shader, layout e ciclo de vida dos recursos.

**Aplicação ao Aceris**

- Criar medições que separem CPU guest, processamento GSP/PICA, rasterização, cópias de memória e apresentação.
- Registrar média e percentis, especialmente p95/p99 e pior quadro, não apenas FPS médio.
- Executar perfis controlados por plataforma/GPU e identificar quando a carga é CPU-bound ou GPU-bound.
- Só eliminar passes, cópias ou leituras que sejam comprovadamente redundantes; comparar as imagens produzidas e manter testes de regressão.
- Construir caches de estado de GPU e shaders com chaves e invalidação explícitas, em vez de manter recursos por suposição.

## 3. PPSSPP — IR e JIT em um emulador multiplataforma

**Referências consultadas**

- [PPSSPP: Developer tools / CPU Core](https://www.ppsspp.org/docs/development/developer-tools/)
- [PPSSPP: Introduction](https://www.ppsspp.org/docs/development/getting-started/)
- [PPSSPP: FAQ sobre JIT](https://www.ppsspp.org/docs/faq/)
- Revisão observada: `hrydgard/ppsspp` `cce6425fae9c58f470a549801b81e9fc5e3bee04` (25/11/2026).
- Licença declarada pela documentação: GPL-2.0-or-later. Nenhum código foi reutilizado.

**Lição principal**

A documentação descreve três níveis úteis: interpretador convencional, interpretador de uma representação intermediária (IR) e recompilação de blocos IR para código nativo. A IR permite aplicar otimizações antes da execução; o JIT nativo dá o maior potencial de aceleração, mas exige geração de código e ferramentas de depuração específicas.

**Aplicação ao Aceris**

A evolução mais segura é escalonada:

1. Manter o interpretador ARM11 como referência semântica.
2. Definir uma IR própria e explícita para operações ARM/Thumb, flags, exceções, acessos de memória e eventos.
3. Introduzir um executor de IR rápido, comparado instrução a instrução com o interpretador.
4. Só então criar backends JIT próprios para x86-64 e AArch64, com cache de blocos, invalidação de código modificado e fallback ao interpretador para instruções/eventos não suportados.

O orçamento de instruções, o estado de exceções e a sincronização com timers, IPC e GPU precisam continuar observáveis. Um bloco JIT não pode ignorar pontos nos quais o guest pode receber uma interrupção, executar SVC, alterar mapeamentos ou escrever código.

## 4. Azahar / Dynarmic — referência diretamente relacionada ao ARM11 do 3DS

**Referências consultadas**

- [Dynarmic — dynamic ARM recompiler](https://github.com/azahar-emu/dynarmic)
- [Dynarmic — licença](https://github.com/azahar-emu/dynarmic/blob/master/LICENSE.txt)
- [Citra CPU architecture notes](https://citra.azahar-emu.org/wiki/cpu/)
- [Azahar — seleção de JIT e fast interpreter](https://github.com/azahar-emu/azahar/blob/master/src/citra_libretro/core_settings.cpp)
- Revisões observadas: Dynarmic `a46601580d5512d324104f985b5f0209dc980ddc` (26/09/2026); Azahar `fd4c9abd98345dbefff9cef07aadaf61ee1f683d` (24/11/2026).
- Licença verificada para o componente Dynarmic: 0BSD. Isso não altera a regra do Aceris de não copiar nem incorporar código de terceiros.

**Lição principal**

A documentação do componente descreve tradução ARM para x86-64 e AArch64, suporte a perfis ARM que incluem ARMv6K e integração com sistemas de memória fornecidos pelo host. As notas históricas de Citra descrevem o JIT Dynarmic e um interpretador como caminhos distintos; a configuração atual de Azahar também expõe opções de JIT e fast interpreter.

**Aplicação ao Aceris**

- O conjunto de instruções alvo é compatível com a direção de investigação: ARMv6K/Thumb para hosts x86-64 e AArch64.
- A integração precisa aceitar o sistema de memória do Aceris, em vez de assumir RAM plana.
- Instrumentação por bloco/instrução deve ser prevista desde o desenho do tradutor.
- O interpretador rápido continua como fallback e como oráculo de comparação.
- Invalidação de blocos, exceções, CP15/MMU, exclusivas, barreiras e auto-modificação são requisitos centrais, não detalhes posteriores.

A documentação histórica de Citra reporta ganhos de desempenho variáveis conforme o jogo. Esses números não são uma previsão para o Aceris e não devem ser usados como meta de benchmark.

## 5. Estratégia de engenharia resultante

As referências mudam a ordem de trabalho, mas não a identidade do Aceris:

| Ordem | Trabalho | Critério de aceitação |
|---|---|---|
| 1 | Remover alocações do caminho VFP vetorial | Testes VFP existentes aprovados; benchmark release comparável |
| 2 | Reduzir custo de acesso ao contexto de thread/escalonador | Mesmo comportamento FIFO/prioridade; testes de ciclo de vida aprovados |
| 3 | Adicionar profiling por estágio no benchmark | CPU, GPU, cópias e apresentação contabilizados sem alterar o resultado do guest |
| 4 | Otimizar caches e rasterização PICA200 | Comparação de pixels e testes de coerência aprovados |
| 5 | Especificar IR própria ARM11/Thumb | Comparação determinística com o interpretador |
| 6 | Prototipar JIT por blocos para x86-64 e AArch64 | Fallback correto, invalidação e testes de exceção/memória aprovados |

### Regras para validar cada otimização

- Rodar benchmarks release com a mesma imagem, mesmo orçamento de instruções, mesmos quadros de aquecimento e medição, mesma resolução e mesma configuração.
- Registrar dispositivo, SoC/GPU, driver, sistema operacional, revisão Git e configuração.
- Comparar contagem de instruções, PC/CPSR, eventos de interrupção e saídas gráficas, além do tempo.
- Separar primeiro arranque (compilação/cache frio) de execução com cache quente.
- Reverter ou reescrever qualquer otimização que melhore o tempo alterando incorretamente o comportamento guest.
- Não afirmar ganhos em Android físico enquanto não houver medição num aparelho real.

## Estado inicial do trabalho

Em 25/11/2026, o Aceris já possui fetch de instruções direto para a região de texto estática, consultas de intervalo no cache D e publicação do modo SMP apenas quando o estado muda. Foi publicada também uma primeira alteração para substituir alocações de `Vec` no caminho das operações vetoriais VFP por armazenamento fixo na pilha. A CI deve validar essa alteração; nenhum ganho numérico de FPS ou tempo foi ainda medido para ela.
