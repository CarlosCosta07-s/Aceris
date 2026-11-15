# Contrato de integração 3DSX com a sessão ARM11

**Status:** especificação de integração; validação do objeto preparado implementada; adaptador ARM11 ainda pendente  
**Base do contrato:** `main`; execução 3DSX ainda não é suportada pela sessão ARM11.

## 1. Objetivo

Permitir que um executável 3DSX validado seja convertido em um processo ARM11 executável pelo mesmo scheduler, dispatcher SVC e barramento de memória usados pelo caminho NCCH, sem transformar artificialmente o arquivo 3DSX em NCCH e sem enfraquecer o carregamento NCCH existente.

Este documento define a fronteira entre os módulos. Não declara suporte de execução 3DSX.

## 2. Estado atual confirmado

O caminho 3DSX já dispõe de:

- parsing estrutural do cabeçalho padrão/estendido, segmentos, BSS, tabelas skip/patch e regiões opcionais;
- materialização de Code, ROData, Data e BSS;
- aplicação atômica das relocations absolutas, relativas de 32 bits e PREL31;
- preparação de buffers e endereços virtuais page-aligned, permissões RX/R/RW, stack RW zerada e limite de memória explícito;
- controle de estado que impede preparar o processo antes das relocations exigidas ou aplicá-las duas vezes;
- `ThreeDsxPreparedProcess::validate()`, que revalida tamanhos dos buffers, alinhamento, limites de 32 bits, permissões, sobreposições, entry point, SP, posição do BSS e regiões zeradas;
- cópia privada do layout preparado, que rejeita alterações posteriores de endereços/permissões depois da aplicação das relocations;
- helper `ThreeDsxStackRequest::conventional_3ds_homebrew()` para o perfil de stack convencional, mantendo a possibilidade de configuração explícita.

- `ThreeDsxPreparedProcess::memory_mappings()`, que revalida o processo e oferece quatro descritores emprestados e imutáveis em ordem Code/ROData/Data/Stack, com base virtual, tamanhos, bytes e permissões R/W/X explícitas;

O resultado é `ThreeDsxPreparedProcess`, definido em `loader/threedsx/process.rs`. Ele contém layout e buffers, mas ainda não é um `Bus`, `Arm11CodeAddressSpace` ou `Arm11ProcessSession`.

## 3. Por que a conversão direta para NCCH é incorreta

A implementação NCCH recebe `PreparedArm11Image` e usa `NcchExtendedHeader` para obter, entre outros dados:

- endereços e tamanhos dos code sets;
- tipo de memória ARM11;
- stack size;
- tamanhos de dados e BSS.

O construtor `Arm11CodeAddressSpace::from_prepared` deriva o layout NCCH e cria regiões separadas de Data e BSS. Em contraste, o layout 3DSX produzido atualmente representa o Data gravável como uma região contínua que já contém os bytes inicializados, a cauda BSS zerada e o padding da página. O formato 3DSX também não declara os metadados NCCH acima.

Portanto, não se deve criar `NcchExtendedHeader`, `CodeSetInfo`, title ID ou qualquer metadado NCCH fictício para reutilizar o construtor existente. O stack size continua sendo configuração do runtime, não dado do 3DSX. Também não se deve mover o início do BSS para depois de uma região Data arredondada se isso alterar o layout virtual já preparado pelo 3DSX.

### Perfil convencional da stack

A documentação pública do mapa ARM11 userland registra a stack principal com topo usual em `0x10000000` e stack size comum de `0x4000` bytes. A API `StartupInfo` do libctru expõe o stack size da thread principal, enquanto a documentação de `svcCreateThread` exige topo de stack alinhado em oito bytes. Como o formato 3DSX não armazena stack size, o Aceris oferece `ThreeDsxStackRequest::conventional_3ds_homebrew()` como perfil explícito e substituível: base `0x0FFFC000`, tamanho `0x4000`, SP inicial `0x10000000`. Isso é uma convenção de runtime, não uma garantia para todo arquivo 3DSX.

