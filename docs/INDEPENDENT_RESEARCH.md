# Pesquisa técnica independente

> Revisão: 17/11/2026. Este documento define como estudar documentação, testes e código-fonte público de outros emuladores como referência técnica, mantendo toda implementação do Aceris original e escrita em Rust.

## 1. Objetivo

Acelerar o Aceris usando especificações públicas, documentação de hardware, descrições independentes de formatos e protocolos, testes próprios e comportamento observável. O resultado de cada pesquisa deve ser uma especificação verificável que oriente uma implementação original em Rust.

A pesquisa não é um atalho para incorporar implementações existentes. Código de outros emuladores pode ser consultado para compreender abordagens e casos-limite, mas o resultado precisa ser convertido em especificação independente, validado por evidências e implementado do zero no Aceris.

## 2. Limites obrigatórios

- É permitido consultar código-fonte publicamente disponível de Azahar, Citra, Lime3DS e outros emuladores como referência técnica.
- A consulta é somente para estudo: não copiar, colar, traduzir, portar, adaptar ou reproduzir trechos, funções, estruturas, testes, fixtures, shaders, patches, assets ou organização interna.
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
2. **Localizar fontes:** priorizar manuais oficiais, documentação de arquitetura e especificações públicas; código-fonte público de emuladores também pode ser consultado como referência técnica, com licença e revisão registradas. Wikis de engenharia reversa e implementações de terceiros devem ser identificadas como fontes não oficiais.
3. **Registrar evidências:** anotar a afirmação, a URL, o tipo de fonte, a data de consulta, o nível de confiança e eventuais divergências.
4. **Escrever o contrato:** descrever entradas, saídas, estados, erros, efeitos colaterais, ordenação e limites de memória em termos independentes, sem reproduzir trechos nem a estrutura expressiva da implementação consultada.
5. **Derivar testes próprios:** construir casos pequenos e determinísticos diretamente do contrato. Para cada caso, registrar os bytes/valores de entrada e o resultado esperado.
6. **Implementar separadamente:** escrever o código Aceris em Rust de forma independente. A consulta técnica não autoriza copiar, traduzir, portar, adaptar ou reproduzir a organização interna do código estudado.
7. **Validar em camadas:** testes unitários, integração no caminho real do guest, CI multiplataforma e, quando possível, comparação com comportamento em hardware legítimo.
8. **Atualizar o estado:** marcar cada afirmação como documentada, inferida, testada sinteticamente ou confirmada externamente. Uma etapa não se torna compatível apenas por compilar.

## 3.1 Registro de referências a código-fonte

Quando uma pesquisa consultar o código de outro emulador, acrescente à ficha:

- Nome do projeto e URL do repositório.
- Arquivo(s) ou área(s) consultada(s), sem copiar trechos para a documentação do Aceris.
- Commit, tag ou revisão exata.
- Licença identificada e eventual obrigação relevante.
- Pergunta técnica investigada e resumo conceitual escrito em linguagem própria.
- Documentação independente ou testes Aceris usados para confirmar ou contestar a conclusão.

O registro de consulta não é uma autorização para incorporar código, assets, testes ou dependências. Se não for possível separar o aprendizado técnico de uma reprodução protegível da implementação, não use aquela referência para orientar a funcionalidade sem decisão explícita do mantenedor e revisão de licença.

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