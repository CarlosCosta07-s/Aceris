# Roadmap de otimização de CPU e GPU

> Estado em 27/11/2026. Este documento separa otimizações implementadas, etapas seguintes e requisitos de validação. Não declara ganhos de desempenho sem medições reproduzíveis.

## Princípios

- Preservar o interpretador preciso como referência de compatibilidade e fallback.
- Manter a implementação do núcleo e das traduções em Rust, com código original do Aceris.
- Fazer otimizações em camadas: primeiro pré-decodificação, depois blocos, depois representação intermediária (IR) e somente então JIT.
- Comparar resultados entre caminho otimizado e interpretador para registradores, CPSR, memória, branches, exceções e efeitos colaterais.
- Medir desempenho em PC e Android separadamente. Uma opção experimental não é um ganho garantido.

## CPU — estado atual

### Cache de instruções A32

O cache direto atual tem 4096 entradas e é indexado pela palavra de instrução. Ele reduz a repetição da extração de campos, mas ainda não armazena blocos por endereço nem traduz sequências inteiras.

Formas A32 pré-decodificadas pelo caminho rápido:

- Operações de dados com imediato rotacionado e registrador com shift imediato.
- LDR/STR e LDRB/STRB com deslocamento imediato.
- LDRH/STRH, LDRSB/LDRSH com deslocamento imediato.
- LDM/STM com listas de registradores.
- MUL/MLA.
- UMULL/UMLAL/SMULL/SMLAL.
- SMLAxy, SMLAWy, SMULWy, SMLALxy e SMULxy.
- B/BL.

As variantes não cobertas continuam no interpretador genérico, incluindo formas halfword com offset por registrador, transferências de doubleword, instruções com combinações sensíveis de PC e demais instruções ainda não modeladas. Os helpers de execução compartilhados evitam que a rota rápida crie uma segunda implementação independente da semântica.

**Validação pendente:** existem testes diferenciais para LDM/STM, transferências halfword imediatas e famílias de multiplicação, além do teste diferencial amplo do cache. Esses testes precisam ser executados em CI e localmente antes de declarar a ampliação concluída.

## CPU — próximas fases

### 1. Cache de blocos básicos

**Primeira versão implementada:** cache direto de 256 entradas por núcleo, com até 16 instruções A32 por bloco. A compilação só ocorre quando o bus confirma que o intervalo pertence a texto executável, contíguo e imutável. O cache usa identidade do backing, deslocamento inicial, PC e modo; no acerto, o bus revalida a faixa inteira. O adapter MMU valida permissões de execução, fronteiras de página e continuidade da tradução física.

Os blocos terminam em branch ou transferência de registradores múltiplos. Encodings genéricos ou instruções que exigem o passo preciso ficam fora do bloco e usam o loop existente. Buses sem contrato explícito de texto imutável (por exemplo, RAM gravável) continuam no caminho por instrução. Assim, a primeira versão evita precisar invalidar blocos após escrita guest, DMA ou sincronização de código, porque não aceita esse tipo de backing.

A API `Arm11Core::block_cache_stats()` expõe consultas, hits, misses, blocos/instruções compilados e fallbacks; `take_block_cache_stats()` permite medir intervalos sem reiniciar o núcleo. Testes verificam a contabilização básica. Ainda é necessário coletar benchmarks em jogos reais e medir o custo das consultas de faixa. Também faltam política de invalidação para futuras regiões dinâmicas e uma extensão de blocos para Thumb.

### 2. Interpretador intermediário (IR)

Quando o cache de blocos estiver correto, traduzir cada bloco para um IR próprio e compacto.

O IR deverá representar explicitamente:

- Leitura/escrita de registradores e atualização de flags.
- Acessos à memória e pontos de falha.
- Condições de execução, branches e saídas do bloco.
- Mudanças de modo, estado de instrução e retorno de exceção.

O interpretador IR deve permitir reduzir o custo de despacho e aplicar otimizações seguras, como simplificação de operações e propagação de constantes locais. Cada operação com efeitos colaterais permanece observável e comparável com o interpretador preciso.

### 3. JIT