Referências públicas: [3DS userland memory map](https://www.3dbrew.org/wiki/Memory_map), [libctru StartupInfo e SVC API](https://github.com/devkitPro/libctru/blob/master/libctru/include/3ds/svc.h).

## 4. Contrato genérico necessário

A fronteira apropriada é um tipo de imagem de processo independente do formato de origem. A API concreta deve ser implementada no módulo proprietário do espaço ARM11, mantendo o adaptador 3DSX em seu próprio módulo.

O contrato genérico deve carregar, no mínimo:

1. **Entry point** em endereço virtual de 32 bits.
2. **SP inicial** validado e alinhado conforme a regra escolhida pelo runtime.
3. **Mapeamentos explícitos**, cada um com endereço base, tamanho mapeado, tamanho inicializado, bytes de backing e permissões de leitura/escrita/execução.
4. **Perfil de processo/runtime** fornecido pelo chamador para parâmetros que o arquivo não contém, como classe/região de memória e política de heap. Esses parâmetros não podem ser inferidos como se fossem metadados do 3DSX.
5. **Limites de memória** e validação de overflow/sobreposição antes de reservar ou instalar qualquer mapeamento.

Requisitos de preservação dos dados:

- Code permanece executável e não gravável.
- ROData permanece legível, não gravável e não executável.
- Data permanece legível/gravável e não executável; sua cauda BSS e padding permanecem zerados dentro do mesmo mapeamento contínuo descrito pelo 3DSX.
- Stack permanece legível/gravável, não executável e zerada no início.
- Os endereços e tamanhos virtuais preparados pelo 3DSX são preservados exatamente; não se pode reinterpretar os buffers como se fossem o layout NCCH.
- A validação é feita para todos os segmentos em conjunto, antes de instalar o primeiro segmento, evitando estado parcialmente mapeado.

A implementação NCCH deve continuar sendo um adaptador para esse contrato genérico, e não ser substituída por uma estrutura especial para 3DSX. Os testes atuais do caminho NCCH precisam permanecer válidos sem mudanças de semântica.

## 5. Integração com a sessão

A sessão existente já oferece um ponto de entrada posterior à criação do espaço: `Arm11ProcessSession::from_address_space`. Ela inicializa o contexto principal a partir do entry point/SP do espaço, habilita VFP, aloca TLS e constrói o scheduler.

O adaptador genérico deverá, portanto:

1. converter um objeto de imagem de processo validado em `Arm11CodeAddressSpace`;
2. criar `Arm11ProcessSession` pelo construtor comum;
3. conectar a mesma memória física/GPU usada pelo sistema, sem inventar backing independente;
4. preservar o comportamento existente de threads, TLS, IPC, SVCs, eventos e relatórios de parada.

O `System` atualmente armazena uma opção chamada `ncsd_session` e seus métodos de carregamento público são NCSD-específicos. Para aceitar formatos diferentes, será necessário generalizar a propriedade da sessão ativa no nível de `System`, mantendo acessores compatíveis para o caminho NCSD durante a transição. O processamento de frames e os serviços GSP devem consultar a sessão ativa genérica, não depender do formato que a originou.

## 6. Testes de aceitação antes de declarar execução

### Testes de memória e permissões

- verificar bytes iniciais e entry point;
- verificar BSS e padding zerados no endereço exato do layout 3DSX;
- verificar stack zerada e SP inicial;
- confirmar escrita negada em Code e ROData;
- confirmar escrita permitida em Data e Stack;
- confirmar execução permitida somente nas regiões executáveis;
- confirmar que uma falha de validação não instala mapeamentos parciais.

### Testes de sessão

- criar uma sessão a partir de uma fixture 3DSX sintética relocacionada;
- verificar PC, SP e TLS do contexto principal;
- executar uma sequência ARM/Thumb sintética conhecida e confirmar os registradores, PC e relatório de parada;
- executar regressões do caminho NCCH no mesmo workflow para detectar regressões no adaptador existente.

### Teste de homebrew

A declaração de primeiro suporte de execução depende de um 3DSX real fornecido legalmente pelo mantenedor, com identidade e resultado esperado documentados. A execução deve registrar PC/CPSR, instruções executadas, motivo de parada e SVC/serviço ausente. Fixtures sintéticas não substituem esse teste.

## 7. Limites de escopo e propriedade

- Este arquivo é uma especificação, não um adaptador executável.
- Não modificar `loader/ncsd/process.rs` ou `loader/ncsd/session.rs` sem autorização expressa do mantenedor; esses arquivos contêm trabalho reservado ao outro agente.
- O trabalho 3DSX permanece isolado em `loader/threedsx/` até que o contrato seja acordado e implementado pelo responsável pelo espaço/sessão.
- Não incorporar código ou assets de outros emuladores. Usar apenas documentação pública de formatos e arquitetura.
- Não incluir ROMs, firmware, chaves, dumps protegidos ou executáveis de terceiros no repositório, CI ou releases.

## 8. Próxima sequência

1. Revisar este contrato com o responsável pelo espaço ARM11/sessão.
2. Implementar o tipo genérico de imagem/mapeamentos e adaptar o construtor NCCH sem mudar sua semântica.
3. Ligar `ThreeDsxPreparedProcess` ao construtor genérico e adicionar testes de permissões/BSS/layout.
4. Generalizar o armazenamento da sessão ativa em `System` e disponibilizar a carga 3DSX para CLI/desktop e Android pelo mesmo `aceris-core`.
5. Validar com um homebrew real fornecido legalmente e registrar os serviços ainda ausentes.

Até a conclusão desses passos, o estado correto continua sendo: **3DSX validado, relocacionado e preparado em memória; não executável pela sessão ARM11 ainda.**


## 9. Validação do objeto preparado e perfil de stack

A validação adicional fica isolada no módulo 3DSX e é chamada pelo próprio `prepare_process` antes de devolver o objeto:

- cada buffer precisa corresponder ao tamanho mapeado e à página declarada;
- Code, ROData, Data e Stack precisam manter as permissões esperadas;
- o entry point deve permanecer dentro do Code executável;
- o topo da stack deve corresponder ao SP, alinhado em oito bytes e representável como endereço ARM11;
- BSS precisa começar imediatamente após Data inicializado e terminar dentro do mapping Data;
- os mapeamentos não podem se sobrepor e a soma total precisa ser consistente;
- padding, BSS e stack inicial precisam permanecer zerados;
- o snapshot privado do layout rejeita mudanças posteriores de bases e metadados, protegendo a coerência com as relocations calculadas.

O helper de stack convencional não fixa a base dos segmentos. A seleção das bases Code/ROData/Data continua explícita até que o perfil completo do primeiro alvo homebrew seja fechado e testado.


## 10. Interface de descritores de memória

`ThreeDsxPreparedProcess::memory_mappings()` é a fronteira pública destinada ao adaptador de memória. A chamada executa `validate()` antes de devolver a lista fixa e ordenada:

- `Code`: leitura + execução, sem escrita;
- `ReadOnly`: somente leitura;
- `Data`: leitura + escrita, sem execução; o backing já inclui Data inicializado, BSS zerado e padding;
- `Stack`: leitura + escrita, sem execução; backing inicialmente zerado.

Cada `ThreeDsxProcessMemoryMapping` contém endereço, tamanho inicializado, tamanho mapeado e uma fatia imutável dos bytes completos. Seus campos são privados e só podem ser lidos por accessors; um consumidor externo não consegue construir ou alterar um descritor arbitrário. Nenhum buffer é copiado pela enumeração. Regiões ROData ou Data vazias são descritas com tamanho zero para o adaptador poder ignorá-las sem inventar uma página.

A API não instala mapeamentos, não cria permissões de MMU e não instancia sessão ARM11. O proprietário do espaço de endereçamento ainda precisa validar o manifesto inteiro antes de instalar o primeiro segmento.

## 11. Manifesto de inicialização

`ThreeDsxPreparedProcess::memory_manifest()` oferece uma visão única e emprestada de tudo que um adaptador precisa para construir o espaço ARM11 desse processo:

- entry point e SP inicial;
- os quatro mapeamentos Code/ROData/Data/Stack, com endereços, bytes e permissões R/W/X;
- endereço virtual e tamanho exato do BSS dentro do mapeamento Data.

O método chama `validate()` antes de devolver o manifesto. Seus campos e os dos descritores são privados, com accessors somente de leitura; o consumidor não pode substituir entry point, SP, endereços, permissões ou fatias por valores arbitrários. Além do endereço e tamanho do BSS, `bss_offset_bytes()` fornece o deslocamento já validado dentro de Data. `ThreeDsxPreparedProcess` também mantém privados seu layout e os quatro buffers de backing, oferecendo somente referências imutáveis por `layout()`, `code_bytes()`, `read_only_bytes()`, `data_bytes()` e `stack_bytes()`. Assim, um consumidor externo não consegue trocar instruções ou dados após a preparação e continuar usando o objeto como imagem validada. O manifesto não copia os buffers, não reserva memória e não altera o espaço ARM11. O BSS continua dentro do Data contínuo, imediatamente após os bytes inicializados, e não deve ser instalado como uma região independente.

## 12. Pipeline único de carga e preparação

`ThreeDsxImage::load_and_prepare_process(reader, request)` centraliza a sequência de leitura de segmentos, aplicação das tabelas de relocation e materialização do processo. `ThreeDsxProcessPreparationRequest` exige que o chamador informe as três bases virtuais, a reserva de stack, o limite total de memória e o limite de words de relocation.

Antes de ler os segmentos, a rotina valida o plano virtual completo — alinhamento, sobreposição com stack, tamanhos arredondados por página e orçamento total. Se o plano for inválido, o reader não é avançado nem os buffers de segmento são alocados. `prepare_process()` reutiliza a mesma validação antes de materializar os buffers. A API reduz o risco de um frontend esquecer a etapa de relocations ou usar limites implícitos. Ela sempre executa a etapa de relocation, inclusive quando as tabelas não têm entradas, e só devolve `ThreeDsxPreparedProcess` se toda a sequência terminar com sucesso. O método não reserva páginas MMU nem cria `Arm11ProcessSession`.
