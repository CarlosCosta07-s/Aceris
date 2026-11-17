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
