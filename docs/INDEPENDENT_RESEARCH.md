# Pesquisa técnica independente

> **Regra vigente do mantenedor, atualizada em 24/11/2026:** é permitido consultar código-fonte de emuladores efetivamente open source, inclusive para investigar algoritmos, estados, protocolos e casos-limite, desde que a licença do repositório e do arquivo seja verificada antes da consulta. A implementação do Aceris continua independente: não copiar, traduzir, portar ou reproduzir estruturalmente código, testes ou assets. Aplicam-se integralmente as [regras obrigatórias para IAs](REGRAS_PARA_IA.md), especialmente a seção 5.

## 1. Objetivo

Acelerar o Aceris usando especificações públicas, documentação de hardware, descrições independentes de formatos e protocolos, testes próprios e comportamento observável. O resultado de cada pesquisa deve ser uma especificação verificável que oriente uma implementação original em Rust.

A pesquisa não é um atalho para incorporar implementações existentes. Código-fonte open source pode ser consultado como referência após auditoria de licença; documentação pública independente, testes próprios e observações externas continuam sendo evidências importantes. Converter o entendimento obtido em especificações e testes próprios, sem incorporar a implementação consultada.

## 2. Limites obrigatórios

- É permitido consultar código-fonte de Azahar, Citra, Lime3DS e outros emuladores somente quando o projeto e o arquivo consultado forem efetivamente open source e suas licenças tiverem sido verificadas.
- A pesquisa pode usar documentação pública independente de hardware, formatos, APIs e protocolos, comportamento observável e código-fonte open source de outros emuladores, desde que a licença do projeto e do arquivo seja verificada e a consulta seja registrada.
- Não faça tradução linha a linha nem reproduza a estrutura expressiva do código consultado. A implementação do Aceris deve ter arquitetura, interfaces, nomes, testes e código próprios, escritos em Rust.
- Não incorpore commits, patches, dependências, assets ou outros recursos de implementação desses projetos. Não trate nomes de funções privadas ou a organização de arquivos como especificação do hardware.
- Registre projeto, URL, arquivo ou área consultada, commit/revisão, licença, data e pergunta técnica. Registre a conclusão conceitual em uma especificação independente, sem colar trechos de código.
- A licença de cada referência deve ser identificada. Se houver dúvida sobre compatibilidade de licença ou risco de obra derivada, pare antes de aplicar aquele conhecimento na implementação e peça orientação a Carlos Costa.
- Cruze conclusões relevantes com documentação pública independente, testes originais do Aceris e, quando possível, comportamento observado em hardware legítimo. Código de um emulador não é, por si só, prova do comportamento físico correto.
- Assets e recursos visuais/sonoros de terceiros continuam proibidos. Novas dependências que forneçam implementação do emulador continuam sujeitas à autorização explícita do mantenedor.
- Toda funcionalidade do produto continua sendo implementada em Rust. Permanecem em vigor o uso exclusivo de `main`, o isolamento entre agentes, a autoria própria e a proteção de jogos, firmware, chaves e dumps.

Estas regras complementam `docs/REGRAS_PARA_IA.md` e `CONTRIBUTING.md`; não criam exceções às demais regras.

## 3. Processo de pesquisa

Cada tarefa técnica deve seguir esta sequência:

1. **Definir a pergunta:** formular um comportamento observável e delimitado, evitando objetivos vagos como “melhorar a GPU”.
2. **Localizar fontes:** priorizar manuais oficiais e documentação independente. Quando o código de um emulador open source for útil, verificar primeiro a licença do repositório, o cabeçalho/licença do arquivo e a revisão exata; registrar a consulta e não copiar sua implementação. Wikis de engenharia reversa devem ser identificadas como fontes não oficiais.
3. **Registrar evidências:** anotar a afirmação, a URL, o tipo de fonte, a data de consulta, o nível de confiança e eventuais divergências.
4. **Escrever o contrato:** descrever entradas, saídas, estados, erros, efeitos colaterais, ordenação e limites de memória em termos independentes, sem reproduzir trechos nem a estrutura expressiva da implementação consultada.
5. **Derivar testes próprios:** construir casos pequenos e determinísticos diretamente do contrato. Para cada caso, registrar os bytes/valores de entrada e o resultado esperado.
6. **Implementar separadamente:** escrever o código Aceris em Rust de forma independente. A consulta técnica não autoriza copiar, traduzir, portar, adaptar ou reproduzir a organização interna do código estudado.
7. **Validar em camadas:** testes unitários, integração no caminho real do guest, CI multiplataforma e, quando possível, comparação com comportamento em hardware legítimo.
8. **Atualizar o estado:** marcar cada afirmação como documentada, inferida, testada sinteticamente ou confirmada externamente. Uma etapa não se torna compatível apenas por compilar.

## 3.1 Registro de fontes independentes

Registre título, autoria, URL, data, tipo de fonte, pergunta técnica, conclusão, nível de confiança e testes Aceris que confirmam ou contestam a interpretação. Para código consultado, registre também repositório, commit, caminho, licença do projeto e do arquivo, e a conclusão técnica parafraseada. Não copie trechos nem reproduza a estrutura expressiva da fonte.

## 4. Níveis de evidência

| Nível | Significado | Como registrar |
|---|---|---|
| E1 — especificação | Manual oficial ou documento do fabricante descreve o comportamento | Referência primária e revisão aplicável |
| E2 — documentação independente | Documentação técnica pública consistente, mas não oficial | Identificar o autor e a natureza não oficial |
| E3 — teste próprio | Resultado reproduzível obtido por teste Aceris independente | Código do teste, entrada e saída esperada |
| E4 — observação externa | Resultado visível em hardware ou superfície de software permitida | Ambiente, versão, passos e captura/log |
| H — hipótese | Inferência ainda não corroborada | Não implementar como fato; criar teste discriminante |

A concordância entre duas páginas que repetem a mesma fonte original não equivale a duas confirmações independentes. Divergências devem permanecer registradas até serem resolvidas.

## 5. Ficha obrigatória por subsistema

Antes de uma alteração funcional de maior risco, registrar:

- **Pergunta técnica:** qual comportamento precisa ser entendido?
- **Escopo:** quais módulos e interfaces do Aceris serão afetados?
- **Fontes:** documentos, revisões, datas e níveis E1–H.
- **Contrato:** formato dos dados, sequência de operações, estados e erros.
- **Invariantes:** o que nunca pode acontecer, inclusive em falhas parciais?
- **Testes:** casos normais, limites, entradas inválidas e regressões.
- **Integração:** qual caminho do guest deve consumir o comportamento?
- **Limitações:** o que continuará sem suporte após a mudança?
- **Verificação:** comandos executados e resultado real de CI/hardware.

## 6. Primeiras conclusões documentais

### 6.1 ARM11

O ARM11 MPCore implementa ARMv6/ARMv6K e executa os conjuntos A32 e Thumb. O Aceris deve seguir a arquitetura e a implementação ARM11 documentadas; não deve presumir instruções ARMv7 ou Thumb-2 só porque são comuns em processadores posteriores.

