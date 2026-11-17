# Pesquisa técnica independente

> Revisão inicial: 17/11/2026. Este documento define como aproveitar conhecimento público sobre o Nintendo 3DS sem consultar nem incorporar código-fonte de outros emuladores.

## 1. Objetivo

Acelerar o Aceris usando especificações públicas, documentação de hardware, descrições independentes de formatos e protocolos, testes próprios e comportamento observável. O resultado de cada pesquisa deve ser uma especificação verificável que oriente uma implementação original em Rust.

A pesquisa não é um atalho para copiar uma implementação existente. Ela serve para reduzir incertezas sobre o comportamento esperado do hardware e transformar essas informações em testes independentes.

## 2. Limites obrigatórios

- Não abrir, pesquisar, baixar, indexar, citar ou utilizar o código-fonte interno de Azahar, Citra, Lime3DS ou qualquer outro emulador.
- Não copiar, traduzir, portar, adaptar ou derivar código, algoritmos, testes, fixtures, shaders, assets, interface ou documentação interna de outro emulador.
- Não usar commits, patches, nomes de funções privadas ou estrutura interna de outro emulador como especificação.
- Usar somente documentação pública independente de hardware, arquitetura, protocolos e formatos; manuais do fabricante; documentação pública de APIs; e resultados de testes próprios.
- Outros emuladores podem ser avaliados apenas por sua superfície observável, conforme `docs/REGRAS_PARA_IA.md`.
- Toda funcionalidade do produto continua sendo implementada em Rust, sem novas dependências de implementação sem autorização do mantenedor.
- Jogos, firmware, chaves, dumps e arquivos protegidos fornecidos para testes permanecem fora do repositório, CI, issues e releases.
- Cada agente respeita os arquivos e subsistemas atribuídos aos demais. Pesquisa documental não concede autorização para editar trabalho de outro agente.

Estas regras complementam `docs/REGRAS_PARA_IA.md` e `CONTRIBUTING.md`; não criam exceções.

## 3. Processo de pesquisa

Cada tarefa técnica deve seguir esta sequência:

1. **Definir a pergunta:** formular um comportamento observável e delimitado, evitando objetivos vagos como “melhorar a GPU”.
2. **Localizar fontes independentes:** priorizar manuais oficiais, documentação de arquitetura e especificações públicas. Wikis de engenharia reversa são úteis, mas suas afirmações devem ser identificadas como não oficiais.
3. **Registrar evidências:** anotar a afirmação, a URL, o tipo de fonte, a data de consulta, o nível de confiança e eventuais divergências.
4. **Escrever o contrato:** descrever entradas, saídas, estados, erros, efeitos colaterais, ordenação e limites de memória sem reproduzir implementação de terceiros.
5. **Derivar testes próprios:** construir casos pequenos e determinísticos diretamente do contrato. Para cada caso, registrar os bytes/valores de entrada e o resultado esperado.
6. **Implementar separadamente:** escrever o código Aceris em Rust, sem consultar código de emuladores.
7. **Validar em camadas:** testes unitários, integração no caminho real do guest, CI multiplataforma e, quando possível, comparação com comportamento em hardware legítimo.
8. **Atualizar o estado:** marcar cada afirmação como documentada, inferida, testada sinteticamente ou confirmada externamente. Uma etapa não se torna compatível apenas por compilar.

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