O JIT só começa após o IR e o cache de blocos terem estabilidade e cobertura de testes. A implementação deverá ser específica por arquitetura hospedeira, sem tornar o restante do núcleo dependente de código nativo.

Requisitos mínimos:

- Gerenciamento explícito de memória executável e política W^X.
- Sincronização/flush de cache de instruções quando exigido pela plataforma.
- Saídas de bloco para exceções, interrupções, branches dinâmicos e instruções não suportadas.
- Invalidação de código nativo junto com o bloco guest correspondente.
- Limites de memória e política de descarte de código.
- Fallback automático ao IR/interpretador quando o JIT estiver desabilitado ou não disponível.

JIT não será habilitado por padrão enquanto não passar por testes diferenciais, estabilidade e benchmarks em cada plataforma.

## GPU e renderer

### Backend acelerado existente

O crate `aceris-graphics` usa `wgpu` para criar uma surface host, enviar a imagem RGBA/BGRX já composta e apresentá-la por um pipeline de triângulo em tela cheia. Isso acelera a apresentação host e evita conversão extra de canais BGRX, mas **não substitui o rasterizador PICA**: comandos e renderização guest ainda dependem do caminho de software do núcleo.

O caminho host mantém fallback de CPU e seleção de backend por `ACERIS_GRAPHICS_BACKEND` (auto, opengl, vulkan, software).

### Cache persistente de pipelines host

O presenter agora tenta usar o pipeline cache de `wgpu` quando o adaptador fornece `PIPELINE_CACHE` e uma chave de cache válida:

- A chave é gerada a partir do adaptador, evitando compartilhar o mesmo arquivo entre GPUs incompatíveis.
- O arquivo fica em `.aceris/gpu_pipeline_cache/` no diretório de dados do Aceris.
- O cache só é carregado quando tem conteúdo não vazio e até 64 MiB; o descriptor do wgpu usa fallback para cache inválido.
- Os dados são gravados em arquivo temporário e renomeados quando o presenter é descartado.
- A persistência via API atual do wgpu é suportada em Vulkan. OpenGL/OpenGL ES e Metal continuam com o pipeline normal, sem depender desse arquivo.

Este arquivo contém dados de pipeline do host, não bytecode de shaders PICA. A criação de pipeline pode aproveitar resultados anteriores do driver, mas o efeito real precisa ser medido em cada dispositivo.

### Próxima etapa: rasterizador PICA acelerado

A migração do rasterizador não consiste em mover apenas a apresentação final para a GPU. Ela exige traduzir os estados e comandos PICA para pipelines, buffers, texturas e shaders do host. O trabalho deverá preservar o renderer de software como referência e fallback.

A implementação deve avançar por recursos verificáveis: desenho de geometria, viewport/scissor, depth/stencil, texturas e sampler, combinação de texturas, blending, fog e formatos de framebuffer. Precisão e diferenças por jogo devem ser testadas antes de ampliar o conjunto de recursos.

### Escala de resolução

A escala de resolução será uma opção separada do tamanho da janela e do escalonamento da imagem final. O seletor só deverá afetar os alvos internos de renderização quando o renderer oferecer suporte real a isso.

Pontos de projeto:

- Começar em 1x como padrão e avaliar níveis maiores apenas quando houver render targets redimensionáveis.
- Preservar coordenadas guest, proporções das duas telas, mapeamento de touch e leitura de framebuffer.
- Dimensionar buffers de cor, profundidade e intermediários em conjunto.
- Respeitar limites de GPU/memória e oferecer fallback para 1x.
- Não chamar aumento de resolução do host de “renderização interna em alta resolução”.

## Medição e critérios de aceite

Cada fase precisa registrar, no mínimo:

- Tempo de execução guest e frames por segundo em cenas repetíveis.
- Contadores de instruções por família e taxa de hit/miss do cache.
- Tempo de CPU para composição e tempo de apresentação GPU.
- Uso de memória dos caches e tempo de inicialização a frio/quente.
- Comparação de saída entre referência e caminho otimizado.

Uma otimização só será considerada concluída quando os testes de correção forem executados, os resultados forem comparáveis e os benchmarks mostrarem o efeito no PC e no Android.