Referências:
- [ARM11 MPCore Processor Technical Reference Manual — DDI 0360](https://documentation-service.arm.com/static/5e8e1cd9fd977155116a4a7a)
- [ARM1176JZF-S Technical Reference Manual — DDI 0301](https://documentation-service.arm.com/static/5e8e294efd977155116a6ca3)

### 6.2 3DSX e imagens de título

O 3DSX tem cabeçalho, segmentos Code/ROData/Data, tabelas de relocation e, opcionalmente, metadados SMDH/RomFS. O carregador precisa validar limites e relocations antes de instalar mapeamentos. Para títulos comerciais, NCCH/CXI descreve o contêiner executável, enquanto CIA e NCSD/CCI são camadas de empacotamento distintas. Hashes, flags de criptografia, extended header, ExeFS e descompressão precisam ser tratados como etapas separadas e verificáveis.

Referências:
- [3DSX Format — 3dbrew](https://www.3dbrew.org/wiki/3DSX_Format)
- [Homebrew Launcher — explicação pública do formato 3DSX](https://smealum.github.io/3ds/)
- [NCCH — 3dbrew](https://www.3dbrew.org/wiki/NCCH)
- [CIA — 3dbrew](https://www.3dbrew.org/wiki/CIA)

### 6.3 IPC, GSP e memória compartilhada

IPC e Shared Memory são mecanismos complementares: mensagens transportam solicitações e handles; blocos compartilhados carregam dados maiores e filas secundárias. A fila GX é publicada em memória compartilhada e iniciada por uma solicitação GSP. A relay queue contém eventos de interrupção e dados de framebuffer. Isso exige validar não só o parser da mensagem, mas também a propriedade dos handles, o backing físico, a publicação de memória, a ordenação e a conclusão.

Referências:
- [IPC — 3dbrew](https://www.3dbrew.org/wiki/IPC)
- [GSP Services — 3dbrew](https://www.3dbrew.org/wiki/GSP_Services)
- [GSP Shared Memory — 3dbrew](https://www.3dbrew.org/wiki/GSP_Shared_Memory)

### 6.4 PICA200

A documentação técnica independente descreve uma GPU com controladores de display, unidades de memory fill, motor de transferência e motor 3D. O GSP não é uma API gráfica de alto nível: é a interface de serviço para submissão de trabalho aos motores. As command lists representam escritas adiadas em registradores do motor 3D. O caminho de desenho envolve fetch de vértices ou vértices imediatos, processamento de shader e saída para renderização. A documentação também descreve endereçamento físico e formatos numéricos específicos.

Consequência para o Aceris: não basta validar o parser de command list ou renderizar um triângulo de fixture. Precisamos provar que os registradores, buffers, shaders e endereços usados pelo desenho vieram do estado preparado pelo guest e que os pixels chegam ao framebuffer selecionado pelo caminho GSP/LCD.

Referências:
- [PICA200 — Zitrus 3DS SDK (documentação técnica)](https://zitrus.gasinfinity.dev/docs/3ds/pica200/)
- [Nintendo OpenGL — 3dbrew](https://www.3dbrew.org/wiki/Nintendo_OpenGL)
- [GSP Shared Memory — 3dbrew](https://www.3dbrew.org/wiki/GSP_Shared_Memory)

A documentação comunitária contém áreas incompletas e comportamentos ainda não confirmados. Todo campo de opcode, formato de atributo, valor padrão ou detalhe de temporização que não esteja estabelecido por uma especificação confiável deve ser tratado como hipótese até existir um teste discriminante.

## 7. Estado observado do Aceris em 17/11/2026

Esta fotografia foi conferida no repositório `main` e no histórico de GitHub Actions nesta data. É um resumo de engenharia, não uma declaração de compatibilidade geral.

- **CI:** o commit `96906da3fdb91b623ed9296009d61e18bf486757` passou no workflow [44014209685](https://github.com/CarlosCosta07-s/Aceris/actions/runs/44014209685). A matriz executa verificações Rust em Linux, Windows e macOS e build Android `arm64-v8a`.
- **CPU:** existe cobertura extensa A32/Thumb, CP15/MMU/TLB, VFP e exceções; o roadmap ainda registra lacunas e testes de conformidade independentes.
- **3DSX:** parser, relocations, preparação e sessão ARM11 têm integração e testes sintéticos. Isso não prova compatibilidade de todo homebrew externo.
- **CIA/NCCH:** há parser e caminho de carga NoCrypto. `docs/REAL_HOMEBREW_TESTING.md` descreve um teste manual ignorado por padrão que exige um CIA local. A inspeção estrutural e a execução limitada de um frame não equivalem a gameplay nem a compatibilidade comercial.
- **GSP/LCD:** há um caminho guest→IPC/GSP→fila GX→PICA→DisplayTransfer→scanout em testes contínuos; o próprio roadmap registra campos PICA/GeometryPipeline ainda preparados por fixture host-side, além de limitações do shader mínimo e do framebuffer.
- **Frontends:** o desktop tem janela experimental X11 no Linux. Windows e macOS ainda não têm backend de janela. Android tem biblioteca nativa Rust e ABI; ainda não há Activity/APK instalável nem validação em aparelho físico.
- **GPU:** o roadmap marca pendências em decodificação tipada de instruções/operand descriptors de vertex shader, integração completa de fetch e draw, fragment pipeline, texturas, blending, stencil, profundidade persistente e validação independente de registradores.

A documentação de estado deve ser lida junto com [ROADMAP.md](ROADMAP.md), [REAL_HOMEBREW_TESTING.md](REAL_HOMEBREW_TESTING.md), [THREEDSX_ARM11_INTEGRATION.md](THREEDSX_ARM11_INTEGRATION.md) e [FRONTENDS.md](FRONTENDS.md).

## 8. Ordem inicial de trabalho orientada por evidências

1. **Executar o teste manual de homebrew já existente no ambiente que possui o arquivo fornecido legalmente.** Registrar PC, CPSR, instruções executadas, motivo de parada, SVCs/serviços observados, listas GSP e atualizações LCD. Não tratar a validação estrutural como boot aprovado.
2. **Formalizar a especificação do vertex shader PICA200.** Criar tabelas próprias de campos, formatos e operand descriptors a partir de documentação independente; separar o que está documentado do que é hipótese.
3. **Criar testes próprios do decoder.** Cobrir campos, valores-limite, codificações reservadas e rejeição explícita de formatos desconhecidos antes de habilitar novas famílias de instrução.
4. **Fechar DRAWARRAYS/DRAWELEMENTS e a origem guest dos dados.** Comprovar a proveniência guest de registradores, bases físicas, índices, vértices e estado do render target. Evitar fixture host-side para qualquer estado que o título real deveria escrever.
5. **Ligar o resultado ao LCD do guest.** Testar persistência/formato/stride do alvo, DisplayTransfer, sincronização e scanout das telas sem injetar metadados finais pelo host.
6. **Usar os logs do primeiro alvo para priorizar o kernel e os serviços faltantes.** Implementar serviço apenas quando um programa de referência demonstrar a necessidade e a documentação definir seu contrato.
7. **Validar PC e Android separadamente.** Build cruzado não substitui janela funcional, APK instalável, superfície real, entrada e ciclo de vida no dispositivo.

A primeira implementação de shader deve permanecer coordenada com o agente responsável pelo subsistema GPU. Este documento não autoriza editar arquivos atribuídos a outro agente.

## 9. Regra de conclusão

Uma funcionalidade só pode ser declarada concluída quando:
- seu contrato está documentado e a procedência das afirmações está registrada;
- os testes próprios cobrem sucesso, falha, limites e regressões relevantes;
- está conectada ao caminho real do guest, sem preparação host-side que esconda a funcionalidade ausente;
- as verificações pertinentes passaram e o workflow correspondente foi confirmado;
- limitações remanescentes estão explícitas;
- a execução em hardware ou dispositivo físico está identificada separadamente da validação sintética.

Nenhuma data estimada ou quantidade de testes substitui esses critérios.


## 10. PICA200 — viewport e mapeamento de profundidade (17/11/2026)

### Pergunta técnica

Quais registradores configuram o retângulo de viewport, a faixa de profundidade e os modos Z/W, e como converter a orientação PICA para superfícies Aceris com origem no canto superior esquerdo?

### Referências documentais independentes

- Mikage, *GPU Internal Registers*: https://docs.mikage.app/GPU/Internal_Registers/ — documentação pública, não oficial, consultada em 17/11/2026. Identifica largura/altura do viewport, inversos, origem signed 10-bit, depth scale/offset e os controles de profundidade. A página lista `0x04C` como sem nome e descreve `0x06D` como habilitação da faixa de profundidade. A licença da documentação não foi identificada nesta consulta.
- 3dbrew, *GPU Internal Registers*: https://www.3dbrew.org/wiki/GPU/Internal_Registers — wiki pública de engenharia reversa, não oficial; consultada em 17/11/2026. Usada como segunda fonte de documentação do mapa de registradores, não como especificação oficial.
- Azahar, repositório https://github.com/azahar-emu/azahar, revisão `11eceda5e0e682a1f44e396bf954a1b9137097ca`: `src/video_core/pica/regs_rasterizer.h` (blob `f0f25fb54e5d6808f480a2bf97bee030cd1241b9`), `src/video_core/pica/regs_internal.h` (blob `0af60a0e556d069f029c9905215cb1df6657061d`) e `src/video_core/renderer_software/sw_rasterizer.cpp` (blob `1e82f87667dda0c6ccc145cb318c789707b5ac23`). Os cabeçalhos dos arquivos declaram GPLv2 ou posterior; o `LICENSE.txt` no mesmo commit apresenta GNU GPLv3. A diferença é registrada como observada, sem conclusão jurídica. Consulta apenas conceitual; nenhum trecho, teste, shader ou estrutura foi copiado/adaptado. Data: 17/11/2026.

### Conclusão independente para o Aceris

- `0x041` e `0x043` armazenam metade da largura/altura do viewport em float24; o tamanho em pixels é duas vezes o valor.
- `0x042` e `0x044` armazenam os recíprocos associados à largura/altura. O Aceris os preserva em bruto para diagnóstico e uso futuro em derivadas/filtragem.
- `0x068` contém origem X/Y signed de 10 bits. A declaração de viewport consultada usa convenção PICA bottom-left/Y-up; o Aceris converte o limite inferior para a superfície top-left/Y-down usando a altura do alvo.
- `0x04D` é a escala de profundidade float24 (Near − Far) e `0x04E` o offset float24 (Near + polygon offset). A transformação usada no caminho de referência é `(z/w) × scale + offset`.
- `0x06D` é descrito pela documentação Mikage como controle de enable da faixa de profundidade. A revisão Azahar consultada representa o bit 0 como enum WBuffering=0/ZBuffering=1 e, no rasterizador software, aplica o fator W interpolado apenas no modo W. O Aceris segue essa interpretação para distinguir Z/W, mas a diferença de terminologia entre as fontes fica registrada como hipótese a confirmar em hardware legítimo.
- `0x04C` não foi implementado: a tabela Mikage o deixa sem nome e o layout Azahar consultado o reserva. Uma tentativa intermediária de tratá-lo como enable independente foi removida após essa verificação adicional.
- A profundidade Z é calculada como `(z/w) × scale + offset`. No modo W, o resultado recebe o W interpolado, equivalente a dividir pelo reciprocal-W interpolado no rasterizador Aceris.

### Limites e nível de confiança

A viewport é sustentada por documentação técnica pública não oficial e pelo layout de registradores estudado. A escala/offset está descrita em documentação; a interpretação Z/W de `0x06D` tem divergência de terminologia e permanece pendente de hardware. Os testes Aceris verificam o contrato implementado, não equivalência física. Ainda faltam comparação em hardware legítimo, precisão/quantização exata, scissor, culling, polygon offset aplicado como estado real, depth/stencil e persistência do depth buffer em memória.

A implementação Aceris foi escrita separadamente em Rust. Não foi copiado, traduzido, portado ou adaptado código-fonte, testes, assets ou shaders dos projetos consultados.


## 11. PICA200 — instruções aritméticas EX2 e LG2 (17/11/2026)

### Pergunta técnica

Quais são os números de opcode e o formato de operandos de EX2/LG2, qual componente é usado como entrada escalar e como o resultado é distribuído pelos lanes de destino?

### Referências consultadas

- 3dbrew, *GPU/Shader Instruction Set*: https://www.3dbrew.org/wiki/GPU/Shader_Instruction_Set — documentação comunitária de engenharia reversa, consultada em 17/11/2026. Descreve EX2 como exponencial base 2 do primeiro componente do SRC1, com resultado replicado nos quatro lanes; LG2 como logaritmo base 2 do primeiro componente, também replicado. A licença da página não foi identificada nesta consulta.
- Azahar, repositório https://github.com/azahar-emu/azahar, revisão `11eceda5e0e682a1f44e396bf954a1b9137097ca`: `src/video_core/shader/shader_jit_x64_compiler.cpp`. A tabela de instruções associa o opcode `0x05` a EX2 e `0x06` a LG2, ambos pelo caminho de instrução unária. O cabeçalho do arquivo declara GPLv2 ou posterior; o `LICENSE.txt` na raiz da mesma revisão contém GPLv3. A diferença entre cabeçalho e licença de raiz é registrada como observada, sem conclusão jurídica. A fonte foi consultada somente para confirmar a tabela de opcodes/formato; nenhum código, teste, fixture, shader ou estrutura foi copiado, traduzido, portado ou adaptado. Data: 17/11/2026.

### Conclusão e implementação independente

- O Aceris reconhece EX2 no opcode `0x05` e LG2 no opcode `0x06`, ambos como formato unário.
- A execução usa o primeiro lane do SRC1 depois do swizzle/negação do operand descriptor e replica o resultado nos quatro componentes de destino; a máscara de destino existente continua controlando quais componentes são escritos.
- A implementação própria em Rust usa `f32::exp2` e `f32::log2`. Isso não declara precisão float24 nem equivalência com as aproximações internas do hardware PICA200.
- Foram adicionados testes de valores positivos e negativos para EX2, logaritmo de potência de dois, swizzle de componente, máscara parcial e os resultados IEEE de `log2(0)` e `log2(negativo)`.
- DST permanece explicitamente não implementada: 3dbrew a lista como opcode `0x04`, mas a tabela JIT consultada em Azahar o deixa como desconhecido. A divergência exige investigação independente antes de habilitar a instrução. LITP também permanece fora do subconjunto porque seus efeitos sobre condition codes não estão implementados.



## 12. PICA200 — DisplayTransfer, layouts tiled e recorte (18/11/2026)

### Pergunta técnica

Quais flags selecionam o layout da entrada/saída do GX DisplayTransfer, quando o bit de crop deve ser informado e quais restrições de dimensão e blocos precisam ser preservadas?

### Referências consultadas

- Mikage, *GPU / External Registers*: https://docs.mikage.app/GPU/External_Registers/ — documentação comunitária de engenharia reversa, não oficial, consultada em 18/11/2026. Descreve os bits 0, 1, 2, 3, 5, 8–10, 12–14, 16 e 24–25 do registrador de flags do Transfer Engine; o bit 1 seleciona entrada linear/saída tiled, o bit 5 seleciona tiled→tiled e é incompatível com bit 1, e o bit 2 é necessário quando a largura de saída é menor que a largura de entrada. A mesma página registra o mínimo 64×16 para tiled→linear, alinhamento de largura dependente do formato, blocos 32×32 e downscale com box filter 2×1/2×2. Licença da documentação não identificada.
- 3dbrew, *GPU / External Registers*: https://www.3dbrew.org/wiki/GPU/External_Registers — wiki pública de engenharia reversa, não oficial, consultada em 18/11/2026. Registra o mesmo mapa de flags e restrições. Como os dados são semelhantes aos da documentação Mikage, esta consulta é corroborativa, mas não é tratada como prova independente de hardware.
- devkitPro/libctru, `libctru/include/3ds/gpu/gx.h`, commit `6ef91576ae20d6bc9f9af261ec5989ffc0c1e0f4` (blob `da32c98bfad0ebdb6267ab33e1eae1fe3a0fe556`): https://github.com/devkitPro/libctru/blob/6ef91576ae20d6bc9f9af261ec5989ffc0c1e0f4/libctru/include/3ds/gpu/gx.h — consultado apenas para compreender os nomes públicos das flags e os modos de escala GX. O README da mesma revisão contém uma licença permissiva no estilo zlib. Não foi usado como especificação de comportamento físico e nenhum código, função, estrutura ou teste foi incorporado.
 
### Contrato adotado no Aceris

- Com bit 1 limpo, a entrada é tiled 8×8 Morton; a saída é linear por padrão.
- Com bit 1 ativo, a entrada é linear e a saída é tiled 8×8 Morton.
- Com bit 5 ativo, a entrada e a saída são tiled 8×8 Morton. A combinação simultânea com bit 1 é rejeitada.
- Quando a largura de saída é menor que a largura de entrada, bit 2 deve estar ativo. A rotina usa dimensões de entrada para endereçar a origem e dimensões de saída para endereçar a superfície de destino, sem assumir pitches idênticos.
- A implementação mantém a política de staging: valida e lê a origem, prepara a saída completa e somente então escreve o destino. Isso preserva o destino em erros de validação/leitura e permite origem/destino sobrepostos.
- Os testes próprios cobrem conversão linear↔tiled, tiled→tiled, flip, crop retangular e ausência de escrita quando falta a flag de crop ou a faixa de origem é inválida.

### Limites e nível de confiança

O trabalho publicado cobre somente blocos 8×8 e execução síncrona no backend software. O modo 32×32, o downscale por box filter, as restrições de alinhamento por formato, a matriz de conversões que produz PPF, os efeitos em combinações atípicas de dimensões e a temporização real precisam de fixtures adicionais e, quando possível, observação em hardware legítimo. A implementação não declara precisão ou compatibilidade física integral.

A alteração do Aceris foi escrita independentemente em Rust. Nenhum código, teste, shader, asset ou dependência de terceiros foi copiado ou adaptado.

## 13. PICA200 — DisplayTransfer e matriz de conclusão PPF (18/11/2026)

### Pergunta técnica

A conclusão do GX DisplayTransfer sempre sinaliza PPF? Quais combinações de formato de origem/destino documentadas produzem a interrupção no modo tiled→linear e tiled→tiled?

### Referências consultadas

- 3dbrew, *GPU/External Registers*: https://www.3dbrew.org/wiki/GPU/External_Registers — tabela de conversões do Transfer Engine para tiled→linear; a seção tiled→tiled declara que usa a mesma tabela. A wiki separa as combinações que geram interrupção e resultado correto das que não geram interrupção. Consulta em 18/11/2026. Fonte comunitária de engenharia reversa, não oficial.
- Mikage, *GPU/External Registers*: https://docs.mikage.app/GPU/External_Registers/ — confirma os bits de modo e as diferenças entre tiled→linear, linear→tiled e tiled→tiled, mas a página consultada não apresenta a mesma tabela detalhada de PPF. Consulta em 18/11/2026. Fonte comunitária não oficial.

### Matriz adotada (tiled→linear e tiled→tiled)

Ordem das colunas: RGBA8, RGB8, RGB565, RGB5A1, RGBA4. `PPF` indica que a fonte documenta interrupção após a transferência.

| Origem \ Destino | RGBA8 | RGB8 | RGB565 | RGB5A1 | RGBA4 |
|---|---:|---:|---:|---:|---:|
| RGBA8 | PPF | PPF | PPF | PPF | PPF |
| RGB8 | — | PPF | — | — | — |
| RGB565 | — | — | PPF | PPF | PPF |
| RGB5A1 | — | — | PPF | PPF | PPF |
| RGBA4 | — | — | PPF | PPF | PPF |

### Implementação Aceris

- `display_transfer_signals_ppf(flags, source_format, destination_format)` codifica a tabela como função pura e pública. Para o modo bit 1 (linear→tiled), preserva temporariamente o comportamento anterior de sinalizar PPF para todos os pares, pois a tabela consultada é descrita explicitamente para tiled→linear e tiled→tiled, não para linear→tiled.
- `System::dispatch_gsp_display_transfer` devolve se o par exige PPF; o dispatcher GX contabiliza a transferência concluída e atualiza a metadata LCD separadamente, e só enfileira PPF quando o helper informa que a combinação dispara a interrupção.
- Teste de tabela-verdade cobre os 25 pares para o modo tiled e um teste separado protege o comportamento atual de linear→tiled.
- O resultado da transferência em memória continua síncrono. A condição PPF não prova latência, ordenação de barramento ou temporização real do hardware.

### Limites

A matriz é documentação não oficial e ainda não foi comparada com hardware físico. Linear→tiled permanece com regra conservadora e precisa de fonte/testes específicos. A integração `gsp_gx_display_transfer_executes_tiled_to_linear_and_reports_completion` agora verifica duas execuções na fila real: RGBA8→RGBA8 enfileira PPF; RGB8→RGBA8 converte os bytes de destino, incrementa a conclusão, mas não acrescenta um segundo PPF. As combinações 32×32/downscale e a temporização real permanecem pendentes.

## 14. PICA200 — tiled→tiled com downscale 2×2 (18/11/2026)

### Pergunta técnica

Como o modo GX tiled→tiled deve gerar níveis menores de textura ou superfícies antialiasing, e quais dimensões/alinhamentos precisam ser validados antes de aceitar a transferência?

### Referências consultadas

- 3dbrew, *GPU/External Registers*: https://www.3dbrew.org/wiki/GPU/External_Registers — descreve tiled→tiled como modo utilizado com downscale 2×2, registra as restrições de saída não maior que a entrada, largura mínima 64, altura mínima 32 e alinhamento em bytes por formato. A mesma seção afirma que a matriz de conversões e a conclusão PPF seguem tiled→linear. Fonte comunitária de engenharia reversa, não oficial, consulta em 18/11/2026.
- Mikage, *GPU/External Registers*: https://docs.mikage.app/GPU/External_Registers/ — documenta bit 5 como tiled→tiled, incompatibilidade com bit 1, bit 16 para blocos 32×32 e bits 24–25 como downscale box-filter 2×1/2×2. Consulta em 18/11/2026. Fonte comunitária não oficial.
- devkitPro/libctru, `libctru/include/3ds/gpu/gx.h`, commit `6ef91576ae20d6bc9f9af261ec5989ffc0c1e0f4`: https://github.com/devkitPro/libctru/blob/6ef91576ae20d6bc9f9af261ec5989ffc0c1e0f4/libctru/include/3ds/gpu/gx.h — enum pública `GX_TRANSFER_SCALE_XY` para 2×2 AA e `GX_TRANSFER_SCALE_X` para 2×1. Usada apenas para nomenclatura API; sem código incorporado.
- Citro3D, `source/texture.c`, versão consultada em 18/11/2026: https://github.com/devkitPro/citro3d/blob/master/source/texture.c — consulta contextual sobre geração de mipmaps e a existência de um caminho CPU separado; não usada como especificação do Transfer Engine. Nenhum código, shader, teste ou estrutura foi copiado, traduzido ou adaptado.

### Contrato implementado

- O modo bit 5 usa entrada e saída em tiles Morton 8×8; a combinação com bit 1 continua recusada.
- O campo de escala deve ser 2 (2×2). O Aceris adota dimensões explícitas: largura e altura da entrada iguais ao dobro das dimensões da saída.
- A saída deve ter largura mínima 64 e altura mínima 32. O alinhamento da linha de saída é de 64 bytes para RGB8/RGBA8 e 128 bytes para RGB565/RGB5A1/RGBA4.
- Cada pixel de saída consome o bloco correspondente de 2×2 pixels de entrada. Os quatro valores RGBA são acumulados em inteiros de 16 bits e divididos por quatro; a saída passa pelo codec do formato de destino.
- A flag crop bit 2 é exigida pelo contrato comum quando a largura da saída é menor que a largura de entrada.
- A origem inteira é staged antes da escrita do destino. Os testes usam cores constantes por bloco 2×2 para verificar o mapeamento sem depender do arredondamento de amostras fracionárias.
- A tabela PPF para tiled→tiled continua a mesma matriz documentada para tiled→linear.

### Limites e hipóteses

A documentação pública não especifica o arredondamento interno do box filter, a precisão intermediária, a ordem de quantização, nem a temporização. O Aceris usa divisão inteira truncada por quatro como modelo determinístico; isso não é declaração de equivalência física. A interpretação de dimensões como entrada = 2× saída é o contrato adotado para uma redução 2×2 explícita, testado sinteticamente, mas ainda não confirmado em hardware legítimo. Downscale 2×1, escala no caminho tiled→linear, blocos 32×32 e combinações 32×32 + scale continuam sem suporte.

A implementação foi escrita independentemente em Rust e não incorpora código, assets, shaders, fixtures ou dependências de terceiros.

## 15. PICA200 — blocos 32×32 no render target e DisplayTransfer (18/11/2026)

### Pergunta técnica

Como o registrador de framebuffer seleciona o modo de blocos 32×32 e qual contrato a transferência exige para ler um framebuffer nesse modo?

### Referências documentais independentes

- 3dbrew, *GPU/Internal Registers*: https://www.3dbrew.org/wiki/GPU/Internal_Registers — descreve `GPUREG_FRAMEBUFFER_BLOCK32` (`0x11B`), bit 0: 0 = blocos 8×8; 1 = blocos 32×32. A própria documentação indica que, para untile do color buffer nesse modo, deve-se usar o bit 16 das flags de DisplayTransfer; também registra que as vantagens do modo não são conhecidas. Fonte comunitária de engenharia reversa, não oficial, consulta em 18/11/2026.
- 3dbrew, *GPU/External Registers*: https://www.3dbrew.org/wiki/GPU/External_Registers — bit 16 seleciona blocos 32×32 em vez dos 8×8 habituais e as dimensões efetivas de saída devem ser múltiplas de 32, mesmo com crop bit 2. Fonte comunitária de engenharia reversa, não oficial, consulta em 18/11/2026.
- Mikage, *Internal Registers*: https://docs.mikage.app/GPU/Internal_Registers/ — confirma a seleção 8×8/32×32 e a necessidade do bit 16 do DisplayTransfer para untile. A documentação não descreve a ordem interna dos pixels do bloco 32×32. Fonte comunitária não oficial, consulta em 18/11/2026.
- Mikage, *External Registers*: https://docs.mikage.app/GPU/External_Registers/ — confirma o bit 16 e o requisito de dimensões de saída múltiplas de 32, inclusive com crop. Fonte comunitária não oficial, consulta em 18/11/2026.

### Modelo implementado no Aceris

- `PicaRenderTargetConfig` escolhe tile edge 8 ou 32 a partir de `GPUREG_FRAMEBUFFER_BLOCK32`.
- O armazenamento usa uma grade row-major de blocos quadrados; dentro de cada bloco, os bits X/Y são intercalados em ordem Morton/Z. Para 8×8, são 3 bits por eixo; para 32×32, são 5 bits por eixo.
- `DisplayTransferLayout` agora distingue `Tiled8x8Morton` e `Tiled32x32Morton`. O bit 16 seleciona a mesma organização para o lado tiled da transferência: entrada em tiled→linear, saída em linear→tiled e ambos os lados em tiled→tiled.
- `display_transfer_buffer_sizes` calcula os spans padded pelo tamanho do tile escolhido; `execute_display_transfer` usa a mesma função de offset para leitura e gravação, preservando o staging e a validação de intervalos.
- A validação exige que as dimensões físicas efetivas de saída sejam múltiplas de 32 no modo bit 16.
- Testes próprios cobrem bijeção Morton 32×32, round-trip rasterizer→memória física→RGBA8, round-trip linear→tiled→linear e rejeição de dimensões de saída que não são múltiplas de 32.
- A integração `gsp_gx_display_transfer_executes_tiled_to_linear_and_reports_completion` adiciona uma terceira chamada pela fila GX, com bit 16, e verifica pixels no framebuffer linear e a terceira notificação PPF.

### Limites e confiança

A documentação identifica a seleção do bloco e a flag de transferência, mas não especifica explicitamente a ordem interna dos pixels dentro de um bloco 32×32. O Aceris generaliza a organização Morton usada em 8×8 para cinco bits por eixo, mantendo tiles em ordem row-major. Isso é uma hipótese explícita e testável, não uma afirmação de equivalência física. Não houve comparação com framebuffer capturado de hardware legítimo. Também permanecem pendentes o sentido exato do bit de flip do render target, o arredondamento físico do box filter e a temporização assíncrona.
## 16. GSP/GX TextureCopy — pitches independentes de origem e destino (18/11/2026)

### Pergunta técnica

Como o GX TextureCopy interpreta largura de linha, gap e tamanho total quando a superfície de origem e a de destino possuem pitches diferentes?

### Referências consultadas

- 3dbrew, *GPU/External Registers — TextureCopy*: https://www.3dbrew.org/wiki/GPU/External_Registers — descreve TextureCopy como cópia raw sem conversão de formato. Os parâmetros de entrada e saída definem largura de linha e gap; o registrador de tamanho define o total de bytes efetivamente copiados, sem contar os gaps. A documentação também informa que a engine lê e escreve linhas até consumir esse total. Fonte comunitária de engenharia reversa, não oficial, consulta em 18/11/2026.
- Mikage, *GPU/External Registers — TextureCopy*: https://docs.mikage.app/GPU/External_Registers/ — documenta larguras/gaps de entrada e saída separadas, cada campo em unidades de 16 bytes, e exemplifica recorte de retângulos entre surfaces com pitches diferentes. Afirma que gaps não entram no tamanho total copiado. Fonte comunitária não oficial, consulta em 18/11/2026.
- 3dbrew, *GSP Shared Memory — TextureCopy*: https://www.3dbrew.org/wiki/GSP_Shared_Memory — descreve os campos da entrada GX: source, destination, total size, parâmetros independentes e flags. Fonte comunitária de engenharia reversa, não oficial, consulta em 18/11/2026.
- Mikage, *GSPGPU:SetTextureCopy*: https://docs.mikage.app/GSPGPUSetTextureCopy/ — confirma o contrato IPC: endereço de origem/destino, tamanho, parâmetros separados e bit 2/bit 3. Fonte comunitária não oficial, consulta em 18/11/2026.

### Interpretação implementada no Aceris

- Em gap mode (bit 2), cada parâmetro de lado é decodificado como `line_width = low16 * 16` e `gap = high16 * 16`. Bit 3 habilita TextureCopy raw.
- O tamanho é interpretado como um fluxo lógico contínuo de bytes copiados. A origem é consumida em blocos de até `source_line_width`; após cada bloco, o cursor avança por `source_line_width + source_gap`. O destino recebe o mesmo fluxo, mas segmentado em blocos de até `destination_line_width`, avançando por `destination_line_width + destination_gap`.
- Assim, o número de linhas de leitura é `ceil(size/source_line_width)` e o número de linhas de escrita é `ceil(size/destination_line_width)`. A última linha de cada lado pode ser parcial; gaps nunca contam para o tamanho total.
- Antes de ler ou escrever, o backend valida separadamente cada span de origem e cada span de destino. Depois faz staging de todo o fluxo de origem e só então grava o destino, mantendo comportamento determinístico em caso de sobreposição.
- `TextureCopyReport` agora informa `source_rows_transferred` e `destination_rows_transferred`, além de bytes transferidos e uso de gaps.

### Cobertura de testes

- `gap_mode_copies_rows_and_skips_independent_source_and_destination_gaps`: mesmo pitch com gaps distintos.
- `gap_mode_copies_different_source_and_destination_line_widths`: origem 32 bytes + gap16, destino16 + gap16; 192 bytes resultam em 6 linhas lidas e 12 gravadas.
- `gap_mode_copies_different_line_widths_with_partial_final_rows`: origem16 + gap16, destino32 + gap16; 200 bytes resultam em 13 linhas lidas e 7 gravadas, com última linha parcial em ambos os lados.
- `gsp_gx_texture_copy_copies_gapped_rows_and_signals_ppf`: envia a configuração assimétrica pela GX queue e verifica os bytes e gaps de destino, além do evento PPF.
- Os testes anteriores de transferência contígua, preservação do destino em faixa inválida, aliases VRAM e regiões sobrepostas continuam no módulo.

### Limites e confiança

A interpretação é sustentada pelas descrições públicas de parâmetros separados e de cópia até o total de bytes, mas ainda não foi comparada a um console físico. A execução do Aceris continua síncrona; a temporização do Transfer Engine, `fail-if-busy`, arbitragem GSP e comportamento em parâmetros que travam o hardware permanecem fora deste modelo. O limite defensivo de 64 MiB é política do emulador, não afirmação sobre o hardware.

## 17. Estado do projeto — 22/11/2026

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.

A última CI verificada é o workflow 2244 (run 44968347424): jobs Linux, Windows, macOS e Android ARM64 concluídos com sucesso. A validação real ainda é limitada ao homebrew Cookie Clicker, que chega à própria tela de erro por falha na inicialização do módulo Sound. Nenhuma conclusão transforma serviços HLE ou GPU em compatibilidade comprovada de jogos.

## PICA200 procedural texture unit — register evidence (22/11/2026)

- The public register map documents `GPUREG_TEXUNIT_CONFIG` bit 10 as the procedural unit enable and bits 8–9 as its coordinate-set selector. `PROCTEX0..5` configure wrapping, RGB/alpha mapping, noise parameters, LOD/width and mip offsets; `PROCTEX_LUT` selects one of five LUT types and a starting index, and `PROCTEX_LUT_DATA0..7` are FIFO aliases.
- The tables documented by the register map are 128-entry noise/RGB-map/alpha-map tables and 256-entry color/color-difference tables. The implementation in this step models only their guest-side FIFO storage and index advancement; it does not claim correct texel generation or sampling.
- Public references: [PICA200 internal register map](https://docs.mikage.app/GPU/Internal_Registers/) and [procedural texture pipeline overview](https://docs.mikage.app/GPU/Procedural_Texture_Generation/). The latter explicitly notes that little is known about the noise module; the observed four-stage model is Noise → Repeat → Base Shape G(u,v) → F(g)/color lookup. No other emulator source code was used.

### Typed decode follow-up (22/11/2026)

- Decoded register fields mirror the published PROCTEX0..5 table: clamp modes, RGB/alpha mapping selectors, separate-alpha/noise flags, coordinate shifts, raw noise amplitude/phase/frequency, minification/mipmap filter, min/max LOD, width and mip offsets. Float/fixed encodings are retained raw.
- LUT helpers decode the documented fixed0.0.12 value plus signed 12-bit difference, RGBA8 color entries and signed 8-bit per-channel color differences. These helpers are data decoding only, not proof of the full sampler algorithm.
- Evidence: [PICA internal register documentation](https://docs.mikage.app/GPU/Internal_Registers/), register descriptions and LUT formats. The function sequence and remaining uncertainty are separately described in [procedural texture generation overview](https://docs.mikage.app/GPU/Procedural_Texture_Generation/) and the DMP CEDEC 2012 report.

### Procedural LUT interpolation evidence (22/11/2026)

- The public register map identifies a 12-bit unsigned value and a 12-bit two's-complement difference for noise/RGB/alpha LUT entries. The independent Citra/Azahar software references describe that signed difference as approximately [-0.5, 0.5) and normalize both fields by 4095; Aceris now uses the same normalization rather than interpreting the difference as [-1, 1).
- The implementation interpolates a selected curve entry as `value/4095 + signed_delta/4095 * fraction`, and a color channel as `current + 2*signed_half_delta*fraction`. It returns floating-point channels without applying unverified rounding/saturation.
- Cross-check (paraphrased; no source code copied): Azahar `src/video_core/pica/pica_core.h` and `src/video_core/renderer_software/sw_proctex.cpp` at commit `fd4c9abd98345dbefff9cef07aadaf61ee1f683d`; Citra `src/video_core/pica_state.h` and `src/video_core/swrasterizer/proctex.cpp` at commit `302b0c3bc4312bc96489c11584a97664894b343f`. Both define the signed delta's half-range and divide it by 4095. This is a software-reference cross-check, not a claim of physical-hardware verification.
- These formulas cover interpolation after an entry/index has already been selected. They do not assert hardware rounding, the complete coordinate-to-segment mapping, or the noise generator.

### G(u,v) equation helper (22/11/2026)

- Public register docs enumerate U, U², V, V², (U+V)/2, (U²+V²)/2, sqrt(U²+V²), min(U,V), max(U,V) and Rmax mapping selectors. The CEDEC 2012 DMP presentation summary says G produces basic shapes such as horizontal/vertical lines, circles and squares.
- The code implements the nine explicit equations as a pure raw helper, with no coordinate-domain assumptions, noise/wrapping, F(g), or clamping. Rmax remains unsupported because a reliable equation is not published in the consulted sources.
- Evidence: [PICA200 register map](https://docs.mikage.app/GPU/Internal_Registers/) and [4Gamer's CEDEC 2012 report, based on DMP's session](https://www.4gamer.net/games/017/G001762/20120822007/).

### F(g) helper and confidence boundary (22/11/2026)

- The procedural-texture overview documents two known F options: identity and triangle. The CEDEC 2012 report explains that a triangle-like F makes colors traverse the lookup table and repeat in reverse, but neither source publishes its exact equation.
- The isolated `Triangle` helper uses the conventional tent wave `1 - abs(2*g - 1)` for g in [0,1]. This is an explicit mathematical interpretation, not a claim about verified PICA200 output. It is not connected to texture sampling until a hardware/reference comparison can confirm it.
- Evidence: [3dbrew procedural texture pipeline](https://docs.mikage.app/GPU/Procedural_Texture_Generation/) and [4Gamer's CEDEC 2012 report](https://www.4gamer.net/games/017/G001762/20120822007/).


### Validação numérica do helper G(u,v) (22/11/2026)

- O contrato de evaluate_raw aceita coordenadas finitas e devolve a avaliação matemática não normalizada. Mesmo sem entradas NaN/inf, operações como U², (U+V)/2, (U²+V²)/2 e sqrt(U²+V²) podem transbordar para valores não finitos em f32.
- O helper foi endurecido para retornar None quando o resultado calculado não é finito. Testes de regressão cobrem os quatro caminhos de overflow e preservam um resultado finito fora de [0,1] para confirmar que não foi introduzido clamp implícito.
- As fontes consultadas continuam limitadas às equações G publicadas. A documentação de registradores lista Rmax e cinco modos de clamp, mas não define as equações para Rmax/Pulse nem as regras completas de wrapping; esses modos não foram implementados por aproximação.

### Procedural texture coordinate routing (22/11/2026)

- The public GPUREG_TEXUNIT_CONFIG register table defines bits 8–9 as the coordinate source for texture unit 3: selector 0 routes Texture 0 coordinates, selector 1 routes Texture 1 coordinates, and selector 2 routes Texture 2 coordinates. Selector 3 is not listed as valid.
- Aceris now decodes this selector into PicaProceduralTextureCoordinateSource and preserves selector 3 as Reserved(3). Tests cover all four encodings. No coordinate transformation or fragment sampling is implied by the snapshot.
- Evidence: [3dbrew PICA200 internal register map](https://3dbrew.org/wiki/GPU/Internal_Registers) and [Mikage PICA200 internal register map](https://docs.mikage.app/GPU/Internal_Registers/).

### Procedural coordinate-pair routing helper (22/11/2026)

- `PicaProceduralTextureConfig::select_coordinates` resolves the decoded Texture0/1/2 source into one raw `[u, v]` pair from the three interpolated texture-coordinate pairs.
- The helper returns that pair unchanged. Out-of-range finite values are not clamped or normalized, and a reserved selector returns `None`. Synthetic tests use distinct pairs and non-unit values to protect this raw routing contract.
- This does not implement coordinate shifts, noise, wrapping, G/F evaluation, procedural LUT sampling, mip selection/filtering, or fragment-pipeline integration.

### PROCTEX0 mapping-selector coverage and source discrepancy (22/11/2026)

- A regression test now covers selector values 0–15 independently in the RGB and alpha nibbles, including the reserved encodings, and toggles the separate-alpha bit independently.
- The register map explicitly lists selectors 0–9: nine equations plus Rmax. The procedural-pipeline overview separately describes G as having “one of six functions”. These descriptions are not reconciled by the public material consulted here; the implementation follows the explicit register table and leaves Rmax unsupported.
- The test validates register decoding only. It does not establish how separate-alpha affects later consumption, choose LUT indices, generate noise, or validate texels on hardware.
- Evidence: [PICA200 internal register map](https://docs.mikage.app/GPU/Internal_Registers/) and [procedural texture generation overview](https://docs.mikage.app/GPU/Procedural_Texture_Generation/).
## Atualização PROCTEX: Rmax, ruído de referência e otimizações (25/11/2026)

### Rmax e saturação

- O código aberto do Azahar em `src/video_core/renderer_software/sw_proctex.cpp` define `RMax` como `((u + v) * 0.5 + sqrt(u*u + v*v)) * 0.5`, com saturação em 1.0; a distância radial também satura em 1.0 antes da LUT. O Aceris reimplementa as fórmulas em Rust original e separa `evaluate_raw` de `evaluate_for_lookup`.
- O caminho de software consultado usa o combinador G selecionado para chamar diretamente a LUT de mapeamento. Não há uma etapa F(g) independente demonstrada por esse código. O helper triangular que existe no Aceris permanece isolado, sem conexão com o sampler, até que a seleção/semântica real seja confirmada.

### Ruído procedural — modelo de referência de software

- A implementação consultada em Azahar (`fd4c9abd98345dbefff9cef07aadaf61ee1f683d`, `src/video_core/renderer_software/sw_proctex.cpp`) decodifica fase e frequência como binary16; usa amplitude fixa assinada dividida por 4095; gera hashes 1D/2D para os quatro cantos da célula; usa a LUT de ruído para os pesos dos eixos; e combina os quatro gradientes por interpolação bilinear.
- A própria fonte comenta que os resultados do ruído foram verificados contra hardware, mas que não se sabe se o algoritmo é o mesmo utilizado internamente pelo hardware. Portanto, o Aceris chama isso de modelo de compatibilidade de software, não de reprodução física bit a bit. Nenhum código C++ foi copiado: o cálculo foi reescrito em Rust.
- A ordem descrita pela referência é importante: calcula os offsets de shift antes de gerar ruído, soma ruído com amplitudes U/V separadas, aplica `abs`, soma shifts e só então executa o clamp. O sampler do Aceris mantém essa ordem.
- O Aceris pré-decodifica frequência/fase binary16 e amplitudes uma vez por estado de desenho; captura a LUT de ruído apenas quando habilitada. Para cada fragmento, calcula os hashes de `x`, `x+1`, `y`, `y+1` uma vez e compartilha os quatro resultados pelos quatro gradientes. Isso reduz hashes redundantes sem alterar a fórmula. Não foi publicada uma medição quantitativa do ganho; os benchmarks CI apenas constroem os exemplos release.
- Testes no Aceris: decodificação binary16 e amplitude assinada; coeficiente determinístico; influência do ruído no texel via sampler; rejeição de fase/frequência não finitas na captura; igualdade do gradiente com os hashes pré-computados em 576 pontos de grade.

### Limites e validação multiplataforma

- Mipmap e bias não nulo continuam explicitamente não suportados; seletores reservados e parâmetros não finitos retornam erro. F(g) triangular não está conectado ao sampler. A unidade 3 está integrada à entrada Texture3 do TEV, mas isso não equivale a suporte completo a PROCTEX.
- Os workflows `45530579072` e `45531640771` passaram em Linux, Windows, macOS e Android ARM64, incluindo formatação, build, Clippy, testes e benchmarks. Esses resultados validam a compilação/testes sintéticos, não a equivalência com console real nem a compatibilidade de um jogo específico.
- Próxima validação necessária: coletar uma captura de um jogo/homebrew que use PROCTEX, comparar o comportamento do sampler no contexto do draw real e medir o impacto com benchmark de rasterização representativo.


## Azahar PICA200 — tabelas de seleção e wrapping (26/11/2026)

### Auditoria de licença e revisão consultada

- Repositório consultado: [azahar-emu/azahar](https://github.com/azahar-emu/azahar), branch `master`, commit `c621eafbf316699c55f4c9c0c689d4e49498d360` (25/11/2026). A licença geral do repositório nesse commit é GPL-3.0.
- Arquivos PICA consultados e cabeçalhos individuais:
  - `src/video_core/pica/regs_texturing.h` — GPLv2-or-later.
  - `src/video_core/renderer_software/sw_texturing.cpp` — GPLv2-or-later.
  - `src/video_core/renderer_software/sw_rasterizer.cpp` — GPLv2-or-later.
  - `src/video_core/pica/regs_lighting.h` — GPLv2-or-later.
- Os arquivos foram lidos para identificar seletores, valores reservados e efeitos observáveis dos registradores. Nenhuma implementação, tabela de código, teste, shader, fixture ou asset foi copiado, traduzido, portado ou reproduzido estruturalmente. Os tipos Rust, as funções de consulta, o tratamento de erros e os testes de Aceris foram escritos de forma independente.
- A página pública 3dbrew para registradores internos documenta os modos de wrap 0–3; os modos 4–7 são tratados aqui como evidência de referência de software (E2), não como confirmação física de hardware. Não inferir suporte de hardware apenas porque Azahar os implementa.

### Seletores de tipo de textura

| Valor | Tipo | Estado no Aceris |
|---:|---|---|
| 0 | 2D | Aceito |
| 1 | Cube | Explicitamente não suportado |
| 2 | Shadow 2D | Explicitamente não suportado |
| 3 | Projection 2D | Aceito pelo decoder atual |
| 4 | Shadow Cube | Explicitamente não suportado |
| 5 | Disabled | Rejeitado se a unidade estiver marcada como habilitada |
| 6–7 | Reservado | Preservado como seletor reservado |

O tipo é interpretado apenas para a unidade 0; as unidades 1 e 2 não reutilizam esse campo.

### Seletores de wrapping S/T

| Valor | Modo | Comportamento modelado |
|---:|---|---|
| 0 | ClampToEdge | Limita coordenadas ao intervalo e aos texels da borda |
| 1 | ClampToBorder | Coordenadas negativas ou além da dimensão usam a cor de borda |
| 2 | Repeat | Repetição periódica |
| 3 | MirroredRepeat | Repetição com espelhamento |
| 4 | ClampToEdge2 | Coordenada negativa repete; coordenada não negativa limita à borda |
| 5 | ClampToBorder2 | Coordenada negativa repete; coordenada positiva fora da textura usa a cor de borda |
| 6 | Repeat2 | Alias de repetição |
| 7 | Repeat3 | Alias de repetição |

Amostragem bilinear preserva os taps externos nos modos de borda para que contribuam com a border color. O valor exatamente igual a 1.0 é tratado como borda nos modos 1/5 e como último texel nos modos clamp-to-edge.

### Configurações de iluminação

O campo de configuração aceita os códigos 0–6 e 8; o código 7 não é usado. A tabela abaixo descreve quais samplers podem participar de cada configuração, sem afirmar que as fórmulas de iluminação estejam integralmente validadas em hardware.

| Configuração | D0 | D1 | SP | FR | RR | RG | RB |
|---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| 0 | Sim | Não | Sim | Não | Sim | Não | Não |
| 1 | Não | Não | Sim | Sim | Sim | Não | Não |
| 2 | Sim | Sim | Não | Não | Sim | Não | Não |
| 3 | Sim | Sim | Não | Sim | Não | Não | Não |
| 4 | Sim | Sim | Sim | Não | Sim | Sim | Sim |
| 5 | Sim | Não | Sim | Sim | Sim | Sim | Sim |
| 6 | Sim | Sim | Sim | Sim | Sim | Não | Não |
| 8 | Sim | Sim | Sim | Sim | Sim | Sim | Sim |

O banco contém 24 LUTs: D0 (0), D1 (1), FR (3), RB (4), RG (5), RR (6), oito tabelas de spotlight (8–15) e oito de distance attenuation (16–23). Os seletores 2 e 7 não são atribuídos a esses samplers. O Aceris já armazena as 24 tabelas; esta etapa torna os códigos de configuração e a matriz de disponibilidade explícitos e testáveis.

### Implementação e limites

- Aceris: `crates/aceris-core/src/gpu/texture.rs` e `crates/aceris-core/src/gpu/lighting.rs`.
- Testes novos verificam seletores de tipo e wrapping, modos assimétricos, aliases 6/7, borda superior e matriz de configuração de iluminação.
- Cube maps, shadow maps completos, geometry shader, gas, clipping e depth offset continuam com limites explícitos. Reconhecer um seletor não significa simular a funcionalidade inteira.
- Confiança: os modos 0–3 também aparecem na documentação independente consultada; 4–7 têm correspondência com a referência de software, mas ainda precisam de comparação com hardware físico.


## Azahar — operação de fragmento e formatos de framebuffer (26/11/2026)

- Arquivos adicionais consultados em `c621eafbf316699c55f4c9c0c689d4e49498d360`: `src/video_core/pica/regs_framebuffer.h` (GPLv2-or-later), `src/video_core/renderer_software/sw_framebuffer.cpp` (GPLv2-or-later), `src/common/color.h` (GPLv2-or-later). A documentação independente consultada foi 3dbrew, páginas GPU/Internal Registers e GPU/External Registers.
- `PicaFragmentOperation::from_register` agora nomeia os seletores Default (0), Gas (1), Reserved (2) e Shadow (3). O dispatcher do Aceris continua executando apenas Default e retorna erro explícito para os outros modos; não houve simulação falsa de Gas/Shadow.
- Blend equation 0–4 permanece mapeada para Add/Subtract/ReverseSubtract/Minimum/Maximum. Os seletores 5–7 foram mantidos como aliases observados de Add conforme a descrição independente 3dbrew; o enum de referência Azahar lista só 0–4, portanto a diferença está registrada como evidência observacional.
- RGB8 aparece no enum e na implementação de framebuffer do Azahar, mas a descrição independente de GPUREG_COLORBUFFER_FORMAT consultada não atribui RGB8 a um valor válido de pixel-size/formato interno. O Aceris mantém rejeição de combinações não confirmadas em vez de inventar o valor de pixel-size. O RGB8 do controlador externo de display é um contrato separado.
- Nenhum código ou tabela de implementação do Azahar foi copiado; foram criados enum e testes Rust originais para o seletor de modo de fragmento.


### Filtros da unidade procedural e limite de mipmapping (26/11/2026)

- Azahar `src/video_core/pica/regs_texturing.h` define seis seletores PROCTEX4 (nearest, linear e quatro combinações com mipmap). O arquivo de implementação `src/video_core/renderer_software/sw_proctex.cpp` (GPLv2-or-later) contém comentário TODO para mipmapping e agrupa esses modos pelas variantes nearest/linear na LUT de cor do nível base.
- Aceris agora expõe `PicaProceduralTextureFilter::from_register`, com cobertura dos seis valores válidos e dos seletores reservados 6–7. A amostragem aplica nearest/linear intranível para todos os filtros reconhecidos, compatível com o fallback observado no renderer software.
- Limite explícito: a seleção de mip level, o cálculo de LOD, `min_lod/max_lod`, offsets 1–3 e bias superior continuam pendentes. Não anunciar suporte de mipmapping procedural completo. Essa etapa não implementa um comportamento que a referência consultada também marca como TODO.
- Nenhum código-fonte, tabela de implementação ou teste foi copiado; os seletores foram modelados em Rust e cobertos com fixtures próprios.


### SVC — identificadores de sessão auditados (26/11/2026)

- A tabela ARM11 do Aceris agora nomeia `CreateAddressArbiter` (0x21), `ArbitrateAddress` (0x22), `GetSystemInfo` (0x2A) e os três serviços de data cache (0x52–0x54), que antes apareciam como literais no dispatcher.
- Os handlers já existiam e foram mantidos; os testes confirmam os valores numéricos e que esses serviços são encaminhados ao dispatcher de sessão, não tratados pelo dispatcher sem estado.
- A tabela pública 3dbrew lista os mesmos IDs e ABI de alto nível. Valores `GetSystemInfo` não modelados, tipos de address-arbiter não suportados e detalhes físicos de manutenção de cache continuam sendo limites; não responder com valores inventados.


## PICA200 — tabelas restantes de fog e framebuffer (26/11/2026)

- A revisão da tabela GPUREG_TEXENV_UPDATE_BUFFER confirmou os seletores 0 (disabled), 1 (linear fog), 5 (fog via LUT) e 7 (Gas), além dos demais valores reservados. A referência Azahar regs_texturing.h nessa revisão nomeia explicitamente 0, 5 e 7; 3dbrew também documenta o modo 1 e as duas registradoras de fog linear. O Aceris preserva os modos 0/1/5 já implementados e agora expõe PicaFogMode para todos os oito valores do campo. Gas (7) é reconhecido, mas continua fail-closed na avaliação de fragmentos.
- O Azahar regs_framebuffer.h inclui RGB8 como seletor 1 do enum de formato. A descrição independente de GPUREG_COLORBUFFER_FORMAT, porém, lista pixel-size 0/2 e formatos 0, 2, 3 e 4, sem uma combinação interna para RGB8. O Aceris agora tem PicaColorBufferFormatSelector para nomear 0–4 e reservar 5–7; RGB8 é reportado como conhecido, mas continua sem suporte de framebuffer até haver evidência do par pixel-size/formato.
- Testes novos cobrem todos os oito seletores de fog e todos os oito seletores do formato do framebuffer, incluindo rejeição explícita de RGB8 nas combinações atuais.
- Fontes: Azahar src/video_core/pica/regs_texturing.h e regs_framebuffer.h, revisão c621eafbf316699c55f4c9c0c689d4e49498d360 (cabeçalhos individuais GPLv2-or-later); 3dbrew, GPU/Internal Registers, consulta de 26/11/2026. Foram usados valores de campos documentados, sem copiar implementação, testes ou estrutura do código GPL.
- Limite: completar a tabela de seletores não implementa o processamento Gas nem habilita RGB8. Esses comportamentos continuam tarefas separadas, com contratos independentes e validação própria.


## PICA200 — parâmetros do estágio Shadow (26/11/2026)

- Foi identificada a posição de GPUREG_FRAGOP_SHADOW como registrador 0x130: o bloco de framebuffer ocupa 0x100–0x11F, os registradores de Gas ocupam 0x120–0x12F e o parâmetro Shadow vem em seguida. A estrutura pública do Azahar posiciona o mesmo campo após os 16 registradores reservados para Gas.
- A documentação independente 3dbrew descreve bits 0–15 como soma de penumbra scale+bias e bits 16–31 como penumbra scale com sinal invertido; ambos são float1.5.10. O Aceris agora captura as duas metades em PicaShadowState, sem converter o formato fixo/flutuante sem validar a regra de arredondamento.
- Teste próprio usa padrão assimétrico 0xBEEF1234 para verificar extração low/high de 16 bits.
- Limite: a operação de fragmento Shadow (GPUREG_COLOR_OPERATION=3), comparação de shadow map e aplicação de penumbra ainda não estão implementadas. Apenas o registrador de parâmetros é capturado.
- Referência de posição/layout: Azahar src/video_core/pica/regs_framebuffer.h, commit c621eafbf316699c55f4c9c0c689d4e49498d360 (GPLv2-or-later); sem incorporação de código ou estrutura de implementação. Semântica dos bits: 3dbrew GPU/Internal Registers, consultado em 26/11/2026.


## PICA200 — captura do plano de clipping (26/11/2026)

- Adicionados os registradores GPUREG_FRAGOP_CLIP (0x047) e GPUREG_FRAGOP_CLIP_DATA0..3 (0x048–0x04B), conforme a tabela pública 3dbrew GPU/Internal Registers.
- PicaClipPlaneState guarda bit 0 de habilitação e quatro coeficientes float1.7.16 em formato bruto, mascarados para 24 bits. O teste cobre a habilitação e cada uma das quatro palavras com valores diferentes.
- Limite: não há ainda avaliação da equação de clipping durante a rasterização. A captura tipada não é anunciada como suporte funcional do clipping.
- Fonte: 3dbrew, GPU/Internal Registers, sessão de 26/11/2026; mapa de registradores de hardware, sem portar código do Azahar.
