# Notas de transferência entre IAs — Aceris

> **Leitura obrigatória ao retomar o trabalho.** Esta nota registra alterações autorizadas pelo mantenedor e evita que uma IA restaure ou sobrescreva trabalho de outra.


## Regra vigente do mantenedor — atualização de 17/11/2026

Esta seção prevalece sobre registros históricos posteriores ou anteriores nesta nota que descrevam uma política diferente para consulta a referências.

Carlos Costa flexibilizou somente a política de referências técnicas: agora é permitido consultar código-fonte publicamente disponível de outros emuladores, inclusive Azahar, Citra e Lime3DS, para estudo técnico. A consulta não autoriza copiar, colar, traduzir, portar, adaptar ou reproduzir código, testes, fixtures, shaders, patches, assets ou estrutura interna. Toda implementação deve continuar original e escrita em Rust, com arquitetura, interfaces, nomes e testes próprios. Registre projeto, URL, arquivo/área, commit/revisão, licença, data e pergunta técnica; em caso de dúvida sobre licença ou obra derivada, pare e peça orientação a Carlos Costa.

**Nenhuma outra regra foi flexibilizada.** Permanecem obrigatórios:
- Produto inteiro em Rust, inclusive núcleo, aplicação, interfaces e frontends.
- Desenvolvimento exclusivamente na branch `main`; não criar branches nem usar pull requests como fluxo de trabalho.
- Não alterar o trabalho de outra IA sem autorização explícita do mantenedor.
- Código e implementação próprios; não incorporar código, patches, testes, fixtures ou dependências de outros emuladores.
- Não incorporar assets ou recursos de terceiros.
- Não adicionar dependências que forneçam implementação do emulador sem autorização explícita.
- Manter jogos, firmware, chaves, dumps e conteúdos protegidos fora do repositório, CI, issues e releases.
- Confirmar HEAD, preservar autoria e executar as verificações proporcionais antes de declarar uma mudança concluída.

As anotações abaixo que dizem “não foi consultado código-fonte” registram o que ocorreu sob a política vigente na data de cada trabalho; não constituem uma proibição atual. Para as regras atuais, consulte também `docs/REGRAS_PARA_IA.md`, `CONTRIBUTING.md` e `docs/INDEPENDENT_RESEARCH.md`.


## Autorização total ao Claude — 26/11/2026

Carlos Costa autorizou Claude a desenvolver qualquer parte do código, independentemente da IA autora, para executar a lista: regras, kernel ARM11, PICA200, CSND/áudio, desempenho, Android físico e outros homebrews. Detalhes em `docs/REGRAS_PARA_IA.md`, seção 7.1. As demais regras continuam valendo.

## Aviso direto para Claude

Claude: ao retomar o desenvolvimento do Aceris, leia esta nota antes de editar qualquer arquivo. Carlos Costa autorizou explicitamente uma intervenção limitada nas dependências que você introduziu no scaffolding inicial (`thiserror`, `tracing` e `tracing-subscriber`). Essa intervenção foi concluída no commit que contém esta nota.

As três dependências foram removidas e substituídas por recursos da biblioteca padrão do Rust:

- `thiserror`: `aceris_common::Error` agora implementa manualmente `Display`, `std::error::Error` e `From<std::io::Error>`.
- `tracing`: o evento informativo de identificação de imagem foi removido do núcleo; o núcleo permanece sem responsabilidade por saída de log.
- `tracing-subscriber`: o frontend desktop apresenta erros com `eprintln!`, sem framework externo de logging.
- `Cargo.lock`: atualizado para conter somente os quatro crates do workspace, sem dependências de registro externas.

**Não restaure essas dependências nem reverta as implementações próprias**, a menos que Carlos Costa autorize isso explicitamente. A autorização dada foi restrita a esta substituição; ela não permite modificar outros arquivos ou trabalhos de Claude.

## Escopo da intervenção

Arquivos alterados por autorização explícita do mantenedor:

- `Cargo.toml`
- `Cargo.lock`
- `crates/aceris-common/Cargo.toml`
- `crates/aceris-common/src/error.rs`
- `crates/aceris-core/Cargo.toml`
- `crates/aceris-core/src/system.rs`
- `frontends/desktop/Cargo.toml`
- `frontends/desktop/src/main.rs`

O manifesto Android foi inspecionado e já dependia somente dos crates internos; permaneceu sem alterações funcionais.

## Estado e verificações

A substituição foi preparada para ser verificada por CI com:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --locked --workspace
```

Não considere os testes aprovados até conferir o resultado do workflow associado ao commit. Toda IA deve relatar apenas verificações realmente executadas.

## Regras permanentes

- Todo trabalho é feito exclusivamente em `main`; não criar branches.
- Uma IA não altera o trabalho de outra sem autorização explícita de Carlos Costa.
- O produto é implementado em Rust.
- ~~Não abrir, ler ou usar código-fonte interno de outros emuladores.~~ *(Substituída em 17/11/2026: a consulta a código open source é permitida nos limites de `docs/REGRAS_PARA_IA.md`, seção 5.)*
- Não incorporar código ou assets de terceiros.
- Em dúvida sobre autoria ou sobreposição, interromper a edição e consultar o mantenedor.

## Colaboração adicional autorizada — 12/11/2026

Carlos Costa autorizou explicitamente o ChatGPT a colaborar no trabalho de Claude para acelerar a implementação do Aceris. A colaboração registrada até aqui cobre a integração do serviço GSP ao núcleo e a correção do bloqueio de CI em AES. Ela não altera as regras permanentes: somente `main`, produto em Rust, sem código/recursos derivados de terceiros e sem consultar código-fonte de outros emuladores.

### Alterações publicadas

- `b976afc` — corrigiu `clippy::needless_range_loop` em `ncch/crypto.rs`, iterando diretamente pelas chaves AES. O intervalo de rodadas e as transformações criptográficas foram preservados.
- `6305754` — adicionou `GspServiceRegistry::deliver_interrupt_to_session`, com pré-validação de todos os destinatários antes da alteração das filas, para clientes vinculados à mesma sessão ARM11.
- `01a682b` — integrou `GspServiceRegistry` ao `System`, com registro, remoção, aquisição/liberação de direitos, entrega de interrupções e caminho `trigger_gsp_cmd_req_queue`.
- `1db6033` — adicionou teste de que `ProcessCommandList` é consumido, mas não submetido à GPU sem direitos de renderização.
- `ee82e30`, `dfeface`, `6b34464` — correções de formatação e remoção de um wrapper de teste que gerava código morto no build normal.
- `df5f652` — conecta PDC0/PDC1 ao limite de quadro após a troca do buffer LCD e reporta as entregas GSP em `SystemFrameReport`.
- `71ced96` — adota `io::Error::other` para o mapeamento de falhas de entrega PDC, conforme Clippy.
- `3b52e63` — acrescenta `System::register_next_gsp_client()` e esclarece os limites das APIs de fila GSP.

### APIs GSP disponíveis para integração

- `GspServiceRegistry::client_registrations()`: enumera inscrições ativas em ordem crescente de ID.
- `GspServiceRegistry::deliver_interrupt_to_session()`: entrega em uma sessão ARM11 compartilhada.
- `GspServiceRegistry::deliver_interrupt_to_sessions()`: entrega quando cada cliente possui uma sessão própria.
- `System::register_gsp_client[_with_flags]()`, `register_next_gsp_client()` e `register_next_gsp_client_with_flags()`.
- `System::unregister_gsp_client()`, `try_acquire_gsp_rendering_right()`, `acquire_gsp_rendering_right()` e `release_gsp_rendering_right()`.
- `System::dispatch_gsp_interrupt()` e `trigger_gsp_cmd_req_queue()`.

### Semântica e limites que devem ser preservados

- `TriggerCmdReqQueue` não exige que o chamador seja o proprietário dos direitos de renderização.
- `ProcessCommandList` não deve submeter comandos à GPU quando o cliente não possui esses direitos. O Aceris consome a entrada da fila e deixa a submissão em zero nesse caso.
- `System` atualmente mantém uma única `ncsd_session`. O caminho `deliver_interrupt_to_session` atende esse bootstrap; a integração completa de vários processos ainda exige que o sistema mantenha e associe uma sessão ARM11 por cliente GSP.
- O registro atual pressupõe que o bloco de 0x1000 bytes já esteja mapeado e gravável e que o event handle já exista na tabela de objetos do processo.
- Os SVCs locais para criação, mapeamento e desmapeamento de memory blocks já estão implementados, incluindo backing compartilhável sobre heap regular e a primitiva host-side de importação entre sessões. Ainda faltam a tradução de descritores IPC COPY/MOVE_HANDLE, a resposta estendida de `RegisterInterruptRelayQueue` e a integração do fluxo ao dispatcher GSP.
- A fila FIFO de `AcquireRight` apenas registra a espera e o handoff. O dispatcher/scheduler ainda precisa suspender e acordar a thread IPC.
- PDC0 e PDC1 agora são despachados pelo ciclo de quadro depois de `LcdController::advance_vblank()`; `SystemFrameReport::gsp_interrupt_deliveries` conta os relatórios de entrega para esses dois eventos.
- P3D e DMA ainda não são gerados automaticamente: precisam ser conectados à conclusão real dos trabalhos PICA200/DMA, não simulados por submissão de comandos.
- As famílias GX além de `ProcessCommandList` continuam pendentes, assim como a arbitragem de busy flags.
- `enqueue_gsp_interrupt_for_client()` permanece como API legada de baixo nível; a rota nova `dispatch_gsp_interrupt()` usa os handles registrados.

### Verificação

O commit `b976afc` passou em `cargo fmt`, Clippy e testes do workspace em Ubuntu, Windows e macOS, além do build Android ARM64. Para a integração GSP/VBlank no HEAD anterior (`3b52e63`), a execução `43293773245` já confirmou Ubuntu (formatação, Clippy e testes) e Android ARM64; Windows e macOS ainda estavam concluindo os testes na última consulta. Verifique a execução mais recente antes de declarar a matriz completa:

https://github.com/CarlosCosta07-s/Aceris/actions/runs/43293773245

## Continuação da colaboração autorizada — CreateMemoryBlock (12/11/2026)

Carlos Costa autorizou explicitamente o ChatGPT a continuar colaborando no trabalho de Claude. Nesta rodada, a intervenção ficou restrita à superfície do kernel ARM11 para blocos de memória compartilhada e à documentação correspondente.

### Alterações publicadas na branch `main`

- `6db9d9f` — trabalho de Claude: integra os SVCs `MapMemoryBlock` (0x1F) e `UnmapMemoryBlock` (0x20), com validação de permissões e testes de mapeamento/desmapeamento.
- `8aa675a` — trabalho de Claude: corrige a referência de módulo para `SharedMemoryBacking` e aplica formatação Rust.
- `64dd939` — declara o identificador SVC `CreateMemoryBlock` (0x1E) e atualiza o teste do dispatcher genérico, que continua encaminhando serviços de processo para a sessão.
- `ae56526` — implementa o handler de `CreateMemoryBlock`: valida o ponteiro de saída, cria um bloco anônimo zerado com permissões de owner/peer, grava o handle no processo e fecha o handle se a gravação falhar.
- `da94e08` — adiciona testes de integração executando a sequência SVC Create/Map/Unmap e valida o estado QueryMemory Shared; inclui ainda rejeição de ponteiro de saída inválido e de endereço-fonte não suportado.

### Limites que devem ser preservados

- O handler atual suporta `addr=0` somente para processos SYSTEM/BASE. Processos APPLICATION e tipos não reconhecidos recebem `0xD92007EA`; descritor ausente ou tipo zero é tratado conservadoramente como APPLICATION.
- Com `addr` não nulo, o handler aceita somente uma faixa alinhada por página, contida em uma única alocação atual do heap regular modelado por `ControlMemory`. Segmentos estáticos, TLS, aliases shared já mapeados, memória linear e intervalos que cruzam várias alocações continuam fora do escopo.
- O backing original do heap começa como `Vec<u8>` privada do host e é promovido para `Arc<Mutex<Vec<u8>>>` sem cópia quando um bloco é criado. Isso garante aliasing de bytes dentro do mesmo modelo de processo, mas não é ainda um alocador físico FCRAM nem uma implementação completa da coerência multi-processo.
- As permissões suportadas são NONE=0, R=1 e RW=3; W-only=2 é inválida e deve ser rejeitada na criação e no mapeamento.
- `MapMemoryBlock` e `UnmapMemoryBlock` operam sobre a tabela de handles local. Existe agora uma primitiva host-side para importar/duplicar um memory block entre duas sessões, mas a tradução de descritores IPC COPY/MOVE_HANDLE ainda falta.
- O GSP registry continua recebendo um endereço de shared memory e event handle já mapeados/criados. O despacho GSP IPC, a resposta estendida de `RegisterInterruptRelayQueue` e o bloqueio/despertar de threads em `AcquireRight` seguem pendentes.
- Não alterar a implementação de memória compartilhada existente sem revisar os commits acima e preservar os testes de alias, permissões, QueryMemory, unmap e rollback.

### Verificação

Os testes foram adicionados ao workspace, mas não foram executados localmente nesta intervenção. Confirme o workflow da branch `main` associado ao HEAD antes de marcar a mudança como aprovada:
https://github.com/CarlosCosta07-s/Aceris/commit/da94e087ebcabd6c1ac8d19705ccd82276697949

## Continuação da colaboração autorizada — memória compartilhada baseada no heap (12/11/2026)

Carlos Costa renovou explicitamente a autorização para que o ChatGPT colabore com Claude. As alterações desta rodada foram feitas somente em `main`, sem abrir branch, e foram limitadas ao parser de exheader, ao backing compartilhável de heap, aos SVCs de memory block, aos testes correspondentes e à documentação.

### Commits publicados

- `2a67ca1` — decodifica o tipo de memória ARM11 a partir do descritor OtherCapabilities no ACI.
- `619d165` — adiciona views com offset/length e promove alocações regulares de heap para backing compartilhável sem copiar os bytes.
- `f5e0af4` — torna a validação de mapping sensível ao intervalo exato da view compartilhada.
- `e465b59` — permite que a tabela de objetos crie um handle sobre um backing já compartilhável.
- `2779df5` — implementa a rota de source-address do CreateMemoryBlock para heap regular e restringe a alocação anônima a SYSTEM/BASE.
- `ae4a3fc` e `aa689ad` — ampliam os testes de sequência SVC, estado QueryMemory e coerência de leitura/escrita entre a origem e o alias.
- `c66a820` — valor ausente/zero de memory type é tratado como APPLICATION por segurança.
- `fc9922f` — valida permissões antes da alocação anônima.
- `6b527d5`, `0546b99` e `e810673` — rejeitam W-only=2 no mapping, na criação do objeto e no SVC.
- `be5fefb` — sincroniza o roadmap.
- `e984f39` — correção de formatação Rustfmt e adição de `SharedMemoryBacking::is_empty` para a API de backing.
- `44f6ede` — atualiza a arquitetura para descrever o caminho de heap compartilhado.
- `15aa9aa` — adiciona `Arm11ProcessSession::import_memory_block_handle`, que duplica o handle em outra tabela de objetos mantendo o mesmo backing e invertendo as permissões owner/peer.
- `17bba72` — testa a importação entre sessões, a leitura compartilhada, o bloqueio de escrita conforme as permissões e a sobrevivência do backing após o fechamento do handle da origem.
- `1dd310e` — ajuste de formatação Rustfmt em uma asserção do teste de heap compartilhado.

### Semântica implementada e limites

- ACI ARM11 Kernel Capabilities começa em `0x170`; o parser procura o descritor OtherCapabilities e extrai o campo de memory type.
- Valor 1 ou ausência/zero do descritor resulta em APPLICATION; 2 em SYSTEM; 3 em BASE; valores ainda não reconhecidos permanecem como `Unknown` e são tratados como APPLICATION pelo SVC.
- `CreateMemoryBlock(addr=0)` só permite processos SYSTEM/BASE. APPLICATION e `Unknown` retornam `0xD92007EA`.
- `CreateMemoryBlock(addr!=0)` permite apenas um intervalo alinhado por página, totalmente contido em uma única alocação de heap regular ainda mapeada pelo caminho atual de `ControlMemory`. A view mantém offset e tamanho próprios, sem copiar o backing.
- Não aceitar, nem simular por cópia, memória estática (.text/.rodata/.data/.bss), TLS, shared alias existente, memória linear ou um intervalo que atravesse várias alocações.
- O heap atual é respaldado por memória host `Vec<u8>`, não por páginas físicas FCRAM reservadas. A promoção para `Arc<Mutex<Vec<u8>>>` fornece aliasing de bytes entre sessões que receberam o handle por `import_memory_block_handle`, não sincronização física de dispositivos nem um subsistema real de processos.
- `import_memory_block_handle` é uma primitiva host-side de duplicação: inverte owner/peer permissions no handle recebido. Ela não traduz descritores IPC, não implementa MOVE_HANDLE e não deve ser apresentada como transporte IPC concluído.
- Permissões suportadas: NONE=0, R=1 e RW=3. W-only=2 não é uma permissão válida no ARM11, por isso é rejeitada.
- O helper de importação entre sessões já existe, mas a camada GSP ainda precisa receber/validar o descritor de handle IPC, importar o bloco no processo consumidor e executar o dispatcher completo. A API de registry existente ainda começa com endereço de shared memory e handle de evento já disponíveis.

### Verificação

O commit `e984f399bbaad181248e86bb5c93cc320fdac282` foi publicado por Claude depois das alterações funcionais e corrigiu formatação/API de backing. A CI não pôde ser confirmada por esta integração: o status combinado consultado não retornou checks, e o ambiente de execução disponível não possui `rustc`/Cargo. Não marcar os novos testes como aprovados até a execução da CI em `main` ser verificada.

## Continuação da integração IPC do GSP — resposta de registro (12/11/2026)

O ChatGPT, com autorização explícita do mantenedor para colaborar no trabalho de Claude, publicou o commit `1f45472c2f9729e0af3ac8d63766fcf712fd8ca8`:

- Adiciona `GspRegisterInterruptRelayQueueResponse` e o cabeçalho de resposta `0x00130082`.
- A serialização contém dois parâmetros normais — código de resultado e ID atribuído à thread/fila GSP — seguidos pelo descritor COPY de um handle e pelo handle de memória compartilhada.
- `GspRegisterInterruptRelayQueueResponse::error` emite thread ID e handle iguais a zero; o handle nulo não efetua transferência.
- Dois testes cobrem a disposição dos cinco words, as contagens de parâmetros indicadas no cabeçalho e a resposta de erro.

Referências de protocolo utilizadas: [IPC Command Structure](https://www.3dbrew.org/wiki/IPC_Command_Structure), [GSP Services](https://www.3dbrew.org/wiki/GSP_Services) e [API documentada de RegisterInterruptRelayQueue](https://rust3ds.github.io/ctru-rs/crates/ctru_sys/fn.GSPGPU_RegisterInterruptRelayQueue.html).

**Limite importante:** este commit só modela/serializa a resposta. Ainda não cria o bloco de memória em um contexto de serviço, não transfere o handle entre tabelas de objetos, não mapeia o backing para o processo cliente e não despacha a mensagem IPC real. Os novos testes foram adicionados, mas não executados nesta sessão; confirme Rustfmt, Clippy e `cargo test --workspace` na CI antes de marcar a etapa como aprovada.

## Continuação da transferência host-side de memory blocks — MOVE (12/11/2026)

O ChatGPT, com autorização explícita do mantenedor para colaborar no trabalho de Claude, publicou o commit `213f882e5b5e4c4182de7232f09e815ee338be51`:

- Adiciona `Arm11ProcessSession::move_memory_block_handle`: importa o memory block para a tabela de objetos de destino com o mesmo backing e permissões owner/peer invertidas; em seguida fecha o handle de origem.
- Se o fechamento da origem falhar, o método fecha o handle recém-criado no destino e devolve o erro, evitando deixar um MOVE parcial.
- O teste `moved_memory_block_handle_closes_source_and_preserves_shared_backing` verifica fechamento do handle de origem, mapeamento com permissões invertidas, leitura e coerência do backing após a transferência.

**Limites:** é uma operação host-side específica para memory blocks. Não decodifica descritores IPC, não transfere eventos/threads/outros tipos de objeto, não implementa ainda o ciclo de vida geral de `COPY_HANDLE`/`MOVE_HANDLE` no dispatcher e não conecta a transferência ao GSP. O serializador estendido de `RegisterInterruptRelayQueue` do commit `1f45472` permanece separado do transporte real do handle.

Os testes foram adicionados, mas não executados nesta sessão. Confirmar Rustfmt, Clippy e `cargo test --workspace` na CI antes de marcar a mudança como aprovada.

## Decodificação e tradução IPC de memory-block handles (12/11/2026)

O ChatGPT, com autorização explícita do mantenedor para colaborar com Claude, publicou:

- `aa76f99` — adiciona `IpcHandleTranslationDescriptor` e `IpcHandleTranslationError` no kernel ARM11. O decoder reconhece o tipo 0, extrai quantidade de handles, flag MOVE e flag ProcessId, rejeitando tipo diferente, bits reservados e combinação contraditória MOVE+ProcessId.
- `c447b3e` — adiciona `Arm11ProcessSession::translate_memory_block_handle_from_ipc`, que aplica um descritor type-0 de um único handle a memory blocks: COPY duplica, MOVE duplica e fecha a origem, handle zero é tratado como nulo e ProcessId/múltiplos handles são rejeitados. Quatro testes cobrem COPY, MOVE, nulo e descritores não suportados.

Base técnica independente: [IPC Command Structure — Handle Translation](https://www.3dbrew.org/wiki/IPC_Command_Structure), que documenta type 0, bit 4 para fechar a origem (MOVE), bit 5 para ProcessId e bits 26–31 para contagem menos um.

**Limite importante:** esta é uma primitive de tradução host-side para um único memory block, não um dispatcher IPC geral. Não transfere eventos, threads ou outros tipos de objeto; não implementa arrays de handles; ainda não é chamada pelo caminho de envio/recepção IPC nem pelo dispatcher GSP. O GSP continua recebendo handles já traduzidos como pré-condição de sua API de serviço.

Os testes foram adicionados, mas não executados nesta sessão. Verificar Rustfmt, Clippy e `cargo test --workspace` em CI antes de considerar aprovada a mudança.

## Leitor comum de mensagens IPC e integração ao decoder GSP (12/11/2026)

Com a autorização explícita de Carlos Costa para colaborar no trabalho de Claude, o ChatGPT publicou as seguintes alterações incrementais em `main`:

- `d31f096` — adiciona `session::ipc` com `IpcCommandHeader`, `IpcMessage` e `IpcMessageDecodeError`. O leitor extrai command ID, contagem de parâmetros normais e contagem total de words da seção de tradução; valida se o TLS contém todos os words declarados; oferece slices separados para as duas seções e ignora somente o conteúdo residual após o tamanho declarado.
- `3366a0e` — expõe o módulo IPC pela sessão ARM11.
- `e2791da` — refatora `decode_gsp_ipc_request` para consumir a visão comum do IPC. O decoder GSP continua exigindo os cabeçalhos e descritores específicos já suportados; os parâmetros agora são obtidos das seções normais/de tradução, sem offsets absolutos no buffer TLS.

### Arquivos alterados

- `crates/aceris-core/src/loader/ncsd/session/ipc.rs` — novo parser comum e cinco testes unitários.
- `crates/aceris-core/src/loader/ncsd/session.rs` — exporta `pub mod ipc`.
- `crates/aceris-core/src/loader/ncsd/session/gsp_service.rs` — integra o decoder GSP ao parser comum.
- `docs/ARCHITECTURE.md` — atualiza o estado e os limites da camada IPC.

### Contrato do parser

- Cabeçalho: command ID nos bits 16–31, número de parâmetros normais nos bits 6–11 e total de words de tradução nos bits 0–5.
- O total declarado inclui o próprio header no cálculo de tamanho do buffer.
- `IpcMessage` não traduz handles nem interpreta cada tipo de descriptor; apenas fornece uma visão validada e emprestada das seções.
- Os bits 12–15 são preservados no header e expostos para validação na camada de serviço; o parser genérico não atribui semântica a eles.
- O decoder GSP continua recusando cabeçalhos e descritores diferentes dos formatos conhecidos.

### Próximas tarefas para Claude

1. Reutilizar `session::ipc::IpcMessage` em novos decoders de serviços, sem duplicar a extração de contagens/offsets TLS.
2. Não ampliar a transferência de eventos com uma simples cópia de `signaled`: as filas de espera são hoje locais à tabela/sessão. A implementação correta exige identidade de processo/thread e um caminho de despertar que alcance o scheduler da sessão destinatária.
3. Planejar o dispatcher `svcSendSyncRequest`/`svcReplyAndReceive` junto da modelagem de portas/sessões e da associação de múltiplas sessões ARM11 no `System`; os métodos de tradução host-side existentes não equivalem a transporte IPC.
4. Preservar os contratos GSP existentes, incluindo a resposta estendida de `RegisterInterruptRelayQueue`, o evento por cliente, o consumo de `ProcessCommandList` sem direitos e o estado FIFO de `AcquireRight`.

### Verificação

Cinco testes unitários foram adicionados ao novo módulo, e os testes existentes do decoder GSP continuam cobrindo os comandos suportados, mensagens truncadas, descritores inválidos e words TLS residuais. Não foi possível executar `cargo fmt`, Clippy ou `cargo test` neste ambiente por ausência de Rust/Cargo. A consulta de status combinado e de workflows para o commit `e2791da` não retornou checks nem execuções; portanto, não há aprovação de CI confirmada nesta rodada.

## Arrays de handles IPC type-0 para memory blocks (12/11/2026)

Com a autorização já registrada de Carlos Costa para colaborar no trabalho de Claude, o ChatGPT ampliou a primitiva host-side de transferência de memory blocks:

- `a809006` — adiciona `IpcHandleTranslationError::DuplicateMoveHandle`, que identifica explicitamente uma tentativa de mover o mesmo handle de origem mais de uma vez no mesmo array.
- `d183f14` — adiciona `Arm11ProcessSession::translate_memory_block_handles_from_ipc`. A API valida o descritor, o tamanho do array, o suporte a ProcessId e o tipo de todos os handles não nulos antes de criar qualquer handle de destino. COPY mantém as origens; MOVE rejeita handles de origem repetidos, importa todos os destinos e somente depois fecha as origens. Entradas zero permanecem como `None`.
- `cd6a769` — acrescenta cinco testes para COPY com nulos e permissões invertidas, MOVE de múltiplos handles, rejeição de um objeto que não é memory block sem efeitos parciais, MOVE duplicado e divergência entre contagem e array.

### Limites preservados

- A implementação só aceita arrays homogêneos de memory blocks. Um array que misture eventos, mutexes, threads ou outros objetos é rejeitado antes de qualquer mudança no destino.
- COPY pode repetir o mesmo handle de origem e produz handles de destino distintos que compartilham o mesmo backing. MOVE não permite repetição do mesmo handle de origem, pois uma única posse não pode ser movida duas vezes.
- O MOVE importa primeiro todos os destinos e depois fecha as origens. O fechamento de um memory-block handle não possui fila de espera; a validação inicial e a exclusividade mutável da sessão mantêm as origens estáveis durante a operação.
- O método permanece host-side: não lê sozinho uma mensagem TLS, não é chamado por `svcSendSyncRequest`, não implementa objetos kernel heterogêneos nem liga sessões IPC ao GSP.

### Verificação

Cinco testes foram adicionados, mas não executados nesta sessão. Rust/Cargo não estão disponíveis no ambiente; verificar Rustfmt, Clippy e `cargo test --workspace` em CI antes de marcar o trabalho como aprovado.

## Adapter entre mensagens IPC e arrays de memory blocks (12/11/2026)

- `c70074d` — adiciona `Arm11ProcessSession::translate_memory_block_translation_section_from_ipc`. O método recebe um `IpcMessage` previamente decodificado, exige que toda a seção de tradução contenha exatamente um descritor type-0 seguido pelo array correspondente e reutiliza a validação/transferência homogênea de memory blocks.
- `3dd9ca3` — adiciona teste que decodifica uma mensagem TLS sintética com um descritor MOVE, transfere dois memory blocks preservando uma entrada nula e confirma a rejeição de uma seção vazia.

**Limite:** o adapter não processa vários grupos de descritores, buffers estáticos/mapeados, tipos de objeto heterogêneos ou handles de evento. A mensagem deve ser decodificada antes por `IpcMessage`; o método não implementa `svcSendSyncRequest` nem altera o dispatcher real.
 
A verificação local continua pendente: o ambiente não disponibiliza Cargo/Rust, e os seis testes novos/alterados nesta sequência não foram executados. Verificar Rustfmt, Clippy e `cargo test --workspace` na CI antes de aprovar.

## Decodificação tipada de descritores IPC (12/11/2026)

Com a autorização de colaboração já concedida pelo mantenedor, o ChatGPT ampliou o parser de mensagens IPC e conectou a leitura tipada à transferência host-side de memory blocks:

- `71e4418` — adiciona `IpcTranslationParameter`, `IpcTranslationSectionError` e `decode_translation_section` em `session::ipc`. O parser percorre todos os grupos da seção e reconhece type 0 (handles), type 1 (cópia para static buffer) e types 5/6/7 (mapeamento temporário com permissões R/W/RW). Ele somente decodifica os valores; não copia buffers, não mapeia memória e não transfere objetos.
- `5c04344` — adiciona testes para grupos combinados de handles, static buffer e mapeamentos temporários, além de truncamento e tipos ainda não suportados.
- `5ed5728`, `746b9f5` e `8d16cd9` — endurecem a validação de bits reservados, a contagem e o tamanho dos mapeamentos.
- `1acea65` — o adapter `translate_memory_block_translation_section_from_ipc` agora decodifica a seção completa antes de transferir handles e exige exatamente um grupo type-0 de handles. Um segundo descritor ou tipo diferente é rejeitado antes da alteração da tabela de objetos.

### Formatos que o parser reconhece

- Type 0: descritor de handles, incluindo contagem, COPY/MOVE e a marca de ProcessId; os words de handle permanecem emprestados do buffer original.
- Type 1: índice de static buffer (bits 10–13), tamanho (bits 14–31) e o word seguinte como endereço de origem.
- Types 5/6/7: permissões de origem R/W/RW, tamanho em bytes nos bits superiores e o word seguinte como endereço de origem.
- Types 2/3 e outros tipos ainda não implementados são recusados explicitamente; não são tratados como se fossem buffers comuns.
- Para types 5/6/7, bit 0 reservado ligado e tamanho zero são rejeitados. Para type 1, bit 0 e bits 4–9 são rejeitados como reservados.
- `IpcMessage::translation_parameters()` oferece a lista tipada, após a validação de extensão do próprio `IpcMessage`.

Referência independente de protocolo: [IPC Command Structure — Message and Translation Descriptors](https://www.3dbrew.org/wiki/IPC_Command_Structure).

### Limites e próximos passos

- O parser de type 1 não consulta a tabela de static buffers do TLS+0x180 nem copia dados para o processo destinatário.
- O parser dos types 5/6/7 não valida o intervalo de memória de origem, não cria mapeamentos temporários e não implementa os caminhos de reply.
- Types 2/3 continuam pendentes por exigirem interpretação de tabela de endereços físicos e vínculo com o modelo de memória.
- A transferência real continua limitada a arrays homogêneos de memory blocks; eventos, mutexes, timers, threads e objetos de sessão não são transferidos.
- O adapter de memory blocks só aceita uma seção com um grupo type-0; várias famílias de descritor em uma mesma mensagem são reconhecidas pelo parser genérico, mas ainda não são despachadas por esse adapter.

### Verificação

Foram adicionados testes unitários para os tipos reconhecidos, erros de truncamento/reservados, PID e grupos adicionais. Não foi possível executar Rustfmt, Clippy ou `cargo test` porque o ambiente continua sem `cargo`, `rustc` e `rustfmt`. Não há CI confirmada para o HEAD desta rodada.



## Cópia de static buffers IPC type-1 (12/11/2026)

- Arm11CodeAddressSpace::thread_local_static_buffer_descriptor_address valida o TLS ativo da thread e resolve com segurança uma das 16 entradas de static buffer em TLS + 0x180.
- Arm11ProcessSession::copy_static_buffer_translation_from_ipc aplica descritores type-1 para uma thread receptora explícita: lê endereço/capacidade registrados no TLS, rejeita capacidade insuficiente, valida a faixa de destino como gravável e lê os bytes da origem antes de qualquer escrita.
- Os descritores são preparados em lote antes de tocar os destinos; um source pointer inválido em um grupo posterior não deixa os buffers anteriores parcialmente atualizados. Tipos handles e mapped-buffer são recusados por este helper e seguem caminhos separados.
- Quatro testes cobrem cópia para o slot TLS, capacidade insuficiente sem escrita, pré-staging transacional diante de origem inválida e recusa de outro tipo de tradução.

Limites: o helper não é ainda um dispatcher IPC geral e precisa ser chamado pelo futuro caminho de recepção/sessão. O registro dos buffers continua sendo responsabilidade do código convidado. Types 5/6/7 só são decodificados; o mapeamento temporário, limpeza de aliases e buffers físicos types 2/3 continuam pendentes. Confirme rustfmt, Clippy e cargo test --workspace via CI antes de considerar a etapa aprovada.


## Tradução IPC de buffers físicos type-2/type-3 (12/11/2026)

- `session::ipc::decode_translation_section` reconhece type 2 (tabela RW) e type 3 (tabela read-only), extrai índice de static buffer, tamanho, source pointer e offset do descritor no trecho de tradução. Tamanho zero e bit reservado são rejeitados.
- `Arm11CodeAddressSpace::linear_physical_chunks_for_range` fornece os pares físicos apenas quando cada byte está coberto por mapeamentos lineares explícitos; chunks virtual e fisicamente contíguos são coalescidos. `is_readable_range` complementa a validação de permissões.
- `Arm11ProcessSession::translate_physical_buffer_tables_from_ipc` exige que o static buffer receptor seja page-aligned, tenha capacidade de até uma página e também seja respaldado por um mapeamento linear físico. A tabela produzida contém `{physical_address, byte_size}` em little-endian e retorna o endereço físico da tabela para substituição no word de payload da mensagem.
- A implementação não fabrica endereços para code/data/TLS/heap host-side ou shared memory sem PA observável. O caminho PXI especial de firmware e as verificações de cache/segurança específicas de hardware ainda exigem integração e testes próprios.
- Testes adicionados cobrem chunks lineares contíguos, rejeição de host backing, criação de uma tabela type-2 e validação de alinhamento do buffer receptor type-3. Executar CI antes de considerar concluído.

## Correção de privilégio CPSR para modos reservados (12/11/2026)

- `89eb900` — `Psr::privileged()` agora reconhece como privilegiados somente FIQ, IRQ, Supervisor, Abort, Undefined e System. User e codificações de modo reservadas não recebem privilégios de controle do CPSR.
- `psr.rs`: teste cobre cada modo privilegiado definido, User e vários valores reservados.
- `arm.rs`: teste A32 confirma que `MSR CPSR_c,r6` não consegue trocar um modo reservado para System.
- A alteração ficou isolada nos arquivos `crates/aceris-core/src/cpu/psr.rs` e `crates/aceris-core/src/cpu/arm.rs`; não modificou `thumb.rs` nem os módulos recentes de IPC.
- Não foi possível executar Rustfmt, Clippy ou `cargo test --workspace` neste ambiente porque Cargo/Rust não estão instalados. A consulta de checks e de Actions para `89eb900` não retornou execuções no momento do registro; os novos testes permanecem pendentes de CI.

## API pública para grupos type-0 de handles IPC (12/11/2026)

- `563fc5f` — adiciona `Arm11ProcessSession::translate_memory_block_handle_groups_from_ipc_message`, entrada pública que recebe um `IpcMessage` já decodificado e traduz todos os grupos type-0 que contenham arrays homogêneos de memory blocks.
- A API retorna `Arm11IpcHandleGroup` em ordem de mensagem, preservando `word_offset` e os handles correspondentes no processo destinatário. O chamador continua responsável por substituir os words de handles na cópia receptora da mensagem.
- Grupos de outras famílias (static buffers, mapped buffers e tabelas físicas) são decodificados estruturalmente, mas ficam para os helpers específicos. Isso não é ainda um dispatcher IPC geral nem o caminho de `svcSendSyncRequest`.
- A validação de handles de origem, contagens e duplicatas MOVE ocorre antes de criar handles de destino; os destinos de todos os grupos são criados antes de fechar qualquer origem MOVE.
- Dois testes cobrem COPY e MOVE em grupos type-0 separados por um descriptor type-1, e rejeição de MOVE duplicado entre grupos sem alteração das tabelas de handles.
- A execução `43315214483` sinalizou apenas formatação Rustfmt nos asserts dos testes; `1d25d5d` aplicou o formato indicado. A CI associada ao HEAD posterior `1d25d5d` está pendente neste registro. Não considerar Clippy/testes aprovados até consultar essa execução.

## Consolidação adicional da CPU ARM11 e validação da matriz (12/11/2026)

### IPC — grupos de handles e buffers físicos

- `5f0c0fe` — preserva o erro específico `UnsupportedTranslationParameter` quando o helper static-buffer é chamado sem qualquer descritor type-1, mas mantém a aceitação de mensagens mistas quando existe type-1. Corrige os fixtures físicos type-2/type-3 para usar o alias virtual `0x14001000`, coerente com a reserva FCRAM e a janela linear antiga.
- `5d1b0b5` — formata a closure do mapeamento de erro conforme Rustfmt.
- `427ed7e` — corrige a passagem do descritor IPC já pertencente ao valor, removendo dereference indevido.
- A execução `43315819768` para `427ed7e` terminou com sucesso em Linux, Windows e macOS (`cargo fmt --all --check`, Clippy com `-D warnings` e `cargo test --workspace`) e no build Android `arm64-v8a`.

### CPU A32 — MRS/MSR e CPSR

- `8235d94` — rejeita `r15/PC` como origem register no MSR A32, tanto para CPSR como para SPSR; acrescenta teste de regressão.
- `f018244` — corrige o endereço de execução do teste para o PC mapeado em zero. A execução `43316198359` concluiu com sucesso a matriz Linux, Windows, macOS e Android ARM64.
- `00c1795` — corrige a máscara dos bits graváveis por MSR em CPSR. O modelo ARM1136 permite NZCVQ, GE[3:0] e E em todos os modos; A/I/F e M[4:0] exigem modo privilegiado; J/T não são alterados pelo MSR nesta implementação ARM1136. Acrescenta testes de User e modo privilegiado com todos os bits de origem ligados.
- `d36a60f` — ajuste Rustfmt dos testes. A execução `43316552656` concluiu com sucesso Linux, Windows e macOS (formatação, Clippy e testes do workspace) e Android ARM64.
- Base arquitetural independente: ARM1136JF-S Technical Reference Manual, seção 2.9.8, CPSR control; não foi consultado código-fonte de outro emulador.

### CPU ARM11 — entrada IRQ/FIQ e SVC

- `fd3c6e8` — entrada IRQ agora define A|I; entrada FIQ define A|I|F. Testes verificam A no IRQ, FIQ e caminho de FIQ mascarado seguido de IRQ.
- `b5d8b31` — aplica o formato Rustfmt à asserção de máscaras. A execução `43316886226` terminou com sucesso em Linux, Windows e macOS (`fmt`, Clippy e workspace tests) e Android ARM64.
- `8ca7d79` — adiciona testes de SVC interceptado pelo HLE (sem entrada de exceção por padrão) e de entrada SVC de hardware no vetor `0x08`, salvamento de SPSR/LR e retorno por `MOVS PC,LR`.
- `726a256` — ajusta o formato Rustfmt das duas asserções de SVC. A execução `43317039866` terminou com sucesso em Linux, Windows e macOS (`fmt`, Clippy e workspace tests) e Android ARM64.
- A correção IRQ/FIQ usa a regra do ARM1136 para ativar o bit A na entrada de Abort, IRQ e FIQ. O estado A anterior é restaurado via SPSR no retorno.

### Estado de verificação

- HEAD de código `726a256`: matriz completa aprovada no workflow `43317039866`.
- As alterações IRQ/FIQ em `fd3c6e8` e `b5d8b31` também foram validadas no workflow `43316886226`.
- A validação confirmada inclui Rustfmt, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` em Linux, Windows e macOS, mais `cargo ndk -t arm64-v8a build -p aceris-android --release`.
- Isso não comprova conformidade integral ARMv6 nem execução de jogos. As próximas auditorias A32 continuam necessárias; `thumb.rs` permanece fora deste escopo para preservar o isolamento de trabalho.

## ARM1136 A32 — saturação paralela e NOP verdadeiro (12/11/2026)

### SSAT16 / USAT16

- `8a3c3a2` — implementa `SSAT16` e `USAT16` A32, saturando os dois halfwords assinados independentemente, definindo `CPSR.Q` sticky quando qualquer lane é limitada e preservando NZCV.
- `8319166` — ajusta o fixture de `USAT16` para testar também a saturação do halfword negativo.
- `0a67bd6` — aplica Rustfmt aos novos caminhos e testes.
- Três testes cobrem saturação signed/unsigned por lane, limites sem saturação, preservação de flags, Q e rejeição de PC nos operandos.
- CI `43317800061` concluída com sucesso em Linux, Windows e macOS (Rustfmt, Clippy com `-D warnings`, `cargo test --workspace`) e Android ARM64 (build release).

### NOP verdadeiro ARM1136

- `462d3e8` — reconhece o encoding A32 de verdadeiro NOP antes da avaliação de condição, para que a condição seja ignorada. O campo Hint não altera o estado no perfil ARM1136JF-S r1p0.
- `988fdb9` — aplica Rustfmt à lista de instruções do teste.
- O teste cobre NOP com condição EQ não satisfeita, AL com Hint não nulo e NV; verifica avanço de PC sem alteração dos registradores ou CPSR.
- CI `43318212186` concluída com sucesso em Linux, Windows e macOS (Rustfmt, Clippy com `-D warnings`, `cargo test --workspace`) e Android ARM64 (build release).
- Referência arquitetural independente: [ARM1136JF-S/ARM1136J-S Technical Reference Manual, seção 2.10.5 — NOP True No Operation](https://documentation-service.arm.com/static/5e8e4457fd977155116ab118). O manual especifica bits 11:8 como SBZ, Hint nos bits 7:0, ausência de efeito do Hint em r1p0 e semântica independente da condição.
- O código e os testes são originais do Aceris; nenhuma implementação de emulador de terceiros foi consultada.

### Verificação consolidada

- HEAD de código `988fdb9`: workflow `43318212186` aprovado em todas as plataformas.
- A matriz confirmou Rustfmt, Clippy e testes do workspace em Linux, Windows e macOS, além do build release Android ARM64.
- O grupo geral de DSP/mídia permanece parcial: SSAT16/USAT16 não concluem a cobertura ARMv6 integral nem substituem a auditoria das demais codificações A32.

## ARMv6 A32 — transferências com privilégio User (12/11/2026)

- `4fdeac6` — adiciona ao trait `Bus` operações explícitas `read8_unprivileged`, `write8_unprivileged`, `read32_unprivileged` e `write32_unprivileged`. Buses genéricos mantêm comportamento padrão; o barramento MMU aplica as permissões solicitadas.
- `4fdeac6` — implementa os caminhos A32 `LDRT`, `LDRBT`, `STRT` e `STRBT` (P=0/W=1). O endereço efetivo é o endereço base original; writeback é pós-indexado e só ocorre após a transação ser concluída.
- O modo atual do processador não é trocado para User. Somente a resolução daquela transação no MMU/TLB recebe `privileged=false`; os acessos normais continuam usando `Params.privileged`.
- O TLB não é ignorado nem invalidado: uma entrada já preenchida é resolvida novamente com o privilégio da transação, inclusive para leitura/escrita e para acessos por byte. O domínio continua sendo avaliado pela regra normal.
- `4fdeac6` inclui dois testes: revalidação User de permissões em entradas TLB já em cache e execução integrada das quatro instruções numa página somente privilegiada, que deve gerar falha sem writeback nem alteração de memória.
- `3ada228` acrescenta o caminho positivo: as quatro instruções completam quando as permissões User permitem acesso, comprovando dados carregados/armazenados, writeback pós-indexado e permanência em Supervisor.
- CI `43321797092` concluída com sucesso em Linux, Windows e macOS (`cargo fmt --all --check`, Clippy com `-D warnings`, `cargo test --workspace`) e Android ARM64 (`cargo ndk -t arm64-v8a build -p aceris-android --release`).
- A validação cobre esses quatro opcodes e os cenários testados; não significa que todos os casos UNPREDICTABLE, faults de alinhamento/endian, todas as variantes LDR/STR ou toda a arquitetura ARMv6 estejam completos.
- Referências arquiteturais independentes: ARM1136JF-S Technical Reference Manual (ARM DDI 0211K), tabelas de instruções de transferência; ARM Architecture Reference Manual ARMv6, descrições A32 de LDRBT/STRT/STRBT. Nenhum código-fonte de outro emulador foi consultado.



## ARMv6K A32 — event hints WFE/WFI/SEV (12/11/2026)

### Implementação

- `cd3ef7f` — separa os hints A32 `WFE`, `WFI` e `SEV` do grupo `NOP32`. Os hints com ação observável respeitam o campo de condição ARM; `NOP32`, `YIELD` e os valores reservados permanecem sem efeito arquitetural visível neste perfil.
- `0675fe8` — acrescenta o registrador de evento ARMv6K por núcleo e a regra de wake-up: IRQ/FIQ não mascaradas geram WakeUpAction e registram o evento; WFE adormecido só retoma quando o registrador é setado; a flag permanece setada depois do wake-up e é consumida pelo próximo WFE executado.
- `3f665c7` — integra os estados ao caminho de sessão. `WaitForEvent` interrompe o lote de instruções enquanto o núcleo aguarda; a espera repetida não aumenta a contagem de instruções. `SendEvent` chega ao chamador como `Arm11RunStopReason::SendEvent`. `Arm11ProcessSession::signal_cpu_event` e `AcerisSystem::signal_cpu_event` permitem injetar eventos externos no contexto atualmente selecionado.
- WFI A32 utiliza o mesmo estado de espera do WFI via CP15: qualquer IRQ/FIQ desperta o núcleo mesmo que CPSR.I/F impeça a entrada da exceção. O avanço do PC ocorre quando a instrução WFI é aposentada.
- Testes A32 em `arm.rs` cobrem WFE com evento já pendente, espera, sinalização externa, consumo tardio do latch, IRQ mascarada/não mascarada, WFI com interrupção mascarada, SEV, condições ARM e hints reservados.
- Testes de sessão cobrem interrupção do lote e retomada após `signal_cpu_event`, contagem de instruções e propagação do stop reason `SendEvent`.

### Limites explícitos

- O emulador ainda não possui um agendador SMP integrado que propague automaticamente `SEV` a todas as instâncias de CPU. A instrução é exposta ao agendador externo por `SendEvent`; o broadcast real deve ser conectado quando o modelo multi-core for desenvolvido.
- A entrada `signal_cpu_event` sinaliza o núcleo atualmente selecionado. Ainda não há API de endereçamento para sinalizar outro núcleo específico.
- Eventos de debug/halting debug não são modelados. O comportamento aqui cobre os caminhos de evento e IRQ/FIQ testados; não representa a implementação integral da arquitetura ARMv6K.
- Nenhum código de Thumb foi alterado; nenhuma implementação de outro emulador foi consultada.

### Referência e verificação

- Base arquitetural independente: ARM11 MPCore Processor Technical Reference Manual (ARM DDI 0360E), seções A32 de SEV/WFE e descrição dos event registers, WakeUpAction e UnMaskedWakeUpAction: https://documentation-service.arm.com/static/5e8e4457fd977155116ab118
- Workflow [43327027270](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43327027270) concluído com sucesso: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` também aprovado.


## ARM11 MPCore — retorno de exceção como WakeUpAction (12/11/2026)

### Implementação

- O ARM11 MPCore TRM (ARM DDI 0360E, seção 2.11.12, páginas 2-47/2-48) define o retorno de exceção de outro MP11 CPU como uma `WakeUpAction` para o núcleo receptor. A mesma seção diz que o registrador de evento é setado por `WakeUpAction`, `SendEvent` de qualquer MP11 CPU ou evento externo EVNTIN.
- `4a2eff3` iniciou a exposição do retorno como evento de CPU/sessão, mas a primeira versão alterou o resultado de `Arm11Core::step` e fez falhar quatro expectativas já existentes que esperavam `StepEvent::Executed`, incluindo o caso de handler ARM retornando ao fluxo Thumb.
- `f42ef6c` corrigiu o contrato: `restore_status_and_branch` agora seta `exception_return_wakeup` no core e mantém o resultado direto de `Arm11Core::step`. O latch público `take_exception_return_wakeup()` é consumido uma vez pelo host scheduler.
- `3064309` aplica o formato Rustfmt solicitado pela CI.
- `Arm11ProcessSession::step` consulta o latch depois de cada instrução e converte a notificação em `StepEvent::ExceptionReturn`; `run_instructions` a expõe como `Arm11RunStopReason::ExceptionReturn`. Assim a camada de agendamento pode encaminhar o wake-up aos demais cores sem alterar o resultado direto da CPU nem invadir o trabalho isolado de Thumb.
- O latch é definido no caminho comum `restore_status_and_branch`, compartilhado por MOVS-to-PC, LDM exception return e RFE.

### Testes e limites

- Teste de CPU confirma que um retorno A32 marca a notificação e que a leitura do latch a consome uma única vez.
- Testes existentes de MOVS-to-PC e RFE continuam esperando `StepEvent::Executed` na API direta do core.
- Teste da sessão confirma que o retorno é reportado como `Arm11RunStopReason::ExceptionReturn`, com PC/CPSR restaurados e contagem de instruções correta.
- A notificação não faz broadcast SMP sozinha. Quando o modelo de múltiplos cores estiver integrado, o scheduler deverá chamar `signal_event()` nos demais núcleos ao receber `ExceptionReturn`. Hoje a sessão modela apenas o core/contexto atualmente selecionado.
- `thumb.rs` não foi editado. O teste de handler ARM chamado a partir de Thumb continua usando o mesmo resultado de CPU `Executed`; a notificação extra fica no latch e só é traduzida pela sessão.
- Eventos de Debug Entry continuam pendentes.

### Verificação

- Workflow [43329839504](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43329839504) concluído com sucesso: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` também aprovado.
- Referência primária: [ARM11 MPCore Processor Technical Reference Manual, ARM DDI 0360E](https://documentation-service.arm.com/static/5e8e1cd9fd977155116a4a7a), seção 2.11.12, páginas impressas 2-47 e 2-48.


## Continuação a partir das contribuições recentes de Claude — 12/11/2026

### Trabalho de Claude preservado no `main`

- `ed153e3` — acrescenta `ScreenSurface::to_png()` e o codificador PNG RGBA8 sem dependência externa. `aceris <JOGO> --dump-lcd <DIR>` exporta `top_left.png`, `top_right.png` e `bottom.png`. Workflow `43331314353`: Linux, Windows, macOS e build Android ARM64 aprovados.
- `c9cd08f` — gerações de exclusividade no backing físico com granularidade de 8 bytes; escritas em mapeamentos lineares invalidam LDREX/STREX. Workflow `43331633910` aprovado na matriz desktop e Android ARM64.
- `2af7b1b` — janela X11 por FFI Rust/dlopen, sem crate externo; somente Linux. `--window [--scale N]` compõe a superfície superior esquerda e a LCD inferior; o backend não apresenta a superfície superior direita em paralelo. Win32/Cocoa, janela Android, entrada, áudio e redimensionamento continuam pendentes. Workflow `43332199817` aprovado.
- As três contribuições foram revisadas sem reescrever arquivos de vídeo, memória física ou janela.

### CP15 — atributos de memória e VA→PA/PAR

#### Implementação

- `9fb463b` — adiciona PRRR (`c10,c2,0`), NMRR (`c10,c2,1`) e SCTLR.TRE (bit 28) à camada CP15, com reset, máscaras e testes.
- `b2dedee` — corrige máscara PRRR para incluir bits [1:0] e expõe acessores usados na integração MMU. Workflow `43333329655` aprovado em Linux, Windows, macOS e Android ARM64.
- `7ea966c` — preserva TEX/C/B/S de descritores XP=1 e XP=0 suportados no `TlbEntry`; resolve tipo primário, shareability e atributos inner/outer através de PRRR/NMRR quando TRE está ligado; acrescenta PAR e consulta VA→PA.
- `bc71f5a` — permite que o MCR VA→PA consulte o adaptador MMU/TLB que está executando a instrução, em vez de fazer uma segunda tradução sobre um bus já traduzido.
- `9ab50f6` — acrescenta teste de execução real de `MCR p15,0,Rn,c7,c8,0` com a MMU ativa.
- `3a8f791` — corrige o tipo de resultado do caminho plano/MMU e restaura o TLB mesmo quando uma leitura da tabela de páginas retorna erro.
- `ee6067c` e `09ca53e` — correções de formato e fixtures de `TlbEntry`; workflow `43337369085` aprovado na matriz Linux, Windows, macOS e Android ARM64.
- `4227ae7` — implementa escrita privilegiada de PAR (`MCR c7,c4,0`) além da leitura MRC; adiciona regressão que comprova atributos de memória preservados em entradas TLB existentes até TLBI.
- `ea07fe7` — formata o teste de revalidação pós-invalidação. Workflow `43337932833` aprovado: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado.

#### Semântica e limites que devem ser preservados

- `MCR p15,0,Rn,c7,c8,op2` com op2 0/1/2/3 executa respectivamente verificação de leitura privilegiada, escrita privilegiada, leitura User e escrita User. O MCR é privilegiado; os dois últimos opcodes só mudam o privilégio usado na verificação de permissão da tradução.
- A entrada é o endereço virtual verdadeiro; FCSE remapeia VA baixa internamente antes do lookup/walk. A operação usa o ASID atual e o TLB atual do núcleo. O caminho ativo de `MmuBus` é reutilizado durante execução normal.
- `MRC p15,0,Rd,c7,c4,0` lê PAR; `MCR p15,0,Rn,c7,c4,0` grava PAR em modo privilegiado. O PAR de sucesso contém o endereço físico da página e os atributos codificados; uma falha de tradução/permissão é reportada no formato de fault atualmente modelado, sem transformar a consulta em Data Abort da instrução.
- O modelo de atributos cobre os campos TEX/C/B/S disponíveis nos descritores curtos XP=1 e XP=0 implementados: seção/superseção, large/small page e extended-small-page de coarse table. A resolução de TRE usa `{TEX[0],C,B}` como índice PRRR/NMRR.
- As alterações PRRR/NMRR não reescrevem entradas já guardadas no TLB: o teste comprova que o mapeamento conserva atributos antigos até a invalidação, e recebe os novos atributos depois de TLBI.
- Limites conhecidos: fine tables/tiny pages não são suportadas; os campos adicionais de FSR do PAR (fora do status que o MMU atualmente conserva) ainda não são codificados; caches ARM observáveis, manutenção de cache, IMB/barreiras e coerência efetiva entre CPU e dispositivos continuam pendentes; a resolução de atributos não significa que as caches físicas já estejam implementadas.
- `thumb.rs` não foi alterado. Nenhum código-fonte interno de emuladores de terceiros foi consultado.

Referência primária independente: ARM11 MPCore Processor Technical Reference Manual, ARM DDI 0360F, seções CP15 PRRR/NMRR, SCTLR.TRE, tabelas de atributos de memória e consulta VA→PA/PAR. A documentação pública do processador é permitida; o código continua original do Aceris.


### Cobertura guest-side adicional de VA→PA e FCSE — 12/11/2026

- `8d7a35b` — adiciona teste de integração executando as quatro codificações `MCR p15,0,Rn,c7,c8,op2`: op2 0 (leitura privilegiada), 1 (escrita privilegiada), 2 (verificação de leitura User) e 3 (verificação de escrita User). O fixture concede AP=01 (somente acesso privilegiado) e confirma PAR de sucesso para op2 0/1 e fault de permissão codificado em PAR para op2 2/3.
- O mesmo commit acrescenta teste guest-side que define FCSE PID, busca a instrução na janela MVA do processo e consulta uma VA baixa; o PAR precisa apontar para o mapeamento físico do MVA resultante.
- `26d3bad` — aplica Rustfmt às duas regressões. Workflow `43338851227` aprovado: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado.
- Os testes não completam a conformidade ARM11: fault status adicional do PAR, fine tables/tiny pages, associatividade/replacement real do TLB e efeitos de caches/IMB/barreiras continuam pendentes.

## Continuação do interpretador Thumb — validação de codificações ARMv6/ARMv6K (12/11/2026)

Carlos Costa solicitou continuar diretamente o trabalho anterior do interpretador Thumb. A intervenção ficou limitada a `crates/aceris-core/src/cpu/thumb.rs` e à documentação correspondente, preservando o restante trabalho existente.

### Alterações desta rodada

- O Thumb rejeita agora a família de codificações `BFxx` no perfil ARMv6/ARMv6K. O NOP de 16 bits é uma adição ARMv6T2; os hints YIELD/WFE/WFI/SEV pertencem a arquitetura posterior. O código não deve aceitar essas codificações apenas como no-op.
- As instruções `PUSH` e `POP` rejeitam listas de registradores vazias com `Error::UndefinedInstruction`, seguindo a política determinística de não executar casos arquiteturalmente UNPREDICTABLE.
- Acrescentado teste do alias ARMv6 `CPY`/MOV entre registradores baixos, preservando flags.
- Acrescentado teste de integração que executa SETEND BE, STR e LDR, verificando byte order no caminho de memória Thumb.
- Substituído o teste que chamava `BF00` de NOP válido por uma regressão que confirma sua rejeição. O teste de operação sem efeito usa agora `MOV r8,r8` (`0x46C0`), sem depender do hint NOP ARMv6T2.
- Roadmap atualizado para refletir o estado real e manter Thumb como parcialmente validado.

### Commits

- `877efa1170cf5f37efce620c5f78bf4c71d53821` — correções do interpretador e novos testes.
- O commit de documentação do roadmap foi publicado logo após a alteração de código.

### Verificação

Os testes foram adicionados ao arquivo Thumb. Esta intervenção não dispõe de `rustc`/Cargo local; não declare os testes como executados até verificar a CI em `main`. A matriz configurada precisa aprovar Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux/Windows/macOS, além do build Android ARM64.

### Referências arquiteturais independentes

- ARM Architecture Reference Manual, ARM DDI 0406C.d, tabelas de instruções Thumb de 16 bits e seção de diferenças ARMv6/ARMv6K; usar a documentação oficial para auditar cada codificação, sem consultar código-fonte de outros emuladores.
- ARM Architecture Reference Manual, descrições T1 de PUSH/POP: listas vazias são classificadas como UNPREDICTABLE. O Aceris rejeita esses casos deterministamente e não afirma que a arquitetura os classifique como UNDEFINED.
- Nenhuma dependência externa foi adicionada e nenhum outro emulador foi consultado.


### Continuação da auditoria Thumb — BLX e CI (12/11/2026)

- `b73d1aa3f9e1fac61c196c0b999b2daaa796081d` — alinha a formatação do novo teste CPY com Rustfmt e acrescenta `BLX Rm` com destino bit 0 limpo, verificando a mudança Thumb→ARM, o alinhamento de PC e o LR de retorno.
- A CI anterior (`43342433229`, HEAD `2e4c301`) falhou antes de executar todos os testes: Rustfmt apontou formatação pendente em `thumb.rs` (corrigida pelo commit acima) e também em `cpu/mmu.rs`; o job Android falhou na compilação do mesmo `cpu/mmu.rs`.
- Erros de compilação reportados no Android, todos em `crates/aceris-core/src/cpu/mmu.rs`: `TLB_CAPACITY` não encontrado na linha 349; método `matches_context` chamado em `&Option<TlbEntry>` na linha 397; e incompatibilidade `Option<TlbEntry>`/ `TlbEntry` na linha 399. A falha de formatação em `mmu.rs:371` também pertence à alteração do TLB.
- Esses erros não estão em `thumb.rs`; não foram alterados nesta tarefa para respeitar o isolamento entre agentes. A correção do MMU exige autorização específica do mantenedor ou intervenção do agente responsável por essa área.
- Após `b73d1aa`, o workflow `43342586393` foi iniciado para a nova HEAD. Seu resultado ainda deve ser verificado; até então, os testes Thumb não podem ser declarados aprovados.


### Resultado verificado da CI para a correção Thumb — workflow 43342586393

- A etapa `cargo fmt --all --check` já não reporta diferenças em `cpu/thumb.rs`; a única diferença apontada por Ubuntu e macOS permanece em `crates/aceris-core/src/cpu/mmu.rs:371`, no formato de `TLB_PAGE_SHIFTS.map(...)`.
- O job Android falhou antes de chegar à validação Thumb por erros de compilação no TLB em `cpu/mmu.rs`: constante `TLB_CAPACITY` ausente, chamada `matches_context` em `Option<TlbEntry>` e retorno com tipo `Option<TlbEntry>` onde se esperava `TlbEntry`.
- O job Windows ainda estava em execução na consulta. Os jobs Linux/macOS e Android não chegaram a Clippy ou `cargo test`; portanto, nenhum teste do interpretador Thumb foi validado por essa execução.
- A área MMU permanece sem alterações nesta tarefa. O próximo agente responsável pelo TLB deve corrigir a implementação e o formato em `mmu.rs`; depois disso, reexecutar a matriz inteira para validar também os novos testes Thumb.


## Continuação do interpretador Thumb — validação ARMv6 e ampliação de testes (13/11/2026)

Trabalho executado diretamente em `main`, com alterações de código limitadas a `crates/aceris-core/src/cpu/thumb.rs`. Nenhuma alteração foi feita no MMU ou em arquivos atribuídos a outra frente.

### Estado e alterações

- O módulo Thumb agora contém 62 testes declarados. Isso é contagem de testes no código, não quantidade de testes executados com sucesso.
- `877efa1` — rejeita deterministicamente as codificações `BFxx` fora do perfil ARMv6/ARMv6K e listas vazias em PUSH/POP; acrescenta regressões para CPY, endian e atomicidade.
- `b73d1aa` — acrescenta caso de BLX por registrador que muda de Thumb para ARM.
- `df5bd80` — valida os bits SBZ de BX/BLX, rejeita BLX com PC como Rm e destinos ARM halfword-aligned (bits[1:0] == 10), sem modificar PC/LR/CPSR nos casos rejeitados; também valida o bit fixo do sufixo BLX imediato.
- `39f02cf` — testa a extensão de sinal do offset alto negativo em um par BL.
- `24a067a` — acrescenta testes de SBC, ASR por registrador, LSR/ASR com imediato zero e transferências halfword/signed em big-endian.
- `6d7de5f` — acrescenta testes de CMN, CMP entre registradores altos e regras de writeback de LDMIA/STMIA quando o registrador base está na lista.
- `6224efc` — remove whitespace final dos testes novos para Rustfmt.

### Cobertura consultada

A tabela A7-1 do ARM Architecture Reference Manual ARM DDI 0100I enumera as instruções Thumb presentes em ARMv6. Ela inclui as famílias clássicas e as extensões v6 como BKPT, BLX, CPS, CPY, SETEND, REV, SXTB/SXTH e UXTB/UXTH. O conjunto ARMv6 do Aceris foi comparado com essa tabela; Thumb-2/ARMv6T2 não foi introduzido como requisito.

Referência: https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf — seção A7.2, tabela A7-1, páginas A7-125/A7-126. As regras específicas de BX/BLX, LDMIA e STMIA foram verificadas nas descrições A7.1 correspondentes.

### CI e bloqueio externo

- Workflow `43343294738`, HEAD `8286c77`: os logs de Rustfmt de Ubuntu/macOS apontaram somente `crates/aceris-core/src/cpu/mmu.rs:371`; não apontaram diferenças em `cpu/thumb.rs`. Os jobs Clippy/testes foram ignorados porque `cargo fmt --check` falhou primeiro.
- O mesmo workflow falhou no build Android antes da validação Thumb por problemas de compilação em `cpu/mmu.rs`: `TLB_CAPACITY` não encontrado na linha 349, `matches_context` chamado em `&Option<TlbEntry>` na linha 397 e incompatibilidade `Option<TlbEntry>`/ `TlbEntry` na linha 399.
- O workflow `43343388687` foi disparado para `6224efc` e estava enfileirado na última consulta. Ainda é necessário verificar Rustfmt após os testes finais, Clippy e `cargo test` quando a compilação do MMU estiver corrigida.
- O MMU não foi editado nesta tarefa para preservar o isolamento entre agentes. Enquanto o TLB não compilar e passar no Rustfmt, não declarar o interpretador Thumb validado pela CI.

### Limites remanescentes

- Testes do módulo não foram executados localmente nesta intervenção; só foram declarados no código.
- A semântica completa de faults/alinhamento de memória Thumb depende da integração comum CPU/MMU/CP15 e precisa ser validada junto com o subsistema de memória.
- A validação de todas as famílias de instruções não significa que cada caso de alinhamento, abort, acesso ao PC, timing ou comportamento UNPREDICTABLE esteja concluído.
- Nenhuma dependência foi adicionada; não foi consultado código-fonte de emuladores de terceiros.


### Auditoria adicional do Thumb ARMv6K — alinhamento e codificações reservadas (13/11/2026)

Base de código: `e6b54ac3857360fca1292682eb6e1fb5b447fbd1`. O módulo `crates/aceris-core/src/cpu/thumb.rs` tem 65 testes declarados.

- `8c08628` — rejeita `STMIA Rn!,{...}` quando Rn está na lista, mas não é o registrador de menor número; mantém a forma válida quando Rn é o menor e testa que a rejeição não altera memória nem registradores.
- `ab6b2d3` — `POP {PC}` agora rejeita o valor carregado com bits[1:0] = `10` antes de alterar SP/PC/CPSR, pois tentaria entrar em ARM com alinhamento inválido.
- `e6b54ac` — rejeita `ADD` e `CMP` na forma de registradores altos quando ambos H1/H2 são zero (ambos operandos baixos), combinação UNPREDICTABLE. A guarda não inclui MOV: em ARMv6, o caso MOV low-to-low nesse formato é CPY e permanece válido.
- A revisão de conformidade mantém `BF00` (NOP Thumb), IT e os hints Thumb `YIELD/WFE/WFI/SEV` rejeitados: NOP T16/IT são ARMv6T2; YIELD/WFE/WFI/SEV T16 aparecem em ARMv7, enquanto as formas A32 de WFE/WFI/SEV são uma extensão separada ARMv6K. Não confundir suporte A32 com suporte Thumb.
- As transferências multi-register continuam dependentes de validação integrada de faults/alinhamento e da semântica do bus/MMU; não se deve afirmar conformidade completa só pelos testes sintéticos locais.
- CI para `e6b54ac` (`43343866363`) falhou antes de executar Clippy/testes: Rustfmt reportou somente a diferença antiga em `cpu/mmu.rs:371`; Android falhou na compilação de `cpu/mmu.rs` com `TLB_CAPACITY` não encontrado, `matches_context` em `Option<TlbEntry>` e incompatibilidade Option/TlbEntry. O log não reportou diferença de formatação em `cpu/thumb.rs`.
- O MMU não foi modificado nesta tarefa. O bloqueio exige intervenção autorizada no arquivo do TLB antes de executar a matriz completa.


### Continuação Thumb — PC em registradores altos e confirmação da CI (13/11/2026)

- `4e726f2f9b7b9f4e083c8add24051bd1c735e425` — atualiza os comentários do módulo para refletir o perfil real do ARMv6 Thumb e acrescenta `high_register_pc_operands_read_current_instruction_plus_four`. O teste executa MOV, ADD e CMP usando PC como operando com instrução no endereço 2 e confirma leitura PC=6, preservando bit[1] nas formas de registradores altos.
- O módulo `crates/aceris-core/src/cpu/thumb.rs` possui agora 66 declarações `#[test]`. A quantidade é contagem do código, não testes executados com êxito.
- Workflow `43344492372` (commit `4e726f2`) confirma que `cargo fmt --all --check` não reportou diferenças em `cpu/thumb.rs`; a única diferença reportada continua em `crates/aceris-core/src/cpu/mmu.rs:371`. Os jobs Clippy/testes são ignorados porque Rustfmt falha antes.
- O build Android ARM64 continua falhando ao compilar `cpu/mmu.rs`: `TLB_CAPACITY` não encontrado em 349; método `matches_context` chamado em `&Option<TlbEntry>` em 397; incompatibilidade entre `Option<TlbEntry>` e `TlbEntry` em 399.
- A CI, portanto, ainda não executou `cargo test --workspace` para validar estes 66 testes. Não declarar o interpretador concluído nem testes aprovados até que o MMU compile e a matriz seja concluída.
- Nenhum arquivo do MMU ou de outra frente foi alterado nesta continuação.


### Auditoria Thumb — restrições de PC em CMP e BX (13/11/2026)

- Commit de código: `ee367cb69e8ffbc54a817f3eed1cdeb4ef761fd3` — valida `CMP (3)`: o primeiro operando `Rn` não pode ser `R15/PC`; a codificação é rejeitada como `UndefinedInstruction` antes de ler/alterar registradores ou CPSR. `PC` permanece permitido como segundo operando `Rm`.
- Foram adicionados três testes: rejeição atômica de `CMP PC,R0`; `BX PC` em endereço de instrução alinhado a palavra, que deve saltar para o próximo word e entrar em ARM; e `BX PC` em endereço apenas halfword-aligned, cujo destino PC+4 tem bits[1:0] = `10` e é imprevisível.
- A regra foi conferida no ARM Architecture Reference Manual, ARM DDI 0100I: A7.1.23 `CMP (3)` (página A7-37/A7-38, restrição de `Rn`) e A7.1.19 `BX` (A7-32, uso de R15). Referência: https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf
- O módulo contém agora 69 declarações `#[test]`. Isso é contagem estática, não resultado de execução.
- O workflow `43345628033` terminou com falha. Os jobs Windows, macOS e Linux reprovaram somente `cargo fmt --all --check` por uma diferença em `crates/aceris-core/src/cpu/mmu.rs:371`; nenhum deles apontou diferença em `cpu/thumb.rs`.
- O job Android ARM64 falhou ao compilar o mesmo `mmu.rs`: `TLB_CAPACITY` não encontrado (linha 349), `matches_context` chamado em `&Option<TlbEntry>` (linha 397) e incompatibilidade de tipo entre `Option<TlbEntry>` e `TlbEntry` (linha 399).
- Clippy e `cargo test --workspace` foram ignorados porque Rustfmt falhou primeiro. Portanto, os 69 testes declarados ainda não foram validados pela CI. O MMU permanece sem alterações; é necessário corrigir seu bloqueio antes da validação integral.
- Nenhum outro arquivo de código, dependência ou implementação de terceiros foi incluído nesta alteração.


### Testes Thumb — acesso desalinhado de palavra no modelo legado (13/11/2026)

- Commit de código: `6b256700e7b1c25a8df84a2daf6c7644fe9661b9` — acrescenta dois testes que exercitam loads/stores Thumb de palavra desalinhados por imediato e por registrador, em little-endian e big-endian.
- No caminho legado ARMv6 (`U=0`, `A=0)), o word load lê `Word[Align(Addr)]` e rotaciona o dado conforme `Addr[1:0]`; o word store escreve em `Word[Align(Addr)]`. Os testes cobrem as codificações T1 `LDR/STR` de offset imediato e offset por registrador.
- Referência independente: ARM Architecture Reference Manual, ARM DDI 0100I, seção A2.8.2 e Tabela A2-10, páginas A2-39 a A2-41. A tabela distingue o comportamento legado `U=0,A=0` do modo ARMv6 de acessos desalinhados `U=1,A=0`. Link: https://documentation-service.arm.com/static/5f8dacc8f86e16515cdb865a
- O caminho de dados Thumb atual ainda utiliza a leitura com rotação clássica e o alinhamento explícito dos stores; estes testes não implementam nem atestam `U=1`. A seleção completa entre legacy/U=1, faults de alinhamento e atributos de memória depende da integração comum CPU/CP15/MMU e continua pendente no roadmap.
- O módulo possui agora 71 declarações `#[test]`; isso é contagem estática, não testes executados com sucesso.
- O workflow `43346423350` terminou com falha em todos os quatro jobs. Linux, Windows e macOS falharam apenas em `cargo fmt --all --check`, e cada log apontou exclusivamente `crates/aceris-core/src/cpu/mmu.rs:371`; nenhum apontou diferença em `cpu/thumb.rs`.
- O Android ARM64 falhou ao compilar `mmu.rs` pelos mesmos três erros do TLB: `TLB_CAPACITY` ausente em 349, `matches_context` aplicado a `&Option<TlbEntry>` em 397 e incompatibilidade entre `Option<TlbEntry>` e `TlbEntry` em 399.
- Clippy e `cargo test --workspace` foram ignorados nos três sistemas desktop porque Rustfmt falhou primeiro. Os 71 testes Thumb ainda não foram executados pela CI; os novos testes de alinhamento não estão validados por execução.
- O MMU permanece intocado nesta intervenção. A CI não pode validar o Thumb até a frente responsável resolver esses erros e a formatação em `mmu.rs`.
- Nenhuma alteração foi feita fora de `thumb.rs` na parte de código, nenhuma dependência foi adicionada e nenhum código de emulador de terceiros foi consultado.


### Thumb — alinhamento das transferências multiword e SCTLR.U/A (13/11/2026)

- Commit de código: `c4d875aa57e6b84e472c4fa9a8418a89923e6000`.
- Corrige `thumb_block_transfer`: no modelo ARMv6 legado (`U=0,A=0)), LDMIA/STMIA iniciam as transações em `Word[Align(Addr)]`, mas calculam o writeback a partir do valor original de `Rn`, preservando os bits baixos no valor escrito de volta.
- Quando `SCTLR.U` ou `SCTLR.A` está ativo, uma base LDMIA/STMIA desalinhada retorna `Error::InvalidAccess` antes de qualquer alteração em registradores ou memória. A regra reutiliza `Cp15::alignment_fault_enabled()`, já existente e usada no caminho SRS A32. A integração final desse erro como Data Abort e o estado completo de DFAR/DFSR continuam dependentes da validação de faults do core/MMU.
- Um novo teste exercita LDMIA e STMIA com `Rn=DATA+1`, nos dois endians; verifica leitura/escrita iniciando em Align(Addr), writeback em `original_base + 8` e fault atômico para SCTLR.U e SCTLR.A separadamente.
- Referência: ARM Architecture Reference Manual, ARM DDI 0100I, A2.8.2, Tabela A2-10, páginas A2-40 e A2-41. `U=0,A=0` determina `Word[Align(Addr)]` para Multi-word; com `U=1,A=0`, multiword desalinhado gera Alignment Fault. Link: https://documentation-service.arm.com/static/5f8dacc8f86e16515cdb865a
- Somado aos dois testes de acesso simples de palavra desalinhada em LE/BE, o módulo contém agora 73 declarações `#[test]`. Isso é contagem estática, não execução aprovada.
- Workflow `43346740894` foi iniciado para o commit `c4d875a`; aguardar resultado. Os workflows anteriores não alcançaram Clippy ou `cargo test --workspace` devido à formatação/compilação pendente em `mmu.rs`. Não declarar os testes aprovados antes de a CI executá-los.
- O trabalho de código permaneceu restrito a `crates/aceris-core/src/cpu/thumb.rs`; nenhum arquivo MMU de outra frente foi alterado.


### Thumb — alinhamento de PUSH/POP e validação CI (13/11/2026)

- Commit de código: `d0725535089ea2e77e77fd69653482034e686257` — aplica as regras de alinhamento de transferências multiword também a `PUSH` e `POP`.
- No caminho legado (`SCTLR.U=0`, `SCTLR.A=0`), `PUSH` calcula o SP decrementado original, acessa as palavras a partir de `Align(newSP)` e mantém o writeback no SP não arredondado. `POP` inicia as leituras em `Align(SP)` e soma o tamanho da lista ao SP original no writeback.
- Quando SCTLR.U ou SCTLR.A está habilitado, `PUSH` rejeita o novo SP desalinhado e `POP` rejeita o SP desalinhado com `Error::InvalidAccess` antes de acessar memória ou alterar registradores.
- Foram acrescentados dois testes: round-trip PUSH/POP desalinhado em LE/BE; e faults atômicos para os dois opcodes, com SCTLR.U e SCTLR.A testados separadamente. O módulo agora contém 75 declarações `#[test]`; essa contagem não significa execução aprovada.
- Referência arquitetural: ARM Architecture Reference Manual ARM DDI 0100I, A2.8.2 / Tabela A2-10, páginas A2-40/A2-41; regras multiword também cobrem os aliases de pilha `PUSH`/`POP`. Link: https://documentation-service.arm.com/static/5f8dacc8f86e16515cdb865a
- O workflow `43346891107` confirmou que o formato de `thumb.rs` foi corrigido: os logs de Ubuntu/macOS/Windows apontaram somente `mmu.rs:371`, sem diferenças em Thumb. Android continuou falhando na compilação do MMU.
- O workflow `43346990325` (HEAD `d072553`) terminou com falha nos quatro jobs: Windows/Linux/macOS falharam somente em `cargo fmt --all --check` por `mmu.rs:371`; Android ARM64 repetiu os erros de TLB (`TLB_CAPACITY` ausente em 349, `matches_context` em `Option<TlbEntry>` em 397 e incompatibilidade Option/TlbEntry em 399). Clippy e `cargo test --workspace` foram ignorados; nenhum dos 75 testes Thumb foi executado nesta matriz.
- O código desta etapa permaneceu limitado a `crates/aceris-core/src/cpu/thumb.rs`. Nenhum arquivo MMU de outra frente foi alterado. A validação end-to-end de Data Abort/DFAR/DFSR e o caminho de alinhamento `U=1` para transferências simples continuam pendentes.


## Thumb ARMv6 — integração U/A de meia palavra e faults multiword (13/11/2026)

### Continuação e commits

- `5d0d17b` — acrescenta teste integrado para `LDMIA`, `STMIA`, `PUSH` e `POP` com `SCTLR.U` e `SCTLR.A` habilitados separadamente. Cada caso percorre o tratamento real da exceção e verifica vetor Data Abort, LR, SPSR, DFAR/DFSR e ausência de alterações parciais em estado/memória.
- `501f24a` — fortalece a regressão: a região de memória comparada agora inclui os endereços efetivamente usados por `PUSH`, e o teste troca de volta para User mode para confirmar que o SP do processo foi preservado depois da entrada em Abort.
- `70f1648` — adiciona `read_armv6_halfword` e `write_armv6_halfword` ao núcleo ARM11 e passa a usá-los nos formatos Thumb T1 de `LDRH`, `LDRSH` e `STRH`, tanto por offset de registrador quanto por imediato.
- `d7e2ed3` — aplica as correções solicitadas pelo Rustfmt às novas regressões e aos caminhos Thumb.

### Semântica aplicada

- `SCTLR.U=1`, `SCTLR.A=0`: `LDRH`, `LDRSH` e `STRH` podem acessar meia palavra ímpar como `Halfword[Addr]`, preservando o endianness corrente e exigindo atributo de memória Normal para o caminho desalinhado.
- `SCTLR.U=0`, `SCTLR.A=0`: meia palavra ímpar é arquiteturalmente `UNPREDICTABLE`; o Aceris a rejeita deterministamente sem efeitos colaterais, usando o erro interno de instrução inválida. Isso não afirma que o caso seja arquiteturalmente `UNDEFINED`.
- `SCTLR.A=1`: acesso de meia palavra ímpar retorna fault de alinhamento antes de alterar destino ou memória. Com a entrada de faults habilitada, a sessão entra em Data Abort e registra DFAR/DFSR.
- Loads/stores simples de palavra continuam usando o helper ARMv6 compartilhado que seleciona rotação/alinhamento legado (`U=0`) ou os bytes exatos (`U=1`) em LE/BE. Multiword (`LDMIA`/`STMIA`/`PUSH`/`POP`) permanece word-aligned quando U ou A exige alinhamento.
- A suíte de Thumb contém agora 82 declarações `#[test]`. A CI do HEAD `d7e2ed3` executou Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS, além do build release Android `arm64-v8a`; workflow `43357433947` concluído com sucesso.
- Referência arquitetural independente: ARM Architecture Reference Manual, ARM DDI 0100I, seção A2.8.2, Tabela A2-10, páginas A2-39 a A2-41. A tabela descreve `Halfword[Addr]` para meia palavra com U=1/A=0, `UNPREDICTABLE` para meia palavra ímpar com U=0/A=0 e Alignment Fault com A=1: https://documentation-service.arm.com/static/5f8dacc8f86e16515cdb865a
- Nenhum código-fonte de outro emulador foi consultado e nenhuma dependência externa foi adicionada.

### Próximos limites

- A implementação Thumb continua parcial: a matriz de instruções ARMv6/ARMv6K precisa de auditoria independente por encoding, efeitos colaterais, comportamento PC e todas as condições UNPREDICTABLE/UNDEFINED.
- A verificação atual não comprova execução de jogos nem substitui testes de conformidade independentes do interpretador.
- Não reabrir as correções anteriores de alinhamento, exceto diante de regressão reproduzível ou nova evidência arquitetural.


## Compartilhamento dos helpers de meia palavra entre A32 e Thumb (13/11/2026)

- `32fa96d` — refatora `halfword_transfer` A32 para usar `read_armv6_halfword` e `write_armv6_halfword`, os mesmos helpers usados pelo interpretador Thumb.
- A verificação de `SCTLR.U/A`, rejeição determinística da meia palavra ímpar no modelo legado e pré-validação de atributos Normal passa a ter uma única implementação para A32/Thumb. As transferências de byte não são afetadas; os caminhos `LDRD/STRD` continuam com suas próprias regras de alinhamento.
- Nenhuma semântica nova foi adicionada por essa refatoração; ela remove a duplicação entre os dois estados e mantém os testes A32/Thumb existentes.
- Workflow `43357747003` concluiu com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` também aprovado.
- Não foi criada branch e nenhuma implementação de terceiros foi consultada.

## Thumb — cobertura adicional de SP, PC e BKPT (13/11/2026)

- `a1330a9` — acrescenta testes para MOV/ADD/CMP com SP como operando alto e BX/BLX com SP como registrador-alvo. O módulo passou a 84 testes.
- `bf7c0e9` — testa o evento BKPT com os imediatos `0xA5` e `0xFF`, avanço do PC em dois bytes e preservação dos registradores gerais, flags e modo. O dispatcher da sessão já converte `StepEvent::Breakpoint` em `Arm11RunStopReason::Breakpoint`, com teste integrado existente em `session.rs`.
- `28b26e0` — fortalece o teste de BKPT para comparar o CPSR inteiro, além dos registradores gerais.
- `9b6f41a` — estende a regressão de escrita no PC com ADD high-register para validar leitura de PC+4, branch sem interworking, manutenção do estado Thumb e flags inalteradas.
- O módulo Thumb contém agora 85 declarações `#[test]`. Não foi alterado o caminho de execução de produção neste conjunto: as mudanças são regressões de CPU.
- CI do commit `9b6f41a0abc63eba76c2644198679d9f0d64c00b`: workflow `43359531774`; no momento da atualização, Ubuntu, Windows e build Android ARM64 concluíram com sucesso, enquanto macOS ainda executava `cargo test --workspace`. Não declarar a matriz concluída até o término desse job.
- Referência: ARM Architecture Reference Manual, ARM DDI 0100I, seção A6.4.2 e descrição de ADD (4), páginas A6-10 a A7-9. O formato permite registradores altos e mantém as flags inalteradas, exceto CMP. Nenhuma implementação de emulador de terceiros foi consultada.

## Thumb — PC relativo e pipeline de branches em endereços halfword-aligned (13/11/2026)

- `81773fd` — acrescenta testes de `LDR` literal e `ADR` com a instrução em endereço 2. Ambos confirmam `Align(PC + 4, 4)` antes da soma do imediato.
- `e050387` — acrescenta cobertura para branches condicionais e incondicionais para frente e para trás, executados em endereço 2. O teste também verifica o caso condicional não tomado e a preservação do CPSR.
- O mesmo commit testa os pares `BL` e `BLX` com prefixo em endereço 2: o prefixo usa `PC + 4` sem alinhamento para preparar LR; `BL` completa o destino Thumb, enquanto `BLX` alinha o destino final a palavra e muda para ARM.
- A suíte Thumb contém agora 89 declarações `#[test]`. Essas alterações são testes de regressão; não mudam o caminho de execução de produção.
- CI do commit `e050387d05e82bfd50da818695bf0dd7cec4dcc2`: workflow `43360946115` concluído com sucesso. Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` aprovados em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado.
- Não foi criada branch, não foram adicionadas dependências e nenhuma implementação de emulador de terceiros foi consultada.

## Thumb — limites dos offsets de branch e sinalização (13/11/2026)

- `0219151` — adiciona testes para os limites positivo e negativo de branches condicionais e incondicionais, além de limites de deslocamento nos pares `BL` e `BLX`.
- `8ba97df` — corrige os fixtures condicionais para usar `BNE`, condição satisfeita no estado inicial de teste (`Z=0`).
- A primeira execução da CI identificou que o fixture de `BL` usava `0xF7FF`, que é prefixo e não sufixo. `3e54357` corrige-o para `0xFFFF`, codificação de sufixo `BL` com imediato baixo máximo.
- `conditional_and_unconditional_branch_offsets_sign_extend_at_limits` verifica `BNE` com offsets `+254` e `-256`, e `B` com offsets `+2046` e `-2048`, todos com a instrução iniciando em endereço 2.
- `bl_and_blx_pair_offsets_sign_extend_at_architectural_limits` verifica o limite positivo do par `BL` e o limite negativo do par `BLX`, incluindo preparo de LR, retorno e alinhamento do destino ARM.
- O módulo Thumb contém agora 91 declarações `#[test]`. Não houve alteração do caminho de execução de produção neste conjunto; são testes de regressão.
- CI final: workflow `43361987199` concluído com sucesso. Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` aprovados em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado. A execução anterior `43361873343` falhou apenas pelo fixture de sufixo incorreto, corrigido em `3e54357`.
- Referência independente: ARM Architecture Reference Manual, ARM DDI 0100I, seção A7.1.17 (BL, BLX), que descreve o par de instruções e o cálculo dos offsets; A7.1.17 também especifica alinhamento de palavra para o destino BLX.
- Não foram criadas branches, adicionadas dependências ou consultadas implementações de outros emuladores.

## A32 — aliases de destino nas famílias de multiplicação DSP (13/11/2026)

- `24c8539` — acrescenta testes independentes para aliases de destino nas operações `SMLAD/SMLSD`: `Rd` coincide separadamente com `Rm`, `Rs` ou `Ra`. O modelo de referência calcula produtos e acumulador a partir dos valores pré-instrução e verifica o resultado, o Q sticky e a preservação de NZCV.
- `24c8539` — acrescenta também cobertura para `SMLALD/SMLSLD` com `Rm` sobrepondo `RdHi` e `Rs` sobrepondo `RdLo`, verificando que os operandos e o acumulador de 64 bits são lidos antes da gravação do par de destino.
- `6ffd484` — aplica os ajustes de Rustfmt identificados pela primeira execução da CI. Nenhum código de produção foi alterado nesta etapa.
- Referência arquitetural independente: ARM Instruction Set Reference Guide, entradas C2.114 (`SMLAD`) e C2.119 (`SMLALD`), que definem a operação e as restrições de registradores. A referência inclui um exemplo de SMLAD com `Rd == Ra`; para SMLALD exige `RdHi != RdLo` e proíbe PC como operando: https://documentation-service.arm.com/static/6245c734b059dc5ff9a8bdab
- Workflow [43504486197](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43504486197) concluído com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` também aprovado.
- O trabalho de código permaneceu restrito a `crates/aceris-core/tests/arm11_dsp_multiply.rs`. Não foram adicionadas dependências nem consultadas implementações de outros emuladores.

## A32 — rejeição atômica de codificações DSP inválidas (13/11/2026)

- `c6143b9` — adiciona `dsp_multiply_rejects_pc_reserved_fields_and_invalid_destination_pairs_atomically`, cobrindo 36 codificações inválidas das famílias `SMLAxy`, `SMULxy`, `SMLALxy`, `SMLAWy`, `SMULWy`, `SMLAD/SMLSD`, `SMLALD/SMLSLD` e `SMMUL/SMMLA/SMMLS`.
- A matriz inclui `PC` em posições proibidas, campos reservados não zerados, par `RdHi/RdLo` repetido, bit reservado de operações duplas, divergência entre os bits de classe `SMMUL/SMMLA/SMMLS` e `Ra=PC` na forma de subtração `SMMLS`.
- Cada caso verifica `UndefinedInstruction` e atomicidade: registradores, CPSR, PC e dado sentinela na memória permanecem inalterados.
- `2781068` — aplica a formatação Rustfmt solicitada. `cd04938` — substitui o loop indexado por `iter_mut().enumerate()` para satisfazer `clippy::needless_range_loop`; nenhum aviso foi suprimido.
- Referência arquitetural independente: ARM Instruction Set Reference Guide, entradas `SMULxy`, `SMLAxy`, `SMLALD` e `SMMLS`, que descrevem operandos e restrições de registradores; o código usa apenas essa documentação técnica, não implementações de emuladores: https://documentation-service.arm.com/static/6245c734b059dc5ff9a8bdab
- Workflow [43505124581](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43505124581) concluído com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` também aprovado.
- O trabalho de código permaneceu restrito a `crates/aceris-core/tests/arm11_dsp_multiply.rs`. Nenhuma dependência foi adicionada e nenhum código-fonte de emulador de terceiros foi consultado.

## A32 — validação de aritmética e lógica com Operand2 imediato rotacionado (13/11/2026)

- `93343df` — acrescenta uma matriz diferencial para ADD/ADC/SUB/SBC/RSB/RSC/CMP/CMN A32 com Operand2 imediato rotacionado. O teste usa nove pares `rotate/imm8`, a grade de operandos-limite já existente e todas as combinações de C de entrada e V inicial. O modelo independente calcula resultado, carry (incluindo not-borrow) e overflow.
- `bacf990` — ajusta a forma de `arm_immediate_instruction` conforme Rustfmt. CI [43505804950](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43505804950) aprovada nas quatro plataformas.
- `71deb47` — acrescenta matriz para AND/EOR/TST/TEQ/ORR/MOV/BIC/MVN imediato. Verifica o carry produzido pelo rotate do shifter quando a rotação é diferente de zero, preservação do carry corrente quando rotate=0, preservação de V e ausência de gravação em Rd para TST/TEQ.
- `0ed0a2b` — corrige a formatação de uma chamada auxiliar. Workflow [43506136687](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43506136687) concluído com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux/Windows/macOS; build release Android `arm64-v8a` aprovado.
- Referência independente: ARM Architecture Reference Manual ARM DDI 0100I, instruções A32 de processamento de dados e cálculo do carry de Operand2. O teste usa a codificação pública `ROR(imm8, 2*rotate)` e uma referência aritmética própria; nenhuma implementação de emulador de terceiros foi consultada.
- As alterações de código desta etapa ficaram limitadas a `crates/aceris-core/tests/arm11_arithmetic_flags.rs`; não houve mudança no caminho de produção.

## A32 — validação do campo SBZ de MOV/MVN (13/11/2026)

- `4e9c14f` — valida `Rn[19:16] == 0` para `MOV` e `MVN`, cujo campo é SBZ na codificação A32. A rejeição ocorre antes da avaliação do Operand2 e antes de qualquer alteração de estado.
- O teste externo `mov_and_mvn_reject_nonzero_rn_sbz_fields_atomically` cobre oito codificações: MOV/MVN com imediato rotacionado, shift imediato, shift controlado por registrador e campo reservado igual a PC. Cada caso exige `UndefinedInstruction` e preservação dos registradores, CPSR, PC e memória sentinela.
- `a0818ae` — corrige a formatação indicada por Rustfmt. Workflow [43510347568](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43510347568) concluído com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` aprovados em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado.
- Referência arquitetural independente: ARM Architecture Reference Manual ARM DDI 0100I, seção A4.1.35 (MOV, A4-68) e A4.1.41 (MVN, A4-82), que identificam o campo `Rn` como SBZ. Não foi consultado código-fonte de emuladores de terceiros.
- A implementação alterada ficou limitada a `crates/aceris-core/src/cpu/arm.rs`; a regressão externa foi adicionada em `crates/aceris-core/tests/arm11_miscellaneous.rs`. Thumb não foi alterado.

## A32 — PC como primeiro operando de processamento de dados (13/11/2026)

- `961352b` — adiciona o teste externo `arm_add_uses_instruction_address_plus_eight_when_pc_is_rn`. `ADD r2, pc, #4` confirma o valor do PC como endereço da instrução + 8 e preserva NZCV quando S=0. `ADDS r2, pc, #0x80` usa um PC próximo de `INT32_MAX`, confirma o resultado `0x80000008` e verifica `N=1`, `C=0`, `V=1`.
- Referência arquitetural independente: ARM Architecture Reference Manual ARM DDI 0100I, seção A4.1.3 (ADD, página A4-6), que usa explicitamente `ADD Rd, PC, #offset` para gerar endereços PC-relativos e define PC como endereço da instrução + 8: https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf
- Workflow [43510706670](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43510706670) concluído com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado.
- Apenas testes externos em `crates/aceris-core/tests/arm11_arithmetic_flags.rs` foram alterados nesta etapa; nenhuma implementação de produção ou código Thumb foi modificado.

## A32 — PC como primeiro operando em CMP e TST (13/11/2026)

- `335b36a` — amplia a cobertura de `PC` em `Rn` para `CMP PC,#8` e `TST PC,#8`. O caso CMP verifica o resultado de comparação com PC+8 e flags aritméticos, sem gravar registrador de destino. O caso TST confirma o valor de PC+8 no AND lógico, a preservação de V e do carry existente quando a rotação do imediato é zero, também sem writeback.
- Referência arquitetural independente: ARM Architecture Reference Manual ARM DDI 0100I, seção A4.1.15 (CMP) e A4.1.117 (TST); PC é o primeiro operando `Rn` e o comportamento de leitura do registrador é o PC da instrução + 8 quando aplicável: https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf
- Workflow [43511129705](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43511129705) concluído com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado.
- Apenas testes externos em `crates/aceris-core/tests/arm11_arithmetic_flags.rs` foram alterados; nenhuma implementação de produção ou código Thumb foi modificado.

## A32 — matriz aritmética com PC em Rn e Operand2 imediato (13/11/2026)

- `bffe570` — acrescenta `arm_immediate_arithmetic_uses_pc_plus_eight_as_rn_across_opcodes`, uma matriz diferencial independente para ADD/ADC/SUB/SBC/RSB/RSC/CMP/CMN com `Rn=PC` e Operand2 imediato rotacionado.
- A matriz percorre sete endereços-base (incluindo valores em torno de `INT32_MAX` e perto do wrap de `u32`), seis pares `rotate/imm8`, C de entrada ligado/desligado e V inicial ligado/desligado: 1.344 combinações no total. O modelo referência usa `base.wrapping_add(8)` para o PC e valida resultado, NZCV, writeback apenas para instruções com destino e PC sequencial.
- Workflow [43511663360](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43511663360) concluído com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado.
- Apenas testes externos em `crates/aceris-core/tests/arm11_arithmetic_flags.rs` foram alterados; nenhuma implementação de produção ou código Thumb foi modificado.

## A32 — matriz lógica com PC em Rn e Operand2 imediato (13/11/2026)

- `180067e` — acrescenta `arm_immediate_logical_uses_pc_plus_eight_as_rn_across_opcodes` para AND/EOR/TST/TEQ/ORR/BIC. MOV/MVN foram intencionalmente excluídos porque `Rn` é SBZ nesses opcodes e sua rejeição já é testada separadamente.
- A matriz percorre sete bases de PC, nove pares `rotate/imm8`, C de entrada ligado/desligado e V inicial ligado/desligado: 1.512 combinações. Verifica `PC+8`, o valor lógico, carry do shifter (ou preservação de C quando rotate=0), preservação de V, ausência de writeback em TST/TEQ e avanço sequencial de PC.
- `1cbf979` — aplica o formato Rustfmt indicado pela primeira execução da CI. Workflow [43512120871](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43512120871) concluído com sucesso: Rustfmt, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado.
- Apenas testes externos em `crates/aceris-core/tests/arm11_arithmetic_flags.rs` foram alterados; nenhuma implementação de produção ou código Thumb foi modificado.

## A32 — execução condicional, CPS, PLD e escrita em PC (13/11/2026)

- `1923e4a` — adiciona `arm11_condition_codes.rs`: `all_a32_conditions_match_every_nzcv_combination` cobre 15 condições (`EQ`–`AL`) × 16 estados NZCV = 240 verificações de execução/skip de `MOV{cond}`; `failed_a32_conditions_suppress_undefined_instruction_exceptions` cobre as 14 condições (`EQ`–`LE`) × 16 estados = 224 verificações de supressão de `UndefinedInstruction` quando a condição falha; o terceiro teste distingue `0xF`, espaço incondicional, da tabela normal.
- Os testes de condição verificam preservação integral do CPSR em MOV sem S, gravação do destino apenas quando a condição passa e avanço sequencial do PC tanto na execução quanto no skip. Para codificação reservada, a condição verdadeira produz `UndefinedInstruction` atomicamente, enquanto a condição falsa suprime o decode e avança PC.
- `2b93fa9` — corrige o decoder de CPS para rejeitar bit 16 diferente de zero, campo literal zero na codificação ARMv6, antes da regra de no-op User. `80b4000` acrescenta teste da codificação inválida em User; `395b575` corrige formatação.
- `519d721` — acrescenta testes de comportamento válido de CPS: alteração para IRQ sem perturbar NZCV/AIF, `CPSID i` que apenas define I, `CPSIE i` que apenas limpa I e CPS corretamente codificado sem efeito em User. `250b406` importa o tipo PSR usado pelas asserções; `37c1f1d` aplica Rustfmt.
- Referência arquitetural de CPS: ARM Architecture Reference Manual ARM DDI 0100I, seção A4.1.16, que fixa bit 16 em zero: https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf. Não foi consultado código-fonte de outros emuladores.
- `398d6b7` — corrige o decoder de PLD: no modo com offset por registrador, bit 4 deve ser zero, mesmo quando o deslocamento é imediato; o decode rejeita bit 4=1 e `Rm=PC`. `e3395ec` adiciona teste positivo de offset com `LSL #3` e negativo com os mesmos campos e bit 4 alterado para 1. `c3d7ad3` acrescenta regressão confirmando que `Rn=PC` continua aceito nos modos offset; a cobertura PC-relativa literal permanece.
- Referência arquitetural de PLD: ARM DDI 0100I, seção A4.1.45 (PLD) limita a instrução aos três modos offset; A5.2.3 e A5.2.4 especificam PC+8 para `Rn` e classificam `Rm=PC` como imprevisível; A5.2.4 fixa bit 4 em zero: https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf. Não foi consultado código-fonte de outros emuladores.
- `8aa5985` — adiciona `arm11_data_processing_pc.rs`; `fd227ca` aplica Rustfmt. A suíte verifica `MOV pc,Rm` como branch A32 com destino alinhado a palavra, sem interworking nem alteração de flags; `MOVS pc,Rm` restaura SPSR e o estado Thumb, alinhando o alvo à meia palavra; e `MOVS pc,Rm` sem SPSR ativo falha como Undefined sem efeitos colaterais.
- Workflow [43519964750](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43519964750) concluído com sucesso: Rustfmt, Clippy `-D warnings` e `cargo test --workspace` aprovados em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado. Os 3 testes de `arm11_data_processing_pc.rs`, os 3 de `arm11_condition_codes.rs` e os 7 de `arm11_unconditional.rs` passaram.
- A matriz independente de PC como `Rn` em Operand2 com registrador deslocado, commit `372b98d`, cobre 8.064 combinações: 4.608 aritméticas e 3.456 lógicas. Workflow [43512759809](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43512759809) passou em Linux, Windows, macOS e Android ARM64.
- Produção desta etapa ficou restrita a `crates/aceris-core/src/cpu/arm.rs`; testes em `crates/aceris-core/tests/arm11_condition_codes.rs`, `arm11_unconditional.rs` e `arm11_data_processing_pc.rs`. Não houve alteração em Thumb nem em áreas isoladas de outro agente. Nenhuma dependência foi adicionada.

## A32 — transferências simples com PC como base de offset por registrador (14/11/2026)

- `0ae72a7` — acrescenta `register_offset_transfers_allow_pc_as_base_with_pc_plus_eight` em `crates/aceris-core/tests/arm11_single_transfers.rs`. O teste confirma `LDR r2,[pc,r4,LSL #2]` e `STR r2,[pc,r4,LSL #2]`: a base `Rn=PC` lê o endereço da instrução + 8, soma o offset deslocado, não altera o registrador de índice nem CPSR e mantém PC sequencial. O teste foi verificado no workflow [43538848458](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43538848458), aprovado nas quatro plataformas.
- `54660e6` — acrescenta `pc_base_register_offsets_obey_zero_shift_encodings_and_add_subtract_direction`, com `LDRB` e `Rn=PC` para `LSL #0`, `LSR #0`, `ASR #0` e `ROR #0` (RRX), incluindo endereço por subtração/wrap e carry de entrada. `1113d3a` ajusta o vetor de `LSR #0`: deslocamento lógico à direita por 32 resulta em zero. `d072a37` aplica Rustfmt.
- Referência arquitetural independente: ARM Architecture Reference Manual ARM DDI 0100I, A5.2.3 (register offset) e A5.2.4 (scaled register offset): `Rn=R15` usa o endereço da instrução + 8; `Rm=R15` é UNPREDICTABLE. O código de produção já respeitava essa regra; esta etapa adicionou somente cobertura de regressão, sem alteração em `arm.rs`: https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf.
- Workflow [43539199969](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43539199969) concluído com sucesso: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` aprovados em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado. A suíte `arm11_single_transfers.rs` passou com 13 testes.
- O ARMv6 classifica o valor de `R15` armazenado por STR/STM como IMPLEMENTATION DEFINED (+8 ou +12), com a mesma escolha obrigatória para as duas famílias. Aceris atualmente usa +8 nos dois caminhos; não foi feita alteração sem evidência específica adicional do ARM1136. Referência: ARM DDI 0100I, A2.4.3, e notas da operação STM.

## CI — build explícito com lockfile (14/11/2026)

- `02aede7` — `.github/workflows/ci.yml` adiciona `cargo build --locked --workspace` à matriz Linux/Windows/macOS, entre Rustfmt e Clippy. A etapa comprova que os pacotes padrão do workspace compilam usando o `Cargo.lock` sem modificações implícitas.
- Workflow [43542724526](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43542724526) aprovado em Linux, Windows e macOS: Rustfmt, `cargo build --locked --workspace`, Clippy `-D warnings` e `cargo test --workspace`; Android `arm64-v8a` release também aprovado. O item correspondente no `docs/ROADMAP.md` foi marcado como concluído.

## A32 — regressões de exceções com origem Thumb (14/11/2026)

- O commit `9f99bd84e50490083b13a652926677ead3f44c5f` adicionou testes externos para IRQ e FIQ aceitos durante a execução Thumb. Verificam entrada no vetor A32, SPSR preservando T, máscaras de interrupção, LR e retorno ao mesmo halfword para completar a instrução interrompida.
- O commit `c02987cf3dec16cee577e281aadf691832a81035` adicionou três testes externos para falhas geradas em Thumb sem MMU: `undefined_during_thumb_saves_halfword_return_and_restores_thumb_state` (LR=PC+2 e retorno por `SUBS pc,lr,#2`), `data_abort_during_thumb_restores_state_and_returns_to_faulting_halfword` (LR=PC+8, DFAR/DFSR, rollback de registradores e retorno por `SUBS pc,lr,#8`) e `prefetch_abort_during_thumb_restores_state_and_returns_to_faulting_address` (LR=PC+4, IFAR/IFSR e retorno por `SUBS pc,lr,#4`). Os três verificam a restauração do CPSR, inclusive o bit T.
- O commit `1f9eca2a940157b7c9f92df590080c223090bc70` adicionou dois testes para falhas de tradução com MMU habilitada: `thumb_prefetch_translation_abort_preserves_thumb_state_and_return_address` e `thumb_data_translation_abort_preserves_thumb_state_and_faulting_instruction`. A fixture configura TTBR0, DACR e SCTLR.M/XP por instruções MCR executadas pelo core; mapeia a região dos vetores e deixa a seção virtual de 1 MiB sem descritor. Os testes verificam IFAR/IFSR ou DFAR/DFSR, LR, SPSR, restauração de T e retorno por `SUBS pc,lr,#4` ou `SUBS pc,lr,#8`. No Data Abort, também verificam que os registradores do guest não recebem efeitos parciais.
- O commit `b75f95193267c346f57f7af291d883540719551b` iniciou a cobertura de IRQ/FIQ Thumb com MMU e vetores altos. A fixture foi corrigida nos commits `dec60a1531bd3d891308254f28ebec15fc36f47f`, `ae93d48935670db2bb60f2c55bebd4f51dc1f8b0` e `e7845f22f54ba72e07baceb9cbc4a3e42ab45e4e`: TTBCR.N=1 divide os endereços entre TTBR0 (base 0xC000) e TTBR1 (base 0x4000), o descritor superior mapeia VA 0xFFF00000 para PA 0, e o backing físico sintético de 1 MiB contém os handlers nos offsets PA 0xF0018/0xF001C. Os testes `irq_during_thumb_uses_high_vector_through_enabled_mmu` e `fiq_during_thumb_uses_high_vector_through_enabled_mmu` verificam as máscaras, LR, SPSR, fetch do vetor alto via MmuBus, retorno por `SUBS pc,lr,#4`, restauração do CPSR/T e retomada da instrução Thumb no mesmo halfword.
- O commit `28a1da78b3edf9569b6b712d298992bb4a51c436` finalizou a asserção de retorno do teste IRQ: depois de executar o handler, o modo deve ser o modo original (Supervisor/Thumb), não IRQ.
- Nenhum código de produção, `thumb.rs`, helper compartilhado ou dependência foi alterado nestas etapas. As mudanças de CPU foram restritas à suíte externa `crates/aceris-core/tests/arm11_exceptions.rs`.
- Workflow [43548499388](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43548499388) aprovado em Linux, Windows e macOS: `cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy com `-D warnings` e `cargo test --workspace`; build release Android `arm64-v8a` também aprovado. Os testes de IRQ/FIQ com vetor alto aparecem como `ok` nos logs das três matrizes desktop. A suíte completa de exceções do workspace passou em todas as três plataformas.


## A32 — abortos Thumb com MMU e vetores altos (14/11/2026)

- O commit `a274dc58316e1522bc04b844de54e1f3300d21c3` amplia a suíte externa `crates/aceris-core/tests/arm11_exceptions.rs` com `thumb_data_translation_abort_uses_high_vector_through_enabled_mmu` e `thumb_prefetch_translation_abort_uses_high_vector_through_enabled_mmu`. Os dois exercitam falhas de tradução reais sob `SCTLR.M`, com `SCTLR.V`, TTBR0/TTBR1 e `TTBCR.N=1`, e fazem o handler ser buscado pela MMU no endereço virtual alto.
- No Data Abort, o Thumb `LDR r0,[r1]` acessa a seção virtual não mapeada em `0x00100000`. A exceção usa VA `0xFFFF0010`, traduzido pelo descritor superior para PA `0x000F0010`; o handler `SUBS pc,lr,#8` retorna ao halfword `0x100`. O teste verifica LR=0x108, SPSR/CPSR.T, A/I, DFAR/DFSR e preservação dos registradores de destino/base sem writeback parcial.
- No Prefetch Abort, o fetch Thumb em `0x00100000` falha por ausência do descritor TTBR0. O vetor `0xFFFF000C` é traduzido para PA `0x000F000C`; `SUBS pc,lr,#4` retorna ao endereço interrompido. O teste verifica LR=VA+4, SPSR/CPSR.T, A/I e IFAR/IFSR.
- A fixture `enable_mmu_high_vectors` agora recebe o controle `exception_big_endian` e programa SCTLR através de MCR executado pelo guest. Os testes IRQ/FIQ mantêm EE=0; os novos casos Data/Prefetch Abort usam EE=1 e verificam CPSR.E na entrada, com restauração do valor original pelo SPSR após o retorno. Isto valida o controle de endianness de exceção, não encerra a auditoria de todos os modos de endianness.
- Os commits `2bed7640788861cc36ee1daadc4d5571837d64ca` e `8baea80f6e71463d8452345befa723bb65ebfc66` tratam, respectivamente, Rustfmt e a atualização de todas as chamadas da fixture com o parâmetro explícito.
- Workflow [43617433734](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43617433734) aprovado em Linux, Windows e macOS: `cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy com `-D warnings` e `cargo test --workspace`; build release Android `arm64-v8a` também aprovado. Os dois testes novos aparecem como `ok` nos logs e a suíte completa passou nas três plataformas desktop.
- Nenhum código de produção, `thumb.rs`, helper compartilhado ou dependência foi alterado; as mudanças de código permaneceram na suíte externa de exceções. A auditoria de abortos em transferências múltiplas, sequências longas e demais combinações de exceção/endianness segue pendente.


## A32/Thumb — Data Abort durante LDMIA/STMIA com MMU (14/11/2026)

- O commit `0273d28bfb16d9156917e4fdc13efc67826e82cf` acrescentou quatro regressões externas a `crates/aceris-core/tests/arm11_exceptions.rs` para transferências múltiplas de duas palavras: `a32_ldmia_translation_abort_keeps_registers_and_writeback_atomic`, `a32_stmia_translation_abort_preserves_completed_store_without_writeback`, `thumb_ldmia_translation_abort_keeps_registers_and_writeback_atomic` e `thumb_stmia_translation_abort_preserves_completed_store_without_writeback`.
- A fixture existente `mmu_enabled_thumb_machine` ativa MMU com identidade na primeira seção de 1 MiB e deixa a próxima seção sem descritor. Os testes iniciam a transferência em VA `0x000FFFFC`: o primeiro beat está mapeado e o segundo, em `0x00100000`, gera translation fault. Código A32 usa `LDMIA r0!,{r1,r2}`/`STMIA r0!,{r1,r2}`; Thumb usa os formatos T16 correspondentes.
- LDMIA verifica que os registradores de destino e a base permanecem idênticos ao snapshot da instrução, que SPSR/LR e modo Abort foram registrados, e que DFAR=`0x00100000` e DFSR=`0x5` (fault de leitura). A primeira palavra de memória permanece intacta.
- STMIA verifica comportamento diferente e deliberado: a primeira palavra escrita antes da falha permanece na memória — stores de beats anteriores não são revertidos —, mas os registradores e a base mantêm o snapshot, portanto não há writeback para a sequência incompleta. DFAR=`0x00100000` e DFSR=`0x805` (translation fault com WnR=1). O primeiro valor DFSR incorreto nos testes foi detectado na CI Linux e corrigido no commit `b42b7d61c22f1e1e1402a0a3a0002c3dbbd0e15b`; o bit WnR é esperado para STM.
- O commit `7b7b936a98bf040559eb9ddc88be091d050d3436` aplicou o espaçamento exigido por Rustfmt. A CI final [43618358531](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43618358531) concluiu com sucesso em Linux, Windows e macOS (`cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy `-D warnings`, `cargo test --workspace`) e Android `arm64-v8a` (build release). Os quatro testes aparecem como `ok` nos logs de Linux e macOS; os jobs Windows e Android também passaram.
- Nenhum arquivo de produção, `thumb.rs`, helper compartilhado ou dependência foi modificado nesta etapa. O trabalho de código ficou restrito aos testes externos de exceção. Cobertura de listas com mais registradores, faults no terceiro/quarto beat, writeback em modos de endereçamento adicionais e sequência de exceções mais longas continua pendente; não inferir suporte integral a todas as variantes LDM/STM a partir desta amostra.

## A32/Thumb — Data Abort no terceiro e quarto beats de LDMIA/STMIA (14/11/2026)

- O commit `fa0481a60d75276da968e7256df275ae956cdca8` acrescentou quatro testes parametrizados em `crates/aceris-core/tests/arm11_exceptions.rs`: `a32_ldmia_translation_abort_on_third_or_fourth_beat_is_atomic`, `a32_stmia_translation_abort_on_third_or_fourth_beat_keeps_completed_stores`, `thumb_ldmia_translation_abort_on_third_or_fourth_beat_is_atomic` e `thumb_stmia_translation_abort_on_third_or_fourth_beat_keeps_completed_stores`.
- Cada teste executa dois cenários. A lista de três registradores começa em VA `0x000FFFF8`; os dois primeiros beats acessam memória mapeada e o terceiro falha em `0x00100000`. A lista de quatro registradores começa em `0x000FFFF4`; três beats são concluídos e o quarto falha no mesmo VA. Os formatos A32 e Thumb T16 são exercitados separadamente.
- LDMIA confirma que nenhum registrador de destino nem o writeback da base é publicado antes de todas as leituras terminarem; a falha no terceiro/quarto beat mantém o snapshot de registradores. DFSR permanece `0x5` (translation fault em leitura).
- STMIA confirma a semântica de beats de escrita já concluídos: 2/3 palavras gravadas antes do fault permanecem no backing memory; a base não recebe writeback para uma sequência incompleta. DFSR=`0x805` inclui WnR para o translation fault de escrita.
- Os commits `fc744f0d63dea9c4e80e423bc38b333bc0baeeeb` e `8575fa74a7ac599d38d0654fb63d95249f6c64c7` ajustaram Rustfmt. O commit `f36927d402ea9c82d89a511dd28fefac821bae5c` substituiu inicialização de registradores por índice por iteração mutável com `enumerate`, satisfazendo Clippy sem `allow` local.
- Workflow [43620709581](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43620709581) aprovado em Linux, macOS e Windows: `cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy com `-D warnings` e `cargo test --workspace`; build release Android `arm64-v8a` também aprovado. Os quatro testes aparecem como `ok` nos logs das três plataformas desktop.
- Nenhum arquivo de produção, `thumb.rs`, helper compartilhado ou dependência foi modificado nesta etapa. Os testes reutilizam a fixture MMU existente e a implementação atual das transferências. Ainda faltam listas maiores que quatro registradores, modos de endereçamento adicionais, falhas em acessos de lista com PC/bancos especiais e abortos em sequências mais longas; não generalizar a cobertura para todas as combinações possíveis de LDM/STM.

## A32 — LDM/STM nos quatro modos de endereçamento e Data Abort em beats posteriores (14/11/2026)

- O commit `36de8a81ba4639dd7d690d628d24df46d0dadc6d` acrescentou `a32_ldm_stm_writeback_covers_all_four_addressing_modes` em `crates/aceris-core/tests/arm11_block_transfer.rs`. O teste percorre IA (Increment After), IB (Increment Before), DA (Decrement After) e DB (Decrement Before), tanto para LDM quanto para STM, usando três registradores. Verifica a posição do primeiro acesso, os valores transferidos em ordem crescente de registrador/endereço e a base final após writeback.
- O commit `2f18586b38257f9fc3d2d67968558a4752fd18bf` acrescentou dois testes parametrizados em `crates/aceris-core/tests/arm11_exceptions.rs`: `a32_ldm_translation_abort_in_all_addressing_modes_keeps_registers_and_writeback_atomic` e `a32_stm_translation_abort_in_all_addressing_modes_keeps_completed_stores_without_writeback`.
- Cada teste executa os quatro modos e duas listas: três registradores, com falha no terceiro beat, e quatro registradores, com falha no quarto beat. A fixture ativa a MMU com identidade na seção virtual 0..1 MiB e deixa a próxima seção sem descritor. Os endereços de transferência começam em `0x00100000 - 4*(N-1)`, para que os primeiros N-1 beats estejam mapeados e o último provoque Translation fault em `0x00100000`; nas variantes DA/DB, a base é calculada de acordo com o deslocamento total e não precisa ser acessada como dado antes da sequência.
- LDM verifica entrada em Abort, DFAR=`0x00100000`, DFSR=`0x5` e preservação integral de R0–R12: nenhum destino e nenhum writeback da base são publicados após leituras parciais. STM verifica DFSR=`0x805`, preserva R0–R12 e confirma que os N-1 stores já concluídos continuam na memória, sem writeback da base para a sequência incompleta.
- Os commits `d001e287b55131ec188e5a5a5dd3026f23d0a607` e `bea7c5ce677bbaac8fe1babedb285ecd5455f1d5` aplicaram os ajustes pedidos pelo Rustfmt. Nenhum aviso de Clippy foi suprimido.
- Workflow [43627181554](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43627181554) concluído com sucesso: `cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy com `-D warnings` e `cargo test --workspace` aprovados em Linux, Windows e macOS; build release Android `arm64-v8a` também aprovado. Os três testes novos aparecem como `ok` nos logs das três plataformas desktop.
- Nenhum arquivo de produção, `thumb.rs`, helper compartilhado ou dependência foi alterado. O trabalho permaneceu restrito a `arm11_block_transfer.rs` e `arm11_exceptions.rs`. A cobertura é específica às listas e casos descritos; não demonstra conformidade integral de todas as instruções de transferência múltipla nem compatibilidade com jogos comerciais.

## A32 — listas LDM/STM extensas, PC e retorno de exceção (14/11/2026)

- O commit `adecd5675e42dbaaac10c01d4a92ada9b11c4ade` acrescentou `a32_ldm_stm_large_register_list_covers_all_four_addressing_modes` a `crates/aceris-core/tests/arm11_block_transfer.rs`. O teste usa a lista `r1-r13` (13 registradores, incluindo SP, sem PC e sem o registrador-base r0) e verifica LDM e STM nos quatro modos IA/IB/DA/DB, com endereço inicial e writeback final corretos. O teste usa a CPU em User mode para comparar diretamente o banco de SP da transferência.
- O mesmo arquivo ganhou `a32_ldm_with_pc_loads_extended_register_list_and_branches_after_writeback`: `LDMIA r0!,{r1-r15}` transfere 15 palavras, verifica R1–R14, writeback de R0 e o branch para o valor carregado em PC. O caso é somente LDM; não generaliza o valor armazenado de PC em STM, que tem regra arquitetural própria.
- O commit `a1382960840b67cf335da674abb01af171197c50` adicionou dois testes em `crates/aceris-core/tests/arm11_exceptions.rs`: `a32_ldm_translation_abort_with_thirteen_registers_preserves_all_destinations_in_all_modes` e `a32_stm_translation_abort_with_thirteen_registers_keeps_completed_stores_in_all_modes`. Ambos percorrem IA/IB/DA/DB com a lista r1-r13 e base r0; 12 beats estão mapeados e o 13º falha por translation fault em VA `0x00100000`.
- No LDM, a entrada em Abort mantém r0-r12 sem alterações e não publica writeback. Como r13 é banco-dependente, o teste volta temporariamente ao modo Supervisor para verificar que o SP Supervisor original também não foi alterado pela leitura parcial. DFAR=`0x00100000`, DFSR=`0x5`.
- No STM, os 12 stores concluídos permanecem visíveis na memória, incluindo os valores de r1-r12; o último acesso (r13) falha, a base r0 não recebe writeback e o teste inspeciona o SP Supervisor original após trocar de banco. DFAR=`0x00100000`, DFSR=`0x805`.
- O commit `fca3b60a0008d7e475379bf76100008fd9b4cc98` acrescentou `ldm_exception_return_loads_multiple_registers_restores_thumb_and_branches` em `crates/aceris-core/tests/arm11_exception_return_preflight.rs`. O caso `LDMIA r4!,{r0,r1,r2,pc}^` carrega três registradores gerais e PC, aplica o writeback de r4, restaura via SPSR o modo User e o bit T, alinha o destino `0x301` para `0x300` e executa no destino um `MOVS r3,#7` Thumb. Isso confirma a retomada Thumb, não apenas a alteração observável do PC.
- Os commits `bfe974d9a6cdaee0a1f30b64dfa37341a65e5220`, `4c01226ce29e4383595f5352269d59a2e6b94a7e`, `636dd1940f6a39ea7e696b6a7ece6fe2b585b83c`, `1e0c8bb83d22ed45b7c5e4210074d99fa7cefec7` e `23152a8c53992fec953c658580ef1ba9cf9b63c3` corrigiram, respectivamente, formatação, nome de constante e tipo do índice de snapshot. O último commit deixa o conjunto de regressões formatado e compilável sem suprimir diagnósticos.
- Workflow [43630249436](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43630249436) concluído com sucesso em Linux, Windows e macOS: `cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy com `-D warnings` e `cargo test --workspace`; build release Android `arm64-v8a` também aprovado. Os cinco testes novos aparecem como `ok` nos logs das três plataformas desktop.
- O código desta etapa ficou restrito aos testes externos `arm11_block_transfer.rs`, `arm11_exceptions.rs` e `arm11_exception_return_preflight.rs`. Nenhum arquivo de produção, `thumb.rs`, helper compartilhado ou dependência foi modificado. A matriz valida os casos explicitados; não prova conformidade integral de todas as combinações ARMv6 nem compatibilidade com jogos comerciais.

## A32 — Data Abort em transferências do banco User e LDM de retorno (14/11/2026)

- O commit `3f1ac9248a9f1b50fcd9bae4fc78d2615878a1f8` acrescentou três regressões à suíte externa `crates/aceris-core/tests/arm11_exceptions.rs`; o commit `ae0262ff897381c97ecbd7c1423106c4979f96a9` aplicou o Rustfmt.
- `a32_ldm_user_bank_data_abort_preserves_user_and_fiq_banks`: em FIQ, executa `LDMIA r0,{r8-r14}^` sem writeback. A base `0x000FFFF0` permite quatro reads e o quinto beat falha por translation fault em `0x00100000`. O teste confirma modo/vector Abort, SPSR_abt igual ao CPSR FIQ interrompido, DFAR=`0x00100000`, DFSR=`0x5`, preservação de r0-r7 e dos sete registradores em ambos os bancos User e FIQ após alternar os modos.
- `a32_stm_user_bank_data_abort_keeps_completed_user_stores_and_both_banks`: executa `STMIA r0,{r8-r14}^` nas mesmas condições. Os quatro stores concluídos antes do quinto beat permanecem na memória e contêm os valores do banco User (não os valores distintos do banco FIQ). O teste confirma DFSR=`0x805`, r0-r7 e ambos os bancos sem alteração.
- `a32_ldm_exception_return_data_abort_preserves_spsr_and_retries_after_abort_handler`: inicia `LDMIA r4!,{r0,r1,pc}^` em `0x100`, base `0x000FFFF8`. Os beats de r0/r1 estão mapeados; o acesso ao PC em `0x00100000` falha. O Abort deve preservar r0, r1 e r4, salvar o CPSR Supervisor em SPSR_abt e deixar intacto o SPSR Supervisor destinado ao retorno User/Thumb. O teste consulta cada SPSR no respectivo modo para confirmar que ambos sobreviveram.
- Para completar a sequência, o teste instala um handler Abort `SUBS pc,lr,#8`, mapeia a seção virtual `0x00100000` para PA 0 e coloca `0x301` no terceiro word. O handler retorna à instrução original; a repetição do LDM conclui o writeback de r4, restaura User/Thumb via SPSR_svc, alinha o PC para `0x300` e executa `MOVS r2,#9` Thumb em seguida. A tradução que havia falhado não cria entrada TLB; por isso o retry usa a nova tabela sem manutenção artificial.
- Workflow [43642027581](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43642027581) concluído com sucesso: `cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy com `-D warnings` e `cargo test --workspace` aprovados em Linux, Windows e macOS; build release Android `arm64-v8a` aprovado. Os três testes aparecem como `ok` nos logs das três plataformas desktop.
- O trabalho de código ficou restrito a `crates/aceris-core/tests/arm11_exceptions.rs`. Nenhum código de produção, `thumb.rs`, helper compartilhado ou dependência foi alterado. A cobertura valida os cenários descritos, não todos os modos/bancos ARMv6 nem compatibilidade com jogos comerciais.
- Referência arquitetural consultada: ARM Architecture Reference Manual ARM DDI 0100I, seção A4.1.21 (LDM, carregamento de User-bank registers com `^` e exceção Data Abort): https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf. Não foi usado código-fonte de outros emuladores.

## ARM11 MMU — manutenção guest-visible do TLB e entradas bloqueadas (14/11/2026)

- O commit `3432dcd2cab456791fe249fcaeb4c781043a8e2e` criou a suíte externa `crates/aceris-core/tests/arm11_tlb.rs`; o commit `e8d067a9479a5ba625931fe550e70e59c46b1de3` aplicou o Rustfmt.
- `guest_tlbi_asid_invalidates_non_global_entries_but_preserves_global_entries` configura o ASID 7 pelo guest via MCR em CP15 c13, carrega dados através de uma seção não global e outra global, altera ambos os descritores L1 e executa `MCR p15,0,r1,c8,c7,2` (TLBIASID). A leitura posterior deve usar a nova tradução para o mapeamento não global e manter a tradução previamente armazenada para o mapeamento global.
- `guest_tlbi_all_preserves_locked_entries_until_mva_asid_invalidates_them` usa CP15 c10,c0,0 para preencher os slots de lockdown 2 e 3. Depois de modificar o descritor da seção de dados, executa `MCR p15,0,r4,c8,c7,0` (TLBIALL): a entrada bloqueada continua válida e o LDR observa o endereço físico antigo. Em seguida, `MCR p15,0,r1,c8,c7,1` (TLBIMVA) invalida a entrada bloqueada específica por MVA/ASID; o LDR seguinte refaz o page walk e lê o novo endereço físico.
- Os testes exercitam a sequência real de instruções A32 MCR/LDR com MMU ativa e tabelas/backing memory sintéticos. Não usam ROM, firmware ou código de outros emuladores. A implementação de MMU/CP15 não precisou de mudança nesta etapa.
- Workflow [43642948784](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43642948784) aprovado em Linux, Windows e macOS: `cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy com `-D warnings` e `cargo test --workspace`; build release Android `arm64-v8a` também aprovado. Os dois testes novos aparecem como `ok` nos logs de Linux, macOS e Windows.
- Alterações de código limitadas ao novo arquivo de teste `arm11_tlb.rs`; nenhuma alteração em produção, `thumb.rs`, trabalho exclusivo do Claude, helper compartilhado ou dependência. Isso verifica apenas as operações e sequências cobertas, não todas as variantes de manutenção/lockdown, nem compatibilidade com jogos comerciais.
- Referência primária consultada: ARM11 MPCore Processor Technical Reference Manual, seções de TLB maintenance e TLB lockdown: https://documentation-service.arm.com/static/5e8e1e0388295d1e18d368b2.

## ARM11 MMU — troca de ASID, invalidação por MVA e FCSE (14/11/2026)

- A suíte externa `crates/aceris-core/tests/arm11_tlb.rs` ganhou dois cenários guest-visible. Os commits `a89b967fab89a9c6bfaf96b6464e61ef32163ba3` adicionaram os testes; `7cf919f3d67abcfede806ce6e2f4468b81a03297` aplicou Rustfmt; `9b3c5a9af58440543ea527b1df9da2446355cead` alinhou a fixture FCSE com as regras ARMv6; `4ee662d33692c40c7fbf746d4d8aab7b1f49302b` completou a sequência guest de troca e invalidação por MVA.
- `guest_tlb_keeps_same_virtual_address_per_asid_and_mva_invalidation_is_asid_scoped` usa duas tabelas de primeiro nível em TTBR0 (0x4000 e 0x8000), ambas com o mesmo VA de código e o mesmo VA de dados. O guest alterna CONTEXTIDR entre ASID 1 e 2 e troca TTBR0 para cachear duas traduções não globais diferentes para a mesma VA. Depois que ambos os descritores mudam, `MCR p15,0,r1,c8,c7,1` invalida a MVA somente para ASID 1: esse espaço refaz o page walk e observa a nova PA, enquanto ASID 2 conserva a tradução antiga em cache.
- `guest_fcse_pid_separates_low_virtual_address_tlb_entries_and_tlbi_uses_mva` mantém o PC em VA alta (fora dos 32 MiB remapeados) e acessa a mesma VA baixa com FCSE PID 1 e 2. Os MVAs são diferentes e permanecem isolados no TLB. Depois de trocar o descritor do PID 1, `MCR p15,0,r3,c8,c7,3` (TLBIMVAA) recebe o MVA modificado e remove somente aquela tradução; o PID 2 mantém a entrada anterior.
- A fixture FCSE usa descritores globais (`nG=0`). O ARM Architecture Reference Manual ARM DDI 0100I, seção B8, declara UNPREDICTABLE combinar PID FCSE não zero com suporte ASID habilitado por descritores `nG=1`; não generalizar o teste para essa combinação. O endereço MVA entregue à manutenção é produzido pelo teste, pois operações de manutenção por endereço recebem MVA e não passam novamente pela transformação FCSE.
- Workflow [43647707393](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43647707393) aprovado em Linux, Windows e macOS: `cargo fmt --all --check`, `cargo build --locked --workspace`, Clippy com `-D warnings` e `cargo test --workspace`; build release Android `arm64-v8a` também aprovado. Os dois testes novos aparecem como `ok` nos três jobs desktop.
- Alterações de código limitadas a `crates/aceris-core/tests/arm11_tlb.rs`. Nenhum código de produção, `thumb.rs`, arquivo exclusivo do Claude, helper compartilhado ou dependência foi alterado. Não demonstra cobertura integral de todas as sequências ARMv6 de contexto/FCSE nem compatibilidade com jogos comerciais.
- Referência arquitetural: ARM Architecture Reference Manual ARM DDI 0100I, apêndice B8 (FCSE e Modified Virtual Address): https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf. Nenhum código de outro emulador foi usado.

## ARM11 MMU — invalidação MVA de small pages e large pages (14/11/2026)

- A suíte externa `crates/aceris-core/tests/arm11_tlb.rs` recebeu dois testes de integração guest-visible. O commit `9b5eb4adc08e90ce5bafb0a53622c6fd13ab63b6` introduziu os cenários; `3e32d89f066d622ffd00b7934744d3c2e988a512` tipou as constantes da fixture; `50d88f363ab9312f55fa786f4701a79535018228` e `91bd25d5f0fda6a187aad9cc3ecdcc126c49a254` alinharam o arquivo ao Rustfmt.
- `guest_tlbimvaa_invalidates_only_the_target_small_page_inside_a_coarse_table` cria uma coarse table L2 e duas small pages (4 KiB) adjacentes. O guest acessa ambas para popular o TLB; o host altera os dois descritores; o guest executa `MCR p15,0,r3,c8,c7,3` (TLBIMVAA) para a MVA da primeira. O acesso seguinte ao alvo encontra a nova PA; o acesso à página irmã continua usando a PA antiga cacheada.
- `guest_tlbimvaa_matches_an_address_inside_a_large_page_and_preserves_neighbor_page` cria duas large pages (64 KiB), cada uma descrita por 16 entradas L2 consecutivas e repetidas. Após popular o TLB, altera os 32 descritores e executa TLBIMVAA usando uma MVA localizada num subtrecho de 4 KiB diferente do endereço acessado, mas ainda dentro da primeira large page. A entrada de 64 KiB deve ser invalidada como um todo e recarregada; a segunda large page permanece com a tradução antiga em cache.
- Os dois cenários executam A32 MCR/LDR com MMU ativa, L1/L2 e memória de dados sintéticas. O resultado verifica que o comparador do TLB respeita o tamanho real da tradução (máscara de 4 KiB ou 64 KiB), e não invalida a seção inteira ou a página vizinha. As codificações de page size e a repetição do large-page descriptor seguem a tabela curta ARMv6 do core; referência: ARM Architecture Reference Manual ARM DDI 0100I, seção de short-descriptor translation tables: https://www.scs.stanford.edu/~zyedidia/docs/arm/armv6.pdf.
- Workflow [43650935476](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43650935476) aprovado integralmente: `cargo fmt --all --check`, build do workspace, Clippy com `-D warnings` e `cargo test --workspace` em Linux, Windows e macOS; build release Android `arm64-v8a` também aprovado. Os dois testes novos aparecem como `ok` nos três jobs desktop.
- Alterações de código limitadas a `crates/aceris-core/tests/arm11_tlb.rs`. Nenhum código de produção, `thumb.rs`, trabalho exclusivo do Claude, helper compartilhado ou dependência foi alterado. O resultado cobre as duas formas de página testadas, não todas as combinações de descritores, manutenção/lockdown nem compatibilidade com jogos comerciais.

## PICA200 — triggers de desenho e executor de vertex shader (14/11/2026)

Claude implementou duas peças pequenas e independentes do pipeline gráfico, ambas em Rust, sem consultar código de outros emuladores. O trabalho foi pedido por Carlos Costa e restringe-se a arquivos do módulo `gpu`.

### Arquivos

- `crates/aceris-core/src/gpu/draw.rs` (novo) — `PicaDrawRequest`, `PicaDrawKind`, `PicaPrimitiveMode` e montagem de triângulos (lista, strip com winding alternado, fan). 12 testes.
- `crates/aceris-core/src/gpu/shader_exec.rs` (novo) — tabela de opcodes e executor de referência do vertex shader. 14 testes.
- `crates/aceris-core/src/gpu/pica200.rs` — somente: campos `pending_draws`/`draws_triggered`/`draws_dropped`, hook `apply_draw_trigger_register_write` chamado em `write_register` e em `execute_command_list`, acessores `take_draw_requests`/`draw_requests_triggered`/`draw_requests_dropped` e limpeza em `reset`.
- `crates/aceris-core/src/gpu/mod.rs` — declara os módulos e reexporta os tipos públicos.
- `docs/ROADMAP.md` — duas linhas do bloco PICA200 reescritas de `[ ]` para `[~]`.

### Contratos

- Um draw é aceito quando o byte baixo de 0x22E/0x22F é escrito e o bit 0 do valor está ligado; a requisição guarda o estado dos registradores naquele instante. A fila é limitada (`MAX_PENDING_DRAW_REQUESTS`) e descartes são contados.
- O executor não faz fluxo de controle nem endereçamento por a0/a1/aL; esses casos retornam erro explícito (`UnsupportedOpcode`, `AddressRegisterUnsupported`). Não os implemente por adivinhação: exigem testes independentes.
- MUL/DP*/MAD usam a regra de zero absorvente; a aritmética é `f32`.

### Verificação realmente executada

Neste ambiente, com Rust 1.97: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` passaram localmente (962 testes unitários do núcleo mais as suítes de integração). O CI remoto não foi consultado.

### Limites e dúvidas abertas

- Os números de opcode, o layout dos campos e as semânticas (zero absorvente, DPH com w=1, NaN em MAX/MIN/SLT/SGE) vêm de documentação pública reverse-engineered, e os testes foram montados à mão a partir desse mesmo layout. Eles garantem consistência interna, não equivalência com o hardware; valide com cenas/testes independentes antes de tratar o executor como correto.
- Os bits de 0x25E fora de 8–9 e o efeito de valores de gatilho diferentes de 1 não são interpretados.

### Próximas tarefas

1. Consumir `PicaDrawRequest`: resolver a base de vértices, aplicar `raw_vertex_offset`, buscar atributos/índices, ligar `v0–v15` via `PicaVertexShaderInputBindings` e rodar `PicaVertexShaderProgram::run` por vértice.
2. Encaminhar os triângulos montados ao `GeometryPipeline` e ao rasterizador.
3. Ampliar o executor com CMP/condition codes, MOVA e a0/a1/aL, depois fluxo de controle, sempre com testes por instrução.

## 3DSX — validação estrutural inicial (14/11/2026)

O ChatGPT adicionou um primeiro estágio de leitura estrutural de executáveis homebrew 3DSX, restrito ao módulo de loader. Esta contribuição não altera GPU, Thumb, CPU ou o sistema de processos.

### Commits publicados em main

- e21dc24 — adiciona crates/aceris-core/src/loader/threedsx.rs, com parser de cabeçalho, cabeçalho estendido, segmentos, BSS, cabeçalhos/tabelas padrão de relocação e limites de SMDH/RomFS.
- 1e2631f e 3f2a7a5 — corrigem os offsets usados pelo teste de cabeçalho estendido.
- 7bc8170 — conecta a validação 3DSX a loader::identify_file; a identificação deixa de aceitar um arquivo apenas pelo magic sem validar a estrutura declarada.
- 5cfabee e 560161d — aplicam Rustfmt ao parser e à integração.
- 6bf70f7 — registra o estado parcial no roadmap.

### Contrato e limites

- O parser aceita o cabeçalho base de 0x20 bytes e o bloco estendido conhecido de 0x2C bytes ou maior, valida três cabeçalhos de relocação de 8 bytes e confere os limites dos segmentos e das tabelas.
- O tamanho de arquivo do segmento Data é calculado como data_size - bss_size; o tamanho de memória inclui a cauda BSS que o loader deverá zerar.
- O parser descreve as tabelas absolutas e relativas, mas **não interpreta nem aplica suas entradas**.
- A validação estrutural não cria PreparedArm11Image, não reserva memória, não configura a sessão ARM11 e não executa um homebrew.
- O parser atualmente exige o layout padrão de 8 bytes para cada cabeçalho de relocação. Não amplie esse suporte sem especificação e testes próprios.
- Nenhuma imagem de jogo, firmware, chave, conteúdo extraído ou executável de terceiros foi adicionado ao repositório. A implementação usa Rust e não incorpora código ou assets de outro emulador.
- Os arquivos modificados nesta contribuição são crates/aceris-core/src/loader/threedsx.rs, crates/aceris-core/src/loader.rs e a documentação de estado; não alterar arquivos do módulo GPU nem o trabalho Thumb sem autorização expressa do mantenedor.

### Verificação

Workflow 43661529636 (https://github.com/CarlosCosta07-s/Aceris/actions/runs/43661529636) concluído com sucesso: cargo fmt --all --check, cargo build --locked --workspace, Clippy com -D warnings e cargo test --workspace em Linux, Windows e macOS; build release Android arm64-v8a também aprovado.

### Próximos passos

1. Definir e documentar um alvo homebrew mínimo, legalmente fornecido pelo usuário, e o subconjunto de sistema que ele requer.
2. Implementar leitura/posicionamento dos segmentos, reserva do address space e aplicação testada das relocações; zerar BSS e validar entrypoint.
3. Só depois conectar a imagem 3DSX à criação da sessão ARM11 e produzir um resultado observável com um arquivo real fornecido legalmente pelo usuário.
4. Manter claro no README/CLI que identificação e parser 3DSX não significam compatibilidade ou execução do formato.
## 3DSX — validação de relocations e materialização de segmentos (14/11/2026)

Continuação da contribuição do ChatGPT, ainda restrita ao loader 3DSX. Não foram alterados arquivos de GPU, Thumb, CPU, sessão ou espaço de endereçamento.

### Alterações adicionais

- 9d0cd13 — valida os runs skip/patch das tabelas absolutas e relativas de cada segmento, rejeitando qualquer intervalo que ultrapasse o número de words do segmento. Isso verifica limites, não aplica relocations.
- f92479e — adiciona ThreeDsxImage::load_segments, que lê os bytes file-backed de Code, ROData e Data, calcula o limite combinado de memória e cria a cauda BSS zerada.
- a256c18 — aplica Rustfmt à nova rotina e aos testes.
- 21e09ce — atualiza o roadmap com o estado exato dessa etapa.

### Contrato e limites atuais

- A materialização recebe um leitor seekable e um limite de memória definido pelo chamador; rejeita o tamanho agregado superior ao limite antes de alocar os segmentos.
- Code, ROData e Data são lidos nos offsets validados pelo parser; BSS é materializado em separado como bytes zero. A rotina não escolhe endereços virtuais nem mapeia permissões.
- Entradas de relocation continuam sem interpretação/aplicação semântica. O parser só valida os dois campos de contagem e se as sequências skip/patch não ultrapassam o segmento.
- ThreeDsxLoadedSegments ainda não é PreparedArm11Image e não deve ser convertido criando metadados NCCH fictícios. O próximo desenho deve fornecer uma entrada genérica e validada para segmentos de processo, sem enfraquecer as garantias existentes de NCCH.
- O frontend desktop e System continuam com o caminho de execução NCSD; reconhecer/ler `.3dsx` não significa que o formato já esteja carregável pela aplicação.
- Não executar um `.3DSX` nem declarar suporte até existirem aplicação de relocations validada, endereços/entrypoint, memória/permissões/BSS, integração com a sessão ARM11 e um teste de homebrew real fornecido legalmente pelo mantenedor.

### Verificação

Workflow 43661943563 (https://github.com/CarlosCosta07-s/Aceris/actions/runs/43661943563) concluído com sucesso em Linux, Windows e macOS: Rustfmt, build locked, Clippy com -D warnings e testes do workspace. O build release Android arm64-v8a também passou.

### Próximo trabalho recomendado

1. Definir o contrato de carregamento 3DSX para os segmentos e o entrypoint, mantendo a distinção entre bytes do arquivo, tamanho virtual e tamanho de memória.
2. Implementar e testar a aplicação de cada tipo de relocation suportado a partir da especificação pública, rejeitando tipos desconhecidos em vez de adivinhar.
3. Preparar um construtor genérico de processo/AddressSpace para segmentos com endereços e permissões explícitos. Revisar a propriedade de process.rs antes de editar; a implementação existente contém trabalho anterior de Claude.
4. Conectar a criação de sessão e executar um artefato homebrew mínimo, legalmente fornecido pelo mantenedor, com saída reproduzível e diagnóstico de SVCs/serviços faltantes.
## 3DSX — expansão das posições de relocation e proteção do BSS (14/11/2026)

Continuação isolada do loader 3DSX, sem alterações no trabalho de Thumb, GPU, CPU, sessão ou processo.

### Alterações adicionais

- 5be0fe6 — adiciona ThreeDsxImage::read_relocation_word_offsets, expandindo as sequências documentadas de u16 skip/u16 patch em índices de palavras por segmento. Exige um limite global de patches para limitar alocação de memória.
- 9d6c3f5 — restringe as validações dos patches aos bytes efetivamente armazenados nos segmentos. O Data inclui BSS no tamanho de memória, mas a cauda BSS não existe no arquivo e não pode ser alvo de correção feita a partir das tabelas.
- 20e0059 — aplica Rustfmt ao leitor de índices de relocation.
- 5cd4380 — corrige a verificação da tabela relativa para usar também o tamanho file-backed, igual à tabela absoluta.
- d1ddf9b — atualiza este registro e o roadmap.

### Contrato e limites

- Os índices retornados são offsets de palavra relativos ao início do segmento em questão. O método apenas expande skip/patch; não interpreta o valor armazenado, não calcula endereços de destino e não modifica Code/ROData/Data.
- O limite global maximum_patched_words é obrigatório e compartilhado pelas seis tabelas (absolute/relative de Code, ROData e Data).
- Ambas as tabelas são limitadas por file_size_bytes; nenhuma entrada pode apontar para a cauda BSS.
- O parser e a materialização ainda não aplicam relocation, não arredondam/mapeiam segmentos em um address space de processo e não criam uma sessão ARM11.
- Não usar os índices para aplicar patches até que os tipos e a semântica de cada relocation estejam confirmados por documentação independente do código de outros emuladores. As regras internas de outros emuladores não foram usadas como implementação.

### CI

- Workflow 43662274175 (commit 20e0059) concluiu com sucesso em Linux, Windows e macOS, incluindo Rustfmt, build, Clippy e testes; Android arm64-v8a também passou.
- Workflow 43662363452 (commit 5cd4380) ainda estava em andamento na última verificação. Não marcar o commit 5cd4380 como aprovado até conferir o resultado final.

### Próximos passos

1. Confirmar a semântica dos tipos de relocation em documentação primária do formato/fornecedor da toolchain, sem ler código interno de emuladores.
2. Implementar patches absolutos/relativos em arrays de segmentos de teste, com fixtures independentes cobrindo overflow, offsets entre segmentos, subtype desconhecido e PREL31 se confirmado pela especificação.
3. Só depois criar um objeto preparado de segmentos com tamanhos virtuais alinhados, entrypoint e permissões explícitas; não fabricar NcchExtendedHeader para reaproveitar o caminho NCCH.
4. Revisar o ownership de process.rs antes de alterar qualquer construtor existente, pois contém trabalho anterior de Claude.
5. Integrar o processo ao caminho de sessão e testar com um arquivo homebrew legalmente fornecido pelo mantenedor.
## 3DSX — aplicação de relocations e preparação de layout de processo (15/11/2026)

Esta etapa continua isolada nos módulos novos de 3DSX. Não foi alterado crates/aceris-core/src/loader/ncsd/process.rs nem crates/aceris-core/src/loader/ncsd/session.rs, preservando o trabalho anterior de Claude em memória, scheduler e serviços.

### Relocations aplicadas

- 0c00380 — adiciona a primeira implementação de apply_relocations; essa tentativa não passou CI por formato/compilação e foi substituída pelos commits seguintes.
- 8cefc2f — altera a aplicação para preparar todos os patches numa lista temporária, validando sites, destinos e tipos antes de modificar qualquer segmento. Um erro não deixa o processo parcialmente relocacionado.
- 4cb6ec2 — adiciona testes para relocação absoluta, relativa de 32 bits, PREL31, destino na região BSS, subtype desconhecido, destino externo, bases desalinhadas/sobrepostas e atomicidade.
- d3d1e13 e 8a832af — corrigem Rustfmt; 8a832af é o commit que passou a matriz completa de CI para a aplicação de relocations (workflow 43663135550).

Contrato atualmente implementado:

- A chamada recebe três bases virtuais explicitamente fornecidas, que devem ser page-aligned, não sobrepostas e dentro do espaço de 32 bits.
- O image offset de 28 bits é resolvido contra Code, ROData e Data após alinhamento de página; o destino pode apontar para BSS e page padding.
- Tabela absoluta aceita subtype 0; tabela relativa aceita subtype 0 como diferença assinada de 32 bits e subtype 1 como PREL31, preservando/zerando o bit reservado conforme ABI.
- Subtipos não suportados, overflow, alvo fora da imagem e bases inválidas são rejeitados. Os patches são staged e aplicados apenas depois de todas as validações.
- ThreeDsxLoadedSegments registra internamente se relocations terminaram com sucesso. A API recusa segunda aplicação. A preparação de processo exige que as tabelas sejam aplicadas quando o arquivo declara relocations.

### Preparação da memória de processo 3DSX

- f10f605 — adiciona crates/aceris-core/src/loader/threedsx/process.rs com ThreeDsxStackRequest, ThreeDsxProcessLayout, ThreeDsxProcessMemorySegment e ThreeDsxPreparedProcess; expõe o módulo em threedsx::process.
- O construtor recebe explicitamente bases de Code/ROData/Data, pedido de stack e limite agregado de memória; valida alinhamento, ranges, sobreposição de stack, limites de 32 bits e limite total.
- Cria buffers page-aligned: Code RX, ROData R, Data RW incluindo BSS zerado e padding, além de stack RW zerada. Entry point é Code base; SP inicial é o topo da stack solicitada.
- beeae90 / 91e645e — introduzem estado privado de conclusão das relocations e exigem que o processo não seja preparado a partir de segmentos ainda não relocacionados quando há tabelas.
- a97ef3e — adiciona testes para exigência de relocations e rejeição de uma segunda passada. O commit precisou apenas de uma correção de formato após o primeiro CI.
- 06d5d3b — corrige o Rustfmt do guard de relocations.
- af445da — atualiza a documentação do módulo e adiciona getter read-only relocations_applied().
- 296ae54 — atualiza o roadmap.

### Estado de CI

- Workflow [43663672991](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43663672991), associado ao HEAD `8614b2a5c9e10da0011df657e0dea375fc98fc67`, concluiu com sucesso em Linux, Windows, macOS e Android `arm64-v8a`.
- A branch `main` foi confirmada nesse mesmo commit. Os workflows anteriores 43663288985, 43663540460 e 43663506572 também validaram estados anteriores do loader/preparação; o workflow do HEAD é a referência atual.

### Limites e próximo ponto de integração

- ThreeDsxPreparedProcess é um layout e conjunto de buffers validados; não é Bus, Arm11CodeAddressSpace, Arm11ProcessSession nem contexto ARM11 executável.
- Não foi fabricado NcchExtendedHeader, CodeSetInfo, stack size, memory type ou metadado de NCCH. A stack é explicitamente solicitada pelo chamador; os defaults/requisitos corretos ainda precisam ser definidos pelo alvo homebrew escolhido.
- Não alterar process.rs ou session.rs do NCSD sem autorização expressa do mantenedor, pois contêm trabalho de Claude. Próximo passo é desenhar com o mantenedor/Claude um contrato genérico para mapear buffers com bases/permissões explícitas e criar uma sessão ARM11 sem degradar a rota NCCH.
- Ainda não há um arquivo homebrew real fornecido/testado. Identificação, materialização, relocation e preparação de layout não equivalem a execução nem a compatibilidade de homebrew.

## 3DSX — contrato de integração com ARM11 (15/11/2026)

- Foi inspecionado o contrato existente sem modificar `loader/ncsd/process.rs`, `loader/ncsd/session.rs`, GPU ou Thumb.
- A revisão confirmou que `Arm11CodeAddressSpace::from_prepared` aceita `PreparedArm11Image`/NCCH, enquanto `Arm11ProcessSession::from_address_space` é o construtor comum de sessão após existir um espaço de endereçamento.
- Há uma incompatibilidade de layout que impede conversão direta: o prepared 3DSX contém Data e BSS contíguos no mesmo mapeamento RW; o caminho NCCH atual modela a cauda BSS como região separada e também depende de memory type e stack size vindos do extended header.
- Criado `docs/THREEDSX_ARM11_INTEGRATION.md` (commit `a9b9c10`) com contrato proposto para uma imagem genérica de processo, preservação de permissões/endereço/BSS, configuração runtime explicitamente fornecida, integração com sessão e testes de aceitação.
- Esta entrega é documentação de arquitetura, não execução 3DSX. O próximo código requer alinhamento com o responsável pelos arquivos de processo/sessão para que a rota NCCH seja adaptada sem perda de semântica.
- CI do HEAD verificada no workflow [43663672991](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43663672991): Linux, Windows, macOS e Android ARM64 aprovados.


### Verificação adicional do contrato de mapeamento (15/11/2026)

- O commit `a5a21a7` adicionou `prepared_segments_preserve_the_complete_3dsx_mapping_contract`: valida que os quatro buffers têm o tamanho declarado no layout, verifica permissões Code RX / ROData R / Data RW / Stack RW, confirma o início/endereço do BSS dentro do Data contíguo e compara o total mapeado com a soma das regiões.
- `9bfa895` aplicou o formato solicitado por Rustfmt; `ecebd93` removeu a conversão redundante apontada por Clippy.
- Workflow [43664492854](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43664492854), para o commit `ecebd9364ba2f6113c09cba95f602cb5ca6fd9c1`, concluiu com sucesso em Linux, Windows, macOS e Android `arm64-v8a` (fmt, build, Clippy, testes e build release Android).
- A regressão cobre a estrutura de memória preparada, não a instalação em um `Arm11CodeAddressSpace` nem a execução de um homebrew. Esses itens continuam pendentes do contrato genérico descrito em `docs/THREEDSX_ARM11_INTEGRATION.md`.


## 3DSX — revalidação do processo preparado e perfil convencional de stack (15/11/2026)

- `bf4b4a2` introduziu `ThreeDsxPreparedProcess::validate()` para revalidar o objeto antes de qualquer instalação: comprimentos, alinhamento, permissões, ranges de 32 bits, entry point, SP, BSS, sobreposições, total de memória e regiões de zeros.
- `ca0ff00` acrescentou testes negativos de buffer truncado, permissões alteradas, BSS movido, bytes não zerados e stack sobreposta; o CI apontou somente formatação e o formato foi corrigido em `3aa81b4`.
- `7e9ac40` adicionou um snapshot privado do layout criado após relocations; a validação agora rejeita mudanças posteriores de base/entrypoint que poderiam tornar os ponteiros relocacionados incompatíveis com os novos endereços. O layout da stack também deve começar sem bytes inicializados.
- Workflow [43665552498](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43665552498) aprovou essa etapa em Linux, Windows, macOS e Android `arm64-v8a`.
- `a5e0db5` acrescentou `ThreeDsxStackRequest::conventional_3ds_homebrew()`, um perfil runtime explícito com base `0x0FFFC000`, tamanho `0x4000` e topo/SP `0x10000000`. O valor não vem do formato 3DSX e pode ser substituído pelo chamador. O primeiro teste precisou elevar o limite combinado da fixture para `0x7000` porque ela mapeia três segmentos de uma página mais a stack de 16 KiB; `bc17a71` corrige esse limite.
- Referências de documentação consultadas: 3dbrew [Memory map](https://www.3dbrew.org/wiki/Memory_map) descreve a stack principal em `0x10000000 - StackSize`, usualmente `0x4000`; a API pública libctru [svc.h](https://github.com/devkitPro/libctru/blob/master/libctru/include/3ds/svc.h) expõe `StartupInfo.stack_size` e documenta o alinhamento de 8 bytes para stack top em `svcCreateThread`. Nenhum código de outro emulador foi consultado nem incorporado.
- `036077b` atualiza `docs/THREEDSX_ARM11_INTEGRATION.md` e o roadmap com o validador e o perfil convencional; essa documentação não transforma o perfil em requisito universal de todo 3DSX.
- Limite atual: ainda não existe adaptador para `Arm11CodeAddressSpace`/sessão, nenhum homebrew real foi executado e o tamanho/base da stack devem continuar configuráveis.


## 3DSX — manifesto imutável de mapeamentos de processo (15/11/2026)

- Commit `6c3cd38` adiciona `ThreeDsxProcessMemoryKind`, `ThreeDsxProcessMemoryMapping<'a>` e `ThreeDsxPreparedProcess::memory_mappings()`.
- A API devolve um array fixo de quatro visões emprestadas, em ordem Code/ROData/Data/Stack, incluindo endereço virtual, tamanho inicializado, tamanho mapeado, backing bytes e permissões explícitas R/W/X. Não aloca nem copia os buffers.
- `memory_mappings()` executa `validate()` antes de expor qualquer descritor. Uma alteração de layout posterior às relocations é rejeitada. ROData e Data vazios podem ser representados com tamanho zero; um adaptador deve ignorar esses ranges em vez de criar uma página artificial.
- Testes novos verificam ordem, endereços, tamanhos, permissões, bytes iniciais e zeros, além de garantir que a API rejeita layout modificado.
- A interface ainda não instala memória nem cria `Arm11CodeAddressSpace`/sessão. Ela fornece um contrato de dados estável ao adaptador que deve pertencer ao responsável pelo espaço ARM11.
- Atualizados `docs/THREEDSX_ARM11_INTEGRATION.md` e `docs/ROADMAP.md` para refletir a nova fronteira.

## 3DSX — manifesto de inicialização (15/11/2026)

- Commit `68cf40f` acrescenta `ThreeDsxProcessMemoryManifest<'a>` e `ThreeDsxPreparedProcess::memory_manifest()`. O manifesto agrupa os quatro descritores emprestados, o entry point, o SP inicial e a faixa exata de BSS.
- A API parte de `memory_mappings()`, que chama `validate()`; portanto não expõe metadados de inicialização de um processo cujo layout ou backing foi alterado. O Data permanece como uma única região RW que contém Data inicializado, BSS e padding zerado.
- Dois testes verificam o BSS no offset exato dentro do Data e a rejeição de um backing alterado. A primeira execução de CI apontou somente um ajuste de Rustfmt no teste, corrigido no commit `43d8c75`; aguardar o resultado dessa execução antes de declarar a etapa aprovada.
- Para o responsável pelo espaço ARM11: este manifesto é a fronteira de entrada do lado 3DSX. Não criar um `NcchExtendedHeader` fictício nem separar artificialmente BSS. Não editar `loader/ncsd/process.rs` ou `loader/ncsd/session.rs` sem autorização expressa do mantenedor; preservar o trabalho atribuído ao outro agente.
- O manifesto ainda não cria ou instala páginas, não conecta backing físico, não inicia scheduler/SVC e não significa que homebrew 3DSX já esteja executável.

## 3DSX — manifesto de memória somente leitura (15/11/2026)

- Commit `155aaf4` torna privados os campos de `ThreeDsxProcessMemoryMapping` e `ThreeDsxProcessMemoryManifest`, oferecendo accessors somente de leitura. Consumidores externos não podem fabricar ou alterar endereços, tamanhos, permissões, backing slices ou dados de inicialização e ainda chamar isso de manifesto produzido pelo loader.
- `ThreeDsxProcessMemoryManifest::bss_offset_bytes()` expõe o offset de BSS dentro da região Data já validada, evitando que o adaptador recalcule esse valor.
- Commit `99b5d0a` ajusta os testes para consumir exclusivamente os accessors públicos, inclusive a consulta ao array de mapeamentos.
- A validação do manifesto continua baseada em `ThreeDsxPreparedProcess::validate()`; isso não cria memória MMU, sessão nem execução. Preservar o BSS dentro do mapeamento Data.

## 3DSX — layout e backing imutáveis (15/11/2026)

- Commit `99f61c2` torna privados os buffers Code, ROData, Data e Stack de `ThreeDsxPreparedProcess`; os métodos `code_bytes()`, `read_only_bytes()`, `data_bytes()` e `stack_bytes()` retornam somente fatias imutáveis.
- Commit `ea067be` também torna privado o layout e expõe `layout()` como referência somente de leitura. A validação já rejeitava alterações no layout, mas o backing inicial podia ser alterado publicamente sem invalidar o objeto; esta alteração fecha essa brecha na API externa.
- Os testes internos continuam podendo fabricar objetos adulterados para validar a rejeição de dados fora do contrato; consumidores externos não recebem acesso mutável às estruturas.
- Commits `8724ad6` e `ae1dfd3` acrescentam casos sintéticos de segmentos vazios e de execução do pipeline parse → relocation → prepare → manifesto. Nenhum deles constitui teste de homebrew real.

## 3DSX — pipeline único de preparação (15/11/2026)

- Commit `fed2fb8` acrescenta `ThreeDsxProcessPreparationRequest` e `ThreeDsxImage::load_and_prepare_process()`. O request exige bases de Code/ROData/Data, stack, limite de memória e orçamento máximo de words relocacionadas.
- O método executa load → apply relocations → prepare e somente retorna o processo quando todas as etapas são concluídas. Também chama a etapa de relocation quando as tabelas estão vazias para manter a mesma regra de estado.
- Testes cobrem o caminho completo relocacionado e a rejeição quando o orçamento de patches é insuficiente. Isso é pipeline sintético de preparação, não sessão ARM11 nem homebrew real.

## 3DSX — pré-validação antes de alocar buffers (15/11/2026)

- Commit `f317b04` extrai `plan_process_memory()` para validar bases, page alignment, overlap da stack e limite de memória total antes de ler os segmentos no caminho `load_and_prepare_process()`.
- `prepare_process()` reutiliza o mesmo planner antes de alocar Code/ROData/Data/Stack, eliminando divergência entre preflight e materialização.
- O teste `load_and_prepare_process_enforces_page_aligned_memory_limit` confirma que um limite suficiente para bytes crus, mas menor que o total page-aligned + stack, falha antes de avançar o reader.

## Integração 3DSX à sessão ARM11 e aos frontends (15/11/2026)

Esta etapa foi autorizada pelo mantenedor ao solicitar a continuação do desenvolvimento até o primeiro marco de homebrew executável. O trabalho foi feito na branch `main`, sem criar branches e sem importar código ou assets de emuladores de terceiros.

- Commit `208191b` conecta o processo 3DSX preparado ao `Arm11CodeAddressSpace`, à sessão ARM11 existente e ao `System`; acrescenta um perfil inicial explícito de endereçamento, stack e limites de segurança.
- Commit `288aa42` aplica Rustfmt aos arquivos do adaptador.
- Commit `36a0498` expõe `System::load_executable_file` pelo ABI Android com `aceris_system_load_executable_file`, conserva a API NCSD antiga e atualiza a documentação.
- Desktop e Android agora encaminham 3DSX e NCSD sem criptografia ao mesmo core/scheduler. O suporte de carregamento não deve ser duplicado em um frontend.
- `crates/aceris-core/tests/three_dsx_launch.rs` monta uma fixture 3DSX sintética, executa três instruções ARM e verifica registradores/PC; outro teste garante que uma imagem inválida não substitui a sessão ativa. O teste do ABI Android também carrega uma fixture sintética e verifica a execução/PC.
- CI do commit `288aa42`, workflow `43754731119`: Rustfmt, build locked, Clippy e testes passaram em Linux, Windows e macOS; build release Android `arm64-v8a` passou.
- CI do commit `36a0498`, workflow `43754894087`: matriz completa aprovada em Linux, Windows e macOS (Rustfmt, build locked, Clippy e testes) e build release Android `arm64-v8a` aprovado.

**Limite obrigatório de interpretação:** os testes atuais executam bytes de instruções fabricados para fixtures sintéticas. Eles comprovam a integração do parser/preparador 3DSX com a sessão ARM11 e os frontends, mas não comprovam que um homebrew de terceiros, não modificado, inicialize, use seus serviços ou produza gráficos/áudio. Não marcar M2 como concluído até executar um arquivo homebrew real, fornecido legalmente pelo mantenedor, e registrar uma saída observável e reproduzível.

**Próxima etapa:** obter do mantenedor um arquivo `.3dsx` homebrew que ele tenha direito de usar. Executá-lo sem alterar o binário, registrar loader/entry PC/CPSR, instruções executadas e primeiro motivo de parada; depois implementar somente o primeiro SVC/serviço ausente demonstrado pelos logs. Manter o trabalho limitado ao necessário para esse alvo. Não introduzir dependências, firmware, chaves, jogos, dumps ou assets protegidos no repositório.

**Preservação da colaboração:** o adaptador 3DSX foi integrado aos arquivos compartilhados `loader/ncsd/process.rs` e `loader/ncsd/session.rs` por autorização explícita do mantenedor nesta etapa. Isso não concede autorização geral para modificar ou reescrever trabalho de outro agente. Ler o diff e preservar as alterações de terceiros; não tocar em `cpu/thumb.rs` ou em outras frentes atribuídas sem autorização específica.


## CIA — primeiro caminho de execução de conteúdo claro (15/11/2026)

- Commit `73783fb` extrai a preparação de executável NCCH para uma função compartilhada, usada pelo carregador NCSD e reutilizável por leitores de conteúdo CIA.
- Commit `c8e61be` acrescenta `CiaContentNcchReader::load_arm11_address_space()`: o conteúdo precisa ter sido aberto pelo leitor que valida o SHA-256 do TMD; em seguida o caminho comum valida extended header, ExeFS e `.code`, aplica BLZ quando declarado e monta o mesmo layout ARM11.
- Commit `73ebb1f` liga `ImageFormat::Cia` ao `System::load_executable_file()`. O sistema seleciona o boot content pelo TMD, exige que o NCCH program ID corresponda ao TMD title ID e, nesta primeira etapa, só aceita conteúdo CIA não criptografado com NCCH NoCrypto. A sessão ativa só é substituída depois que toda a preparação termina.
- Commits `269d1c9`, `8386a3d` e `c172997` corrigem Rustfmt e um borrow conflict no fixture sintético.
- O teste `loads_clear_no_crypto_cia_boot_content_into_arm11_session` monta um CIA sintético com ticket/TMD e hashes coerentes, carrega o boot NCCH, executa duas instruções ARM e confere os registradores. Não inclui nenhum pacote comercial nem conteúdo de usuário no repositório.
- O ABI Android e o frontend desktop já encaminham o formato pelo mesmo `System::load_executable_file()`; a documentação do ABI Android e o roadmap foram atualizados.

### Análise da amostra CIA fornecida pelo mantenedor

- A amostra local `0004000000F23F00 Cookie Clicker port for 3DS (CTR-P-LPPD) (v0.0.0) (W).standard.cia` tem 3.072.960 bytes. O conteúdo começa no offset `0x3900`, tem tamanho `0x2e7000`, e a seção meta começa em `0x2ea900` com tamanho `0x3ac0`; a seção meta termina exatamente no EOF.
- O TMD declara um único conteúdo, índice/boot index 0, Title ID `0004000000F23F00`, tipo `0x0000` (sem AES-CBC externo) e tamanho `0x2e7000`. O SHA-256 do conteúdo em `0x3900` confere com o hash do TMD. Ticket e TMD contêm o mesmo Title ID.
- O conteúdo começa com NCCH no offset interno `0x100`; o Program ID é `0004000000f23f00`, igual ao Title ID, e o Product Code é `CTR-P-LPPD`. O NCCH declara NoCrypto (bit `0x04` em `flags[7]`) e extended header de `0x400` bytes; o SHA-256 do extended header confere.
- O ExeFS começa no offset interno `0x2a00`, tem `0x1c0a00` bytes e hash region de `0x200`. Conferem o hash do superbloco e os hashes de `.code` (`0x154384` bytes), `banner`, `icon` e `logo`.
- O extended header marca `.code` como comprimido (bit 0 das flags = 1). O trailer BLZ declara saída `0x2ff000`, igual ao tamanho descomprimido esperado pelo layout dos três code sets. Um decodificador independente que reproduz o algoritmo BLZ implementado em `loader/ncsd/ncch/blz.rs` produziu exatamente esse tamanho e consumiu toda a entrada; a primeira palavra ARM obtida é `0xEA000006`.
- O RomFS está no offset interno `0x1c4000`, tem `0x123000` bytes e hash region de `0x200`; o hash do superbloco confere. A verificação independente da cadeia IVFC passou: 1 bloco de hash mestre, 3 blocos de Level 1 e 286 blocos de Level 2. O cabeçalho Level 3 e suas tabelas passaram pelas verificações de limites/ordem; a caminhada da árvore encontrou 2 diretórios e 46 arquivos.
- As verificações acima são uma inspeção independente dos bytes (offsets, SHA-256, IVFC, BLZ e tabelas RomFS), **não uma execução do CIA pelo `System::load_executable_file()` Rust**. Para tornar essa última etapa reproduzível no checkout do mantenedor, foi adicionado o teste ignorado `loads_external_cia_and_executes_one_arm11_instruction`. Ele usa `ACERIS_TEST_CIA` e `load_executable_file()`; não lê nenhum caminho fixo nem adiciona a amostra ao repositório/CI.

### Limites e próxima etapa

- Ainda não há verificação criptográfica das assinaturas RSA/ECDSA do ticket e do TMD. SHA-256 e consistência estrutural não autenticam essas assinaturas.
- `load_cia_no_crypto_file()` é o caminho sem chaves para conteúdo CIA claro e NCCH NoCrypto. Para conteúdo CIA AES-CBC e/ou regiões NCCH AES-CTR, o núcleo oferece `load_cia_with_keys()` e `load_cia_file_with_keys()`, com `TicketCommonKeyStore`, `menu_info_key` e `content_key` explicitamente fornecidos. Não existe descoberta automática, chave embutida ou persistência automática de chaves.
- O teste de duas camadas criptográficas é sintético e aprovado na CI, mas não autentica assinaturas nem comprova compatibilidade com chaves de um console real.
- Ainda não existe instalação persistente, catálogo/registro de títulos, gestão de tickets instalados, atualização de títulos, save data ou remoção de títulos.
- O teste opcional `loads_external_cia_and_executes_one_arm11_instruction` está ignorado por padrão e só roda quando `ACERIS_TEST_CIA` aponta para um CIA local. A amostra fornecida continua fora do repositório e da CI.
- A CI [43761991014](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43761991014), no commit `b685d83`, aprovou `cargo fmt --all --check`, `cargo build --locked --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` em Linux, Windows e macOS, além do build release Android `arm64-v8a`.
- Próxima etapa: executar o teste opcional no checkout local com o CIA do mantenedor e registrar se o carregador Rust passa por todos os hashes, pelo BLZ e pela inicialização ARM11; depois investigar os primeiros serviços/SVCs necessários para continuar a execução real. A instalação persistente vem depois da verificação de assinaturas e do projeto seguro de configuração de chaves.


## CIA — suporte explícito a conteúdo AES-CBC por chaves fornecidas (15/11/2026)

Esta seção atualiza o estado do trabalho depois do primeiro carregamento CIA sem criptografia descrito acima.

- Commit `33cf04c` acrescenta `CiaDecryptedContentNcchReader::load_arm11_address_space()`. O leitor já removeu a camada AES-CBC externa e verificou o SHA-256 do conteúdo contra o TMD; a preparação NCCH compartilhada valida extended header, ExeFS e `.code`, aplica BLZ quando declarado e recebe as chaves NCCH do chamador.
- O mesmo commit acrescenta `System::load_cia_with_keys()` e `System::load_cia_file_with_keys()`. O sistema seleciona o boot content, obtém a title key por `TicketCommonKeyStore::decrypt_ticket_title_key()`, verifica o conteúdo CBC e confere NCCH program ID contra o TMD title ID. A sessão ativa só é substituída depois da preparação completa.
- O chamador deve configurar explicitamente `TicketCommonKeyStore` e fornecer `menu_info_key` e `content_key`. Não há descoberta automática, chaves embutidas nem persistência automática de material criptográfico. `load_executable_file()` permanece no caminho simples sem chaves; o novo caminho requer chamada explícita pela API do núcleo.
- O teste `loads_encrypted_cia_boot_content_with_caller_supplied_ticket_keys` cria um CIA sintético com ticket title key e conteúdo CIA AES-CBC criptografados, NCCH NoCrypto e hashes TMD coerentes; verifica o carregamento e executa duas instruções ARM11.
- O teste integrado `loads_encrypted_cia_and_encrypted_ncch_into_arm11_session` coloca AES-CTR no extended header e no ExeFS `.code` com chaves separadas, depois aplica AES-CBC ao conteúdo inteiro do CIA. Ele valida ticket/title key, hash TMD, hash do extended header, superbloco/arquivo ExeFS e execução ARM11.
- O workflow [43761991014](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43761991014), no commit `b685d83`, passou integralmente: Rustfmt, build locked, Clippy e `cargo test --workspace` em Linux, Windows e macOS, além do build release Android `arm64-v8a`.
- O teste opcional `loads_external_cia_and_executes_one_arm11_instruction` permite tentar o CIA local através de `load_executable_file()` sem inserir bytes do pacote no repositório ou CI. Ele requer a variável `ACERIS_TEST_CIA` e permanece ignorado na execução padrão.
- A amostra Cookie Clicker foi inspecionada em nível de bytes, SHA-256, IVFC, BLZ e tabelas RomFS; isso ainda não substitui a execução do teste opcional pelo carregador Rust local.

### Limites atualizados e próximos passos

- As assinaturas RSA/ECDSA do ticket e do TMD ainda não são verificadas. Hashes e consistência estrutural não substituem autenticação criptográfica.
- A API de carregamento com chaves existe apenas no núcleo Rust; ela ainda não foi ligada ao ABI Android ou à interface desktop, nem existe fluxo seguro de configuração de chaves na interface.
- O caminho de inicialização com chaves é demonstrado por fixtures artificiais e CI; não confirma chaves reais de console.
- Ainda não existe instalação persistente, catálogo/registro de títulos, gestão de tickets instalados, atualização de títulos, save data ou remoção de títulos.
- Próxima etapa: executar o teste opt-in com a amostra no checkout local; capturar exatamente o primeiro erro ou estado após uma instrução ARM11; implementar somente o primeiro serviço/SVC comprovadamente necessário; seguir para assinatura/autorização e desenho seguro de chaves antes da instalação persistente.


## ABI Android — carregamento explícito de CIA criptografado (15/11/2026)

Com a autorização de continuidade do mantenedor, o ChatGPT estendeu o contrato C ABI do crate Android sem alterar o trabalho de Thumb:

- Adiciona `aceris_system_load_cia_file_with_keys`, que cria um `TicketCommonKeyStore` temporário por chamada e encaminha para `System::load_cia_file_with_keys()`.
- O chamador informa índice de common key (0–5), tipo de material e buffers AES-128 para o common key, menu-info e content. Modo KeyY exige KeyX; modo normal exige ponteiro KeyX nulo.
- Publica as constantes `ACERIS_CIA_KEY_MATERIAL_KEY_Y` (0) e `ACERIS_CIA_KEY_MATERIAL_NORMAL` (1).
- As chaves não são guardadas em `AcerisSystemHandle`, não são gravadas em disco e não há descoberta automática. O código não promete apagamento seguro de memória.
- Testes do ABI cobrem rejeição de índice/tipo/modo inválido e o encaminhamento de configurações KeyY/normal ao carregador do núcleo; eles não substituem a execução de um CIA criptografado com chaves reais.
- As assinaturas RSA/ECDSA de ticket/TMD continuam sem verificação; a API não habilita instalação persistente de títulos.
- A matriz de CI deste commit deve ser consultada antes de declarar formatação, Clippy, testes e build Android aprovados.


## PICA200 — estágio de vértices de um draw (`shade_draw`) (15/11/2026)

Claude ligou os triggers de desenho ao executor de vertex shader. Trabalho pedido por Carlos Costa, restrito ao módulo `gpu`.

### Arquivos

- `crates/aceris-core/src/gpu/draw_exec.rs` (novo) — `Pica200::shade_draw`, `PicaShadedDraw`, `PicaShadedVertex`, `PicaDrawExecError`. 13 testes.
- `crates/aceris-core/src/gpu/mod.rs` — declara o módulo e reexporta os três tipos.
- `docs/ROADMAP.md` — item dos triggers atualizado.

### Contrato

- `shade_draw(request, memory, resolved_physical_byte_base, program, shader_entry)`: índices, contagem, offset e modo de primitiva vêm do `PicaDrawRequest` (capturado no trigger). O layout de vértices e as permutações de entrada do shader vêm do banco de registradores **vivo**; chame antes que a command list os altere.
- O chamador resolve o endereço físico base e informa o entrypoint, porque nenhum registrador de entrypoint é decodificado.
- Cada vértice distinto é shadeado uma vez; os triângulos referenciam `vertices` por índice e preservam o winding da montagem.
- Saídas o0–o15 são devolvidas sem output map; quem decide qual registrador é a posição é a etapa seguinte.
- Erros explícitos: índice fora do u32 após o offset, fixed attribute sem valor, dois atributos no mesmo registrador (precedência não documentada), registrador acima de v15, falha de fetch e falha do shader com o índice do vértice.

### Verificação realmente executada

Neste ambiente (Rust 1.97), sobre o `main` `bd64b41`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` (1285 testes, 0 falhas). O CI remoto não foi consultado.

### Premissas não verificadas contra hardware

- `GPUREG_VERTEX_OFFSET` é um inteiro com sinal somado a todo índice, em draws de arrays e de elements.
- Componentes que o formato do atributo não fornece valem (0, 0, 0, 1).
- Os testes montam registradores a partir do mesmo layout documentado que o código usa; provam consistência interna, não compatibilidade.

### Próximas tarefas

1. Decodificar o entrypoint do shader e o output map (o0–o15 → posição, cor, texcoord).
2. Converter as saídas em `GeometryVertex`/`RasterVertex` e ligar ao `GeometryPipeline`.
3. Clipping/culling com semântica confirmada, depois fluxo de controle e CMP/MOVA no executor.

## PICA200 — entrypoint e output map do vertex shader (15/11/2026)

### Arquivos

- `crates/aceris-core/src/gpu/output_map.rs` (novo) — `REG_VSH_ENTRYPOINT` (0x2BA), `REG_SH_OUTMAP_O0` (0x050, sete registradores), `PicaVertexOutputMap`, `PicaResolvedVertex`, `PicaOutputMapError`, `Pica200::vertex_shader_entrypoint` e `Pica200::vertex_output_map`. 8 testes.
- `crates/aceris-core/src/gpu/draw_exec.rs` — `shade_draw_at_registered_entrypoint` e um teste ponta a ponta (draw → shader → output map → `GeometryVertex`).
- `crates/aceris-core/src/gpu/mod.rs` — módulo e reexportações.

### Contrato

- O mapa é decodificado só pelos bytes de semântica; o registrador de contagem de saídas não é consultado. O estado de energia (todos os registradores em zero) mapeia `position.x` quatro vezes e por isso é rejeitado como `DuplicateSemantic`: programe o mapa antes de resolver.
- Cada grupo (posição, cor, texcoord0/1/2) é tudo-ou-nada; semântica repetida e ausência de posição são erros, sem completar lanes com padrões inventados.
- `to_geometry_vertex(default_color)` limita a cor a 0..=1, arredonda para 8 bits e trata NaN como 0; sem cor mapeada, usa a cor informada pelo chamador.
- Quaternion (0x04–0x07), view (0x12–0x14) e texcoord0.w (0x10) são aceitos e ignorados.

### Verificação realmente executada

Sobre o `main` `f4d7e8f`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` (1294 testes, 0 falhas), todos com saída 0 neste ambiente. O CI remoto não foi consultado.

### Premissas não verificadas

- Os números de semântica, o endereço dos registradores e o uso dos bits 0–15 do entrypoint vêm de documentação pública recordada de memória, e os testes usam essa mesma numeração. Confirme com uma fonte independente antes de depender deles.
- A conversão de cor para 8 bits (clamp e arredondamento) é uma escolha de bring-up, não comportamento confirmado do hardware.

### Próximas tarefas

1. Ligar `PicaResolvedVertex` ao `GeometryPipeline::draw_triangle` e ao `SoftwareRasterizer` para fechar o caminho draw → pixels.
2. Clipping/culling com semântica confirmada; interpolação de texcoord quando houver texturas.
3. Ampliar o executor com CMP/MOVA/a0 e fluxo de controle.

## PICA200 — caminho draw → pixels (15/11/2026)

### Arquivos

- `crates/aceris-core/src/gpu/render.rs` (novo) — `GeometryPipeline::draw_shaded`, `Pica200::render_draw_request`, `PicaRenderReport`, `PicaRenderError`. 6 testes, um deles do trigger de desenho aos pixels sem shading escrito à mão.
- `crates/aceris-core/src/gpu/mod.rs` — módulo e reexportações.

### Contrato

- As saídas do shader são coordenadas de clip; use um `GeometryPipeline` com transformação identidade, a menos que o vértice de entrada ainda precise de uma matriz. A faixa de profundidade (`ClipDepthRange`) e o viewport são escolhas do chamador; a faixa real do PICA200 não está confirmada.
- Cada vértice passa pelo output map uma vez; os triângulos são rasterizados na ordem do draw. Não há culling de face: os dois sentidos são desenhados. A primeira falha interrompe o draw e deixa no rasterizador o que já foi desenhado.
- `render_draw_request` lê layout de vértices, entrypoint e output map dos registradores vivos; chame antes que a command list os altere.

### Armadilha de codificação (custou duas tentativas)

No registrador de formato de atributos, cada nibble é **tipo (2 bits) + largura em bits (2 bits)**, não contagem de componentes. A contagem de componentes de um atributo é o número de entradas do array (`ATTRIBBUFFERi_CONFIG2` bits 28–31) que o referenciam pelo mapa em `CONFIG1`. Para `(x, y)` em `u8`: formato `0x1`, duas entradas apontando para o atributo 0, `CONFIG1 = 0`, `CONFIG2 = (2 << 16) | (2 << 28)`. O valor `0x5` seria `u8` com largura de 16 bits.

### Verificação realmente executada

Sobre o `main` `9beaf77`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` (1300 testes, 0 falhas), todos com saída 0 neste ambiente. O CI remoto não foi consultado.

### Limites

- Só cor por vértice. Sem textura, iluminação, blending, stencil, culling nem formatos reais de framebuffer.
- O teste ponta a ponta usa um triângulo de 8x8 pixels e confere dois pixels e uma cota mínima de cobertura; não valida regras de rasterização do hardware.

### Próximas tarefas

1. Culling e regras de winding com fonte independente.
2. Interpolação de texcoord e unidade de textura.
3. Escrever o resultado no framebuffer do GSP (formato e endereço reais) em vez do buffer de software.

## PICA200 → LCD — apresentação do buffer de cor (15/11/2026)

### Arquivos

- `crates/aceris-core/src/video/present.rs` (novo) — `present_to_framebuffer`, `minimum_framebuffer_stride`, `FramebufferWriteError`. 11 testes.
- `crates/aceris-core/src/video/lcd.rs` — apenas visibilidade: `LcdPixelFormat::bytes_per_pixel` e `expand_4/5/6` passaram a `pub(super)`.
- `crates/aceris-core/src/video.rs` — declara o módulo e reexporta os três nomes.

### Contrato

- É o inverso exato de `decode_row` do scanout: `RGBA8` grava `[A, B, G, R]`, `RGB8` grava `[B, G, R]` (sem alfa), os formatos de 16 bits são little-endian com os campos na ordem que o scanout lê. A redução para 4/5/6 bits arredonda ao mais próximo e `reduce(expand_n(x)) == x` para todos os níveis (teste exaustivo); RGB5A1 usa alfa >= 128 como bit ligado.
- O stride precisa ser não nulo, múltiplo de 8, sem o bit 31 e pelo menos uma linha. Use `minimum_framebuffer_stride` para imagem compacta. Os bytes entre linhas (quando o stride é maior) não são tocados.
- O intervalo inteiro é validado antes de escrever, então uma falha não deixa quadro pela metade.
- Convenção deste repo: linhas `width × height`, sem a rotação e sem o tiling do hardware real. O rasterizador deve ter o tamanho do painel (400x240 no topo, 320x240 embaixo) para o scanout ler a imagem inteira.

### Verificação realmente executada

Sobre o `main` `198429c`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` (1311 testes, 0 falhas), todos com saída 0 neste ambiente. O Clippy pegou um import usado só em testes na primeira rodada e foi corrigido antes do commit. O CI remoto não foi consultado.

### Limites

- Nada chama `present_to_framebuffer` ainda: falta decidir de onde vêm o endereço, o formato e o stride (registradores de render target do PICA200 ou `DisplayTransfer` do GSP) e quando apresentar dentro de `System::run_frame_with_budget`.
- O `DisplayTransfer` real converte um render target em tiles 8x8 e grava rotacionado; isso não está modelado e o layout daqui pode precisar mudar se o scanout passar a seguir o hardware.

### Próximas tarefas

1. Decodificar os registradores de render target (endereço, formato, dimensões) e fazer o draw escrever neles.
2. Ligar `present_to_framebuffer` (ou o `DisplayTransfer`) ao ciclo de frame, para a saída aparecer no frontend.
3. Alinhar o layout de scanout com o hardware (tiling e rotação) com fonte independente.


## PICA200 — render target e armazenamento tiled 8x8 (15/11/2026)

Nova etapa implementada pelo ChatGPT em arquivo novo, sem alterar os módulos de shader, draw, rasterização ou o trabalho de Thumb:

- crates/aceris-core/src/gpu/render_target.rs — decodifica GPUREG_COLORBUFFER_LOC (0x11D), GPUREG_COLORBUFFER_FORMAT (0x117), GPUREG_FRAMEBUFFER_BLOCK32 (0x11B) e GPUREG_FRAMEBUFFER_DIM (0x11E).
- PicaRenderTargetConfig reconstrói o endereço físico (valor do registrador deslocado à esquerda em 3), largura (bits 0–10) e altura (bits 12–21 mais um). Registra o bit 24 como flag de flip ainda não aplicada.
- Formatos implementados: RGBA8 (32-bit format 0), RGB5A1 (16-bit format 2), RGB565 (16-bit format 3) e RGBA4 (16-bit format 4). RGB8 não faz parte do decoder de formato do render target PICA.
- Codec implementa blocos 8x8 em ordem Morton/Z: tiles em ordem de linha; pixels em ordem de bits X/Y intercalados dentro do tile. Width/height não múltiplos de 8 são completados por pixels de padding zerados.
- write_rasterizer() valida que as dimensões do rasterizador correspondem ao target, valida o intervalo físico completo antes da escrita e devolve relatório de tiles/bytes. read_rgba8() oferece caminho inverso para inspeção/transferência futura.
- Modo de bloco 32x32 é decodificado mas recusado pelo codec; não é tratado incorretamente como 8x8. A flag de flip é exposta sem aplicação automática até confirmar a semântica.
- Seis testes novos cobrem decode de registradores, erros de dimensão/formato, bijeção Morton, round-trip RGBA8 no backing físico, quantização nos formatos 16-bit, e recusa de bloco 32x32/dimensões divergentes antes de modificar memória.
- Referências de protocolo: https://docs.mikage.app/GPU/Internal_Registers/ e https://docs.mikage.app/GSP_Shared_Memory/. A documentação descreve os registros de framebuffer, a cor tiled e DisplayTransfer para converter o render target em framebuffer linear de LCD.

### Limites não resolvidos

- O codec é uma primitive de bring-up com blocos 8x8 e ordem Morton; ele não implementa o modo 32x32, DisplayTransfer, rotação, escala, PPF assíncrono, cache flush, nem a chamada a partir do ciclo de frame.
- A flag de flip vertical do FRAMEBUFFER_DIM continua apenas exposta porque a própria descrição de referência a marca como incerta.
- A numeração de formato e o packing de canais são baseados no registro público; os testes provam round-trip interno, não compatibilidade validada em hardware real.
- Não há declaração de jogo executável ou compatibilidade PICA200 completa.

A CI deste commit precisa passar em Linux, Windows e macOS, além do build Android ARM64, antes de aprovar esta etapa.


## GSP DisplayTransfer — execução pelo GX queue (15/11/2026)

- `GSP_GX_COMMAND_DISPLAY_TRANSFER` (ID 0x03) agora é decodificado por `GspGxCommand::display_transfer` e executado pela fila GSP quando o cliente possui rendering rights.
- Os spans exatos de entrada/saída são calculados antes de resolver endereço físico. VRAM aceita o endereço físico e o alias 0x1F000000; outras regiões usam os mapeamentos DMA registrados por ControlMemory.
- Após a conversão, o dispatcher emite PPF. O `GspGxCommandQueueReport` conta `display_transfers_completed`. Sem rendering rights, a entrada é consumida mas não executada.
- Os testes de integração validam tiled 8x8→linear em VRAM, bytes de saída, relatório de conclusão e consumo sem direitos de renderização.
- Limite temporal: a cópia CPU é síncrona, com PPF imediato após conclusão; não representa latência assíncrona/ciclo-a-ciclo do transfer engine real.

### Verificação do PPF

O teste de integração `gsp_gx_display_transfer_executes_tiled_to_linear_and_reports_completion` agora registra um cliente GSP com event handle OneShot, adquire os direitos de renderização e verifica o PPF na relay queue real (`GspInterruptRelayQueue`) além do estado sinalizado do objeto kernel. A verificação cobre a ordem de conclusão → enfileiramento → sinalização; continua sendo um backend síncrono de CPU, não uma medição da latência real do hardware.


## GSP GX — RequestDMA síncrono e limitado (16/11/2026)

O ChatGPT implementou o comando GX RequestDMA (ID 0x00) na colaboração autorizada de serviços GSP, sem modificar os arquivos de CPU/Thumb nem os módulos de draw/shader de Claude:

- `GSP_GX_COMMAND_REQUEST_DMA` e `GspDmaRequest` decodificam origem (word 1), destino (word 2), tamanho em bytes (word 3) e o pedido de flush da origem (word 7).
- `System::dispatch_gsp_dma_request` resolve os dois intervalos completos por meio da rota comum de GSP (VRAM física/alias ou mapeamentos lineares registrados), valida origem e destino antes de alterar memória e usa um buffer temporário para preservar a semântica de cópia quando os intervalos se sobrepõem.
- O backend aceita tamanhos de 1 a 64 MiB. Não impõe alinhamento de endereço/tamanho, porque RequestDMA é uma exceção às restrições de alinhamento descritas para os outros comandos GX.
- A chamada só é executada quando o cliente tem rendering rights. Após a escrita de destino, despacha a interrupção `DMA` (ID 6) para o cliente que mantém os direitos. Sem direitos, o comando é consumido sem modificar memória.
- `flush_source` é exposto no decoder e em `GspGxCommandQueueReport::flush_source_requested`; o Aceris ainda não tem um modelo de cache de dados separado, portanto o pedido é observado, mas não causa um flush real. Não há temporização assíncrona, semáforo CDMA, QTM nem Break equivalente para falhas.
- Testes adicionados: decodificação de campos byte-granulares; cópia de um endereço linear legado para VRAM com span não alinhado de 13 bytes e validação da relay queue/event handle `DMA`; consumo sem direitos sem alteração do destino.

### Verificação

A implementação está publicada na branch `main` no commit `d4d76e71f11789465441e9a03476b83204b22e51`. A CI [43789709791](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43789709791) terminou com sucesso: format, build, Clippy e testes do workspace em Ubuntu/macOS/Windows, além do build Android ARM64. A verificação local com Cargo não estava disponível nesta sessão.


## Integração GSP framebuffer-info → PDC — 15/11/2026

Após GX DisplayTransfer (ID 0x03), o caminho real de fila com contexto do cliente lê as estruturas compartilhadas top/bottom, valida índice, flags, stride e formato, traduz os endereços de framebuffer por DMA/VRAM, grava o endereço no slot A/B selecionado, configura formato/stride/status do PDC e limpa o bit de dados novos. Painéis sem metadata nova alternam o seletor de próximo buffer. A integração ocorre antes da entrega de PPF; o scanout continua no ciclo de frame existente. Não declarar compatibilidade com jogos sem teste real.


## GSP GX TextureCopy — cópia raw com gaps (16/11/2026)

O ChatGPT implementou o comando GX TextureCopy (ID 0x04) na área de integração GSP já autorizada, sem modificar CPU/Thumb nem os módulos de draw/shader atribuídos ao Claude.

- GspGxCommand::texture_copy decodifica origem, destino, tamanho, parâmetros de linha/gap e flags.
- O novo gpu/texture_copy.rs realiza cópia sem conversão de pixels. Sem gap mode, copia um span contínuo; com bit 2, copia linhas e aplica gaps separados na origem e no destino. Os campos de largura/gap usam unidades de 16 bytes.
- O backend exige bit 3 para selecionar o modo TextureCopy, valida alinhamento de 8 bytes, tamanhos mínimos documentados e limite defensivo de 64 MiB.
- Todos os spans de linha são resolvidos e validados antes da primeira escrita; os bytes de origem são armazenados temporariamente para preservar cópias sobrepostas.
- O dispatcher exige rendering rights, incrementa texture_copies_completed e emite PPF somente após concluir a cópia.
- Limite explícito: a implementação exige a mesma largura de bytes copiados por linha na origem e no destino; larguras diferentes são recusadas para não inventar semântica. Cache flush e temporização assíncrona não são simulados.
- Testes adicionados cobrem decode, cópia contígua, gaps independentes, alias VRAM, overlap, seleção obrigatória do modo TextureCopy, rejeição de larguras diferentes, alinhamento e validação de todas as linhas antes de modificar o destino. Um teste de integração cobre a fila GSP, a escrita de bytes e o PPF.

Arquivos alterados nesta etapa: crates/aceris-core/src/gpu/texture_copy.rs (novo), crates/aceris-core/src/gpu/gsp_queue.rs, crates/aceris-core/src/gpu/mod.rs, crates/aceris-core/src/system.rs, docs/ROADMAP.md, docs/ARCHITECTURE.md e este registro.

A CI de main precisa confirmar Rustfmt, build, Clippy com -D warnings, cargo test --workspace em Linux/Windows/macOS e build Android ARM64 antes de considerar a etapa aprovada. Não houve verificação local: o ambiente desta sessão não possui Rust/Cargo.


## GSP GX FlushCacheRegions — implementação inicial somente de validação (histórico, 16/11/2026)

O ChatGPT implementou a decodificação e o despacho parcial do comando GX FlushCacheRegions (ID 0x05), sem alterar módulos de CPU/Thumb nem os módulos de draw/shader atribuídos ao Claude.

- `GSP_GX_COMMAND_FLUSH_CACHE_REGIONS` e `GspGxCommand::flush_cache_regions` decodificam os três pares `{address, size_bytes}` dos words 1–6.
- O dispatcher só processa o comando quando o cliente possui rendering rights. Sem o direito, interrompe a fila e grava `0xD8202A06` (GSP no-right).
- As regiões são processadas em ordem. A primeira região com tamanho zero é ignorada sem validar seu endereço e encerra o processamento das regiões posteriores. A primeira faixa não vazia que não puder ser resolvida pelo mapa GPU linear/VRAM também interrompe o comando e registra `GSP_INVALID_ADDRESS_RESULT`.
- `GspGxCommandQueueReport::cache_regions_validated` conta apenas as faixas não vazias que passaram pela validação; `cache_flush_requested` registra a solicitação.
- **Limite deliberado:** isto não executa `svcFlushProcessDataCache` nem um flush físico. O Aceris ainda não possui um modelo independente de data cache ARM11/dirty lines. Não interpretar `cache_regions_validated` como bytes efetivamente escritos de volta ou invalidados.

Testes adicionados: decode dos três pares; três faixas válidas; região de tamanho zero no meio que interrompe a terceira faixa sem validar seu endereço; interrupção na primeira região não mapeada sem processar a terceira; rejeição sem rendering rights com o resultado GSP correspondente.

Arquivos alterados: `crates/aceris-core/src/gpu/gsp_queue.rs`, `crates/aceris-core/src/gpu/mod.rs`, `crates/aceris-core/src/system.rs`, `docs/ROADMAP.md`, `docs/ARCHITECTURE.md` e este registro.

Este registro descreve apenas a etapa inicial de validação de endereços. Foi supersedido pela seção seguinte, que implementa o cache ARM11 e executa o writeback/invalidate de regiões FCRAM.


## ARM11 data cache e coerência CPU/GPU/DMA (16/11/2026)

O ChatGPT implementou uma primeira camada observável de cache write-back na memória de processo ARM11 e conectou as operações GSP de flush aos caminhos de consumo de memória.

### Alterações publicadas em `main`

- `814e954` — adiciona linhas ARM11 de 32 bytes, com bytes residentes e estado dirty, para aliases lineares físicos mapeados. CPU loads consultam as linhas e CPU stores as atualizam sem escrever imediatamente no backing físico.
- `9e1b926` — inicializa a cache também no construtor de address space 3DSX.
- `c5c6575` e `6ea696d` — expõem operações por processo e acrescentam Store (writeback mantendo a linha clean), Invalidate (descarta sem writeback) e Flush (writeback + invalidate).
- `ea38758` — despacha SVC 0x52/0x53/0x54 para InvalidateProcessDataCache, StoreProcessDataCache e FlushProcessDataCache, aceitando o pseudo-handle do processo atual `0xFFFF8001`.
- `d7914f5`, `fa5fbe8` — conectam FlushCacheRegions, RequestDMA.flush_source e ProcessCommandList.flush_buffer à manutenção do cache antes da leitura pelo agente GPU/DMA; VRAM é validada mas não passa pelo data cache ARM11.
- `bb36bd8` — expõe a criação do alias linear no processo para os testes de integração.
- `c490eee`, `af15c2b`, `51d8616`, `e37dd90` — atualizam testes GSP para mapear o alias também no address space do processo e verificar writeback real em RequestDMA, ProcessCommandList e FlushCacheRegions.
- `80e0804` — adiciona teste direto de coerência CPU/GPU: dirty não visível no backing antes do writeback, Store escreve mantendo a linha, GPU write deixa uma linha CPU stale, Invalidate refaz o load e Flush escreve e remove a linha.
- `92007bb` — otimiza a validação de intervalos para percorrer spans mapeados e linhas de 32 bytes, evitando varredura byte a byte em buffers DMA grandes; preserva validação completa antes da manutenção.
- `c43363a` — adiciona teste guest-side do SVC `FlushProcessDataCache` com o pseudo-handle `0xFFFF8001`, verificando o retorno e o writeback físico.
- `a247fc2`, `93c97fb`, `ad54df4`, `fa7a285` — atualizam comentários e formatação das integrações.
- `f8f0077` — substitui o mapa ilimitado por D-cache de 16 KiB, 4 vias, 128 conjuntos e linhas de 32 bytes, com LRU aproximado por conjunto e writeback de vítimas dirty.
- `ce653ea` — testa quatro vias, colisão de conjunto, atualização da recência e writeback da linha dirty expulsa.
- `05e34c6` — invalida as linhas residentes antes de remover uma alocação linear e liberar as páginas FCRAM, evitando que uma linha dirty antiga sobrescreva uma alocação física reutilizada.
- `c4e8ee4` — atualiza o teste de ControlMemory para diferenciar visibilidade de CPU cacheada, Store, escrita externa, Invalidate e descarte de dirty lines na liberação.
- `98b74ad`, `a37e4fe` — centralizam o pseudo-handle do processo atual (`CURRENT_PROCESS_HANDLE`) e removem o literal do handler.
- `a02aecc` — testa os SVCs Store (0x53) e Invalidate (0x52), além da rejeição do pseudo-handle errado.
- `dd40e16` — estende o teste SVC para verificar também que Invalidate descarta dirty data sem fazer writeback.
- `d7fd2d4`, `d6701fb` — atualizam arquitetura e roadmap para a geometria bounded do cache.
- `5435a7d` — adiciona teste de capacidade total: ocupa os 128 conjuntos com quatro linhas dirty cada, confirma 512 linhas residentes e verifica writeback quando a 513ª linha colide com o conjunto zero.
- `0d48007` — ajusta a fixture de capacidade para reservar FCRAM em múltiplos de página (0x5000), sem reduzir os 0x4020 bytes efetivamente exercitados.
- `10d6caa`, `12ab466` — documentam a capacidade de 512 linhas e corrigem o roadmap: `FlushCacheRegions` já executa writeback+invalidação nas faixas FCRAM mapeadas.
- `eeff3de` — adiciona regressão para os aliases linear antigo e novo apontando para o mesmo intervalo físico: a linha escrita pelo alias antigo é lida e limpa pelo novo, permanece stale após escrita externa e é atualizada após invalidação pelo outro alias.
- `aa8878e` — ajusta o teste de aliases para usar a API de leitura de backing físico existente.
- `0c8b8f6`, `45053bc` — incluem a regressão de aliases na arquitetura e no roadmap.

### Contrato atual

- Geometria do D-cache modelado: 16 KiB, 4 vias, 128 conjuntos, 32 bytes por linha e 512 slots no total. Os campos DSize do CTR ARM11 documentam 16 KiB, 4 vias e 8 words por linha; Aceris usa LRU aproximado como política de substituição do software model.
- A chave é `(backing_id, line_offset)`, não o endereço virtual: aliases para o mesmo trecho de backing convergem na mesma linha. Backings físicos FCRAM, SharedMemory (incluindo views do heap dinâmico) e cada segmento estático privado têm identidades distintas e estáveis. Em colisão de conjunto, a vítima LRU aproximada é escrita de volta pelo destino associado à linha antes de reutilizar a via.
- Aliases lineares antigo/novo, aliases virtuais de um mesmo SharedMemory e views de heap dinâmico convergem para a mesma identidade e offset de backing. As regressões cobrem também dois processos com mapas em VAs diferentes.
- O cache é write-back: CPU store deixa a linha dirty; CPU load da mesma linha observa o valor local.
- StoreProcessDataCache limpa dirty lines e mantém as linhas residentes. InvalidateProcessDataCache remove as linhas sem writeback. FlushProcessDataCache faz writeback e remove as linhas.
- Os três serviços SVC aceitam apenas `CURRENT_PROCESS_HANDLE` (`0xFFFF8001`); handles de outros processos ainda não são suportados.
- GSP RequestDMA executa flush da origem quando solicitado; ProcessCommandList executa flush do command buffer quando solicitado; FlushCacheRegions aplica manutenção aos intervalos mapeados. Stores de CPU sujos precisam ser publicados antes da leitura externa. Escritas DMA/GPU atualizam o backing comum, mas não invalidam D-cache automaticamente. Um processo consumidor que tenha uma linha clean stale precisa executar sua própria `InvalidateProcessDataCache`; uma linha dirty de outro processo permanece privada até clean/writeback + barreira do produtor. VRAM ainda é acessada diretamente sem D-cache ARM11.
- A manutenção valida o intervalo completo antes de writeback/evicção. O writeback usa o destino retido na linha: segmento estático, backing dinâmico/shared ou FCRAM físico. A LCD lê explicitamente o backing físico já comprometido por `System::scanout_lcd`; ela não faz snooping nem drena filas da CPU.
- Ao liberar uma alocação linear, a sessão invalida as linhas associadas antes do unmap e do free físico; dirty data da alocação que está sendo liberada é descartada, não escrita de volta em páginas que podem ser reutilizadas.

### Limites e validação pendente

- A cache de backing abrange segmentos estáticos do processo, heap dinâmico, Shared Memory e FCRAM físico linear. SharedMemory compartilha identidade do backing e versão de monitor exclusivo entre clones/views; FCRAM compartilha identidade do backing e versão de monitor exclusivo entre clones. A escrita por outro processo/dispositivo altera o backing, mas não atualiza/snoopa a D-cache local. Após clean/writeback + barreira do produtor, o consumidor deve invalidar explicitamente sua própria linha clean stale. Linhas dirty não são sobrescritas nem expostas automaticamente.
- A LCD consome somente o backing físico via scanout explícito. Um framebuffer ainda pendente na FIFO da CPU não aparece na LCD até o writeback ser drenado. O teste de integração exercita CPU write → FlushProcessDataCache → DMB/DSB → GSP RequestDMA → framebuffer físico → scanout LCD.
- A associatividade e capacidade declaradas (16 KiB, 4-way, 32 bytes) são representadas; LRU é apenas aproximação de replacement de hardware. Latência, prefetch, outer cache, temporização do barramento, concorrência assíncrona e snooping multicore não estão modelados.
- Invalidate de uma linha dirty descarta deliberadamente os dados não escritos, conforme a semântica de invalidação sem clean.
- Os testes de backing aliases, dois processos, heap dinâmico, segmentos estáticos e cadeia DMA/LCD estão na suíte do workspace. A CI do HEAD deve confirmar fmt, build, Clippy, testes Linux/Windows/macOS e build Android ARM64 antes de declarar a etapa aprovada.

Referências técnicas consultadas: [ARM11 MPCore Technical Reference Manual — CTR/DSize](https://documentation-service.arm.com/static/5e8e1cd9fd977155116a4a7a) para 16 KiB, quatro vias e linhas de oito words; [3dbrew SVC](https://www.3dbrew.org/wiki/SVC) para os serviços 0x52/0x53/0x54 e suas assinaturas.

## RequestDMA — regressão de coerência no destino FCRAM (16/11/2026)

O ChatGPT acrescentou um teste de integração em `system.rs` para cobrir a visibilidade de uma escrita DMA no destino FCRAM:

- A CPU escreve dados dirty na origem; `RequestDMA.flush_source` deve torná-los visíveis ao agente DMA.
- A CPU primeiro lê o destino e mantém uma linha clean residente.
- O DMA grava o destino pelo backing físico compartilhado.
- A CPU continua lendo o valor antigo até executar `InvalidateProcessDataCache`.
- Após a invalidação, a CPU relê o valor gravado pelo DMA.

Commit `2f2baa3` adiciona a regressão; `eee8918` aplica a formatação exigida pela CI. Commit `2ac2bb5` corrige o comentário de módulo em `gpu/gsp_queue.rs`, que ainda afirmava incorretamente que FlushCacheRegions apenas validava faixas.

Arquivos alterados nesta etapa: `crates/aceris-core/src/system.rs`, `crates/aceris-core/src/gpu/gsp_queue.rs` e este registro.

**Limites preservados:** nenhuma alteração foi feita nos módulos de CPU/Thumb ou draw/shader reservados ao Claude. Não há implementação de coerência snoop automática; o teste confirma a visibilidade explícita por flush/invalidação. A aprovação final depende da CI de `main` no commit `eee8918`.

## RequestDMA — coerência entre aliases FCRAM (16/11/2026)

O ChatGPT acrescentou em `crates/aceris-core/src/system.rs` a regressão `gsp_gx_request_dma_cache_coherence_uses_physical_identity_across_aliases` (commit `a61c6e3`).

- Uma única alocação FCRAM é mapeada pelos aliases linear antigo e novo no mesmo processo ARM11.
- A CPU escreve uma origem dirty pelo alias antigo e preenche uma linha clean do destino pelo alias novo.
- RequestDMA lê a origem pelo alias antigo, com `flush_source=1`, e grava o destino pelo alias antigo.
- O teste confirma o conteúdo no backing físico, confirma que as duas VAs ainda observam a linha clean stale e invalida a linha usando o alias novo.
- Depois da invalidação, leituras por ambos os aliases devem observar os bytes escritos pelo DMA. Isso comprova que a chave de cache física e a manutenção de cache convergem para a mesma linha independentemente da VA usada.

A regressão foi adicionada à suíte do workspace; aguardar o CI associado ao commit para validar Rustfmt, Clippy, testes completos e build Android ARM64. Ela cobre um interleaving determinístico CPU → DMA → CPU; não representa concorrência real entre threads do host nem snooping automático.

**Limites preservados:** nenhum módulo de CPU/Thumb ou draw/shader atribuído ao Claude foi alterado. A seleção do D-cache a partir dos atributos CP15 `TEX/C/B/S` e `PRRR/NMRR` continua pendente e deve ser integrada com revisão explícita da propriedade desses módulos antes de editar a ponte MMU/Bus.

## GSP FlushCacheRegions — regressão de invalidação observável (16/11/2026)

O ChatGPT ampliou `gsp_gx_flush_cache_regions_validates_all_three_nonzero_ranges`, renomeando-o para `gsp_gx_flush_cache_regions_cleans_and_invalidates_fcram_ranges` em `crates/aceris-core/src/system.rs` (commit `9467eb7`).

- O teste registra a quantidade de linhas D-cache antes do acesso e confirma que a escrita CPU aloca uma linha dirty.
- O comando GSP `FlushCacheRegions` percorre três faixas FCRAM não vazias; o relatório continua exigindo três regiões processadas com sucesso.
- Além de confirmar os bytes escritos de volta no backing físico, a regressão confirma que a linha D-cache residente foi removida pelo comando.
- Em seguida, uma escrita externa modifica os mesmos bytes no backing; a próxima leitura CPU, sem SVC invalidate adicional, precisa observar o novo valor. Isso verifica o efeito observável de clean+invalidate do comando.

A alteração foi publicada em `main`; o CI associado ainda precisa confirmar Rustfmt, build, Clippy, testes do workspace em Linux/Windows/macOS e build Android ARM64. A escrita externa do teste é determinística e sequencial; não representa concorrência multicore nem snooping automático.

**Limites preservados:** nenhum arquivo de CPU/Thumb ou draw/shader foi modificado. A política por atributos CP15 e as barreiras/write buffers permanecem fora desta alteração.

## GSP ProcessCommandList — invalidação da cache do buffer (16/11/2026)

O ChatGPT ampliou o teste de integração do despacho `ProcessCommandList` em `crates/aceris-core/src/system.rs`, publicado no commit `73d3bf2`.

- O teste agora registra a quantidade inicial de linhas D-cache e confirma que as duas palavras da command list deixam uma única linha dirty.
- A fila GSP envia `ProcessCommandList` com `flush_buffer=1`; a GPU deve receber os words escritos pela CPU e atualizar o registrador PICA200 `0x100`.
- Após a submissão, o teste confirma que a linha foi removida da D-cache, demonstrando que o flush faz clean+invalidate, e não apenas writeback.
- Uma escrita externa subsequente altera a primeira palavra do buffer físico; a leitura CPU seguinte precisa observar `0xCAFE_BABE` sem invalidação manual adicional.

A regressão foi adicionada, mas aguarda Rustfmt, Clippy, `cargo test --workspace`, build multiplataforma e build Android ARM64 no CI associado ao HEAD. O agente externo é modelado por uma escrita direta e sequencial ao backing físico; não representa snooping nem concorrência host-side.

**Limites preservados:** a alteração ficou em testes de integração `system.rs` e documentação. Nenhum módulo de CPU/Thumb ou draw/shader reservado ao Claude foi tocado.

A CI de `ProcessCommandList` no commit `73d3bf2` detectou somente uma divergência Rustfmt em uma chamada de escrita. O ajuste de estilo foi publicado em `5ccfe18`; a CI deste HEAD precisa confirmar a validação completa antes de marcar o teste como aprovado.

## RequestDMA — assert de clean+invalidate da origem (16/11/2026)

O teste `gsp_gx_request_dma_cache_coherence_uses_physical_identity_across_aliases` foi ampliado no commit `4890e91`:

- Após `RequestDMA` com `flush_source=1`, verifica que a linha dirty da origem foi removida da D-cache.
- Confirma que a linha clean do destino continua residente e stale através dos dois aliases até `InvalidateProcessDataCache`.
- Confirma que a invalidação pelo alias novo remove a linha física compartilhada e que as duas VAs passam a observar o destino DMA atualizado.

A nova asserção integra a suíte; a validação completa continua dependente da CI de `main`. Não há concorrência real nem snooping automático neste teste.

## CI confirmado — aliases FCRAM e FlushCacheRegions (16/11/2026)

A execução [43947785182](https://github.com/CarlosCosta07-s/Aceris/actions/runs/43947785182), para o commit `e6a000a65d26de7bc554ea1085b10216de469b0c`, terminou com sucesso em toda a matriz:

- Linux, Windows e macOS: `cargo fmt --all --check`, `cargo build --locked --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` aprovados.
- Android `arm64-v8a`: `cargo ndk -t arm64-v8a build -p aceris-android --release` aprovado.

Essa execução cobre a regressão de coerência por aliases FCRAM e o teste que comprova clean+invalidate em `FlushCacheRegions`. Ela não inclui as alterações posteriores de `ProcessCommandList` (commit `73d3bf2`) nem a asserção adicional de invalidação da origem `RequestDMA` (commit `4890e91`); estas permanecem dependentes da CI mais recente.

## GSP TextureCopy — coerência CPU/GPU no destino FCRAM (16/11/2026)

O ChatGPT acrescentou o teste de integração `gsp_gx_texture_copy_to_fcram_requires_cpu_invalidation` em `crates/aceris-core/src/system.rs` (commit `58898f1`).

- Uma alocação FCRAM é mapeada pelo alias linear antigo e pelo novo, tanto na sessão ARM11 quanto no mapa DMA/GPU.
- O teste prepara uma linha clean na D-cache por leitura CPU do destino, com backing físico inicialmente zerado.
- A fila GSP executa `TextureCopy` em modo contíguo, copiando 16 bytes de VRAM para o destino FCRAM pelo alias linear novo.
- O backing físico precisa conter os bytes copiados enquanto as leituras CPU pelos dois aliases continuam stale até `InvalidateProcessDataCache`.
- A invalidação pelo alias novo remove a linha física compartilhada; a leitura pelo alias antigo passa a observar os bytes copiados.
- O teste também confirma a interrupção PPF na relay queue e o event handle OneShot sinalizado após a conclusão.

A regressão foi publicada em `main`; validar Rustfmt, build, Clippy, testes de workspace em Linux/Windows/macOS e Android ARM64 pelo CI do commit. O modelo é sequencial e explícito: não simula snooping automático, temporização assíncrona nem concorrência host-side.

**Limites preservados:** a alteração ficou no teste de integração em `system.rs` e nesta nota. Nenhum módulo de CPU/Thumb ou draw/shader atribuído ao Claude foi modificado.

## GSP FlushCacheRegions → TextureCopy — ordenação CPU→GPU (16/11/2026)

O ChatGPT acrescentou `gsp_gx_flush_then_texture_copy_reads_dirty_cpu_fcram_data` em `crates/aceris-core/src/system.rs` (commit `77d5cad`).

- A CPU escreve 16 bytes na região FCRAM linear; o teste confirma que a linha D-cache fica dirty enquanto o backing físico ainda contém zeros.
- Uma única GX queue contém dois comandos ordenados: `FlushCacheRegions` para a faixa de origem e, imediatamente depois, `TextureCopy` de FCRAM linear para VRAM.
- O relatório precisa mostrar dois comandos consumidos, uma região limpa/invalida e uma cópia concluída, sem erro.
- O backing FCRAM precisa conter os bytes CPU originais após o flush; a VRAM de destino precisa conter a mesma sequência após TextureCopy.
- A linha D-cache deve ter sido removida, e a conclusão da cópia deve enfileirar PPF e sinalizar o event handle OneShot.

O teste cobre a ordenação explícita de manutenção antes de o mecanismo de cópia ler FCRAM. Ele não atribui um flush implícito ao TextureCopy, nem representa temporização assíncrona ou concorrência host-side. A CI de `main` precisa validar Rustfmt, build, Clippy, testes Linux/Windows/macOS e Android ARM64.

**Limites preservados:** somente testes de integração em `system.rs` e esta nota foram alterados. Nenhum módulo de CPU/Thumb ou draw/shader reservado ao Claude foi modificado.

A primeira CI do teste `gsp_gx_texture_copy_to_fcram_requires_cpu_invalidation` (commit `58898f1`) detectou que a fixture tentava registrar o mesmo intervalo físico duas vezes no mapa DMA/GPU, operação rejeitada por `OverlappingPhysicalMapping`. O teste foi corrigido em `c1730d1`: o mapa GSP mantém somente o alias linear já registrado, enquanto a sessão ARM11 expõe ambos os aliases para as leituras e para a invalidação. O novo CI precisa confirmar a correção e a suíte completa.

## PICA200 — draws renderizados no render target (Claude, 16/11/2026)

Escopo: somente o módulo `gpu` (decisão do mantenedor: item 3 da ordem de prioridades). Não toquei `cpu/`, `memory.rs` nem `system.rs`.

- `Pica200::execute_command_list_with_draws(words, on_draw)`: chama `on_draw(&Pica200, &PicaDrawRequest)` no instante de cada trigger aceito. Erro do observer interrompe a lista; as escritas anteriores permanecem e a lista não conta como submetida. Draws tratados assim **não** entram na fila de `take_draw_requests`. `execute_command_list` mantém o comportamento anterior (enfileira).
- `PicaRenderSession` / `Pica200::execute_command_list_rendering(words, memory, settings, resolve_vertex_base)`: relê `render_target_config()` por draw, carrega o buffer de cor da memória ao selecionar o alvo, renderiza com o shader/uniforms vivos e grava no flush (fim da lista, erro ou troca de alvo). Pixels desenhados antes de um erro são gravados; o erro original é o reportado.
- `SoftwareRasterizer::load_color_rgba8` (método aditivo em `rasterizer.rs`, arquivo de outro agente).
- Profundidade começa em 1.0 ao carregar um alvo e é descartada na troca; formatos estreitos perdem precisão no ciclo ler→desenhar→gravar.
- `ATTRIBBUFFERS_LOC` bruto não tem semântica confirmada: o chamador devolve a base física dos vértices.

**Para quem mantém `system.rs`:** o caminho `ProcessCommandList` pode chamar `execute_command_list_rendering` com um resolvedor que traduza o endereço pelo mapa DMA/GPU. Não alterei esse caminho.

Verificado localmente sobre `main` em 71ea686: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` com saída 0. A CI remota não foi consultada.

### Integração em `system.rs` (autorizada pelo mantenedor, 16/11/2026)

- `System::set_pica_rendering(Option<PicaRenderSettings>)` e `System::last_pica_render_report()`. Com `None` (padrão) nada muda: a lista é só interpretada e os draws vão para a fila.
- Com `Some`, `submit_pica_command_list_from_physical_memory` (usado pelo GSP `ProcessCommandList` e pelas submissões diretas) chama `execute_command_list_rendering`. A base dos vértices vem de `(GPUREG_ATTRIBBUFFERS_LOC & 0x0FFF_FFFF) << 3` (endereço físico), suposição do encoding de hardware **não validada**.
- Teste: `gsp_process_command_list_renders_a_draw_into_the_tiled_target_when_enabled` percorre fila GX → flush de cache → render → leitura do alvo tiled.
- Alteração restrita ao caminho de submissão, ao struct `System` (dois campos novos) e a testes; cache/DMA/outros caminhos intocados.

Verificado localmente sobre `main` em 3150e76: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remota não consultada.

## Coerência cache/memória — render de draws (Claude, 16/11/2026)

Ordem do mantenedor, item 1. Só testes em `system.rs` (fixture `render_fixture` compartilhada) e docs; nenhum comportamento novo. Os testes confirmam que o modelo explícito (sem snooping) vale também para o caminho de render:

- `gpu_render_into_fcram_is_not_snooped_by_a_clean_cpu_line`: a GPU escreve a memória física; a CPU vê linhas limpas antigas até `InvalidateProcessDataCache`.
- `unflushed_cpu_vertex_data_is_not_seen_by_the_gpu_but_flushed_data_is`: vértices sujos sem flush chegam zerados à GPU; com flush, o mesmo draw os vê.
- `a_dirty_cpu_line_flushed_after_the_draw_overwrites_the_gpu_pixels` e `invalidating_a_dirty_cpu_line_before_the_draw_preserves_the_gpu_pixels`: o write-back de linhas sujas depois do draw destrói a saída; invalidar antes a preserva.

**Ainda pendente neste item (não implementado):** DisplayTransfer com origem/destino FCRAM não tem teste de coerência; atributos CP15 C/B/TEX não selecionam a política do D-cache; regiões fora dos aliases lineares FCRAM, barreiras e write buffers não são modelados; cache por processo. O item 1 portanto **não está concluído**, apenas ampliado no caminho de render.

Verificado localmente sobre `main` em 95aedc1: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remota não consultada.

## Identidade de backing, Shared Memory e LCD — 16/11/2026

### Implementação publicada

- `memory.rs`: identidade monotônica para novas alocações de backing.
- `gpu/physical_memory.rs`: identidade estável por backing FCRAM compartilhado entre clones e geração por linha de 32 bytes; gravações por DMA/GPU/LCD incrementam essa geração.
- `loader/ncsd/process.rs`: `SharedMemoryBacking` compartilha identidade, gerações de linha e versões de monitor exclusivo entre clones/views. Backings do heap dinâmico passam a ter identidade desde a alocação, preservando-a quando viram Shared Memory.
- A D-cache usa `DataCacheKey { backing_id, line_offset }` e retém um alvo de writeback. Cache e FIFO passam a operar sobre segmentos estáticos, heap dinâmico, Shared Memory e FCRAM físico, sem tratar endereços virtuais de aliases como backings diferentes.
- Uma escrita publicada em SharedMemory/FCRAM atualiza a geração da linha. Ao ler, cada processo invalida uma linha limpa cuja geração ficou desatualizada; linhas dirty alheias continuam invisíveis até clean/writeback explícito. Isso é coerência por geração/invalidação sob demanda, não hardware snooping.
- Unmap de regiões Shared, linear e heap drena writebacks antes de retirar o mapa/backing.
- `Arm11ProcessSession::data_memory_barrier` expõe a drenagem DMB/DSB da sessão.
- `System::scanout_lcd` define a fronteira explícita da leitura LCD. O scanout lê somente memória física já comprometida e não limpa D-cache nem drena o write buffer da CPU.
- Testes de regressão: aliases Shared na mesma sessão; processo separado com cache limpa stale; heap dinâmico convertido em view compartilhada; segmentos estáticos; CPU → Flush/DMB/DSB → RequestDMA → framebuffer físico → LCD.

### Limites explícitos

- Gerações só invalidam cópias limpas quando uma escrita chega ao backing. Uma linha dirty de outra sessão não é snoopada e permanece privada até sua publicação.
- A execução de DMA e LCD continua síncrona/determinística neste estágio. Ainda não modela concorrência real de barramento, latências, outer cache, write combining físico ou snooping multicore.
- A matriz CI iniciada no HEAD após as alterações de formatação deve confirmar build, Clippy e testes desktop, além do build Android ARM64. Não declarar a etapa aprovada antes do resultado final da execução.

## ARM11 write buffer e barreiras CP15 — 16/11/2026

### Alterações publicadas

- d3c791d — cria uma FIFO limitada de writebacks da D-cache ARM11 (oito entradas de linha, 32 bytes), com encaminhamento das leituras CPU para as linhas pendentes, drenagem FIFO e back-pressure determinística ao preencher.
- 4a7894f — expõe drenagem e contagem de entradas pendentes na sessão ARM11.
- cba457f — separa ISB de drenagem de write buffer; o interpretador não tem pipeline/I-cache de instruções independente.
- 8ec2351 — StoreProcessDataCache e FlushProcessDataCache, quando chamados pelos SVCs, completam os writebacks antes de retornar.
- af792ec — RequestDMA com flush de origem, ProcessCommandList com flush_buffer e FlushCacheRegions drenam a FIFO antes da leitura externa pela GPU/DMA.
- b105364 — adiciona regressões de writeback pendente, forwarding, flush e visibilidade somente após a barreira.
- b13ad0b — cobre FIFO limitada, back-pressure e DMB.
- 4facf36 — atualiza testes de renderização para exigir drenagem antes da observação de saída pelos dispositivos.
- 9923d9a — inicializa a FIFO também no construtor de processos 3DSX.
- 146439d — sincroniza o roadmap com o comportamento implementado.

### Semântica atual

- Store/Flush de linhas D-cache dirty e expulsão de vítimas LRU enfileiram writebacks físicos em ordem FIFO. Store mantém a linha residente e limpa; Flush remove a linha.
- Stores de Normal memory não-cacheável, write-through e write-back/no-write-allocate em miss também entram na FIFO; stores adjacentes são coalescidos em transações de até 32 bytes, sem atravessar o limite da linha.
- Leituras da própria CPU sobre bytes pendentes recebem encaminhamento da FIFO. Leituras Device/Strongly Ordered drenam antes de acessar o backing, e stores nesses tipos continuam síncronos. Leitores externos que acessam diretamente GpuPhysicalMemory só observam o que já foi drenado.
- A fila tem capacidade de software de oito linhas. Ao ficar cheia, a inserção conclui a entrada mais antiga antes de aceitar a próxima; esse limite é uma escolha determinística do modelo e não afirma reproduzir a profundidade física do ARM11.
- CP15 MCR c7,c10,4 drena a FIFO (DSB/Drain Write Buffer); MCR c7,c10,5 executa DMB por meio do ponto de ordenação do barramento; MCR c7,c5,4 executa o ponto ISB. Como o interpretador não mantém pipeline ou I-cache separados, ISB não precisa drenar writebacks.
- MmuBus encaminha os pontos de barreira para o barramento subjacente. A sessão oferece drenagem explícita para o dispatcher GSP e testes.
- Unmap de um alias linear drena os writebacks pendentes antes de remover o mapa, evitando que um endereço FCRAM seja reutilizado enquanto ainda há uma gravação antiga na fila.
- Os SVCs de Store/Flush concluem a drenagem antes de retornar. O caminho GSP também drena depois do flush da origem DMA, do command list e de FlushCacheRegions, antes do dispositivo ler os bytes.

### Limites ainda abertos

- O write buffer agora recebe writebacks da D-cache e stores bufferable de Normal memory nos aliases lineares FCRAM; sua fila limitada e coalescência são um modelo determinístico, não uma medição da profundidade ou da temporização física do ARM11.
- A D-cache e o write buffering continuam limitados aos aliases lineares FCRAM. Memória estática, heap dinâmico e shared memory ainda não usam as mesmas tags/linhas nem um modelo completo de coerência interprocesso.
- Não há outer cache, timings de barramento, concorrência assíncrona, snooping multicore ou I-cache/pipeline independente. ISB é, portanto, um ponto de sincronização vazio no interpretador atual.
- A execução dos workflows associados às últimas alterações deve ser conferida no GitHub antes de registrar aprovação. Não considerar estas implementações como conformidade física completa.

### Próximas ações

1. Confirmar a CI completa do HEAD que contém as novas tags por backing e a regressão end-to-end antes de marcar esta etapa como aprovada.
2. Ampliar a matriz de testes para atributos TEX/C/B/S e PRRR/NMRR nas diferentes categorias de backing, mantendo explícita a diferença entre Device/Strongly Ordered e Normal.
3. Conectar IMB/I-cache e validar os efeitos de manutenção de instruções sem inventar pipeline ou temporização ainda inexistentes.
4. Projetar testes de interleavings concorrentes reais entre processos e dispositivos; não confundir gerações sob demanda com snooping automático.
5. Evoluir os modelos de outer cache, temporização e arbitração apenas quando houver comportamento arquitetural documentado e fixtures reproduzíveis.

## Cadeia guest → IPC/GSP → command lists → PICA200 → LCD contínua (Claude, 17/11/2026)

Prioridade do mantenedor: concluir essa cadeia. O teste `arm11_guest_runs_the_srv_gsp_pica_lcd_chain_in_one_continuous_run` (só `system.rs`, sem comportamento novo) executa a cadeia inteira em uma única chamada a `run_instructions`, com um programa ARM11 escrito à mão (BL/BX, loop LDR/STR, MRC do TLS):

1. `CreateEvent`, `ConnectToPort("srv:")`, `srv:RegisterClient`, `srv:GetServiceHandle`, `RegisterInterruptRelayQueue`, `MapMemoryBlock` e `TryAcquireRight`, com handles lidos pelo guest das respostas em TLS (r8 evento, r9 srv:, r10 gsp::Gpu, r11 shared memory).
2. Um carregador de tabela copia a command list (todos os registradores de desenho), os vértices, o ring GX, os metadados de framebuffer do topo e o cabeçalho de `TriggerCmdReqQueue`.
3. DMB/DSB, `TriggerCmdReqQueue` e `WaitSynchronization1` no evento; marcador em r6.

O host não cria evento, client GSP, shared memory nem direito de renderização, e não escreve registradores PICA nem de framebuffer do PDC. O teste afirma que nenhum cliente existe e que os registradores estão zerados antes do run. Mutações verificadas: DRAWARRAYS=0 e não carregar o handle GSP fazem o teste falhar (a primeira tentativa de mutação com `sed` não casou com o texto formatado e foi refeita com Python).

**Limites honestos:** (1) tabela de dados relocada pelo teste (endereços de uma sonda determinística, com asserção); (2) não há SVC guest de alocação linear, então o mapeamento linear/DMA de FCRAM é do host; (3) `PDC_CONTROL` do topo habilitado pelo host (boot do módulo GSP); (4) matriz e viewport via `set_pica_rendering`, pois não há registradores de viewport modelados, e o shader é mínimo de dois opcodes; (5) o evento já está sinalizado quando o guest espera, o bloqueio real segue coberto só por `gsp_completion_interrupt_wakes_waiting_arm11_thread`; (6) o teste de DisplayTransfer FCRAM do item de cache ainda precisa de redesenho (conjunto de trabalho de 16 KiB expulsa linhas por LRU).

**Próximos passos naturais, em ordem:** viewport/escala de profundidade por registradores e transformação feita pelo shader do guest (hoje a matriz é do host); `ControlMemory` linear guest-side; habilitação do PDC pelo HLE do GSP.

Verificado localmente sobre `main` em b49df8c: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remota não consultada.

### Itens 3 e 2 dos próximos passos (Claude, 17/11/2026)

- **PDC pelo HLE do GSP:** `apply_gsp_framebuffer_info` agora liga `PDC_CONTROL` (`PDC_CONTROL_ENABLED`, exportado de `video`) do painel que recebeu metadados de framebuffer; painéis sem framebuffer publicado ficam desligados. O teste da cadeia não escreve mais nenhum registrador do LCD e afirma topo ligado, fundo desligado.
- **`ControlMemory` linear no guest:** o teste da cadeia aloca os quatro buffers lineares com `SVC ControlMemory` (ALLOC|LINEAR, hints em `NEW_LINEAR_BASE`); o host não chama mais `map_test_linear_fcram` nesse teste. O programa guest agora é montado por código (rótulos/BL/ADD calculados), não por índices manuais. Mutação: reduzir a 3 alocações falha com "guest ControlMemory did not allocate 0x30060000".
- Restam: viewport/escala de profundidade por registradores com a transformação no shader do guest (hoje matriz/viewport vêm de `set_pica_rendering`).

Verificado localmente sobre `main` em 2388b23: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remota não consultada.


## Benchmark local de títulos — ferramenta independente (ChatGPT, 17/11/2026)

Escopo: novo exemplo isolado `crates/aceris-core/examples/benchmark.rs` e documentação `docs/PERFORMANCE_TESTING.md`. Não foram alterados `system.rs`, módulos de CPU/GPU, frontend desktop/Android nem arquivos do trabalho reservado a outro agente.

- O exemplo aceita 3DSX, NCSD (`.3ds`/`.cci`) e CIA nos caminhos já aceitos pelo carregador.
- Para imagens criptografadas, o usuário pode fornecer material explicitamente por variáveis de ambiente. O programa não embute, descobre, imprime nem grava chaves; os arquivos e chaves não entram no repositório nem no CI.
- Mede aquecimento e uma amostra configurável de chamadas `System::run_frame_with_budget`; registra throughput do host, média e percentis de tempo, instruções guest, command lists GSP, scanouts LCD, motivos de parada, PC/CPSR finais e quadros sem instruções.
- O relatório distingue throughput do host de FPS real: o orçamento é determinístico, o tempo virtual é nominal e a cadeia PICA200 ainda possui etapas incompletas. Não usar os números como prova de gameplay ou de velocidade ciclo-precisa.
- Comando documentado: `cargo run --release -p aceris-core --example benchmark -- <GAME> --warmup 30 --frames 300`.
- `README.md` aponta para a metodologia de pesquisa independente e para o guia de benchmark.

O primeiro build do exemplo foi bloqueado por rustfmt e depois Clippy `too_many_arguments`; ambas as causas foram corrigidas agrupando métricas em `BenchmarkSummary`. O workflow final do commit `8a042d37a42b6faf006ae98170ab3b51c1700684` está em execução na matriz Linux/Windows/macOS e Android ARM64; confirmar o resultado completo antes de marcar a validação multiplataforma como concluída.

**Limite atual:** a ferramenta prepara e mede a execução limitada no PC; não foi executada contra um arquivo de jogo real nesta sessão. O teste CIA manual documentado em `REAL_HOMEBREW_TESTING.md` continua dependendo do arquivo local do mantenedor. O crate Android ainda não é um aplicativo instalável, então não há benchmark de aparelho Android nesta etapa.


## Benchmark de desempenho Android ARM64 — extensão autorizada (ChatGPT, 17/11/2026)

Carlos solicitou ampliar o benchmark de títulos locais para permitir medições também no processador físico Android, além do PC. O trabalho ficou restrito à infraestrutura isolada do benchmark; não foram alterados `frontends/android/src/lib.rs`, `system.rs`, módulos de CPU/GPU, loader, nem arquivos reservados a outro agente.

### Alterações publicadas em `main`

- `7c59353` — expõe `run_cli()` no exemplo do benchmark, preservando o entrypoint do exemplo desktop.
- `3c5257c` — adiciona `frontends/android/src/bin/benchmark.rs`, que reutiliza por caminho o mesmo código-fonte do exemplo, sem duplicar a implementação.
- `9571252` — declara o binário `aceris-benchmark` no pacote Android.
- `f7cb374` — adiciona build do binário Android ARM64 e upload do artefato `aceris-android-benchmark-arm64-v8a`.
- `6d88b02` — documenta a execução física via ADB em `docs/PERFORMANCE_TESTING.md`.
- `6c7be98` — amplia a CI para compilar binários release do benchmark para cada desktop da matriz e publicar artefatos.
- `ac95475` — documenta o uso dos binários desktop baixáveis.

### Verificação observada

O workflow [44032702899](https://github.com/CarlosCosta07-s/Aceris/actions/runs/44032702899), no commit `6d88b028ac008cb3fc7a4235cd6417a97748e75f`, concluiu com sucesso o job Android, incluindo:
- build da biblioteca nativa Android;
- build `cargo ndk -t arm64-v8a build -p aceris-android --bin aceris-benchmark --release`;
- upload do artefato `aceris-android-benchmark-arm64-v8a` (artifact ID `14305328610`).

A revisão mais recente da CI que inclui também os artefatos de desktop precisa ser conferida separadamente antes de considerar a matriz completa aprovada.

### Limites explícitos

- O executável Android é um runner de linha de comando para ADB; não é APK, Activity, nem frontend gráfico.
- Ele mede o throughput do mesmo `aceris-core` no processador ARM64 do dispositivo, com as chamadas `run_frame_with_budget` limitadas por orçamento. Não mede FPS cycle-accurate nem prova gameplay.
- Não houve execução em aparelho Android físico nesta intervenção; só foi confirmada a compilação e publicação do artefato.
- Alguns firmwares podem bloquear execução direta em `/data/local/tmp`; o guia documenta essa limitação.
- Imagens de jogos e qualquer material de chave continuam locais; nada disso foi adicionado ao repositório ou CI.

Os próximos passos dependem de executar o mesmo título localmente em PC e Android, comparar relatórios e usar PC/CPSR, motivos de parada, instruções, GSP e LCD para priorizar os bloqueios reais de compatibilidade.


### Captura LCD pós-benchmark (ChatGPT, 17/11/2026)

Extensão do benchmark isolado, a pedido do mantenedor:

- O CLI aceita `--dump-lcd <DIR>` e exporta `top_left.png`, `top_right.png` e `bottom.png` após o período cronometrado.
- A gravação PNG ocorre somente depois das amostras, portanto não entra no tempo médio/p50/p95/p99.
- O mesmo código-fonte é compartilhado pelo exemplo desktop e pelo binário Android.
- Documentação PC/ADB atualizada para coletar os relatórios e as superfícies.

Arquivos alterados nesta extensão: somente `crates/aceris-core/examples/benchmark.rs` e `docs/PERFORMANCE_TESTING.md`. A CI associada aos commits de captura ainda precisa confirmar Rustfmt, Clippy, testes, build desktop e build Android ARM64 antes de declarar essa nova opção aprovada.

Limite: as capturas mostram apenas o conteúdo das superfícies que o núcleo produziu ao final do benchmark; não equivalem à apresentação ao vivo no frontend nem demonstram correção de renderização do título.


## Leitura resiliente na verificação de conteúdo CIA — 17/11/2026

- Commit funcional `91b5e5e5bbd8786142b18a78ff83387bbb44ff30` e commit de formatação `c89c01d8da2c0cd36edbe13a0672a836858eb710` alteram somente `crates/aceris-core/src/loader/cia/archive.rs`.
- A verificação SHA-256 dos conteúdos agora repete leituras que retornam `io::ErrorKind::Interrupted`, tanto no corpo do fluxo quanto na leitura final usada para detectar bytes além do tamanho declarado no TMD.
- Foram adicionadas regressões com leitor sintético que alterna interrupções e leituras curtas: uma confirma a validação correta do hash e outra confirma que um byte excedente continua sendo rejeitado mesmo quando a primeira tentativa da checagem final é interrompida.
- A semântica de tamanho exato, SHA-256, limites do leitor e tratamento dos demais erros permanece inalterada. Nenhum arquivo de CPU, GPU/PICA, memória, LCD ou sessão ARM11 foi modificado.
- O primeiro workflow do commit funcional identificou apenas diferenças de Rustfmt; elas foram corrigidas em `c89c01d`. A validação completa do commit de correção ainda está pendente.


## PICA200 — viewport guest-side e mapeamento de profundidade (17/11/2026)

Carlos autorizou a intervenção nas áreas anteriormente atribuídas a outro agente. A implementação desta etapa é nova e escrita em Rust no Aceris; referências de terceiros foram usadas apenas para estudo técnico, conforme o registro em `docs/INDEPENDENT_RESEARCH.md`.

### Comportamento ligado ao caminho de desenho

- O estado PICA decodifica os registradores guest `0x041` e `0x043` como meia largura/meia altura float24, `0x068` como origem X/Y signed de 10 bits, e preserva `0x042`/`0x044` brutos para diagnósticos e uso futuro.
- `0x04D` e `0x04E` são os coeficientes float24 de escala e offset da profundidade. O caminho software calcula `(z/w) × scale + offset`.
- `0x06D` é interpretado pelo Aceris como seleção Z/W: Z quando bit 0 está ligado; W quando está apagado. A documentação Mikage o descreve como enable de faixa de profundidade, enquanto o enum Azahar consultado chama os valores de WBuffering/ZBuffering. Registrar essa interpretação como hipótese até comparação em hardware.
- `0x04C` não é usado. As fontes consultadas o deixam sem nome ou o tratam como palavra reservada; uma interpretação inicial como controle de transformação independente foi retirada antes de consolidar o comportamento.
- `PicaRenderSession` lê a viewport e os controles de profundidade do banco PICA ativo no instante do draw. A origem PICA bottom-left/Y-up é convertida para a superfície software top-left/Y-down.
- `GeometryPipeline` preserva `z/w` para o mapeamento PICA e anexa o estado de profundidade ao vértice rasterizado. No W-buffer, o resultado da escala/offset é multiplicado pelo W interpolado (divisão pelo reciprocal-W interpolado); no Z-buffer permanece como profundidade normalizada.
- As fixtures de renderização e o teste contínuo guest→GSP→PICA→LCD programam viewport, escala, offset e modo Z/W por escritas PICA. A command list do teste de cadeia é construída e copiada pelo próprio guest; as asserções verificam o estado após a execução.

### Arquivos e commits desta etapa

- `pica200.rs`: decodificação dos registradores viewport/depth e testes; a tentativa intermediária de incluir `0x04C` foi revertida em `014715c21fc78369f75ae6636c5c96616deb5d30`.
- `geometry.rs`: transporte do mapa de profundidade para `RasterVertex`.
- `rasterizer.rs`: cálculo Z/W, validação consistente por triângulo e testes do mapeamento; commits funcionais `f01de8f`, `a36f163` e `2c24a0b`.
- `render.rs`: leitura do estado guest e configuração de viewport/profundidade no draw.
- `gpu/mod.rs`: exports dos tipos de profundidade/viewport; o export provisório de `0x04C` foi removido em `fa571e068bd656ed2e28cf3b22ecf23e2ea37952`.
- `system.rs`: fixtures host/guest programam somente os registradores conhecidos `0x04D`, `0x04E` e `0x06D`; os writes experimentais de `0x04C` foram removidos em `db389083e0f1c9719812b1914a2376509d7c7a2b`.
- `docs/INDEPENDENT_RESEARCH.md`: divergências e nível de confiança registrados.

### Limites que continuam abertos

- A matriz 4×4 usada para transformar vértices ainda é fornecida por `set_pica_rendering`; esta etapa não tornou o vertex shader ou o conjunto de uniforms guest-driven.
- O PICA clipper continua no modelo genérico de seis planos. Scissor, face culling, polygon offset/depth bias, precisão/quantização exata e todos os estados de stencil/blending não foram concluídos.
- A profundidade ainda é armazenada no alvo software em memória do host e não possui backing persistente no framebuffer físico; troca/limpeza de alvo e concorrência GPU precisam de integração adicional.
- Os inversos de viewport são preservados sem uso em derivatives/texture filtering. A interpretação Z/W do bit 0 de `0x06D` ainda precisa de validação em hardware; não foi adicionada uma hipótese sobre `0x04C`.
- A CI anterior falhou em Rustfmt, antes de build, Clippy e testes completos. Conferir o workflow associado ao HEAD depois dos ajustes e correções conceituais antes de declarar validação multiplataforma.

**Resultado:** viewport e mapeamento básico de profundidade guest-side estão ligados ao draw, com a interpretação Z/W marcada como hipótese. Isto é um avanço parcial da pipeline PICA200, não uma declaração de compatibilidade gráfica comercial.


## PICA200 — EX2 e LG2 no executor do vertex shader (17/11/2026)

Carlos autorizou a intervenção nas áreas centrais do núcleo. Esta contribuição altera somente `crates/aceris-core/src/gpu/shader_exec.rs` no código de produção; o executor permanece em Rust e sem dependências externas.

### Implementação

- Commits `c6f6fe52498fcfb4589347ce6ce5ea3d1d5840af` (implementação) e `c250811c76ec24e93e7c438bc858d50967f3f47c` (Rustfmt) alteram somente `crates/aceris-core/src/gpu/shader_exec.rs`. A primeira CI encontrou apenas uma divergência de formatação numa asserção, corrigida no commit de estilo.
- Commit de implementação `c6f6fe52498fcfb4589347ce6ce5ea3d1d5840af` adiciona os opcodes `EX2` (`0x05`) e `LG2` (`0x06`) à enumeração, à tabela de decodificação e ao formato unário.
- A entrada é o primeiro lane do SRC1 depois do swizzle e da negação do operand descriptor. O escalar `exp2` ou `log2` é replicado para os quatro lanes; a máscara do destino continua aplicada no writeback existente.
- Testes novos: `ex2_and_lg2_broadcast_the_first_swizzled_source_lane` e `ex2_and_lg2_respect_destination_masks_and_ieee_domain_results`. A regressão da tabela de opcodes confirma EX2/LG2 reconhecidos e mantém MOVA, CMP, CALL, DST e LITP rejeitados.
- A implementação usa funções IEEE `f32`; não afirma reproduzir precisão float24 ou aproximações numéricas do PICA200.

### Pesquisa e limites

- A pesquisa técnica independente está registrada na seção 11 de `docs/INDEPENDENT_RESEARCH.md`: 3dbrew documenta os resultados escalares/broadcast; a revisão Azahar `11eceda5e0e682a1f44e396bf954a1b9137097ca`, arquivo `src/video_core/shader/shader_jit_x64_compiler.cpp`, confirma os slots 0x05/0x06 e o formato unário. Nenhum código, teste, fixture, shader ou estrutura de terceiros foi incorporado.
- DST permanece desligada: a documentação 3dbrew associa 0x04 a DST, mas a tabela JIT consultada em Azahar o deixa desconhecido. A divergência está registrada e não foi resolvida por suposição. LITP permanece desligada porque seu comportamento de condition codes dependeria da frente CMP ainda não implementada.
- Roadmap atualizado para indicar executor e decodificador parciais. Ainda faltam CMP/condition codes, MOVA, a0/a1/aL, fluxo de controle, LITP/DST e validação independente em hardware.
- A workflow `44077933723` para o código após Rustfmt (`c250811c76ec24e93e7c438bc858d50967f3f47c`) foi observada parcialmente: Linux concluiu fmt, build, Clippy, testes, build do benchmark e upload com sucesso; Android concluiu os builds ARM64 e upload; macOS concluiu fmt, build, Clippy e testes, com o build do benchmark ainda em execução; Windows concluiu fmt, build e Clippy, com `cargo test --workspace` ainda em execução na última consulta. A workflow `44077950879`, disparada pela documentação no HEAD `d833ea84a288134a64d431f25449bbf94682b9da`, também seguia em execução. Não declarar matriz multiplataforma integralmente aprovada antes da conclusão dos jobs Windows/macOS e da workflow do HEAD atual.

## WaitSynchronizationN (SVC 0x25)

- Implementado em wait-any em `loader/ncsd/session.rs` e `session/kernel.rs`. O kernel registra em `handoffs` qual handle liberou cada thread, para devolver o índice em r1 sem mudar as assinaturas dos acordadores.
- Wait-all com mais de um handle retorna `KernelServiceError` (não modelado, de propósito).
- Premissas NÃO verificadas contra hardware: ABI (r0 timeout baixo, r4 timeout alto, r1 handles, r2 contagem, r3 wait_all) e `RESULT_OUT_OF_RANGE = 0xE0E0_1BFD`.
- Verificado localmente: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace` com saída 0. CI remoto não consultado.

## PICA200: remoção do estado de desenho host-side

- A matriz de transformação já vinha dos uniforms float do guest (vertex shader); agora `PicaRenderSettings` também não tem `pipeline` (matriz) nem `depth_function`. Restam `clip_depth_range` e `default_color`.
- Função e habilitação de profundidade vêm de 0x107 (`Pica200::depth_test_state`). `DepthFunction` ganhou Equal/NotEqual/Greater/GreaterEqual.
- Os guests dos testes de sistema agora escrevem 0x107 = 0x41 (teste habilitado, função less); sem isso o teste fica desabilitado.
- Pendente: ligar o render por padrão sem `set_pica_rendering`, culling (reg 0x40), scissor, máscaras de escrita, convenção real de clip em Z (NegativeOneToOne é suposição).
- Verificado localmente: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remoto não consultado.

## PICA200: scissor e culling no fragment rasterizer

- `PicaFragmentRasterState` ganhou `cull_mode` e `scissor`; `PicaFragmentRenderSession` os lê dos registradores do guest no trigger do draw. Alteração mínima em `fragment_rasterizer.rs` e `fragment_render.rs` (arquivos de outra sessão); nada além dessas bordas foi tocado.
- Convenção: `area` interna usa Y para baixo, então o enrolamento visto pelo guest (Y para cima) tem sinal oposto. KeepClockwise/KeepCounterClockwise e a codificação 1/2 são premissas não verificadas em hardware.
- Testes: decodificação dos registradores, culling nas duas orientações, scissor include/exclude particionando os pixels e origem inferior-esquerda das linhas.
- Não há teste ponta a ponta via GSP para estes registradores; o `PicaRenderSession` legado os ignora.
- Verificado localmente: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remoto não consultado.
- Commit seguinte, sem integração: `crates/aceris-core/src/input.rs` (modelo de entrada: botões, circle pad, toque), ainda não ligado a `System`, ABI Android nem desktop.

- `input.rs`: modelo comum de entrada (botões em ordem de hardware, circle pad ±156, toque 320x240, bordas press/release). Ainda não ligado a `System`, à ABI Android nem ao desktop; layout dos bits e faixas vêm de documentação pública, não verificados em hardware.

## Texturas: formatos, mipmaps e LOD por derivadas

- `texture.rs` (arquivo de outra sessão, alterado em coordenação com o pedido do mantenedor): novos formatos I4/A4/ETC1/ETC1A4; `PicaTextureUnit` ganhou `max_level`, `min_level` e `lod_bias_fixed` (campos públicos novos: quem construir a struct por literal precisa preenchê-los); `sample_lod` novo, `sample` continua no nível 0.
- `fragment_pipeline.rs`: `PicaFragmentVaryings.texture_lod` (None por padrão), `shade_triangle_sample_with_derivatives` e o helper `interpolate_triangle_varyings`. `fragment_rasterizer.rs` passou a chamar a variante com derivadas; `state.minifying` vira fallback.
- Ruído de derivada abaixo de 1e-3 em LOD conta como zero para preservar o caso 1:1.
- Todas as premissas de layout listadas no ROADMAP são não verificadas em hardware; os testes de ETC1 usam blocos construídos por mim a partir da definição pública, não texturas reais.
- Verificado localmente: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remoto não consultado.

## Blending e máscaras: quantização por escrita

- `PicaFragmentRasterizer` guarda o formato do alvo (`format`) e quantiza cada pixel escrito por `PicaColorBufferFormat::quantize` (encode+decode); canais mascarados mantêm o pixel armazenado antes da quantização. Antes, a precisão de 8 bits se acumulava entre draws em alvos 16 bits.
- Testes novos: quantização por formato, 16 logic ops, 15 fatores de blend, saturação/equações independentes e acúmulo de blends em 565. Dois deles falham sem a correção (verificado revertendo-a).
- Não implementado: `COLORBUFFER_READ`, bloco 32x32, gas/shadow. Arredondamento do blend continua em float com `round`, não validado contra hardware.
- Verificado localmente: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remoto não consultado.



## GSP DisplayTransfer — tiled→tiled e crop 8×8 (18/11/2026)

- Escopo publicado: somente `crates/aceris-core/src/gpu/transfer.rs` para código e testes. Commits: `3ce66d518cb460465769bd25121c16079877e125` (tiled→tiled + crop); `47e0c6dedc6241db89f734dae8eb6ac06cf73d9c` (fixtures e dimensões alinhadas ao mínimo documentado); `729e51082bc3a4aaffca12fb2c05cc2f543bfada` (formatação de `transfer.rs`).
- A flag bit 5 seleciona tiled→tiled; a combinação com bit 1 é recusada. A flag bit 2 é exigida quando a largura de saída é menor que a de entrada. A rotina endereça a origem pela largura de entrada e a saída pela largura de saída, validando os tamanhos de memória independentemente e concluindo o staging antes do writeback.
- Testes adicionados: `tiled_to_tiled_transfer_preserves_the_tiled_pixel_order`, `tiled_to_linear_crop_copies_the_requested_top_left_rectangle`, `horizontal_crop_requires_the_guest_crop_flag` e o teste do mínimo tiled→linear. O crop válido usa entrada 96×64 e saída 64×32.
- Fontes e limites estão registrados na seção 12 de `docs/INDEPENDENT_RESEARCH.md`. O código continua limitado a tiles 8×8. Bit 16 (blocos 32×32), bits 24–25 (box downscale), alinhamento de largura por formato, matriz de conversões que completa PPF e temporização real permanecem abertos.
- CI do commit `47e0c6dedc6241db89f734dae8eb6ac06cf73d9c`: workflow `44352488069`; Android ARM64 release concluiu com sucesso. Linux e Windows pararam em `cargo fmt --all --check`; os logs também apontam uma divergência em `crates/aceris-core/src/gpu/texture.rs:1070`, fora do arquivo deste escopo e alterado em trabalho anterior de outro agente. O commit `729e510` corrige exclusivamente o estilo de `transfer.rs`; a nova CI precisa ser inspecionada antes de declarar testes/build/Clippy aprovados. A execução macOS ainda estava em fila na última consulta.
- O mantenedor autorizou explicitamente, em 18/11/2026, alterações em qualquer arquivo/subsistema. `texture.rs:1073–1075` recebeu somente correção de Rustfmt no commit `d8e26d5`; sem mudança funcional no trabalho anterior de texturas. O ambiente desta sessão não possui `cargo`, `rustc` ou `rustfmt`, portanto a validação depende da CI remota.

## GSP DisplayTransfer — matriz PPF no dispatcher (18/11/2026)

- Commits de código: `dd3b1105750520c1fc0d69244732e45461f40fca` (matriz/tabela de testes), `459bc8c22aa4b0524b547ea7f0f963513c2885e9` (export GPU), `30a72bc84417e7dfa555a8166c05797494620bab` (dispatcher GSP condicional).
- A função pública `display_transfer_signals_ppf` representa os 25 pares documentados para tiled→linear e tiled→tiled. Linear→tiled preserva sinalização para todos os pares até que a documentação específica seja localizada.
- `dispatch_gsp_display_transfer` retorna a condição PPF separada do sucesso de cópia. O dispatcher ainda registra a transferência e atualiza os metadados LCD, porém enfileira `GspInterruptId::Ppf` somente quando a matriz indica conclusão com interrupção.
- Testes unitários: matriz 5×5 completa e preservação do comportamento linear→tiled. A próxima etapa deve adicionar um teste de integração pela GX queue que observe a ausência de evento PPF para par sem interrupção e confirme que os bytes de destino/contador de conclusão ainda mudam conforme o comportamento descrito.
- Referência: seção 13 de `docs/INDEPENDENT_RESEARCH.md`, 3dbrew GPU/External Registers (tabela PPF) e Mikage GPU/External Registers (flags de modo). Ambas são fontes comunitárias não oficiais; falta confirmação em hardware.
- O commit `d8e26d5` corrigiu somente formatação no teste HILO8 de `texture.rs`, com autorização explícita do mantenedor. A execução CI anterior para `20d4a8c` ainda mostrava o estilo antigo porque rodou sobre snapshot anterior ao commit `d8e26d5`; a workflow do HEAD mais recente deve ser consultada antes de declarar fmt/build/test/Clippy aprovados.
## Integração GSP — ausência de PPF para RGB8→RGBA8 (18/11/2026)

- Commit `ecd74977b6512500fa999d957507b77f5e2eb119` amplia o teste `gsp_gx_display_transfer_executes_tiled_to_linear_and_reports_completion` em `system.rs`.
- Depois do primeiro comando RGBA8→RGBA8 (que gera um PPF), o teste prepara uma imagem tiled RGB8 e envia um segundo DisplayTransfer RGB8→RGBA8 pela GX queue real.
- Confirma bytes convertidos corretos no destino e `display_transfers_completed == 1` para a segunda execução; confirma que a relay queue continua contendo exatamente o PPF original, sem um segundo evento.
- A CI deste HEAD precisa confirmar Rustfmt, build, Clippy e testes. Nenhum destes resultados deve ser inferido dos commits anteriores.

## GSP DisplayTransfer — box-filter tiled→tiled 2×2 (18/11/2026)

- Commits: `87115ae40676b0fba0189b6703fa2944d2e5f4f9` implementa 2×2; `7e3bec83089b810270ee3427da969e8be0bed549` adiciona guards de dimensões/alinhamento e estende a tabela PPF ao modo tiled→tiled; `f050e3e3b5181076b4cfee67db525dc15eea84ef` corrige as dimensões do teste de rejeição; `046c1b98cefca0d9116dd752fee706e1c23ae1f2` aplica Rustfmt; `a7aac9a9ef62239e1fa186d490754c2cd6951d2f` ajusta o acumulador para iteradores.
- Com bit 5, o código exige escala 2×2, largura/altura de entrada duas vezes as de saída, saída mínima 64×32 e alinhamento da linha 64 bytes em RGB8/RGBA8 ou 128 bytes nos demais formatos.
- O loop lê quatro pixels de fonte por pixel de destino, aplica flip às linhas de amostra antes da média, acumula RGBA em u16 e divide por quatro, converte para o formato de destino e grava em tiled 8×8. A política de staging continua em vigor.
- Teste positivo: `tiled_to_tiled_2x2_box_filter_generates_a_half_size_mipmap`, entrada 128×64 e saída 64×32. Cada footprint 2×2 recebe cor uniforme para testar o mapeamento sem depender do arredondamento.
- Guard tests: rejeição sem escala 2×2, dimensão de entrada/saída incompatível e alinhamento da largura RGB8. A tabela PPF 5×5 passou a verificar também bit 5.
- Evidência: seção 14 de `docs/INDEPENDENT_RESEARCH.md`; 3dbrew descreve o modo como 2×2, as restrições 64×32 e os alinhamentos por formato. Mikage descreve os bits de escala. Nenhuma dessas fontes substitui comparação em hardware físico.
- O filtro usa média inteira truncada (soma/4). O hardware pode arredondar de modo diferente; precisão e arredondamento ainda não foram comparados fisicamente. O requisito entrada = 2× saída é a interpretação adotada e coberta por teste sintético, ainda sem captura de hardware.
- A CI do commit `1f47fc156e72f8f791956ff43afa4bc8bd42a3cb` passou integralmente em Linux e Windows (fmt, build, Clippy, testes, benchmark) e em Android ARM64 (library + benchmark); macOS estava em fila na última consulta. A CI da implementação 2×2 ainda deve ser consultada após a correção de formatação `046c1b98` e o commit de estilo `a7aac9a9`. O ambiente de edição não possui cargo/rustc local.

## PICA200 — suporte modelado a blocos 32×32 (18/11/2026)

- Commits: `508529e7e291e01633a186f8da72dce9a832ccd3` (codec de render target), `ceca40574ea83a5268c28e111e84b7046e61c2cf` (DisplayTransfer), `96e852d2575670615ea0f4db787e9e3877464acc` (passagem do layout ao cálculo de tiles), `d07eb092a19ed7b88f4b90e6e29019e58ad2c4f0` (Rustfmt).
- `render_target.rs`: `block_edge()` seleciona 8 ou 32; tamanho, offset e Morton são calculados por tile edge. O round-trip 64×32 testa dois blocos 32×32 e o codec de 8×8 permanece coberto.
- `transfer.rs`: bit 16 escolhe `Tiled32x32Morton` no lado tiled. `storage_size`, `tile_count` e `pixel_offset` usam o layout selecionado; o modo bit 16 rejeita dimensões efetivas de saída que não sejam múltiplas de 32.
- O contrato de ordem interna 32×32 é inferencial: grade de tiles row-major e Morton de 5 bits por eixo. 3dbrew/Mikage documentam a seleção 8×8/32×32 e a flag bit 16, mas não dão a ordem interna. Não declarar confirmado em hardware.
- Commit `049d3baa0756aa47440047845ffc010fde8b038a` acrescenta integração GX para bloco 32×32; `8af846c944e95969de1dfd96a5c6fe6e2acce45c` troca um helper de fixture indisponível por `expected.clone()`.
- CI do commit `508529e` passou Linux com fmt/build/Clippy/testes e Android ARM64; Windows estava em testes e macOS em fila na última consulta. CI dos commits de DisplayTransfer/GSP deve ser verificada no HEAD atual antes de declarar todos os jobs aprovados. O ambiente não possui cargo/rustc local.
## GSP/GX TextureCopy — larguras de linha independentes (18/11/2026)

- Commits de implementação/testes: `9f4b0e25e43e2f5a3d4745081afdce03a3fa4331` remove a rejeição de larguras diferentes e passa a separar contagem/stride de origem e destino; `219a84658492c795360958d6633ddfe8fbc09c32` modifica a integração GSP para origem 32 bytes + gap16 e destino16 + gap16; `90d0d0ecbbd9ec08d55612db3f2dfa54bb5ad428` adiciona o caso inverso com tamanho de 200 bytes e linhas finais parciais; `aa436fb8dddf0f3a73991bd92928d4ea1f58a573` e `798502d5461bac4450456c52ea26ad6db27711aa` aplicam Rustfmt; `1d36e54061e278a483bab029b55b760d216bff04` atualiza report/`div_ceil`; `a34b4312ad48b8b05ec3df1e50e961f5cce34e53` troca os ceil dos fixtures por `usize::div_ceil` após Clippy.
- `execute_texture_copy` agora mantém stream de bytes em staging; lê linhas usando o pitch de origem e escreve linhas usando o pitch de destino. Valida todos os spans de ambas as faixas antes da primeira leitura/escrita.
- `TextureCopyReport` expõe `source_rows_transferred` e `destination_rows_transferred`, removendo o antigo campo único ambíguo `rows_transferred`.
- Seção 16 de `docs/INDEPENDENT_RESEARCH.md` documenta as fontes (3dbrew/Mikage) e a interpretação adotada. O contrato ainda precisa ser confrontado com hardware legítimo; não declarar temporização física.
- CI `44360390588` para `a34b4312` passou Linux em fmt, build, Clippy e `cargo test --workspace`; o build do benchmark estava em execução na última consulta. Android ARM64 release e benchmark passaram. Windows estava em build/Clippy e macOS na fila. Consultar os jobs atuais antes de declarar a matriz completa aprovada.
## Entrada ligada a System, Android e desktop

- `System` ganhou o campo `input` com `input()/input_mut()`. ABI Android: `aceris_system_set_buttons/_set_circle_pad/_touch/_touch_release/_input_reset` (toque em pixels da superfície com a mesma regra de layout da composição). Desktop: `WindowEvent::{Game, Pointer, FocusLost}`, `GameKey`, `KeyboardInput` e `touch_from_window_point` em `window.rs`; X11 agora seleciona KeyRelease/ButtonRelease/Button1Motion/FocusChange.
- O autorepeat do X11 manda release+press em sequência; todos os eventos de um poll são aplicados antes do quadro, então o estado final fica pressionado.
- O guest passou a ver a entrada via `hid:USER` (seção abaixo).
- Verificado localmente: fmt, clippy `-D warnings` e `cargo test --workspace` com saída 0. CI remoto não consultado; o backend X11 só foi coberto pelos testes de mapeamento (sem servidor X aqui).

## hid:USER e memória compartilhada HID (18/11/2026)

- Novo `crates/aceris-core/src/hid.rs` (`HidSharedMemory`): imagem de 0x2B0 bytes, anéis de 8 entradas para PAD (0x28) e toque (0xC8), ticks atual/anterior, índice da última entrada, bits 28–31 do circle pad (limiar ±41), ZL/ZR omitidos. Fonte: 3dbrew/libctru; sem verificação em hardware.
- `system.rs`: `srv:GetServiceHandle("hid:USER")` cria `ServiceSessionKind::Hid`; `GetIPCHandles` (0x000A0000) responde `0x000A0047, 0, 0x14000000, memória, 5 eventos`. `run_frame_with_budget` chama `update_hid(true)` antes de executar a CPU: amostra `InputState`, escreve no backing, invalida linhas da D-cache do guest e sinaliza pad0/pad1.
- Toques em arquivos de outras áreas (aditivos): `loader/ncsd/process.rs` (`SharedMemoryBacking::write_bytes`, `invalidate_shared_backing_data_cache`), `session/kernel.rs` (`ServiceSessionKind::Hid`, `insert_hid_session`), `session.rs` (`create_hid_session`, `memory_block_backing`, `system_tick`, `invalidate_shared_backing_cache`).
- Decisão de modelo: o serviço HLE age como agente coerente (SCU) e invalida as linhas stale do guest; sem isso o guest lia valores antigos, pois as D-caches por processo exigem invalidação explícita.
- Pendente: comandos hid restantes (volume, calibração), acelerômetro/giroscópio, `SetSoundVolume`, e demais serviços para rodar jogos (fs:USER, apt:U, cfg:*, am, ptm, dsp::DSP, ndm, etc.).
- Verificado localmente: fmt, clippy `-D warnings` e `cargo test --workspace`, todos com saída 0. CI remoto não consultado.

## cfg:u e apt:U (19/11/2026)

- `services.rs` guarda os dados estáticos (blocos de config, região/modelo); `system.rs` tem `dispatch_cfg_request` e `dispatch_apt_request`, e o srv agora usa `ServiceSessionKind::{Hid,Cfg,Apt}` via `create_hle_session`.
- apt: `AptService` (mutex de lock, eventos de notificação/retomada, flags initialized/enabled). Header de GetLockHandle `0x000100C2` e de Initialize `0x00020043` derivados da leitura de libctru; sem verificação em hardware.
- Pendente: ReceiveParameter/GlanceParameter (necessários para jogos que leem parâmetros de lançamento), fs:USER, dsp::DSP, ptm, am, ndm, cecd, frd.
- Verificado localmente: fmt, clippy `-D warnings` e `cargo test --workspace`. CI remoto não consultado.

## fs:USER (RomFS) (19/11/2026)

- Novo `fs.rs` (`RomFsRegion`, leitura sob demanda) e `NcsdNcchReader::into_romfs_region_reader` (loader/ncsd.rs, aditivo). `System` guarda `romfs: Option<RomFsRegion>`; recarregar um NCSD zera romfs/hid/apt.
- Sessões de arquivo usam `ServiceSessionKind::FsRomFsFile`; o arquivo único é a região RomFS inteira (IVFC + níveis), que o libctru interpreta sozinho.
- Limitação importante: RomFS criptografado (retail) não é decriptado; use dumps decriptados. Chaves NCCH/AES de retail estão fora do escopo do repositório.
- Pendente: caminhos por arquivo, SD/save/extdata (jogos que gravam progresso), `dsp::DSP`, `ptm`, `am`, ReceiveParameter do apt.
- Verificado localmente: fmt, clippy `-D warnings` e `cargo test --workspace` (ver mensagem do commit). CI remoto não consultado.

## apt parâmetros e ptm:u (19/11/2026)

- `AptService` ganhou `pending_parameter` e `cpu_time_limit`; `AptParameter` modela remetente/comando/dados. O primeiro Enable enfileira Wakeup e sinaliza `resume_event`.
- ReceiveParameter/GlanceParameter respondem com 9 palavras (0x000D0104/0x000E0104); o ponteiro de saída é lido em TLS+0x184 (descritor de buffer estático do guest). Teste manual injeta um parâmetro com dados.
- `ptm:u` (`ServiceSessionKind::Ptm`): respostas `cmd|0x80, 0, valor`; valores são padrões de desenvolvimento.
- Pendente: `dsp::DSP` (áudio) e `am`, `ndm`, `cecd`, `frd`, `gsp::Lcd`, `y2r`, `mic`; SharedFont; save data.
- Verificado localmente: fmt, clippy e `cargo test --workspace` (ver commit). CI remoto não consultado.

## dsp::DSP handshake (19/11/2026)

- `DspService` em `system.rs`: componente carregado, evento de semáforo, máscara, eventos de interrupção. Reiniciado ao carregar um NCSD.
- Fonte dos comandos: `dsp.c` do libctru (palavras de requisição e resposta lidas). Suposições minhas: LoadComponent e SetSemaphore sinalizam o evento; RecvData(0) = 1 quando carregado; pipes vazios.
- Próximo passo real de áudio: protocolo de pipes (pipe 0 estados, pipe 2 áudio), DSP RAM mapeada, conversão de endereços e mixagem de sons para uma saída de áudio no frontend. É um trabalho grande e precisa de um jogo/homebrew de referência para validar.
- Verificado localmente: fmt, clippy e `cargo test --workspace` (ver commit). CI remoto não consultado.

## Backends desktop Windows e macOS (19/11/2026)

- `frontends/desktop/src/window/win32.rs` (user32/gdi32 via `extern "system"`, `SetDIBitsToDevice`, procedimento de janela com fila thread-local) e `cocoa.rs` (runtime Objective-C via `objc_msgSend` convertido por assinatura, `nextEventMatchingMask:`, `CGImage` como conteúdo da camada). Sem subclasses nem delegates.
- `native_input.rs`: mapas VK/kVK e decodificação de lParam/roda, testados em todas as plataformas; um teste garante que os mapas Windows/macOS equivalem ao do X11.
- Verificação feita: `RUSTFLAGS="--cfg aceris_typecheck_native" cargo check -p aceris-desktop` e `cargo clippy ... -D warnings` no Linux compilam os dois módulos (nunca linkados/executados). Execução real e aparência: NÃO verificadas. O CI remoto (consultável com `gh run view`) já deu o primeiro sinal real: Windows passou em build, clippy e testes com win32.rs; o macOS falhou na primeira tentativa por `link` duplicado em cocoa.rs (corrigido em seguida).
- Riscos conhecidos a checar em máquina real: convenção de retorno de `BOOL` em `objc_msgSend`, orientação do `CALayer.contents`, posição do cursor em janelas com barra de título (macOS), DPI scaling no Windows.
- Novo `[workspace.lints.rust] unexpected_cfgs` no `Cargo.toml` raiz declara o cfg `aceris_typecheck_native`.
- Verificado localmente: fmt, clippy (com e sem a flag) e `cargo test --workspace`.

## Aplicativo Android nativo (19/11/2026)

- `frontends/android/src/app.rs` apareceu no meu checkout como arquivo não versionado e não compilava (usava `frontend::pad` inexistente e um `touch` com tipo errado); adotei e corrigi (constantes `frontend::buttons`, conversão de tipos, tolerância de ±1 na conversão de toque). Se outra sessão está trabalhando nele, alinhar com ela.
- `native_activity.rs`: callbacks de `ANativeActivity` só gravam estado sob mutex e acordam o `ALooper`; a thread do app (como no `android_native_app_glue`) anexa/desanexa a fila de entrada, processa eventos, aplica `App::step` e copia o RGBA para o buffer da janela. Referência de janela com `ANativeWindow_acquire/release`; o desenho segura o mutex para a janela não ser liberada durante o quadro.
- Verificação feita: type-check/clippy no Linux com `--cfg aceris_typecheck_native`; o CI faz `cargo ndk` para arm64-v8a. Execução em dispositivo: NÃO verificada. Riscos a checar: layout de `ANativeActivityCallbacks`, uso de `ALooper_pollOnce` com ident, comportamento de `AInputQueue_preDispatchEvent`, formato de buffer RGBA.
- Verificado localmente: fmt e clippy (com e sem a flag), testes do workspace. Ver o CI remoto do commit.

## Motor de áudio DSP HLE e saída de áudio (19/11/2026)

- `crates/aceris-core/src/dsp.rs` (novo): `Dsp` com RAM compartilhada (`SharedMemoryBacking` de 0x80000 bytes), pipe 2, 24 vozes e mixagem; `Resampler` linear usado pelos frontends. `system.rs`: `dsp::DSP` agora é despachado por ID de comando (antes por cabeçalho exato; os cabeçalhos de ReadPipe/ReadPipeIfPossible do stub antigo estavam errados — libctru usa `(cmd,3,2)` = `0x000E00C2`/`0x001000C2`). `ensure_dsp_engine` cria o motor e mapeia a RAM em `0x1FF00000` no primeiro uso (LoadComponent, conversão de endereço ou pipe).
- Fluxo modelado (de `ndsp.c` do libctru): RegisterInterruptEvents(PIPE=2, canal 2) → LoadComponent → pipe 2 recebe 0 → resposta u16 15 + 15 endereços DSP → o guest converte cada endereço e `endereço|0x10000` (dois bancos) → a cada quadro grava a configuração no banco `frameId&1`, depois o contador de quadros. O DSP usa o banco com contador mais novo (regra com wrap em 0xFFFF), aplica os bits dirty (e os zera), mistura e escreve status/`final_samples` no mesmo banco. Não escrevo contadores de volta (a leitura do libctru é do próprio contador do guest).
- `run_frame_with_budget`: enquanto o DSP roda (pipe 2 iniciado), o orçamento de instruções e o tempo virtual são divididos em 3–4 fatias por quadro de vídeo; após cada fatia roda `run_dsp_frame` (flush do cache do guest nos dois bancos, mixagem, invalidação do cache do guest sobre a RAM e sinalização do evento de pipe). Sem DSP ativo o comportamento é idêntico ao anterior (uma fatia). Período do quadro DSP: 4.888.706 ns.
- Interpretações minhas, não verificadas: ganho usa só `gain[0][0..2]` (mix 0, esquerda/direita); ADPCM com predictor de 3 bits e coeficientes 11 bits fracionários; `enable` desliga a voz no mesmo quadro em que a fila esvazia; `partial_embedded_buffer_dirty`, filtros e efeitos ignorados; `RecvData(0)` continua retornando 1 após LoadComponent (o `ndspFinalize` depende disso).
- Saída: `frontends/desktop/src/audio.rs` + `audio/alsa.rs` (libasound por dlopen, `snd_pcm_set_params` 48 kHz estéreo, não bloqueante, 100 ms) e `audio/winmm.rs` (waveOut, 8 buffers de 1024 quadros); `frontends/android/src/aaudio.rs` (libaaudio por dlopen, API 26+) alimentado por `App::take_audio`. `emulate()` abre o dispositivo ao entrar no jogo e alimenta após cada quadro. macOS: `audio/coreaudio.rs` (AudioToolbox/AudioQueue, 8 buffers de 1024 quadros, callback só marca o buffer como livre); escrito sem Mac, verificado só por type-check e pelo CI.
- Testes: 16 no `dsp` (layout, pipe, mixagem PCM8/PCM16/ADPCM, taxa, loop, volume/clipping, reset, bancos) e `dsp_ndsp_style_startup_maps_ram_and_mixes_a_guest_voice` (IPC completo guiado pelo guest, escrita pelo cache do guest, leitura do mix de volta pelo guest); 2 no `Resampler`.
- NÃO verificado: nenhum som foi ouvido; nenhum jogo ou homebrew real inicializou o `ndsp` contra este motor; offsets e semântica podem divergir do hardware. Próximo passo útil: homebrew de referência que toque um tom via libctru, e ouvir cada sink em máquina real.

## Save data/SDMC no fs:USER, serviços mínimos e diagnóstico (20/11/2026)

- `crates/aceris-core/src/storage.rs` (novo): `Storage` mapeia cada família (`savedata`, `extdata`, `systemsavedata`, `sdmc`) para um subdiretório da raiz escolhida pelo frontend; caminhos do guest só aceitam componentes normais (sem `..`, `\` ou NUL). `system.rs`: `FsState` guarda handles de arquivo (`HashMap<sessão, (File, kind)>`), diretórios e arquivos; `ServiceSessionKind` ganhou `FsHostFile`, `FsDirectory` e `Stub(u8)`. Caminhos vêm do buffer estático da requisição (tipos 1 vazio, 3 ASCII, 4 UTF-16); caminho binário não é aceito.
- Comandos e layouts (IDs do 3dbrew, de memória do que li; NÃO verificados): ReadDirectory preenche entradas de 0x228 bytes (nome UTF-16 em 0, is_directory em 0x21B, tamanho u64 em 0x220; nome 8.3 não preenchido). Códigos: 0xC8804478 não encontrado, 0xC82044BE já existe, 0xC8E04702 caminho inválido, 0xC82044B1 diretório não vazio, 0xC860478A falha de escrita — os três últimos são aproximações minhas.
- Sem `set_storage_root`, OpenArchive de save/SDMC devolve não encontrado (como console sem esses arquivos). Jogos que formatam o save quando ele não existe chamam FormatSaveData, que só cria o diretório.
- `System::unhandled_requests()`: registra nomes de serviço desconhecidos (`GetServiceHandle` agora devolve 0xD8E06406) e comandos que terminaram em erro "inválido". Serve para guiar a próxima implementação com um jogo real; o desktop imprime a lista ao sair do jogo.
- Stubs: `gsp::Lcd` e `ndm:u` aceitam qualquer comando; `ac:u` (SetClientVersion, GetWifiStatus = desconectado) e `frd:u` (HasLoggedIn/IsOnline = 0) respondem só o mínimo; demais comandos devolvem 0xD900182F e são registrados.
- Testes: 4 em `storage`, `fs_user_save_data_files_round_trip_through_host_storage` (IPC completo) e `stub_services_answer_and_unknown_requests_are_recorded`.
- Não verificado: nenhum jogo real usou essas respostas.


## Primeiro homebrew real: Cookie Clicker (Lua Player Plus, CIA) (21/11/2026)

- Arquivo fornecido pelo usuário: `Cookie_Clicker_port_for_3DS ... v0.0.0_W.standard.cia` (3 MB, TID 0004000000F23F00). Conteúdo único NCCH **sem criptografia** (flags de no-crypto), então `load_cia_no_crypto_file` serve; nenhuma chave é necessária. `.code` comprimido (BLZ).
- Correções feitas por causa dele: (1) `code_file_layout` comparava `data + .bss` com a região física do segmento de dados; o .bss não faz parte da região em arquivo, então agora só os bytes de dados são limitados (antes: "invalid ARM11 code layout"). (2) Página de configuração (0x1FF80000) e página compartilhada (0x1FF81000) mapeadas somente-leitura em todo processo (`map_kernel_shared_pages` em `system.rs`): versão do kernel, ENVINFO retail, APP_MEMTYPE 2 (64 MiB), tamanhos de região 0x04000000/0x02C00000/0x01400000 (offsets 0x40/0x44/0x48 do 3dbrew, valores escolhidos por mim, NÃO verificados em hardware; a página compartilhada está zerada, relógio não emulado). (3) Desktop: `--frames <N>` (modo sem janela roda N quadros extras e imprime pendências) e `ACERIS_FRAME_BUDGET` (instruções por quadro; o padrão de 100 000 é lento demais para esse jogo, ~4 000 000 aproxima o ARM11 real).
- Resultado observado (headless, 300 quadros, orçamento 4 000 000, ~100 s de CPU host): o runtime Lua Player Plus inicia, lê o RomFS, publica framebuffers e **desenha sua própria tela de erro**: `[string "?"]:155: cannot initialize Sound module`. O jogo em si não rodou ainda.
- Pendências que o relatório mostrou: serviço `csnd:SND` (causa do erro acima), `am:u`, `cfg:i`, `cfg:s`, `http:C`, `ir:rst`, `ssl:C`; comandos APT 0x004B00C2 e 0x01020000; FS 0x080D0144. Próximo passo: implementar `csnd:SND` (Initialize/ExecuteCommands/Acquire-ReleaseSoundChannels etc.) e repetir.
- Velocidade: ~3 s de CPU host por 100 quadros com orçamento de 4M; precisa de otimização antes de ser jogável em tempo real.

### Continuação: csnd:SND e alias do GSP (21/11/2026)

- `csnd:SND` (`ServiceSessionKind::Csnd`, `dispatch_csnd_request`): Initialize cria bloco compartilhado (tamanho arredondado a páginas) + mutex e responde `[0x00010042, 0, 0x04000000, mutex, memória]`; Shutdown, ReleaseSoundChannels, cache ops e Reset só confirmam; ExecuteCommands grava 1 em `bloco+offset+4` (palavra "terminado", interpretação minha); AcquireSoundChannels devolve máscara `0xFFFFFF00`; Acquire/ReleaseCapUnit contam unidades. IDs do 3dbrew de memória, NÃO verificados. O bloco de comandos não é interpretado e nenhum som sai por este serviço. Sem teste unitário próprio ainda (só exercitado pelo CIA).
- Bug real encontrado: o HLE do GSP mapeava seu alias do bloco compartilhado em 0x10002000, dentro da janela mapeável do guest; o `MapMemoryBlock` do csnd escolhido pelo libctru (0x10002000) falhava com 0xE0E01BF5. `GSP_SHARED_MEMORY_BASE` agora é 0x1FFD0000 (fora da janela 0x10000000–0x14000000 e acima das 64 páginas de TLS).
- `ACERIS_TRACE_SVC=1` agora também imprime `-> svc N r0=.. r1=..` (resultado) após cada SVC.
- Estado no Cookie Clicker: `csndInit` do main funciona (Initialize/Acquire OK). O erro `cannot initialize Sound module` continua: vem de `Sound.init` (0x10f7b0 no guest), que cai no caminho da função 0x1e8848 (init de áudio própria do LPP: cfgu, dsp via 0x1ebd88 com `dsp::DSP`, etc.). Nenhum pedido a `dsp::DSP` aparece no trace, então a falha ocorre antes dele, sem IPC visível. Próximo passo: rastrear PCs/retornos de 0x1e8848 (um trace de PC opcional no emulador ajudaria) para ver qual verificação retorna erro. Para desmontar: o `.code` foi descomprimido (BLZ) e aberto com `llvm-objdump --triple=armv6` sobre um ELF criado com `llvm-objcopy -I binary`; endereços de arquivo = VMA − 0x100000.


## Atualização documental consolidada — 22/11/2026 (ChatGPT)

Esta entrada foi acrescentada por solicitação explícita do mantenedor para alinhar a documentação pública ao HEAD observado. O conteúdo histórico e as entradas de outros agentes foram preservados; nenhum código-fonte foi alterado nesta atualização.

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.

### Fatos novos a refletir em futuras transferências

- Commit bf71c973: CIA homebrew real Lua Player Plus Cookie Clicker carrega, executa e chega à tela de erro produzida pelo jogo; correção de validação de .bss, mapeamento das páginas de configuração/compartilhada e opções --frames/ACERIS_FRAME_BUDGET.
- Commit 0dc68f9a: HLE inicial csnd:SND; csndInit passa, mas Sound.init ainda falha dentro do LPP. IDs/protocolo CSND continuam não verificados em hardware. O alias HLE de memória GSP foi movido para fora da janela mapeável do guest; trace opcional registra retorno de SVC.
- O último workflow observado é CI #2244 / run 44968347424, com jobs Linux, Windows, macOS e Android ARM64 concluídos com sucesso.
- O teste Cookie Clicker não é gameplay, não prova áudio e não é jogável em tempo real; 300 quadros com orçamento 4M levaram aproximadamente 100 s de CPU host.
- *(Registro histórico incorreto em 22/11/2026: a regra já havia sido flexibilizada em 17/11/2026. A consulta a código open source é permitida nos limites de `docs/REGRAS_PARA_IA.md`, seção 5.)*
- NativeActivity Android e backends desktop Windows/macOS existem; compilação CI não prova execução em aparelho nem funcionamento de áudio/entrada.

### Próxima investigação recomendada

Rastrear PCs/retornos guest em Sound.init (rotina citada no handoff como 0x1e8848) para identificar qual condição retorna erro antes de qualquer IPC DSP. Não adicionar novos stubs sem trace que demonstre a necessidade. Preservar o isolamento de arquivos de cada agente e trabalhar exclusivamente em main.

### Correção do diagnóstico do Sound.init (22/11/2026, Claude)

- CORREÇÃO do diagnóstico acima: o erro `cannot initialize Sound module` e a hipótese sobre `0x1E8848` estavam errados (a tela de erro era de uma execução anterior ao csnd). A causa real era o Shutdown do csnd não zerar o estado: o `main` do LPP faz `csndInit` + `csndExit` só para sondar, e o `Sound.init` do Lua (linha 155 do script) refaz `csndInit`; o segundo Initialize devolvia handles antigos já fechados (0xD8E007F7) e depois o jogo gravava em um bloco de comandos nulo (`0x1EB7A8`). Agora Shutdown reinicia `CsndService`.
- Resultado: o Cookie Clicker roda 300+ quadros sem erro, enviando listas GPU e SetBufferSwap; as telas capturadas ainda estão pretas (o jogo provavelmente ainda carrega imagens/OGG). Não consegui rodar além de ~550 quadros em 10 minutos de CPU: **a velocidade do emulador é o gargalo agora** (~2 quadros/s com orçamento de 4M instruções).
- Ferramentas de depuração novas (variáveis de ambiente do desktop/núcleo): `ACERIS_TRACE_SVC=1` (SVCs e resultados), `ACERIS_WATCH_PC=<lo>-<hi>` (hex; loga cada instrução buscada na faixa, com lr/r0-r3), `ACERIS_STEP_AFTER=<quadro>` (1 instrução por quadro depois dele), `ACERIS_FRAME_BUDGET=<n>`.
- Próximo passo: perfilar o interpretador ARM11 e a rasterização (qual domina?) e só então voltar a validar o jogo; depois `am:u`/`cfg:i`/`cfg:s` e os comandos APT 0x004B00C2/0x01020000, FS 0x080D0144.

## PICA200 — armazenamento FIFO das LUTs de textura procedural (22/11/2026)

- `Pica200` agora modela os registradores `GPUREG_TEXUNIT3_PROCTEX0..5` (constantes públicas) e o seletor `GPUREG_PROCTEX_LUT` (`0x0AF`) com seus oito aliases FIFO `0x0B0..0x0B7`.
- As cinco tabelas documentadas são armazenadas separadamente: ruído (128 entradas), mapeamento RGB (128), mapeamento alpha (128), cor (256) e diferença de cor (256). As escritas preservam a palavra de 32 bits, avançam o índice e fazem wrap no comprimento da tabela; seletores reservados não alteram a memória LUT. `Pica200::reset` limpa as tabelas.
- `procedural_texture_lut_entry` mantém acesso às palavras brutas; `procedural_texture_curve_entry`, `procedural_texture_color_entry` e `procedural_texture_color_difference_entry` decodificam os formatos documentados para uso futuro pelo sampler.
- **Limite funcional mantido de propósito:** `PicaFragmentPipeline::from_pica` continua recusando a unidade 3 ativa. Isso evita transformar uma captura correta de registradores em texels inventados. A geração completa ainda requer ruído determinístico, repetição, G(u,v), F(g), leitura/interpolação das LUTs de mapeamento e cor, mipmap e filtragem. A documentação pública descreve a arquitetura em quatro etapas, mas declara que detalhes do ruído e do funcionamento interno são pouco conhecidos.
- Testes adicionados cobrem upload por command list, roteamento das cinco tabelas, wrap em 128/256, seletores reservados, limites de leitura e reset. São testes sintéticos do modelo de registradores, não comparação com console físico. O ambiente desta sessão não dispõe de `cargo`/`rustc`; a CI do commit precisa ser consultada antes de declarar build, Clippy ou testes aprovados.

### Decodificação dos parâmetros e entradas das LUTs procedurais (22/11/2026)

- `Pica200::procedural_texture_config()` agora fornece um snapshot tipado de PROCTEX0..5: modos de clamp, seletores RGB/alpha, separação de alpha, controle de ruído, shifts, amplitudes/fases/frequências brutas, filtro de minificação, min/max LOD, largura, bias e offsets de mipmap.
- Campos com representação float/fixed não verificada continuam em inteiros brutos; enums preservam valores reservados, sem remapear silenciosamente para um modo válido.
- `procedural_texture_curve_entry` interpreta value fixed0.0.12 e difference signed-12 two's-complement para ruído e LUTs de mapeamento. `procedural_texture_color_entry` e `procedural_texture_color_difference_entry` decodificam RGBA8 e quatro diferenças signed-8.
- Testes sintéticos cobrem o snapshot dos seis registradores e os formatos de LUT. O pipeline continua recusando a unidade 3 ativa até que a ordem matemática e a amostragem sejam verificadas; ruído, no particular, ainda não tem especificação pública suficiente.

### Interpolação documentada das LUTs procedurais (22/11/2026)

- `PicaProceduralTextureCurveEntry::sample_linear` calcula `value / 4096 + (signed_difference / 2048) * fraction`, usando o mapeamento two's-complement documentado para os campos de diferença de 12 bits. Frações fora de [0, 1] e NaN são rejeitadas.
- `Pica200::interpolate_procedural_texture_color` aplica o dobro do delta signed-8 (a tabela armazena metade da diferença entre a cor atual e a próxima) e devolve canais em ponto flutuante no domínio de bytes, sem adivinhar arredondamento ou saturação final.
- Testes cobrem fração 0, 0.5 e 1, diferenças negativas e parâmetros inválidos. Isso implementa apenas a interpolação de uma entrada já selecionada; ainda falta confirmar como coordenadas e funções escolhem os segmentos e como a saída alimenta a cor final.

### Avaliação isolada das funções G(u,v) documentadas (22/11/2026)

- `PicaProceduralTextureMappingFunction::evaluate_raw(u, v)` agora calcula os nove seletores com fórmula pública explícita: U, U², V, V², (U+V)/2, (U²+V²)/2, sqrt(U²+V²), min(U,V) e max(U,V). A função recebe coordenadas como foram fornecidas e não aplica ruído, wrapping, shifts, normalização, F(g) nem saturação.
- `Rmax` e valores reservados retornam `None`, pois o nome aparece no mapa de registradores mas a equação não foi confirmada. Entradas não finitas também são recusadas.
- Os testes verificam os nove resultados com U=0.25 e V=0.5, além de Rmax, seletores reservados, NaN e infinito. Isso é um helper matemático independente e não ativa Texture Unit 3 na pipeline de fragmentos.

### Helper isolado para F(g) procedural (22/11/2026)

- `PicaProceduralTextureF` expõe as duas opções publicamente descritas: `Identity` e `Triangle`.
- `Triangle` usa a interpretação convencional de uma única onda triangular no domínio [0,1], `1 - abs(2*g - 1)`. A documentação de 3dbrew confirma a existência da opção triangular e a matéria CEDEC/4Gamer explica que ela faz a cor percorrer a LUT e voltar; nenhuma das fontes consultadas publica a expressão exata. Portanto, o helper está isolado e explicitamente marcado como hipótese matemática, ainda não conectado ao pipeline nem validado em console.
- `evaluate` rejeita entradas não finitas ou fora de [0,1] em vez de aplicar clamp silencioso. Testes cobrem os pontos 0, 0.25, 0.5, 0.75 e 1, além de NaN/infinito e valores fora do domínio.


### Robustez numérica adicional do helper G(u,v) (22/11/2026)

- PicaProceduralTextureMappingFunction::evaluate_raw agora verifica também o resultado da equação. Coordenadas finitas podem gerar infinito por overflow em U², média, média dos quadrados ou distância radial; esses resultados passam a retornar None em vez de contaminar etapas posteriores.
- Testes cobrem overflow em quatro funções e confirmam que um resultado finito fora de [0,1] continua sendo devolvido sem clamp, preservando o contrato raw.
- Nenhuma fórmula de wrapping, Pulse, Rmax, ruído ou seleção de índice foi inferida nesta alteração. A unidade procedural permanece recusada na pipeline até haver especificação suficiente para produzir texels confiáveis.

### Roteamento de coordenadas da unidade procedural (22/11/2026)

- PicaProceduralTextureConfig agora inclui coordinate_source, decodificado de GPUREG_TEXUNIT_CONFIG bits 8–9: 0 = Texture0, 1 = Texture1, 2 = Texture2. O valor 3 é preservado como Reserved(3), sem fallback.
- PicaProceduralTextureCoordinateSource é exportado pelo módulo GPU. Testes cobrem os três seletores válidos e o seletor reservado.
- Esse campo apenas registra qual par de coordenadas guest deve alimentar a unidade 3. O renderizador ainda rejeita a unidade procedural ativa; não há ruído, repeat, seleção das LUTs, mipmapping ou amostragem conectados.

### Resolução do par de coordenadas da unidade procedural (22/11/2026)

- `PicaProceduralTextureConfig::select_coordinates` resolve `coordinate_source` para o par U/V correspondente entre as três saídas de coordenadas interpoladas. Os pares são devolvidos sem alteração e seletores reservados retornam `None`.
- Testes usam três pares deliberadamente diferentes, incluindo valores fora de [0,1], para comprovar que esse estágio apenas seleciona a origem e não aplica clamp, wrapping, shift ou normalização.
- Isso prepara a entrada da unidade procedural, mas ainda não executa transformação de coordenadas, ruído, G/F, LUT, mipmap, filtragem ou amostragem, nem muda o comportamento da pipeline de fragmentos.

### Cobertura dos seletores G em PROCTEX0 (22/11/2026)

- Um teste de regressão percorre todos os 16 valores possíveis dos nibbles de função RGB e Alpha, cobrindo as nove fórmulas nomeadas, Rmax e seletores reservados.
- Os seletores RGB e Alpha são exercitados com valores diferentes na mesma escrita, e o bit 14 (separar alpha) alterna independentemente. Isso valida somente a decodificação dos campos; não determina como o hardware consome o flag nem executa as LUTs.
- A visão geral pública da geração procedural fala em seis funções G, enquanto a tabela de registradores lista seletores de 0 a 9 (nove expressões e Rmax). Mantemos os seletores explicitamente enumerados pelo mapa e Rmax sem fórmula, sem tentar reconciliar essa diferença com suposições.
- Nenhuma mudança foi feita na pipeline: Texture Unit 3 continua recusada quando ativa.
## Serviços pedidos pelo Cookie Clicker: am:u, cfg:i/s, APT e FS (22/11/2026, Claude)

- Fontes: somente 3dbrew/enum público do libctru recordados de memória + o que o próprio jogo envia (trace); nenhum código de outro emulador foi consultado. NADA disto foi verificado em hardware.
- `cfg:i` e `cfg:s` agora abrem sessão e usam o mesmo conjunto de comandos do `cfg:u` (as variantes reais têm mais comandos; os extras continuam não modelados).
- `am:u` abre sessão (`StubKind::Am`); nenhum comando de gerenciamento de títulos é modelado e qualquer um é registrado em `unhandled_requests()` (`am:u command 0x…`).
- APT `0x01020000` (CheckNew3DS) responde "não é New 3DS" (coerente com a página de configuração Old 3DS). APT `0x004B00C2` (AppletUtility) responde sucesso sem tocar no buffer de saída: os utility ids (o jogo usa 7 e 4) não estão documentados o bastante para modelar; se algum jogo depender da saída, isto precisará mudar.
- FS `0x080D0144` (ControlArchive): ação 0 (commit) só confirma, pois arquivos do host são gravados na hora; ação 1 (timestamp) preenche o buffer de saída com zeros (timestamp real não modelado); ação 2 e demais confirmam. O handle do arquivo não é validado.
- Testes: `cfg_variants_and_am_open_and_unmodelled_am_commands_are_recorded` e `apt_answers_check_new_3ds_and_applet_utility_and_fs_control_archive_writes_timestamp`.
- O relatório do Cookie Clicker agora só lista `http:C`, `ir:rst` e `ssl:C` (aparentemente só sondados por `httpcInit`/`irrstInit`/`sslcInit`; sem comandos pedidos até 20 quadros).

## Desempenho: caminhos rápidos do cache de dados ARM11 (22/11/2026, Claude)

- Perfil por amostragem (gdb) do Cookie Clicker mostrou ~80% do tempo no modelo de D-cache, lendo byte a byte (`read_cached_byte`, `touch_data_cache_line`, clones de `DataCacheTarget`). Mudanças em `loader/ncsd/process.rs`, sem alterar a semântica do modelo:
  - `touch_data_cache_line` não reordena quando a linha já é a mais recente;
  - `cache_key_for` calcula a chave sem clonar o alvo; `try_read_cached_hit` (leitura de 1/2/4 bytes dentro de uma linha residente) e `try_write_cached_hit` (política WriteBack) evitam o caminho completo; faltas, acessos que cruzam linha e outras políticas continuam no caminho antigo;
  - `Bus::read16`/`read32` do espaço de endereços agora usam o caminho rápido em vez de 2/4 chamadas `read8`.
- Medição (60 quadros, `ACERIS_FRAME_BUDGET=4000000`, mesmo PC final 0x00175008 antes e depois): 69 s → 19,6 s (≈3,5x). Isto continua longe de tempo real (~3 quadros/s).
- Teste novo: `arm11_cached_hit_fast_paths_match_the_byte_path_and_keep_lru_order`. A suíte de coerência de cache existente continua passando.
- Próximos candidatos: `locate()` (varre segmentos/regiões a cada acesso), `BTreeMap` do cache, busca de instrução, e só depois a rasterização.

## ARM11 MPCore: timer privado, distribuidor GIC e CPU interface (23/11/2026, ChatGPT)

- **Implementação original em Rust, somente em `main`:** novo `crates/aceris-core/src/cpu/mpcore_timer.rs` com os registradores Load/Counter/Control/Interrupt Status, prescaler de 8 bits, one-shot, auto-reload, IRQ enable, status sticky e W1C; o relógio é alimentado pelo tempo virtual da sessão. Timer em `0x17E00600`, alias CPU0 em `0x17E00700`.
- Novo `crates/aceris-core/src/cpu/mpcore_gic.rs`: modelo de um único CPU da interface e do distribuidor MPCore. Mapeamento em `0x17E00100` (interface atual), `0x17E00200` (alias CPU0) e `0x17E01000` (distribuidor). Cobre 128 IDs, enable/disable, pending set/clear, active set/clear, priority, target, ICFGR, PMR/BPR, IAR/EOI, RPR/HPPIR e spurious ID.
- `Arm11CodeAddressSpace` mapeia as janelas MMIO e avança o timer; o PPI 29 é ligado ao distribuidor. `Arm11ProcessSession` combina o output CPU0 do GIC com a linha IRQ externa, preservando as duas fontes separadas. A leitura de IAR move o IRQ de pending para active e o EOI encerra o estado ativo.
- Testes sintéticos adicionados para timer, prescaler, fractional time, auto-reload, W1C, MMIO alinhado/word-only, alias CPU0, prioridade/PMR, trigger edge/level, pending/active, IAR/EOI, byte lanes e os limites de 128 IDs.
- **Limitações deliberadas:** só o timer PPI29 está ligado ao GIC; watchdog/PPI30, SGI e afinidade multi-core, entrega de SPI por dispositivos, FIQ distribuído, clock sincronizado por ciclos de instrução, preempção/quantum e validação em hardware continuam pendentes. GSP (PSC0/PSC1/PDC0/PDC1/PPF/P3D) continua no caminho de relay queue + event handle + `BindInterrupt` HLE. As linhas GSP são documentadas como level-sensitive e ainda não modelamos a limpeza de cada fonte após acknowledge; não as inserir no GIC antes de implementar essa semântica.
- O timer usa provisoriamente 134.055.928 Hz (metade do clock Old 3DS ARM11 nominal 268.111.856 Hz); esta frequência efetiva do timer deve ser confirmada por leitura direta da seção de timer do ARM11 MPCore TRM ou por medição independente em console.
- Referências independentes usadas: ARM11 MPCore TRM DDI0360 r2p0 (`https://support.arm.com/documentation/ddi0360/f/mpcore-distributed-interrupt-controller/interrupt-distributor`); 3dbrew ARM11 Interrupts (`https://3dbrew.org/wiki/ARM11_Interrupts` — IDs privados 0x00–0x1F, timer PPI29, watchdog PPI30, SPIs GSP 0x28–0x2D e respectivas configurações); 3dbrew Memory Layout (`https://3dbrew.org/wiki/Memory_layout` — região MPCore 0x17E00000/0x2000); Azahar usado somente como referência pública de compatibilidade observável, sem consultar código interno.
- **CI e correções:** a primeira CI após ligar o distribuidor encontrou um helper de readback ICFGR removido acidentalmente; `pack_trigger_config` foi restaurado em `aa2d2551`. Na execução 45274267849 (HEAD parcial `f2dc0f3f`), Android ARM64 passou; Windows e Linux falharam apenas no `cargo fmt --check`, cujos ajustes foram aplicados em `21d042b3`, `6dd82d58` e `70582af7`. O próximo workflow para o HEAD posterior ainda precisa concluir antes de declarar build, Clippy e testes aprovados.
- Não foi possível executar `cargo`, `rustc` ou `rustfmt` localmente nesta sessão; nenhum resultado local de build/teste foi declarado.


## GIC ARM11 — entradas físicas partilhadas e estado active+pending (23/11/2026, ChatGPT)

Esta etapa ficou isolada em `crates/aceris-core/src/cpu/mpcore_gic.rs`; não alterou `system.rs`, `loader/ncsd/process.rs`, `loader/ncsd/session.rs`, GPU, Thumb ou os módulos de renderização de Claude.

### Alterações em `main`

- `40238ce` — generaliza a antiga entrada exclusiva do timer PPI29 para `set_interrupt_input_level(interrupt_id, asserted)`, mantendo um nível por cada um dos 128 IDs e recusando índices fora do intervalo.
- `3c5bea7` — corrige a fixture de teste SPI: habilitar um SPI não o encaminha automaticamente para CPU0; o byte ITARGETSR correspondente também tem de incluir CPU0.
- `3d2123e` — corrige a máquina de estados level-sensitive: depois do IAR, a linha que continua alta mantém o interrupt pending ao mesmo tempo que active. A desassertion da fonte limpa pending mesmo enquanto active; EOI encerra active e deixa pending somente se a fonte ainda estiver alta.
- `bc0bd02` — acrescenta leitura guest-visible de GICD_ISPENDR durante e depois do estado active+pending.

### Contrato implementado e coberto

- Edge-triggered: uma transição low→high faz latch de pending; uma nova borda enquanto o ID já está active mantém outra ocorrência pending até EOI.
- Level-sensitive: a linha alta mantém pending, inclusive durante active (active+pending). Quando o periférico baixa a linha, pending é removido; se EOI ocorrer antes da desassertion, o mesmo ID regressa a pending após EOI.
- O CPU interface não encaminha novamente ao processador um ID ainda active/active+pending. HPPIR/IAR podem portanto devolver o ID espúrio enquanto o ID ativo aguarda conclusão.
- SPIs só chegam a CPU0 quando ITARGETSR seleciona CPU0; não se deve alterar o reset/default target para fazer os testes passarem.

### Referências públicas independentes

- ARM GIC fundamentals: `https://support.arm.com/documentation/198123/latest/Arm-GIC-fundamentals` — transições Pending→Active+Pending e Active+Pending→Active para fontes level-sensitive, e Active→Inactive por EOI.
- ARM GIC architecture specification, handling: `https://support.arm.com/documentation/ihi0048/b/Interrupt-Handling/General-handling-of-interrupts?lang=en`.
- 3dbrew ARM11 Interrupts: `https://3dbrew.org/wiki/ARM11_Interrupts` — os IDs GSP 0x28–0x2D são level-sensitive, 1-N.
- 3dbrew GSP Shared Memory: `https://3dbrew.org/wiki/GSP_Shared_Memory` — ring 0x40/client, até 0x34 entradas, PDC com threshold 0x20, PDC entregue a todos os processos e outras interrupções entregues ao detentor de rendering rights.

### CI e limites observados

- Run `45284591425`, HEAD `3d2123e`: Android ARM64 release do aplicativo e benchmark aprovados; Linux passou fmt, build e Clippy. A suíte Linux teve 1 falha fora de `mpcore_gic`: `system::tests::arm11_guest_runs_the_srv_gsp_pica_lcd_chain_in_one_continuous_run`, que termina em `InvalidAccess { addr: 0x1FFD0800 }`; 1.446 testes passaram e 1 foi ignorado. A execução macOS/Windows ainda não tinha terminado no momento do registro.
- Run `45284693083`, HEAD `bc0bd02`: matriz de validação em andamento; aguardar resultado antes de declarar aprovação final.
- O teste guest chain e o caminho de sessão associado contêm trabalho anterior de outro agente; não foram modificados nesta etapa.
- A API de nível GIC está preparada, mas as fontes físicas GSP ainda **não estão conectadas** ao distribuidor. Não inferir o ponto de desassertion a partir da mera existência do relay event/queue. Cada dispositivo/comando precisa expor a sua causa pending e o ato concreto que a limpa antes de fechar essa integração.
- Não foram usados atrasos fixos importados de Azahar. Qualquer temporização de conclusão GPU permanece dependente de relógio virtual e evidência independente de hardware.


### Resultado confirmado da CI — GIC e cadeia guest (23/11/2026, revisão após 0bd293b)

- Run `45284693083`, HEAD `bc0bd02`: Android ARM64 passou (app e benchmark release); Linux e macOS passaram fmt, build e Clippy e falharam somente em `cargo test --workspace`; Windows estava na etapa de testes.
- Run `45284851808`, HEAD `0bd293b` (documentação atualizada, mesmo código Rust de GIC): Android ARM64 passou (app e benchmark release); Linux, macOS e Windows passaram fmt, build e Clippy e falharam somente em `cargo test --workspace`, todos pelo mesmo teste `system::tests::arm11_guest_runs_the_srv_gsp_pica_lcd_chain_in_one_continuous_run`, em `InvalidAccess { addr: 0x1FFD0800 }`.
- Em ambas as execuções, a falha Linux observada foi `system::tests::arm11_guest_runs_the_srv_gsp_pica_lcd_chain_in_one_continuous_run`, em `crates/aceris-core/src/system.rs:8731`, com `InvalidAccess { addr: 0x1FFD0800 }`. Na execução 45284851808 o módulo core reportou 1.446 testes aprovados, 1 falha e 1 ignorado.
- Os testes de `cpu::mpcore_gic::tests` passaram na execução 45284851808, incluindo `level_sensitive_gsp_spi_stays_active_and_pending_until_device_clears_line`, `level_sensitive_gsp_spi_is_redelivered_after_eoi_if_device_stays_asserted`, `edge_input_retrigger_while_active_remains_pending_until_eoi` e a leitura guest-visible de GICD_ISPENDR.
- A falha em `system.rs` é independente do arquivo alterado para o GIC e coincide com uma cadeia guest de integração que contém trabalho anterior de outro agente. Não foi alterada nem contornada nesta etapa. Diagnosticar o endereço exige rastrear PC/instrução e a origem do ponteiro no contexto da sessão; não se deve supor que ele seja TLS apenas por sua faixa de endereço.
- `docs/AI_HANDOFF.md` e `docs/AZAHAR_REFERENCE_STUDY.md` foram atualizados após a leitura desses resultados. A CI completa do HEAD ainda não está verde.

## Cookie Clicker CIA: renderização guest ligada e correção do formato de vértice (2026-11-24)

- Nenhum frontend habilitava a renderização PICA (`set_pica_fragment_rendering`): agora `System::enable_guest_gpu_rendering()` é chamado no desktop e no Android (desktop: `ACERIS_NO_GPU=1` desliga, para A/B). `ACERIS_TRACE_GPU=1` imprime o relatório de cada lista.
- Com a renderização ligada o GX queue ficava `halted` na 2ª lista por `InvalidStride { calculated: 6, configured: 20 }`. Correção 1: o stride do array é a distância entre registros e pode exceder os componentes (padding); só `calculado > configurado` é rejeitado (`minimum_record_size` já separava os dois).
- Correção 2 (erro de modelo): o nibble de formato de atributo tem bits 0–1 = tipo (byte, ubyte, short, float) e bits 2–3 = número de componentes − 1 (cabeçalho público do libctru, `GPU_ATTRIBFMT`); a largura vem só do tipo (float = 32 bits). O código antigo tratava bits 2–3 como largura (float de 16/24 bits), o que não existe na entrada de vértices. Float24 permanece só nos atributos fixos/registradores. Confiança: média-alta (documentação pública; sem teste em hardware). Teste de float reescrito (12 bytes, 3 floats de 32 bits).
- Teste `arm11_guest_runs_the_srv_gsp_pica_lcd_chain_in_one_continuous_run` (falhava no CI): (a) o guest agora mapeia o bloco GSP em 0x10002000 como o libctru, enquanto o alias do HLE fica em 0x1FFD0000; (b) o fixture declara `gsp::Gpu` na ACL do NCCH sintético (`..._with_services`).
- Resultado no jogo (450 quadros, orçamento 4M): 52 ProcessCommandList, 51 MemoryFill, 75 DisplayTransfer; cada lista tem 4 triângulos enviados (6 de saída) mas `pixels_written: 0`. Próximo passo: descobrir por que nenhum pixel é escrito (viewport/scissor/depth/shader/clipping).
- Regra da seção 5 de `REGRAS_PARA_IA.md` respeitada: nada do código de outros emuladores foi consultado.

## Cookie Clicker CIA: primeira imagem na tela (2026-11-24, noite)

- **Layout de vértices corrigido (segunda parte do erro de modelo).** Cada seletor de um array (`ATTRIBBUFFERi_CONFIG1/2`) nomeia um *atributo inteiro* (0–11) ou padding (12–15 = 4/8/12/16 bytes); o atributo ocupa `componentes × largura` bytes, com componentes vindos do nibble de formato (bits 2–3 + 1). O código antigo tratava cada seletor como um escalar. Evidência no jogo: `FORMAT_LOW=0x7B` (float×3 + float×2) com `CONFIG2=0x20140000` (2 atributos, stride 20 = 12 + 8). Teste novo `selectors_name_whole_attributes_with_per_format_component_counts`; 4 testes antigos reescritos. Fontes independentes: libctru `GPU_ATTRIBFMT`/`C3D_BufCfg` (cabeçalhos públicos) e a medição acima. Confiança: alta.
- **Linhas de textura invertidas.** `t = 0` seleciona a última linha armazenada (dados guardados de baixo para cima); o amostrador nearest usa `altura − 1 − linha` e o bilinear amostra com `1 − t`. Testes de textura/pipeline ajustados (`1 − t` endereça a linha armazenada diretamente). A DisplayTransfer com flag de flip (bit 0) que o jogo usa para enviar texturas já estava correta.
- **Resultado:** em 450 quadros (orçamento 4M) a tela superior mostra "PORT BY" e "CRECKERYOP" em orientação correta (tela de abertura do port); cada lista escreve 192 000 pixels. A tela inferior ainda está preta (provável: o jogo não desenha nela nessa fase, ou depende de algo ainda não modelado — investigar).
- **Fonte consultada (regra 5, regras atualizadas em 2026-11-24):** repositório `azahar-emu/azahar` (licença do repositório GPL-3.0; cabeçalho dos arquivos: "GPLv2 or any later version"), ramo `master` (SHA não registrado: leitura via raw.githubusercontent), arquivos `src/video_core/renderer_software/sw_rasterizer.cpp` e `sw_clipper.cpp`, consultados em 2026-11-24. Pergunta: orientação do `t` de textura e da transformação de viewport. Entendimento (parafraseado, nenhum trecho copiado): o `t` é invertido antes da busca (linha = altura − 1 − t) e a viewport não inverte Y. Implementação própria: só a inversão de `t` (o Y do viewport do Aceris não foi alterado, pois o resultado visual já está correto e testes do outro agente dependem dele). Confiança: média (comportamento de outro emulador não prova o hardware).
- **Estado dos testes:** 19 testes (render/draw_exec/output_map/system) falham no HEAD upstream por causa do commit `792b4907` do outro agente (máscaras esparsas de saída do shader: fixtures sem a máscara recebem "output map has no position"); não foram causados por esta mudança.

## PICA200 PROCTEX — unidade 3 integrada ao TEV e ruído de referência (25/11/2026)

- `PicaProceduralTextureSampler` está conectado ao caminho real de `PicaFragmentPipeline`: quando `TEXUNIT_CONFIG` habilita a unidade 3, o texel procedural alimenta `inputs.textures[3]` e segue pelo TEV. O sampler captura estado e LUTs por desenho, sem depender de alterações posteriores dos registradores.
- O caminho cobre seleção de coordenadas, cinco modos de clamp, shifts Odd/Even, funções de mapeamento RGB/alpha, Rmax, LUTs de ruído/mapeamento/cor/diferença, filtros Nearest/Linear e alpha separado. A escala das entradas de curva segue `/4095` para valor e delta assinado.
- O gerador de ruído segue o modelo de referência de software de Citra/Azahar: fase/frequência binary16, amplitudes fixas assinadas, hashes determinísticos da grade, interpolação pela LUT de ruído e bilinear. A fonte Azahar declara que os resultados foram conferidos contra hardware, mas que não se sabe se o algoritmo é o mesmo do circuito físico; não afirmar equivalência bit a bit.
- Ordem do sampler: normaliza U/V para positivo; calcula shifts a partir das coordenadas originais; aplica ruído e amplitudes independentes; aplica os shifts; executa clamp; calcula G e consulta LUT de mapeamento; consulta/interpola a LUT de cor; em alpha separado, usa a LUT alpha diretamente.
- Otimizações no hot path: `ProceduralNoiseState` pré-decodifica frequência, fase e amplitude uma vez ao capturar o desenho; a LUT de ruído só é capturada quando o ruído está ativo. O cálculo dos quatro gradientes reutiliza os hashes de `x`, `x+1`, `y`, `y+1`, evitando recalcular os mesmos hashes por canto. Há regressão com 576 posições de grade.
- Rmax usa `((u + v)/2 + sqrt(u² + v²))/2`; Rmax e distância radial são saturados a 1.0 antes da consulta da LUT. Testes cobrem fórmula, saturação e caminho do seletor 9 até o sampler.
- `PicaProceduralTextureF::Triangle` permanece isolado e não alimenta o sampler. O renderer de referência consultado combina G diretamente com a LUT e não demonstra uma etapa F independente nesse caminho; não conectar a onda triangular sem evidência adicional.
- **Limites fail-closed:** mipmap, bias diferente de zero, seletores reservados e parâmetros binary16 não finitos/fora de faixa são rejeitados. A unidade procedural ainda não é suporte total a PROCTEX e não foi validada em console físico.
- Referências de comportamento (parafraseadas; Rust original, sem copiar implementação): 3dbrew `GPU/Internal_Registers` e `GPU/Procedural_Texture_Generation`; Azahar `src/video_core/renderer_software/sw_proctex.cpp` no commit `fd4c9abd98345dbefff9cef07aadaf61ee1f683d`. A licença do repositório de referência não altera a autoria do código Rust do Aceris.
- Commits desta etapa: `78b0401` (sampler TEV), `3fc76c1` / `f542598` / `d543bd4` (Rmax e integração), `1b0f6d3` / `35fae11` (normalização `/4095` e expectativa), `076005d` / `06e4b45` / `bb9e845` (ruído de referência e correções), `bcd351b` (parâmetros pré-decodificados), `6b1f1db` / `4ca9475` (reuso de hashes e formatação).
- CI integral aprovada: workflow `45530579072` (ruído e pré-decodificação) e `45531640771` (reuso de hashes), ambos com Linux, Windows, macOS e Android ARM64; build, Clippy, testes e benchmarks concluídos. Nenhuma dessas aprovações demonstra compatibilidade de jogo ou prova que a tela preta original foi resolvida.
- Próximos passos: testar com capturas guest reais que habilitem a unidade 3; comparar texels/telas de casos de referência; medir o impacto de performance com benchmark de rasterização representativo; só depois estudar mipmaps e bias com evidência suficiente.

## Desempenho ARM11 — busca ordenada de regiões de memória (24/11/2026, ChatGPT)

- `5510f84` substituiu as varreduras lineares de `linear_regions`, `shared_regions` e `dynamic_regions` em `Arm11CodeAddressSpace::locate` por buscas binárias com `slice::partition_point`. Os vetores já são mantidos ordenados pelas rotinas de mapeamento/desmapeamento.
- A busca por endereço físico FCRAM deriva os dois endereços virtuais possíveis (alias antigo e alias novo), consulta ambos pela mesma busca ordenada e preserva a prioridade anterior do alias antigo quando os dois estão presentes. A verificação de segmentos estáticos continua examinando os cinco segmentos fixos.
- Foram acrescentados testes para limites e lacunas de segmentos, regiões compartilhadas e heap inseridas fora de ordem, os dois aliases lineares e a resolução de acessos físicos com MMU.
- `3092788` contém somente ajustes de formatação dos testes apontados pela primeira CI.
- **Validação:** a execução 45533029468, no commit `5510f84`, aprovou o job Android ARM64, mas os três jobs desktop pararam em `cargo fmt --all --check` antes de build, Clippy ou testes. A formatação foi corrigida em `3092788`; ainda não há resultado de CI para essa revisão final. Não foi possível executar Rust localmente nesta sessão.
- **Desempenho ainda não quantificado:** a mudança reduz as buscas nessas listas de O(n) para O(log n), mas nenhum ganho de FPS ou tempo total foi medido. Repetir o benchmark com a mesma imagem local, aquecimento, orçamento de instruções e quantidade de quadros antes de afirmar aceleração mensurável.

## Diagnóstico de memória — backing esparso de heap e blocos compartilhados (24/11/2026, ChatGPT)

- `SharedMemoryBacking` agora mantém páginas host de 4 KiB em um mapa esparso. `zeroed(size)` reserva somente o tamanho lógico; leituras em páginas ausentes retornam zero e escritas materializam apenas as páginas tocadas.
- O heap de `ControlMemory` deixou de criar um `Vec` e zerar a extensão inteira na alocação. Views de memória compartilhada continuam referenciando o mesmo armazenamento raiz, preservando aliases de heap, mapeamentos entre processos e os caminhos de cache/writeback.
- A escrita que cruza limites de página prepara os payloads em memória temporária antes de registrá-los, evitando alterar bytes do backing se a reserva de uma das páginas falhar.
- Adicionados contadores de páginas/bytes materializados e de gerações de exclusive monitor no backing, além de `Arm11MemoryUsageReport` para separar reserva virtual, páginas esparsas, D-cache, write buffer, TLS/IPC e páginas físicas da GPU. O relatório deduplica roots por identidade para não contar duas vezes um heap exposto também como shared memory.
- O exemplo `benchmark` imprime o diagnóstico depois das amostras, identificando explicitamente que os dados não representam o RSS integral do processo.
- Testes novos: `shared_backing_materializes_only_pages_touched_by_writes` e `memory_usage_report_deduplicates_heap_shared_alias_backing`. Os testes anteriores de alias entre processos, heap, cache e writeback permanecem como regressões.
- **Validação:** ainda pendente de CI para este commit. Nenhum build, Clippy ou teste local foi declarado nesta etapa.


## Otimização de buffers por quadro — referência BLACK (24/11/2026, ChatGPT)

- **Pergunta técnica:** como aproveitar técnicas do port de BLACK para reduzir custo de alocação e apresentação em um emulador 3DS, sem transplantar a arquitetura de recompilação ou alterar a ordem do hardware emulado?
- **Referências consultadas:** `gerisson1996/BLACK-NX-PORT`, README e `MIGRACAO.md` (arquitetura por camadas, recompilação estática, separação das camadas Android/Switch); `RanggaCasper/Black-Android`, commit `8e9c6fc7d570503ea68d039cff16c09339ee6d30`, licença MIT, arquivos `src/d3d/render_thread.cpp`, `native/vulkan.cpp`, `native/main.cpp` e `LICENSE`. Consulta concluída em 24/11/2026.
- **Entendimento obtido:** o BLACK Android mantém uma fila de lotes de renderização, reutiliza objetos de lote após sua conclusão, usa buffers de upload por quadro e espera somente em dependências que exigem readback/flush ou sincronização de apresentação. Também mantém a gestão de superfície/ciclo de vida separada do runtime do jogo. Essas técnicas são específicas de um port de jogo com backend Vulkan assíncrono; não justificam criar uma thread de renderização para o Aceris enquanto a PICA200 é processada sincronamente no núcleo.
- **Implementação independente em Rust:** o app thread Android retém o frame RGBA em um `Vec<u8>` persistente; `App::step_into` e `compose_game_into` reescrevem esse buffer por tick. O overlay dos controles também fica retido no estado de `App`; `render_virtual_pad_overlay_into` o limpa e atualiza sem nova alocação depois do primeiro uso. No desktop, `emulate` retém o canvas BGRX escalado e `compose_bgrx_into` compõe diretamente no buffer final, evitando alocar primeiro um canvas intermediário e depois outro buffer ampliado. As APIs Android antigas (`App::step` e `compose_game`) continuam disponíveis como wrappers que retornam `Vec`; o helper antigo `compose_bgrx` fica compilado apenas nos testes, pois o executável usa diretamente a variante in-place.
- **Regressões adicionadas:** teste de equivalência entre as APIs que retornam `Vec` e as variantes in-place; teste de estabilidade da capacidade/endereço dos buffers Android e desktop; teste de que dimensões inválidas não destroem o conteúdo do buffer de overlay e que overflow de escala é rejeitado.
- **Efeito esperado, sem quantificação:** remove alocações recorrentes dos buffers de saída e overlay no caminho de gameplay Android e reutiliza o buffer final BGRX no desktop. No desktop, a composição também deixa de criar um buffer base separado para depois ampliá-lo. Não elimina a renderização do canvas intermediário de intro/biblioteca Android, nem as cópias para o buffer da janela nativa. Não foi medido FPS, uso de RAM total nem redução de latência nesta etapa.
- **Limites preservados:** sem fila de renderização paralela, sem mudança de semântica ARM11/PICA200/DMA, sem novas dependências e sem APIs de GPU introduzidas. A futura adoção de batching/timeline fences requer um backend GPU que possa executar os comandos de forma assíncrona e testes próprios de coerência/readback.
- **Procedência:** nenhum código, shader, teste, asset ou estrutura do BLACK foi copiado ou adaptado. O projeto Black NX foi usado somente como referência documental; o código Black-Android foi lido apenas para compreender conceitos de gestão de lote, upload e ciclo de vida. Implementação e testes são originais do Aceris.

### Overlay virtual direto — composição esparsa da interface (25/11/2026, ChatGPT)

- Gargalo identificado: o caminho Android materializava um overlay RGBA do tamanho total da superfície a cada quadro, zerava todos os pixels e percorria a imagem inteira para misturar os poucos pixels ocupados pelos controles. A retenção de capacidade eliminou alocações repetidas, mas não o custo de zerar e percorrer a tela toda.
- Implementação original em Rust: blend_virtual_pad_overlay_into rasteriza e mistura os 12 controles diretamente no frame final, visitando somente as caixas delimitadoras dos botões. Pixels sobrepostos preservam a ordem original de desenho (a última forma prevalece); as letras são consultadas pela mesma tabela de glifos do renderer de referência. O Android não mantém nem limpa mais um segundo frame transparente.
- Testes de equivalência: a nova composição direta é comparada byte a byte com a sequência anterior (renderizar overlay de tela cheia + alpha blend), em portrait, landscape e auto, com diferentes máscaras de botões, incluindo todos pressionados. Há também regressões para tamanho incorreto e dimensões inválidas sem escrita parcial.
- Efeito esperado: elimina o buffer RGBA extra do overlay, o clear de toda a superfície e a varredura de mistura por todos os pixels; o trabalho acompanha as áreas dos botões. O frame principal ainda é composto e copiado para o ANativeWindow como antes. Não foi medido FPS nesta etapa.
- Limite: esta otimização altera apenas a interface host dos controles virtuais; não modifica pixels produzidos pela PICA200, composição das duas LCDs, temporização guest ou ordem de comandos.
- Correção da primeira execução de CI: a fixture de buffer inválido usava 64 bytes para uma imagem 4×4, que é um frame RGBA válido. O teste foi corrigido para fornecer 64 bytes a uma imagem 5×4; a implementação não foi alterada por essa falha.

## Buffers de áudio e escalonamento nearest-neighbour — Android (25/11/2026, ChatGPT)

- O app thread Android movia o Vec de amostras para fora do estado do App e o descartava após AAudio.feed, fazendo o buffer de mistura perder sua capacidade a cada quadro. Agora ele é devolvido vazio a App após a chamada de saída, retendo a mesma alocação para o próximo frame. O buffer interno de resampling AAudio já era reutilizado e não foi alterado.
- A rotina de blit RGBA da interface substitui a divisão inteira por coluna por um passo inteiro e acumulador de resto. A origem de cada coluna continua seguindo floor(coluna × source_width / destino_width); o mapeamento de linhas permanece inalterado.
- Testes: reciclagem preserva capacidade em atualizações sucessivas; o novo escalonador é comparado com a implementação de referência em várias dimensões, fatores de escala e recortes parcialmente fora do alvo.
- Efeito esperado: menos alocação de amostras por quadro e menos divisões inteiras no loop de pixels do frontend. Não foi medido FPS nem latência de áudio nesta etapa; o envio AAudio continua com timeout zero.

## Teste local dedicado ao CIA Cookie Clicker — 25/11/2026

- Foi acrescentado em `crates/aceris-core/tests/cia_real_homebrew.rs` o teste ignorado `validates_and_runs_real_cookie_clicker_cia_without_console_keys`, separado do teste existente do Super Mario 64.
- O novo teste espera o Title ID `0x0004000000F23F00`, boot content 0, ticket correspondente, conteúdo sem criptografia CIA externa e NCCH com NoCrypto. Em seguida valida a preparação ARM11, carrega pelo caminho padrão e exige que pelo menos uma instrução guest seja executada.
- O arquivo fornecido pelo mantenedor nesta sessão tem SHA-256 `d047701f8e9b4127436046f5a650dede06e03bc1b5f1b271981d4ea27db58093`. Ele não foi copiado para o repositório, CI ou artefatos.
- A documentação de execução foi adicionada a `docs/REAL_HOMEBREW_TESTING.md`.
- **Validação local pendente:** este ambiente de edição não possui `cargo` nem `rustc`; portanto o teste não foi executado aqui. O workflow CI compila o alvo de integração, mas ignora sua execução por depender do CIA local. Não declarar boot novo confirmado até executar o comando manual no ambiente que contém o arquivo.


### CI do teste local Cookie Clicker — 25/11/2026

- O commit `d5cdc15b13e454a8c580bb8dbb50b522c05d7693` passou integralmente no workflow [45600245040](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45600245040): Linux, Windows, macOS e Android ARM64.
- Linux, Windows e macOS passaram formatação, build, Clippy, `cargo test --workspace` e build release do benchmark; Android passou os builds release do app e do benchmark.
- O novo teste `validates_and_runs_real_cookie_clicker_cia_without_console_keys` foi compilado como parte da suíte, mas não executado porque está marcado `#[ignore]` e depende do CIA local. A validação real desse arquivo permanece pendente de execução no ambiente que o contém.


### Cookie Clicker DSP service lookup diagnosis — 25/11/2026

- Structural CIA inspection confirmed the TMD content digest, ticket/TMD Title ID `0x0004000000F23F00`, boot index 0, NoCrypto and the NCCH ACI permission `dsp::DSP`.
- Static guest-code analysis found the `Sound.init` entry at VMA `0x0010F7B0`, LPP audio initialization at `0x001E8848`, and DSP setup at `0x001EBD88`. The DSP setup obtains the `dsp::DSP` service handle through the guest service-manager wrapper before sending the DSP IPC request.
- Because the ACI explicitly permits `dsp::DSP`, do not weaken the ACL. The likely diagnostic boundary is the preceding `srv:GetServiceHandle`/RegisterClient flow; runtime evidence is still required to identify the exact failure.
- `System::unhandled_requests()` now records failed service-manager lookups (RegisterClient missing, malformed request, invalid name, ACL denial, unknown service and handle-allocation failure). The manual Cookie Clicker CIA test also asserts that its parsed ACI allows `dsp::DSP`.
- No runtime DSP success is claimed. The uploaded CIA and decompressed guest code remain outside Git.


- Regression coverage for the service-manager diagnostics: the NCCH ACL test asserts an explicit `ssl:C` denial entry, the pre-registration test asserts the missing-`RegisterClient` entry, and the DSP handshake test confirms a successful `dsp::DSP` lookup produces no false-positive diagnostic.


### Diagnóstico HLE visível no frontend Android — 25/11/2026

- Ao sair de um jogo com a tecla Voltar, o frontend Android agora lê `System::unhandled_requests()` antes de descartar o sistema e mostra a primeira falha de `srv:GetServiceHandle` na linha de status da biblioteca. Se não houver falha de SRV, mostra a primeira solicitação não atendida.
- Se a execução parar por erro, a mensagem de interrupção inclui também o primeiro diagnóstico HLE disponível.
- Isso torna os resultados da investigação de `dsp::DSP` visíveis no próprio aparelho, sem depender de logcat ou de um frontend desktop. O estado é exibido após sair do jogo; não é uma sobreposição durante o gameplay.


### CI do diagnóstico visível no Android — 25/11/2026

- O commit `3b85bf258d8d197ad1dafc79333f8fdbcf7c8574` passou no workflow [45603812470](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45603812470) em Linux, Windows, macOS e Android ARM64.
- Formatação, builds, Clippy, testes completos e benchmarks release passaram nos desktops; builds release do app e do benchmark passaram no Android.
- O diagnóstico de serviço HLE fica visível na biblioteca ao sair do jogo pelo botão Voltar. O CIA não foi incluído na CI e o teste manual ignorado continua pendente de execução com o arquivo local.

## Cookie Clicker: menu renderizado nas duas telas e desempenho (2026-11-25)

- Em 900 quadros (orçamento 4M, ~8 min) o jogo mostra o **menu inicial nas duas telas** (título + cookie em cima; play/Options/mascote embaixo). A tela de abertura "PORT BY CRECKERYOP" vem antes. Serviços ainda reportados: `http:C`, `ir:rst`, `ssl:C` (não registrados) e `cfg:i`, `cfg:s`, `ptm:sysm` (negados pela ACL do NCCH do jogo — comportamento esperado).
- **Desempenho (instrumentado com gdb + binário com `line-tables-only`):** (1) `Bus::fetch16/fetch32` (padrão = `read*`); o espaço de endereços lê o segmento de texto estático direto, sem o modelo de D-cache (boot de 60 quadros: 25,1 s → 15,6 s, PC final idêntico). (2) `cache_lines_for_range` consulta o `BTreeMap` do cache por intervalo em vez de enumerar cada linha possível (evita 9 000 inserções por flush de framebuffer). (3) `set_arm11_smp_mode` só toma o lock do SCU quando o valor muda (era um `Mutex` por instrução, ~15%). 400 quadros: 126 s → 114 s. Ainda ~3,5 quadros/s contra 60 necessários: restam alocações em VFP (`vfp_vector_register_lanes` aloca `Vec` por operação), `Result<StepEvent, Error>` grande, lookup BTreeMap do escalonador por instrução e rasterização por pixel.
- Teste novo: `instruction_fetch_reads_text_directly_and_matches_data_reads`.

## Cookie Clicker jogável em modo headless (2026-11-25)

- Novo `--tap X,Y@QUADRO[+SEGURAR]` no desktop (headless, repetível; tela de baixo 320x240; segura 3 quadros por padrão). Teste de parsing incluso.
- Validado de ponta a ponta com o CIA real: toque em (160,80) no quadro 600 sai do menu e entra na tela de jogo (HUD e loja em cima, cookie e botão Back embaixo); 8 toques em (160,120) nos quadros 700–840 resultam em "8 COOKIES" no HUD. Isso exercita HID (toque) → Lua → GPU → LCD. 1000 quadros sem erros; PC final 0x001F208C (espera ociosa).
- Não validado: Android em dispositivo (este ambiente não tem SDK/NDK/targets Android nem dispositivo — nada foi instalado ou testado), áudio audível (csnd só reconhece comandos; não há mixagem nem saída), desempenho (≈3,5 quadros/s no desktop contra 60 necessários).


## Benchmark ponta a ponta — entradas reproduzíveis e trace por quadro (25/11/2026, ChatGPT)

- **Objetivo:** transformar o benchmark de sessão guest em um ensaio repetível de gameplay, correlacionando cada quadro com seu custo no host e com a atividade visível da cadeia guest.
- **Implementação isolada:** somente `crates/aceris-core/examples/benchmark.rs` recebeu as opções `--tap X,Y@FRAME[+HOLD]` e `--frame-trace <CSV>`. Os toques usam `System::input_mut()` e são aplicados antes do quadro correspondente, inclusive durante aquecimento. O índice começa em zero no primeiro quadro guest e inclui warm-up; `--warmup 0` mantém os quadros iguais aos números da sequência.
- **Trace agregado:** para cada quadro medido, o CSV registra tempo total de `System::run_frame_with_budget`, instruções ARM11 executadas, listas GSP/PICA submetidas, scanouts LCD, entregas de interrupção GSP, motivo de parada, PC e CPSR. A injeção do toque é anterior ao cronômetro; a atualização HID e o restante do quadro permanecem dentro dele. Nenhum temporizador por instrução, lock de profiling ou alteração semântica foi adicionado ao núcleo.
- **Regressões:** testes do parser cobrem toque padrão (3 quadros), duração explícita, mínimo de 1 quadro e entradas inválidas. A CI executa `cargo test -p aceris-core --example benchmark`.
- **Validação:** workflow [45653161410](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45653161410), commit `9929bcfbf98c2e6c2794dae15224e0e43278b225`, passou integralmente em Linux x86-64, Windows x86-64, macOS ARM64 e Android ARM64. Nos três desktops passaram Rustfmt, build do workspace, Clippy com `-D warnings`, `cargo test --workspace`, testes do exemplo, build release do benchmark, perfil sintético e perfil específico de backing esparso. O Android compilou o app, o executável benchmark e o exemplo release `memory_profile`.
- **Como repetir o workload real:** `docs/PERFORMANCE_TESTING.md` contém um comando PowerShell de 1.000 quadros para o Cookie Clicker, orçamento de 4.000.000 instruções, toque em (160,80) no quadro 600, oito toques em (160,120) nos quadros 700–840, captura LCD e saída CSV.
- **Ligação com o perfil real já registrado:** o ensaio headless anterior mostrou o menu e depois gameplay com HUD “8 COOKIES”, 1.000 quadros sem erro e aproximadamente 3,5 quadros/s. O perfil GDB já registrado acima mediu redução de 25,1 s para 15,6 s no boot de 60 quadros e de 126 s para 114 s em 400 quadros depois de otimizações do fetch de texto, busca de linhas D-cache e lock do SCU. Esses números são histórico de ensaios anteriores, não novas medições desta etapa.
- **Limitação desta sessão:** não havia CIA Cookie Clicker montado no ambiente nem disponível na Library/conversa. Portanto a nova sequência de 1.000 quadros e o CSV **não foram executados nesta sessão**; a CI valida compilação/testes da ferramenta, não gameplay com o CIA. Não declarar um novo perfil real concluído até rodar o comando local com o arquivo do mantenedor e inspecionar o CSV.
- **Próximo diagnóstico, após obter o CSV:** comparar p50/p95/p99 e quadros mais lentos; separar menu ocioso, quadro de entrada/play e sequência de cliques; correlacionar tempo com instruções/listas/scanouts/PC. Depois coletar um perfil de amostragem do host com a mesma sequência para atribuir custo a funções. As alocações VFP, o tamanho do resultado `StepEvent`, o lookup BTreeMap do scheduler e a rasterização por pixel permanecem hipóteses do perfil anterior, não gargalos confirmados por este trace.
- **Fronteiras preservadas:** nenhum arquivo de `system.rs`, CPU/Thumb, MMU, sessão, GPU ou frontend foi alterado nesta etapa. Nenhuma alteração foi feita ao trabalho reservado a Claude. Apenas `main` foi usada.


## Super Mario 64 3DS Port (Luigi Version): falha no bootstrap de memória — 25/11/2026

- **Imagem examinada:** CIA fornecido pelo mantenedor nesta conversa, Title ID `0x000400000FAA5C00`, NCCH NoCrypto. SHA-256 local do contêiner: `77407e7a0290e2935eec7460883c66e31a75744e76e193cb62776b2368fee4e6`. O CIA, o ExeFS extraído e qualquer dump permanecem fora do Git e da CI.
- **Ferramenta:** benchmark release oficial do Aceris, inicialmente do artefato Linux da CI. Ele carregou o CIA e preparou ARM11 em `PC=0x00100000`, `CPSR=0x00000010`.
- **Primeiros cinco quadros** com orçamento de 4.000.000 instruções:
  - quadro 1: 202,166 ms, 2.348.557 instruções, para em SVC `0x38`;
  - quadro 2: 0,004 ms, 15 instruções, para em SVC `0x39`;
  - quadro 3: 0,001 ms, 7 instruções, para em SVC `0x3A`;
  - quadro 4: 0,005 ms, 49 instruções, para em SVC `0x3C`;
  - quadro 5: 0,002 ms, 24 instruções, para em SVC `0x3C`.
  Não houve GSP command lists nem LCD scanouts. As três capturas LCD ficaram pretas. Os 24,73 quadros/s agregados dessa janela não são uma medida de FPS: quase todo o tempo pertence ao primeiro trecho de bootstrap e os quadros seguintes param em SVCs.
- **Falha reproduzida:** com seis quadros de aquecimento e o mesmo orçamento, o quadro 6 falha em `0x08007FF8`. O benchmark recebeu diagnóstico adicional no commit `9228a03201d135b13f78b2b9a2605ce09e1824b4` para imprimir registradores, PC/CPSR e resumo de memória. O contexto registrado foi: thread 1, PC `0x001D8C1C`, SP `0x08008000`, stack principal declarada de `0x40000`, heap de aplicação mapeado reportado como 0 KiB. A instrução em PC é `push {r4, lr}`; SP−8 é precisamente `0x08007FF8`.
- **Sequência SVC observada:** `0x38`, `0x39`, `0x3A`, `0x23` (CloseHandle retorna `0xD8E007F7`), `0x01` (ControlMemory, operação 3, endereço `0x08000000`, tamanho zero, retorna `0xD8E007F1`), `0x3C` (Break), depois `0x01` para operação linear `0x10003`, também com tamanho zero. O despacho atual da sessão não possui handlers para ResourceLimit `0x38`–`0x3A` nem para Break `0x3C`; eles caem no caminho de SVC não tratado. As semânticas públicas dos IDs estão documentadas em [3dbrew SVC](https://www.3dbrew.org/wiki/SVC). A janela `0x08000000` é a região de heap mapeada por ControlMemory segundo [3dbrew Memory layout](https://www.3dbrew.org/wiki/Memory_layout).
- **Ligação estática entre os SVCs e o tamanho zero:** no bootstrap ARM11 em `0x001D7608`, o código inicializa dois buffers de saída de 64 bits com zero, chama `0x39` e `0x3A` para o mesmo tipo de recurso e calcula a diferença entre limite e uso atual. Como esses SVCs não são atendidos, os buffers permanecem zerados e o resultado é zero. O SVC `0x38` também não substitui `r1`: o pseudo-handle de processo `0xFFFF8001` permanece no registrador, é passado às consultas e depois a `CloseHandle`, que retorna handle inválido. O trace runtime confirma `ControlMemory(size=0)`. Isso torna a ausência das consultas ResourceLimit a causa upstream mais provável do pedido de alocação vazio.
- **Interpretação e limite:** a falha de heap/stack secundária é reproduzível, mas a correção ainda exige conferir o contrato do jogo e os valores legítimos de ResourceLimit antes de definir política de memória. Não atribuir tamanhos arbitrários nem mascarar acesso inválido com mapeamento genérico.
- **Resultado técnico desta etapa:** foi identificado um bloqueio real e reproduzível do caminho de execução de um homebrew real, mas **não** um gargalo de desempenho CPU/GPU. O título ainda não chega a trabalho PICA/LCD. O próximo critério de avanço é o bootstrap mapear corretamente a região requisitada e produzir o primeiro scanout não preto; só depois faz sentido fazer profiling de quadros de jogo.
- **CI do diagnóstico de contexto:** workflow [45655499126](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45655499126), commit `9228a03201d135b13f78b2b9a2605ce09e1824b4`, passou integralmente em Windows, Linux, macOS e Android ARM64. Incluiu formatação, build, Clippy, testes completos, testes do benchmark, build release e perfis desktop; Android compilou o frontend e os exemplos de benchmark.
- **Arquivos alterados nesta análise:** apenas `crates/aceris-core/examples/benchmark.rs` (diagnóstico de erro no benchmark) e documentação (`docs/PHASE1_BASELINE.md`, `docs/PERFORMANCE_TESTING.md`, este handoff). Nenhum arquivo `system.rs`, CPU/Thumb, sessão/SVC, memória, GPU ou frontend foi modificado. Coordenar qualquer mudança nesses módulos com o agente responsável e o mantenedor.



## PICA framebuffer flip e arquitetura de backends host — 25/11/2026

- **Referência técnica externa:** revisão Azahar `71915de9e326da0bfe76902bf9269406a3dc3adc`, consultada somente para comportamento e responsabilidades arquiteturais. O projeto Azahar é GPL-3.0 e os arquivos consultados incluem cabeçalhos GPL-3.0-or-later e GPL-2.0-or-later. Nenhum código, shader, teste ou asset foi copiado/adaptado para Aceris. A implementação permanece Rust original e licença MIT.
- **PICA — flip de framebuffer:** `GPUREG_FRAMEBUFFER_DIM.bit24` já era decodificado como `vertical_flip_enabled`, mas o caminho `PicaFragmentRenderSession` não aplicava o estado. Agora ele espelha o retângulo do viewport, repassa `flip_viewport` para o rasterizador, inverte o mapeamento Y de cada vértice em `to_screen`, espelha a interpretação do scissor e normaliza o sinal de winding antes do face culling. O codec de armazenamento tiled não reordena bytes; o flip afeta a transformação de desenho. A semântica consultada indica bit 24 igual a zero habilita flip e um desabilita; ainda falta validação com hardware PICA físico.
- **Testes:** novas regressões verificam viewport espelhado, origem negativa, scissor inferior-esquerdo no modo normal e invertido, e preservação de culling CCW/CW nos dois modos. O teste end-to-end `pica200_indexed_fragment_render` agora declara bit 24 em 1, porque sua imagem esperada é explicitamente a orientação não invertida. Isso não altera o caminho guest: corrige somente o fixture antigo, que implicitamente deixava o flip ativo.
- **CI do código:** workflow [45669287001](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45669287001), commit `393125f90735db549b78f4ed99a18f32c537b6da`, concluído com sucesso em Windows, Linux, macOS e Android ARM64. Os três desktops passaram Rustfmt, build, Clippy (`-D warnings`), `cargo test --workspace`, testes do exemplo, build release do benchmark e perfis de componentes/memória. Android compilou o app, o executável de benchmark e `memory_profile`. Nenhum teste de gameplay foi executado nessa CI.
- **OpenGL/Vulkan — estado real:** Aceris ainda não tem contextos/surfaces GL/ES, Vulkan instance/device/queues/swapchain, pipelines host, descritor/cache de recursos, nem tradutores GLSL/SPIR-V para o estado PICA. Os frontends continuam apresentando superfícies LCD RGBA via buffers CPU (no desktop X11, `XPutImage`). Não adicionar bindings/context crates sem aprovação do mantenedor: as regras do repositório exigem essa aprovação.
- **Arquitetura registrada:** `docs/GRAPHICS_BACKENDS.md` separa presenter (upload/composição de LCD RGBA) de rasterizador PICA acelerado. O segundo precisa receber snapshots imutáveis de draw independentes da API, preservar memória guest/DMA/readback e manter o rasterizador CPU como referência/fallback. `docs/ROADMAP.md` agora lista as etapas e critérios distintos para OpenGL e Vulkan.
- **PICA ainda pendente:** GAS/shadow, cubemap/projeção, filtros/mipmap/bias da unidade procedural onde não suportados, validação física dos formatos/LOD, layout de superfície 32×32 e comparações determinísticas de imagens. A unidade procedural 3 já está conectada ao TEV para o subconjunto documentado no código; o helper F(g) segue isolado.
- **Fronteiras:** só os arquivos GPU de flip, o fixture end-to-end, comentário de suporte procedural e documentação de backends/roadmap/handoff foram alterados. Nenhum arquivo CPU/Thumb, MMU, sessão/SVC ou frontend foi alterado nesta etapa. Só `main`; nenhuma branch criada.


## Continuação do presenter OpenGL/Vulkan — 25/11/2026 (ChatGPT)

- O mantenedor autorizou expressamente as dependências necessárias aos bindings/superfícies OpenGL, OpenGL ES e Vulkan. A camada host foi publicada em `crates/aceris-graphics` com `wgpu 30.0.2`, `raw-window-handle 0.6.2` e `pollster 0.4.0`; implementação do produto permanece Rust original.
- A apresentação host está conectada ao desktop (Xlib, Win32 e AppKit) e Android NativeActivity (ANativeWindow), com seleção `ACERIS_GRAPHICS_BACKEND` e fallback CPU. O Android retém referência adicional de ANativeWindow enquanto a surface existir.
- O presenter recebe frames LCD já compostos no core/software. Ele **não** executa o vertex shader PICA, o fragment/TEV PICA nem DRAWARRAYS/DRAWELEMENTS e não sincroniza diretamente render targets guest. Não anunciar rasterização PICA acelerada.
- Em 25/11, o upload desktop BGRX8 deixou de copiar/swizzlar a imagem para um buffer RGBA temporário: os bytes alimentam textura BGRA8 diretamente, e o shader força alpha=1 porque o quarto byte é padding. Android continua RGBA. A otimização elimina a passagem de conversão CPU, mas não foi medida em FPS.
- CI anterior: workflow [45679244054](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45679244054) passou build workspace e testes isolados do crate gráfico em Windows/Linux/macOS; Android ARM64 passou os builds release. O Clippy global falhou por 30 `manual_is_multiple_of` em `aceris-core` (loader/memory), e os testes workspace ficaram skip. A CI agora inclui Clippy isolado de `aceris-graphics` antes do Clippy global.
- Nenhuma edição foi feita em `crates/aceris-core/src/gpu/*`, `system.rs`, CPU/Thumb, MMU ou SVC nesta continuação. Esses módulos permanecem sob coordenação com o agente responsável; o próximo passo de rasterização PICA requer acordo sobre propriedade dos arquivos e interface do pacote de draw.
- Sem teste em GPU física/Android real: as verificações de CI são compilações/testes host sem dispositivo. Não declarar suporte real do driver ou paridade visual PICA.

## Reformulação do launcher Android e desktop — 25/11/2026

- **Problema identificado nas capturas do mantenedor:** a primeira UI responsiva desenhava painéis/ícones com coordenadas escaladas, mas passava coordenadas lógicas sem escala à fonte bitmap. Isso deslocava títulos, caminho, botão de atualizar e ícone de pasta, causando sobreposição.
- **Correção original em Rust:** o renderer de texto agora aplica a mesma transformação lógica/física dos painéis. O texto de ações de biblioteca vazia e os títulos da home são centralizados por largura medida da fonte. Foram adicionados testes de escala/centralização.
- **Tipografia:** fonte bitmap própria agora oferece letras minúsculas e glifos portugueses acentuados; os rótulos deixam de aparecer inteiramente em caixa alta.
- **Estrutura:** biblioteca separa Jogos (.3ds/.cci), Homebrew (.3dsx) e Configurações. Cada seção tem contagem e filtro próprios. Estado vazio orienta quais extensões são aceitas e mostra a pasta atual e ação de atualização.
- **Entrada:** Android tenta iniciar o item com um único toque; desktop mantém seleção e ativação por Enter/segundo clique. As áreas de toque foram alinhadas às posições renderizadas.
- **Layout:** Android desenha na superfície NativeActivity completa; desktop usa uma composição widescreen 960×600. A abertura mantém tela cheia com metade da resolução interna e ampliação nearest-neighbour.
- **Fundos customizados:** foi restaurado o suporte a sequências RGBA8 400×480 no renderer responsivo desktop, com cover-crop, mistura abaixo dos controles e preservação da paleta.
- **Pesquisa de UX:** PPSSPP, Dolphin, Azahar, Eden, DuckStation, PCSX2, My Boy! e a documentação pública do DraStic foram consultados apenas para padrões de navegação, organização de coleção e controles. Nenhum código, layout, fonte, ícone, capa ou outro asset foi copiado ou adaptado. O estudo detalhado está em docs/MOBILE_REFERENCE_STUDY.md.
- **Limites honestos:** capas e metadados reais, pesquisa, favoritos, recentes, ordenação, seleção de diretórios, perfis por jogo, editor de controles e temas alternativos ainda não estão implementados. A UI não anuncia opções de backend gráfico que não tenham fluxo real de aplicação.
- **Fronteiras:** alterações restritas aos renderers de UI/fonte, frontend Android, frontend desktop e documentação. Nenhum arquivo CPU/Thumb, MMU, sessão/SVC, GPU/PICA ou código reservado a outro agente foi editado. Somente main; nenhum branch foi criado.
- **Validação pendente:** o workflow CI para o HEAD de integração deve confirmar rustfmt, build, Clippy e testes Linux/Windows/macOS, além do build Android ARM64. Mesmo uma CI verde não substitui teste visual em GPU/dispositivo físico; não declarar o launcher validado no telefone até nova captura do mantenedor.

## Reformulação horizontal, abertura e áudio externo — 25/11/2026

- O Android foi travado em sensorLandscape. Renderer principal em Rust redesenhado em grade horizontal com Início, Jogos, Favoritos, Recentes, Homebrew e Configurações, barra lateral fixa e cartões em três colunas. Favoritos e Recentes persistem em .aceris/favorites.txt e .aceris/recent.txt dentro da pasta de jogos.
- Ordenação A–Z/Z–A, filtro por seção, contagens e navegação por setas/D-pad/mouse/toque foram ligados a callbacks reais. Marcar recentes só ocorre após a tentativa de boot bem-sucedida.
- A abertura procedural foi reduzida de 150 quadros/2,5s para 84 quadros/1,4s, preservando o desenho/curvas originais. Android amostra internamente até ~600×360; RGBA e conversão BGRX reaproveitam buffers.
- Android AAudio marca seu stream com `AAUDIO_USAGE_GAME` quando a função API28 está disponível. JNI Rust attach do app thread consulta `AudioManager.getActivePlaybackConfigurations()` e `AudioPlaybackConfiguration.getAudioAttributes().getUsage()` a cada 500ms; `USAGE_MEDIA` externo silencia a saída Aceris com zeros sem fechar o stream. Ao parar a mídia, áudio retorna.
- A detecção é deliberadamente anônima: não lê player/package name nem faixa/artista/capa. Não pede permissão de notificações. API anterior a 28 fica sem detecção para evitar autoidentificação incorreta. Metadados de faixa requerem NotificationListenerService/MediaSession autorizado e não foram implementados devido à fronteira Rust-only/NativeActivity `hasCode=false`.
- A referência JNI da Activity é criada somente em API28+ e liberada no callback de destruição depois do join do app thread. O monitor só é dono do attach/detach da JVM.
- Sem cópia de código ou assets de PPSSPP/Dolphin/Azahar/Eden/DuckStation/PCSX2/My Boy!/DraStic. A pesquisa documenta Recentes/Favoritos, grade, controles, profiles, save states e rewind como padrões; search, metadata real, perfis por título, save state/rewind e editor de controles seguem planejados, sem controles falsos.
- **Validação pendente:** CI para a revisão final deve confirmar rustfmt, build/testes Linux/Windows/macOS e Android ARM64. Teste físico obrigatório para orientação, latência visual, touch, pause/resume, Spotify, YouTube Music e outros players. Não declarar experiência de produto final sem essa verificação.
- **Fronteiras:** mudanças em frontend Android (NativeActivity/AAudio/App/manifest), renderer de biblioteca/intro e frontend de navegação desktop. Nenhum arquivo CPU/Thumb, MMU, GPU/PICA, sessão/SVC ou arquivos reservados a outro agente foi alterado. Somente main; nenhum branch.
## Revisão final da home horizontal, persistência e detecção de mídia — 25/11/2026

- Revalidação da UI horizontal: barra lateral e seis seções mantidas; grade de três colunas; D-pad/setas horizontais e verticais; Favoritos, Recentes, ordenação, continuar jogo e atualizar biblioteca.
- Corrigido o alvo de toque do botão visível em Configurações na paisagem. A ação agora usa as mesmas coordenadas do painel desenhado e tem teste de regressão.
- A abertura de 84 quadros mantém buffers RGBA/BGRX reutilizáveis; o loop de navegação desktop também reaproveita os buffers de renderização e apresentação em vez de alocar dois vetores por atualização.
- Corrigida a persistência de Favoritos/Recentes: os nomes são gravados em linhas reais, não como os caracteres literais barra+n. Teste novo fecha/recria a biblioteca e verifica a recuperação dos dois arquivos.
- Favoritos exibidos na contagem são limitados a entradas que ainda existem no diretório. Jogos inválidos não entram no histórico; Android e desktop só marcam Recentes depois de load_executable_file retornar sucesso.
- O indicador de mídia agora distingue monitor disponível de nenhuma mídia ativa. Ele só aparece como automático quando o host Android tem AAudio com AAUDIO_USAGE_GAME e conseguiu anexar o monitor em API 28+.
- A consulta pública AudioManager.getActivePlaybackConfigurations() retorna configurações anônimas para apps comuns; AudioPlaybackConfiguration.getAudioAttributes().getUsage() permite diferenciar USAGE_MEDIA de USAGE_GAME sem expor UID ou nome do player. A consulta acontece a cada 500 ms e o Aceris zera somente as amostras da própria saída AAudio enquanto há mídia externa ativa. Fontes oficiais: https://developer.android.com/reference/android/media/AudioManager, https://developer.android.com/reference/android/media/AudioPlaybackConfiguration e https://developer.android.com/reference/android/media/AudioAttributes.
- Metadados de faixa não implementados: título, artista, capa e nome do app exigem MediaSessionManager.getActiveSessions() por um NotificationListenerService habilitado ou permissão de sistema MEDIA_CONTENT_CONTROL. O serviço precisa ser uma subclasse concreta declarada no Manifest; o aplicativo continua hasCode=false e Rust-only. Não pedir acesso a notificações sem uma implementação de serviço válida e não mostrar metadados fictícios.
- Estado de validação: a CI dos commits desta revisão está em andamento. A compilação Android bem-sucedida de um commit anterior não substitui o resultado do HEAD atual. Ainda é obrigatório testar orientação, navegação, inicialização, Spotify, YouTube Music, outros players, parada/retomada da mídia e saída do app em dispositivo real.
## Crate `aceris-gl`: fundação OpenGL sem dependências (2026-11-25)

- Novo crate `crates/aceris-gl` (zero crates de terceiros): `dlopen`/`dlsym` à mão, EGL (plataforma *surfaceless* da Mesa no Linux, padrão no Android; contexto GL 3.3 core / ES 3.0), tabela de ~40 funções GL resolvidas em tempo de execução, `OffscreenContext` (compilar programas com log do driver, FBO RGBA8, `read_rgba`). Ainda **não está ligado** ao `System`/GPU: o rasterizador em uso continua sendo o da CPU.
- Decisão: o display continua via framebuffer LCD em CPU (`XPutImage`); o OpenGL entra como backend de rasterização offscreen com readback, porque os color buffers emulados vivem na memória do guest.
- Validação: 3 testes (clear+readback, triângulo com shader, erro de compilação) passam no Mesa llvmpipe (GL 4.5 core) com `Xvfb` desnecessário. Os testes se ignoram (com aviso) quando EGL/GL não existem. **Vulkan não foi feito**: o ambiente tem `libvulkan` mas nenhum ICD, então código Vulkan não poderia ser validado.
- Android (EGL/GLES3) está escrito (`cfg(target_os = "android")`) mas **nunca foi compilado nem executado**.

## PICA200: iluminação por fragmento (2026-11-25)

- Novo módulo `crates/aceris-core/src/gpu/lighting.rs`: decodifica o ambiente de luz (registradores 0x140–0x1D9, enable em 0x08F + disable em 0x1C6), 24 LUTs de 256 entradas (`Pica200::lighting_lut_entry`; escrita pelas portas 0x1C8–0x1CF com índice/seletor em 0x1C5, ponteiro com wrap em 256) e avalia por fragmento as cores *fragment primary/secondary* usadas como fontes do TEV. Quaternion (semânticas 0x04–0x07) e view (0x12–0x14) agora saem do output map, são interpoladas em perspectiva e chegam ao fragmento; lanes parcialmente mapeadas assumem identidade/+Z em vez de abortar.
- Implementado: cores 10-bit/255, posições float16, spot fixed 1.1.11, bias/scale float20, direcional/posicional, difusa de dois lados, fatores geométricos, LUTs D0/D1/FR/RR/RG/RB/SP/DA com seletor de entrada, escala, abs, desativações por configuração/por luz, `clamp_highlights`, fresnel na última luz para alfa primário/secundário, bump NormalMap/TangentMap (com/sem renormalização).
- **Não verificado / aproximado** (sem hardware nem teste de jogo): sinal do input SP (usei `-dot(L, spot)`), input CP (projeção do half vector sobre o tangente), entrada do fator geométrico (usei N·L/|H|²), indexação das LUTs (abs: ×256, com sinal: ×128 com wrap), delta fixed1.0.11, e a escolha de exigir 0x08F=1 **e** 0x1C6=0. **Sombras (shadow factor) não são aplicadas** (fator 1.0). Gas, geometry shader, planos de clip e depth offset continuam pendentes.
- Fontes consultadas: Azahar (`azahar-emu/azahar`, branch `master`, `src/video_core/pica/regs_lighting.h`, cabeçalho “GPLv2 or any later version”, 2026-11-25; também `renderer_software/sw_lighting.cpp` na mesma data) para layout de registradores, enums e quais samplers cada configuração aceita; 3dbrew (GPU/Internal Registers) não pôde ser lido nesta rodada. Entendimento: descrição de campos/bits e do fluxo luz→LUT→cor; nenhuma linha copiada. Implementação própria em Rust. Confiança: layout de registradores e tabela de samplers alta; fórmulas finais média (ver itens acima).
- Testes: 9 em `lighting.rs`, 1 de pipeline (`enabled_lighting_feeds_the_fragment_primary_color_into_tev`), 1 ajustado no output map. 1503 testes de `aceris-core` passam.

## Kernel: ResourceLimit e SVC Break no bootstrap de processos — 25/11/2026

- Implementados em Rust os SVCs 0x38, 0x39 e 0x3A para o processo corrente: criação de handle tipado ResourceLimit, consulta do limite COMMIT e consulta do COMMIT atual.
- O limite COMMIT acompanha a configuração atual do kernel shared page: APPMEMALLOC = 0x04000000 (64 MiB, Old 3DS / APP_MEMTYPE 2). Esse é o perfil configurado pelo Aceris, não um valor universal para todos os tipos de processo ou New 3DS.
- O COMMIT atual é derivado dos tamanhos guest mapeados de text, read-only, data, BSS, stack e regiões dinâmicas (incluindo TLS e heaps). Memory blocks criados mas ainda não mapeados, e outros tipos de ResourceLimit, não são contabilizados ainda.
- As APIs validam o pseudo-handle do processo, o tipo do kernel handle, os ponteiros/alinhamento, a contagem assinada e a faixa completa dos buffers. Somente RESLIMIT_COMMIT (tipo 1) é atendido; os demais tipos falham explicitamente até existir accounting correspondente.
- svcBreak (0x3C) deixa de cair como SVC desconhecido: o runner para com um diagnóstico de breakpoint que preserva o motivo em r0, em vez de retornar sucesso fictício ou continuar após um panic/assert.
- Testes novos cobrem criação/fechamento/type-safety do ResourceLimit, consulta de limite/uso, rejeição de tipo/handle inválido e parada guest-side por svcBreak.
- Próximo marco: repetir o bootstrap SM64 com a mesma amostra local, confirmar que os valores COMMIT não são zero e que ControlMemory recebe tamanhos de heap alinhados e não nulos; depois implementar apenas o próximo SVC/serviço efetivamente observado.
- Nenhum arquivo de GPU, CPU/Thumb, frontend ou serviço HLE foi alterado nesta etapa. Alterações restritas a session/kernel.rs, session/svc.rs, session.rs e este handoff. Somente main; nenhuma branch criada.
## Kernel: GetThreadId — 25/11/2026

- Implementado SVC 0x37 GetThreadId: recebe ponteiro de saída em r0 e handle/pseudo-handle de thread em r1; grava o identificador interno guest-visible em memória, valida handle e endereço.
- O caminho reutiliza a resolução tipada do kernel, incluindo CURRENT_THREAD_HANDLE e handles regulares, sem confundir GetThreadId com GetProcessIdOfThread (0x36).
- Teste cobre thread atual, thread secundária e handle inválido. CI ainda precisa validar os commits.
 
## IPC: srv:IsServiceRegistered — 25/11/2026

- Implementado o comando IPC 0x000E00C0, com validação de RegisterClient, parâmetros, ausência de descriptors de tradução, comprimento/bytes do nome e ACL NCCH.
- A resposta reflete o registry HLE desta build: HID, CFG, APT, FS, PTM, DSP, CSND, serviços stub explícitos e gsp::Gpu são marcados registrados; serviços apenas autorizados pelo exheader, mas sem endpoint HLE (como http:C/ir:rst), retornam false.
- O comando não cria sessões nem simula conectividade. A ACL continua sendo aplicada também à consulta, e serviços sem endpoint não são anunciados como disponíveis.
- Teste adicionado para gsp::Gpu=true, http:C=false e ssl:C negado por ACL. CI pendente.

## Validação de orçamento ResourceLimit → ControlMemory — 25/11/2026

- Adicionado teste integrado ao Arm11ProcessSession: obtém ResourceLimit, consulta o teto e o COMMIT atual, calcula o orçamento restante alinhado a 4 KiB e usa uma página desse orçamento em ControlMemory(MEMOP_ALLOC).
- O teste verifica a criação real do mapeamento no heap de aplicação e confirma que a consulta de COMMIT atual sobe exatamente pelo tamanho mapeado. Isso é uma regressão sintética de ABI/kernel, não substitui reexecutar um CIA real nem prova o bootstrap de um jogo.
- CI e execução do teste ainda precisam ser confirmadas.
## Kernel/IPC: ciclo de portas do Service Manager — 25/11/2026

- O kernel agora possui objetos tipados Port e SessionEndpoint, com pares client/server, limite de sessões simultâneas, fila FIFO de endpoints servidores e encerramento de peer/contabilização da vaga ao fechar um dos lados. Handles duplicados de porta mantêm o nome registrado até o último alias fechar; fechar um cliente antes de AcceptSession descarta o endpoint servidor ainda pendente. O fechamento final da porta remove o nome do registry.
- Arm11KernelObjectTable e Arm11ProcessSession expõem registro/desregistro e consulta de portas, criação de pares de sessão e AcceptSession; a SVC 0x4A valida o ponteiro de saída, grava o handle servidor e retorna-o em r1.
- O dispatcher srv: implementa RegisterService, UnregisterService, GetServiceHandle e integração de IsServiceRegistered. A ACL é conferida antes de uma consulta; nomes permitidos mas ainda não registrados deixam a thread bloqueada em vez de receber falso erro imediato.
- System retém cada pedido pendente com thread, handle srv: de origem e nome. Quando RegisterService publica a porta, ou quando um endpoint fecha e libera um slot, o sistema cria a sessão cliente, grava a resposta na TLS da thread e somente então a acorda. As filas pendentes também são descartadas quando um novo processo substitui o anterior.
- Testes acrescentados no kernel, na SVC e no dispatcher para pareamento client/server, fila FIFO, limite e liberação de sessões, AcceptSession, Register/Unregister, IsServiceRegistered, GetServiceHandle pendente e despertar após registro/slot disponível; também cobrem porta duplicada e fechamento de cliente antes da aceitação do servidor.
- Limite desta etapa: o Aceris ainda usa um único processo/namespace de handles. Os endpoints client/server já têm identidade e ciclo de vida, mas não implementam o transporte genérico de mensagens de serviço por ReplyAndReceive (SVC 0x4B–0x4F); não anunciar comunicação arbitrária entre processos.
- Arquivos de código tocados: crates/aceris-core/src/loader/ncsd/session/kernel.rs, session.rs, session/svc.rs e crates/aceris-core/src/system.rs. Formatação, build, Clippy e testes ainda dependem da CI desta revisão; não declarar aprovação até verificar o workflow.


## PICA200 — tabelas de textura/iluminação e correções de CI (26/11/2026)

- Pesquisa registrada em `docs/INDEPENDENT_RESEARCH.md` e especificação tabular criada em `docs/AZAHAR_PICA_REGISTER_COMPATIBILITY.md`. A revisão consultada do Azahar foi `c621eafbf316699c55f4c9c0c689d4e49498d360`; os cabeçalhos individuais de cada arquivo foram conferidos. A consulta foi usada como referência de seletores e comportamento, sem copiar implementação, testes, shaders, fixtures ou assets.
- `PicaTextureType` nomeia os seletores de tipo 0–5 e preserva 6–7 como reservados; a política atual continua rejeitando cube/shadow.
- `PicaTextureWrap` decodifica todos os oito seletores S/T. Os modos 4/5 implementam a assimetria entre coordenadas negativas e não negativas; 6/7 são aliases de repeat. O limite (u/v=1) é forçado ao último texel apenas nos modos clamp-to-edge, permitindo que os modos border mantenham a cor de borda.
- `PicaLightingConfiguration` nomeia os códigos 0–6 e 8 (Config7 é codificada como 8); `PicaLightingSampler` e helpers de índice descrevem a matriz de disponibilidade e os oito slots por luz de spotlight/distance attenuation. Testes cobrem a matriz completa e os limites dos bancos.
- A CI anterior encontrou 30 diagnósticos Clippy `manual_is_multiple_of` em 12 arquivos do `aceris-core`. Foram aplicadas as sugestões exatas, compatíveis com `rust-version = 1.87`, sem suprimir lint.
- O loader dinâmico de `aceris-gl` foi dividido por plataforma: `dlopen/dlsym` em Unix e `LoadLibraryA/GetProcAddress` em Windows. Foi adicionado teste Windows para abrir `kernel32.dll` e resolver `GetCurrentProcessId`, removendo a referência POSIX que causava LNK2019 no MSVC.
- A dependência de workspace não utilizada `aceris-gl` foi removida de `[workspace.dependencies]`; o crate permanece membro do workspace. O README foi atualizado para Rust 1.87, dependências reais da camada de apresentação, estado headless do homebrew e validação Android ainda pendente.
- **Validação:** as alterações devem ser confirmadas pelo novo workflow de CI. Não declarar Rustfmt, build, Clippy ou testes aprovados até inspecionar os resultados. Nenhum console físico ou aparelho Android foi testado nesta etapa.
- **Fronteiras:** a autorização explícita do mantenedor permitiu alterações nos módulos GPU/PICA, loader, memória, CPU, `aceris-gl`, manifestos Cargo e documentação. Somente a branch `main` do Aceris foi usada.


## PICA200 — tabela do modo de operação de fragmento (26/11/2026)

- `PicaFragmentOperation::from_register` agora nomeia Default/Gas/Reserved/Shadow (0/1/2/3). Somente Default é executado; Gas e Shadow continuam pendentes e são rejeitados explicitamente.
- O blend equation selector 5–7 permanece alias de Add por evidência independente 3dbrew, apesar de o enum do Azahar listar as cinco equações principais 0–4. Não remover essa compatibilidade sem evidência adicional.
- RGB8 foi identificado no enum de framebuffer de Azahar, mas não possui combinação de pixel-size validada na descrição dos registradores PICA internos. Não habilitar por suposição; consultar a especificação/hardware antes.
- Referências e distinções de evidência foram adicionadas a `docs/AZAHAR_PICA_REGISTER_COMPATIBILITY.md` e `docs/INDEPENDENT_RESEARCH.md`.


## PICA200 — tabela de filtros procedurais e fallback de nível base (26/11/2026)

- `PicaProceduralTextureFilter::from_register` nomeia os seletores válidos 0–5 e preserva 6–7 como reservados; teste novo percorre a tabela inteira.
- Os quatro modos com mipmap deixam de ser rejeitados imediatamente: nearest mipmap modes usam nearest na LUT de cor do nível 0; linear mipmap modes usam a interpolação linear na LUT do nível 0.
- A mudança acompanha o fallback presente em `azahar/src/video_core/renderer_software/sw_proctex.cpp`, que declara TODO para mipmapping e agrupa os quatro modos por nearest/linear. É reconhecimento do modo e comportamento base-level, não implementação de mip LOD.
- Continuam pendentes seleção de nível via LOD, min/max LOD, offsets de mip 1–3 e bias superior. Seletores reservados 6–7 seguem fail-closed.
- A CI anterior mostrou testes/builds passando até Clippy nos três desktops; as falhas remanescentes eram import `c_char` não usado e Rustfmt em um `assert_eq!`. Ambos foram corrigidos nesta revisão; aguardar a nova CI antes de declarar sucesso.


## Kernel SVC — IDs nomeados e rastreabilidade (26/11/2026)

- Centralizados os IDs de SVC já encaminhados para address arbiter (0x21/0x22), GetSystemInfo (0x2A) e manutenção de data-cache (0x52/0x53/0x54) em `session/svc.rs`. O dispatcher deixou de usar literais para esses serviços.
- Teste novo valida os seis IDs e a delegação para o dispatcher que tem acesso a thread/endereço/estado de kernel.
- Referência independente: 3dbrew, tabela SVC (consulta 26/11/2026). A implementação de cada handler continua parcial onde a tabela pública não determina os valores exatos ou a semântica física.


## PICA200 — tabelas complementares de fog e framebuffer — 26/11/2026

- PicaFogMode agora classifica o campo de fog do GPUREG_TEXENV_UPDATE_BUFFER: 0 Disabled, 1 Linear, 5 Fog LUT, 7 Gas e os demais valores reservados. O caminho continua executando somente Disabled/Linear/Fog LUT; Gas é reconhecido e rejeitado explicitamente até existir rasterização validada.
- PicaColorBufferFormatSelector nomeia os códigos de formato 0–4 e reserva 5–7. RGB8 (1) é conhecido pelo enum de Azahar, mas permanece fora do render target por falta de confirmação do pixel-size/formato interno.
- Testes adicionados para os campos completos de três bits, incluindo os casos fail-closed. As novas tabelas não devem ser descritas como implementação de Gas ou RGB8.
- Referências consultadas: Azahar regs_texturing.h e regs_framebuffer.h, revisão c621eafbf316699c55f4c9c0c689d4e49498d360; 3dbrew GPU/Internal Registers. Nenhum código GPL ou teste do Azahar foi incorporado.


## PICA200 — snapshot de parâmetros Shadow — 26/11/2026

- Adicionado REG_FRAGOP_SHADOW (0x130) e PicaShadowState. O snapshot separa os dois campos float1.5.10 documentados: soma escala+bias da penumbra (bits 0–15) e escala com sinal invertido (bits 16–31).
- Teste próprio cobre a extração das duas metades. Os valores permanecem brutos, sem conversão numérica presumida.
- Isto não implementa a operação Shadow: o modo 3 de GPUREG_COLOR_OPERATION, shadow-map compare e composição da penumbra continuam pendentes.
- Fontes: Azahar regs_framebuffer.h, revisão c621eafbf316699c55f4c9c0c689d4e49498d360; 3dbrew GPU/Internal Registers.


## PICA200 — snapshot do plano de clipping — 26/11/2026

- `PicaClipPlaneState` foi adicionado para GPUREG_FRAGOP_CLIP (0x047) e quatro coeficientes em 0x048–0x04B. O bit de habilitação e os valores float1.7.16 de 24 bits são preservados em representação bruta.
- Teste próprio confirma a extração dos quatro coeficientes e do bit enable.
- O rasterizador ainda não aplica a equação de clipping, portanto isso é somente captura do estado do registrador.
- Fonte: 3dbrew GPU/Internal Registers.


## Kernel — encerramento do processo e comparação de arquitetura (26/11/2026, ChatGPT)

- Comparação de alto nível registrada em `docs/INDEPENDENT_RESEARCH.md` após consulta read-only ao Azahar, commit `6d6e846afa7b02f3dd85183a968b40cd1561f700`, com os limites de licença e não adaptação preservados. Foram usados também contratos públicos do 3dbrew para memória, SVC, processos e IPC.
- **Implementado em Rust original:** o caminho SVC ExitProcess agora chama a rotina de teardown da sessão. Todas as threads ainda vivas são terminadas pelo mesmo caminho central usado por ExitThread, incluindo liberação de TLS, remoção de esperas/handoffs, devolução de mutexes e sinalização de término. Depois são limpas as filas de deadline/arbiter/wait-all, a tabela de handles/objetos e as associações de interrupção; contextos não-main terminados são recuperados.
- O contexto principal encerrado é mantido para diagnóstico do estado final. A session/address-space ainda fica retida pelo `System` até descarregamento ou substituição do título; este passo não reivindica liberação imediata de todos os backings de heap/linear nem múltiplos processos simultâneos.
- Teste novo: término com uma thread secundária, TLS, handle de thread, evento e binding de IRQ; verifica que não restam threads executáveis, handles ou bindings do processo.
- Arquivos de código: `crates/aceris-core/src/loader/ncsd/session.rs` e `crates/aceris-core/src/loader/ncsd/session/thread.rs`. Arquivos de pesquisa/registro: `docs/INDEPENDENT_RESEARCH.md`, `docs/AI_HANDOFF.md` e `docs/ROADMAP.md`.
- **Validação pendente:** não declarar fmt/build/Clippy/testes aprovados até a nova CI deste commit terminar. O HEAD anterior `889126b361504a64cf09771a019ebfeb82a167b8` tinha CI aprovada em Linux, Windows, macOS e Android ARM64.
- Pendências não cobertas nesta etapa: RESERVE/MAP/UNMAP/PROTECT completos no ControlMemory; accounting de todos os tipos de ResourceLimit; OpenProcess/OpenThread e múltiplos processos com namespaces separados; ReplyAndReceive 0x4F e transferência genérica de mensagens/handles/buffers; serviço HLE somente deve ser ampliado a partir de traces de inicialização reproduzíveis.


## Kernel — ABI GetSystemInfo e contabilidade FCRAM linear (26/11/2026, ChatGPT)

- Corrigido o contrato de SVC 0x2A: r0 recebe o ponteiro de saída s64, r1 o tipo, r2 o parâmetro; o resultado da syscall é escrito em r0. Antes, o Aceris retornava zero nos registradores r1:r2 sem preencher o buffer, o que podia fazer libctru/jogos enxergarem a memória APP como totalmente livre.
- Suporte intencionalmente limitado a `SystemInfoType=0,param=1` (uso da região APPLICATION), que é o contador necessário para sizing do heap. Queries agregadas, SYSTEM/BASE e memória exclusiva do kernel retornam InvalidEnum até existir accounting real para esses pools; não retornar zero fictício.
- Valor da APP: segmentos estáticos mapeados + regiões dinâmicas (heap/TLS) + ranges físicos lineares únicos. Adicionado `linear_physical_allocated_bytes` ao `Arm11MemoryUsageReport`, unindo intervalos físicos sobrepostos para impedir dupla contagem dos aliases linear antigos/novos. Mapeamentos shared continuam fora deste contador para evitar somar views/ownership sem contabilidade física.
- Testes adicionados para ponteiro de saída, bytes calculados, endereço inválido, enum ainda não modelado e deduplicação de aliases físicos.
- Mudanças de código: `crates/aceris-core/src/loader/ncsd/session.rs` e `crates/aceris-core/src/loader/ncsd/process.rs`. CI da revisão anterior `16510ddba6e05a9597e14adb820fa1b51edf8032` estava sendo executada ao registrar este handoff; confirmar todos os jobs antes de declarar validação final.
- Limites mantidos: sem accounting completo de SYSTEM/BASE, FCRAM do kernel, KSharedMemory externo, nem todas as classes de ResourceLimit. Isso não completa todo o gerenciador de memória.


## Kernel — GetProcessInfo: threads vivas (26/11/2026, ChatGPT)

- Adicionado `Arm11ThreadScheduler::live_thread_count()`, que conta apenas contextos ainda não encerrados, em contraste com `thread_count()` que inclui contextos preservados para handles/diagnóstico.
- `SVC GetProcessInfo` agora suporta type 7 com a contagem viva; types 6 e 8 devolvem zero, conforme os campos documentados do kernel 3DS. Types 0–3 e 9–18 permanecem rejeitados até existir contabilidade/semântica real.
- Teste verifica transição de 2 threads vivas (main + worker) para 1 após o worker ser terminado.
- Referência pública: https://www.3dbrew.org/wiki/SVC. Implementação independente; sem cópia/port de Azahar.


## Kernel — GetResourceLimitCurrentValues para objetos de sincronização (26/11/2026, ChatGPT)

- Adicionado Arm11KernelObjectTable::synchronization_resource_count para EVENT=3, MUTEX=4, SEMAPHORE=5 e TIMER=6. Conta objetos únicos no conjunto ativo + aposentado; aliases de handle não aumentam o contador e objetos retidos por owner/waiter continuam contabilizados.
- GetResourceLimitCurrentValues agora retorna THREAD=2 via scheduler (thread_count) e contadores 3–6 via tabela de objetos. GetResourceLimitLimitValues continua suportando apenas COMMIT=1; limites por classe dependem das categorias/limites do exheader e não foram estimados.
- Shared memory (7), address arbiter (8) e CPU time (9) permanecem pendentes por lacunas de lifetime/accounting. Ainda não alegar ResourceLimit completo.
- Testes novos em kernel table e sessão verificam contagens e aliases. Referências externas: libctru svc.h e 3dbrew KResourceLimit.


## CSND — mixer de canais e interpretação da lista de comandos (26/11/2026, Claude)

- Novo módulo `crates/aceris-core/src/csnd.rs` (Rust original): 32 canais com PCM8, PCM16 e IMA-ADPCM lidos da memória física, timer de 16 bits (clock = 2048× a taxa de saída de ~32728 Hz), interpolação linear, modos de loop (manual, repetição, one-shot, repetição com tamanho travado), hold, recarga de estado ADPCM no loop, volume por lado, volume mestre e fades de volume para SetPlayState/SetPlayStateRaw.
- `csnd:SND` agora interpreta de fato `ExecuteCommands`: percorre a lista encadeada de comandos de 0x20 bytes no bloco compartilhado (com limite contra listas cíclicas), executa comandos de canal (0x000–0x010), de captura (0x100–0x106, só registradores), ApplyMasks (0x200) e UpdateState (0x300), marca `finished` e publica o estado por canal. Antes da leitura, o cache de dados do guest sobre o bloco é escrito de volta (`flush_shared_backing_cache`, novo); depois, as linhas são invalidadas.
- Initialize guarda os quatro offsets; AcquireCapUnit/ReleaseCapUnit alocam de verdade as duas unidades; Flush/Store/InvalidateDataCache validam a faixa e operam o cache do processo; Reset para todos os canais; Shutdown preserva canais e máscaras, como no hardware.
- Saída: `System::run_csnd` renderiza ao fim de cada fatia de frame; `take_audio_samples` soma CSND à saída do DSP (ou entrega só CSND quando o DSP não está rodando). Fila limitada a ~0,5 s.
- Testes: 10 unitários em `csnd.rs` e 1 ponta a ponta em `system.rs` (comando escrito pelo guest via cache → IPC → flag `finished` e estado visíveis ao guest → amostras audíveis em `take_audio_samples`).
- Limites: PSG/ruído produzem silêncio; captura não grava amostras; a renderização acontece no fim da fatia, então um som iniciado e parado dentro da mesma fatia pode não soar; cadência do fade é aproximada. Ainda não validado com o Cookie Clicker nem com áudio audível em hardware.
- Referências: 3dbrew "CSND Services"/"CSND Registers"; libctru `csnd.h`; Azahar (GPLv3) `src/audio_core/csnd.{h,cpp}` e `src/core/hle/service/csnd/csnd_snd.cpp`, revisão `6d6e846afa7b02f3dd85183a968b40cd1561f700`, consulta somente leitura em 26/11/2026. Nenhum código foi copiado ou portado; a estrutura do Aceris é própria.


## Início da implementação da experiência do usuário — biblioteca CIA (26/11/2026, ChatGPT)

- Carlos Costa pediu o início da implementação do plano `docs/USER_EXPERIENCE.md`. A primeira entrega é deliberadamente pequena e focada na Etapa A: expor na biblioteca os contêineres `.cia` que o carregador já aceita nas condições documentadas.
- `crates/aceris-core/src/video/library.rs` agora inclui a extensão `.cia` na varredura compartilhada por desktop e Android. Os rótulos de formatos da biblioteca foram atualizados, e o teste de varredura verifica a extensão sem distinção entre maiúsculas/minúsculas.
- `docs/FRONTENDS.md` e `docs/USER_EXPERIENCE.md` esclarecem que listar um CIA não implementa instalação persistente, gerenciamento de atualização nem descriptografia: o carregamento continua limitado a conteúdo de boot em claro e NCCH NoCrypto nas condições já suportadas.
- Nenhum jogo/CIA, firmware, chave ou dado extraído foi incluído no repositório. Nenhuma dependência foi adicionada.
- **Validação:** a mudança precisa passar pelo CI do novo commit. Não declarar Rustfmt, build, Clippy ou testes aprovados até consultar os workflows correspondentes.
- **Fronteiras:** alteração limitada à varredura/renderização de formatos da biblioteca e documentação de experiência; não altera `app.rs`, `native_activity.rs`, CPU, MMU, kernel, serviços, GPU/PICA, DSP ou CSND.


## Biblioteca — alternância Grade/Lista (26/11/2026, ChatGPT)

- A pedido de Carlos, a biblioteca horizontal recebeu uma opção funcional entre visualização em grade (padrão) e lista nas páginas Jogos, Favoritos, Recentes e Homebrew.
- A alternância está no cabeçalho, mantém seleção coerente ao trocar de modo e atualiza a navegação: no modo Lista, cima/baixo percorre um título por vez e esquerda/direita não desloca a seleção; no modo Grade, o comportamento de três colunas permanece.
- O hit-test da Lista corresponde às linhas efetivamente renderizadas. O controle de favorito da linha é independente da ação de selecionar/abrir o título; os alvos permanecem limitados ao cartão visível.
- O fallback de retrato continua sendo uma lista adaptável automática. A escolha Grade/Lista é mantida somente durante a sessão e ainda não é persistida.
- Foram adicionados testes para alternância pelo cabeçalho, navegação vertical/horizontal e seleção/ativação/favoritos por toque.
- Alteração de código restrita a `crates/aceris-core/src/video/library.rs`; documentação atualizada em `docs/USER_EXPERIENCE.md` e `docs/FRONTENDS.md`. Nenhum frontend de outro agente, core de emulação, dependência ou asset foi alterado. Nenhum dado de jogo/firmware/chave foi adicionado.
- **Validação:** os testes e o lint desta revisão precisam ser confirmados pela CI do commit. Não declarar aprovação antes de consultar os workflows.
- **Fronteiras:** somente a branch `main`; nenhuma branch ou PR foi criada.


## Biblioteca — pesquisa com entrada de texto real (26/11/2026, ChatGPT)

- A biblioteca filtra em memória por título, nome de arquivo e caminho, ignorando caixa e diacríticos comuns do português. A busca não é recursiva e não consulta Title ID nem metadados externos.
- O campo aparece nas páginas Jogos, Favoritos, Recentes e Homebrew. A seleção/scroll reinicia quando a consulta muda; Recentes preserva a ordem de execução.
- Android: `NativeActivity` encaminha Unicode do NDK para teclados físicos. Para toque, o renderer Rust oferece teclado virtual básico com letras latinas, espaço, apagar, limpar e OK. Back, Enter e Backspace respeitam o foco; não é um IME completo.
- Desktop: X11 usa `XLookupString`, Windows recebe `WM_CHAR` após `TranslateMessage`, e macOS lê `NSString.characters` como UTF-8. Escape fecha o foco de pesquisa antes de sair; Enter encerra a entrada e ativa o item selecionado.
- Testes adicionados no core para busca por título/caminho sem acento e fluxo do teclado touch; o frontend Android testa entrada física, Backspace e Enter.
- **Fronteiras:** alteração limitada à biblioteca do core, entrada de texto NativeActivity/App Android, eventos compartilhados de janela, adaptadores X11/Win32/AppKit, loop do launcher desktop e documentação. Nenhum módulo de CPU, MMU, kernel, GPU/PICA, DSP ou CSND foi tocado. Nenhuma dependência, asset ou conteúdo de jogo foi adicionado.
- **Validação:** Rustfmt, builds, Clippy, testes Linux/Windows/macOS e Android ARM64/APK precisam ser confirmados nos novos workflows. Teste em Android físico e validação visual/teclado em desktops ainda são necessários.
- **Fronteiras:** somente `main`; nenhuma branch ou PR foi criada.


## Biblioteca — base do scanner para várias pastas e recursão opcional (26/11/2026, ChatGPT)

- Adicionada a API Rust `ScanOptions` e `scan_games_from_directories(&[PathBuf], ScanOptions)` em `crates/aceris-core/src/video/library.rs`.
- A API recebe uma lista de raízes, aceita recursão opcional (desligada por padrão), deduplica diretórios/arquivos quando as raízes se sobrepõem, ordena deterministicamente e ignora raízes ausentes ou ilegíveis sem descartar as demais.
- Diretórios simbólicos descobertos durante a travessia não são seguidos. A profundidade recursiva é limitada a 64 níveis para evitar percursos patológicos. O scanner continua apenas identificando extensões candidatas (.3ds/.cci/.cia/.3dsx), sem validar conteúdo ou carregar jogos.
- `scan_games(&Path)` permanece como wrapper de compatibilidade não recursivo, portanto os frontends continuam com o comportamento anterior nesta etapa. A interface ainda não oferece adicionar/remover pastas, alternar recursão ou escolher diretórios via SAF no Android.
- Testes incluídos: padrão não recursivo, varredura de várias raízes sobrepostas sem duplicatas, raiz ausente não cancela as demais e links simbólicos de diretório não são seguidos (Unix).
- Documentação atualizada em `docs/FRONTENDS.md` e `docs/USER_EXPERIENCE.md`; removidos bullets duplicados sobre a pesquisa nas mesmas páginas.
- **Fronteiras:** somente scanner compartilhado de biblioteca e documentação. Nenhuma mudança em frontends Android/desktop ou nos módulos de emulação. Nenhuma dependência, asset, ROM, CIA, firmware ou chave adicionada. Somente `main`; sem branches ou PRs.
- **Validação:** executar Rustfmt, build, testes e Clippy da CI deste commit antes de marcar esta base como aprovada.


## Biblioteca — identidade de Favoritos e Recentes por caminho (26/11/2026, ChatGPT)

- Corrigida a identidade persistida dos jogos: antes, Favorites/Recent armazenavam somente o nome do arquivo, o que confundia dois títulos diferentes chamados, por exemplo, `game.3ds` em pastas distintas.
- Novas entradas usam identificador versionado `path-v1:` derivado dos bytes UTF-8 do caminho completo e codificado em hexadecimal (seguro para o armazenamento linha a linha). Não é criptografia; é apenas codificação estável.
- Na inicialização, registros antigos que continham apenas o nome são migrados como caminhos relativos à pasta única original da biblioteca. Assim, o registro legado não passa a marcar por engano um arquivo homônimo que esteja em outra pasta. Favoritos e Recentes já usam o novo ID para leitura, ordenação, contagem, seleção da Home e persistência.
- A migração é escrita imediatamente quando o arquivo antigo precisa ser convertido. IDs repetidos são deduplicados sem alterar a ordem de Recentes.
- Testes cobrem dois arquivos com mesmo nome em raízes diferentes, persistência após reabrir a biblioteca e migração de registros legados apontando para a pasta original.
- Limite explícito: caminhos não UTF-8 não recebem identidade persistente nesta versão, seguindo a limitação já existente da antiga lógica que exigia nome de arquivo UTF-8. Os frontends ainda usam uma única pasta; o trabalho prepara a identidade para múltiplas raízes, mas não habilita essa UI.
- **Fronteiras:** somente `crates/aceris-core/src/video/library.rs` e documentação. Sem mudanças nos frontends, módulos de emulação ou dependências. Sem assets/conteúdo de jogos, firmware ou chaves. Somente `main`, sem branch ou PR.
- **Validação:** confirmar Rustfmt, build, testes e Clippy nos workflows gerados para este commit; teste físico Android/desktop permanece necessário.


## Biblioteca — persistência da configuração do scanner (26/11/2026, ChatGPT)

- Adicionada `LibraryScanConfig`, contendo lista ordenada de pastas e `ScanOptions`. Inclui operações de adicionar/remover raiz, bloqueia duplicações (incluindo caminhos já canonicalizáveis) e impede remover a última pasta.
- Adicionadas `load_library_scan_config` e `save_library_scan_config`. O arquivo versionado `.aceris/library_scan_v1.txt` é gravado no diretório de configurações explicitamente fornecido pelo host; assim desktop/Android poderão escolher uma área de dados gravável apropriada. Cada caminho absoluto UTF-8 é codificado em hexadecimal, tornando a representação segura para linhas e nomes contendo espaços, acentos ou sinais de igual.
- Configuração ausente, versão desconhecida ou conteúdo inválido retorna à pasta original informada pelo host com recursão desativada. Salvar lista vazia é erro `InvalidInput`; caminhos não UTF-8 também são rejeitados explicitamente em vez de sofrer truncamento.
- Adicionado `scan_games_from_config`, que aplica a configuração persistida à API multi-raiz já existente.
- Testes cobrem round-trip de duas pastas + recursão, deduplicação/remoção com proteção da última raiz, fallback após arquivo inválido e rejeição de lista vazia.
- **Importante:** esta etapa é infraestrutura do core, não habilita ainda a configuração na interface. Desktop e Android continuam chamando o scanner de uma única pasta; SAF/content URIs não são convertidos em `PathBuf` e exigem integração Android separada. A gravação recebe um diretório de configurações do host para não pressupor que todas as raízes sejam graváveis.
- **Fronteiras:** somente `crates/aceris-core/src/video/library.rs` e documentação. Sem mudanças nos frontends, módulos de emulação ou dependências. Sem assets/conteúdo de jogos, firmware ou chaves. Somente `main`, sem branch ou PR.
- **Validação:** executar Rustfmt, build, testes e Clippy na CI deste commit; nenhum teste físico de seletor de pastas foi feito.


## Biblioteca — integração do scanner configurável ao modelo Library (26/11/2026, ChatGPT)

- `Library` agora mantém `LibraryScanConfig` e o diretório gravável onde a configuração deve ser persistida. O construtor antigo `Library::new(entries, directory_label)` permanece compatível e começa com uma única raiz, sem recursão.
- Novo construtor `Library::from_scan_settings(settings_directory, fallback_directory)` lê a configuração versionada e monta a lista inicial varrendo todas as raízes configuradas. A pasta fallback continua usada para recuperar configuração ausente/inválida e para compatibilidade dos metadados de biblioteca já existentes.
- Adicionados os métodos `scan_config()`, `add_scan_directory`, `remove_scan_directory` e `set_recursive_scan`. As operações salvam o novo arquivo primeiro; somente depois de escrita bem-sucedida substituem o estado em memória e atualizam os jogos. Falha de escrita não altera configuração nem catálogo ativos. Remoção da última raiz continua proibida.
- `Library::reload()` agora percorre a configuração atual em vez de chamar diretamente o scanner de uma única pasta.
- Testes cobrem reabertura com múltiplas raízes, ativação/desativação de recursão, adição/remoção, duplicação, proteção da última raiz e falha de persistência sem alteração do estado vivo.
- **Importante:** desktop e Android ainda usam o construtor legado, logo o comportamento visível permanece em uma única pasta até a próxima integração dos frontends e dos seletores de pasta. SAF/content URIs continuam fora desta implementação.
- **Fronteiras:** somente `crates/aceris-core/src/video/library.rs` e documentação. Sem alterações no código dos frontends, módulos de emulação, dependências ou assets. Sem dados de jogos, firmware ou chaves. Somente `main`, sem branch/PR.
- **Validação:** executar Rustfmt, build, testes e Clippy na CI gerada pelo commit.


## Frontends — carregar configuração persistente da biblioteca (26/11/2026, ChatGPT)

- Desktop e Android deixaram de inicializar a lista exclusivamente com scan_games(games_dir)/Library::new; ambos usam Library::from_scan_settings com o diretório de configurações <games_dir>/.aceris.
- A configuração .aceris/library_scan_v1.txt passa a ser respeitada pelos dois hosts: múltiplas raízes já salvas e recursão opcional são aplicadas na abertura; configuração ausente/inválida retorna à pasta original com recursão desativada.
- Adicionado teste do App Android que grava duas raízes, ativa recursão e verifica que a inicialização recupera os dois títulos.
- A alteração em frontends/android/src/app.rs ficou limitada ao construtor App::new e a um teste novo. Os métodos de estado de mídia existentes foram preservados sem alteração. O desktop teve somente o import e o inicializador da biblioteca ajustados.
- Limite restante: ainda não há controles visuais para adicionar/remover pastas ou alterar a recursão; Android segue sem seleção SAF e URIs SAF não são tratadas como caminhos de arquivo. A mudança conecta a configuração já persistente, mas não cria o fluxo de seleção de pastas.
- Validação: aguardar CI do commit. Nenhum teste/build local foi executado nesta sessão; não declarar sucesso antes dos workflows.
- Fronteiras: somente main; sem branches ou PRs. Nenhum módulo de emulação, dependência, asset, ROM, CIA, firmware ou chave foi alterado/adicionado.


## Biblioteca — controles para gerenciar pastas na página Configurações (26/11/2026, ChatGPT)

- A página Configurações lista as raízes de varredura (até três linhas visíveis, com indicação de raízes adicionais), permite remover uma raiz existente e alternar a busca recursiva.
- O fluxo Adicionar pasta abre um campo de caminho que aceita texto do teclado físico ou teclado virtual. A confirmação verifica que o caminho aponta para um diretório existente antes de persistir a nova raiz; caminhos duplicados e falhas de gravação geram diagnóstico sem trocar o catálogo ativo.
- O teclado virtual da biblioteca oferece os caracteres barra e ponto na terceira fileira para caminhos comuns no Android. O caminho precisa estar acessível à aplicação.
- Desktop: Enter confirma o caminho quando o campo está focado, em vez de executar a atualização genérica da biblioteca.
- Testes novos cobrem inclusão por caminho, busca recursiva, remoção, proteção da última raiz e caracteres de caminho no teclado virtual.
- Limite restante: não há seletor nativo de pastas para desktop e o Android ainda precisa de integração Storage Access Framework (SAF); digitar um caminho não contorna as restrições de armazenamento do Android moderno.
- Validação: nenhum teste/build local foi executado nesta sessão; validar Rustfmt, build, testes e Clippy pela CI do commit. Teste físico Android continua pendente.
- Fronteiras: somente main; sem branches ou PRs. Código do produto permanece Rust, sem dependências, assets, jogos, CIA, firmware ou chaves adicionados.


## Biblioteca — correção de Rustfmt e vida útil do rótulo do caminho (26/11/2026, ChatGPT)

- A CI de `0d31efc` revelou diferenças de Rustfmt no painel de Configurações e um erro E0716 no texto temporário enviado ao renderer Android ARM64.
- O rótulo de caminho agora é construído em uma variável `String` antes de ser passado a `responsive_text`, corrigindo a vida útil da referência.
- Os blocos de Configurações, teclado virtual, hit-test e fixtures dos testes foram alinhados ao formato do workspace. Nova CI ainda deve confirmar a validação.
- Nenhum subsistema de emulação, dependência ou arquivo de outro agente foi alterado.


## Correção da ramificação da renderização vertical (26/11/2026, ChatGPT)

- A CI da revisão anterior revelou que a substituição da seção Configurações havia removido o token `else` que inicia a renderização das páginas de jogos no modo vertical. O delimitador foi restaurado; o erro era sintático e bloqueava Rustfmt e os builds.
- A CI da revisão atual deve confirmar a análise sintática, o formato e o build Android. Ainda não declarar testes aprovados antes dos resultados.


## Ajuste de expectativa do teste da biblioteca (26/11/2026, ChatGPT)

- O teste da interface alterna explicitamente a varredura recursiva para ligada antes de adicionar/remover uma raiz. A remoção mantém a opção global, portanto o teste agora verifica que ela permanece ligada depois de reabrir a configuração.


## Rustfmt — alinhamento final do fluxo de pastas (26/11/2026, ChatGPT)

- Os quatro trechos apontados pelo verificador de formatação foram ajustados ao estilo exato do workspace: braços de `match` e as duas tabelas do teclado virtual.
- Aguardando nova CI para confirmar formatação, compilação multiplataforma e APK Android.


## Compatibilidade do botão de atualização em paisagem (26/11/2026, ChatGPT)

- A suíte apontou que o teste antigo usava o retângulo do botão local de atualização removido do painel paisagem. A atualização agora é a ação compartilhada do cabeçalho, no canto superior direito; o teste foi movido para esse alvo visível.
- A atualização continua acessível em Configurações nos dois layouts: botão local no painel vertical e ação global no cabeçalho paisagem.



## Biblioteca — filtro por formato de arquivo (26/11/2026, ChatGPT)

- Adicionado ao modelo Rust compartilhado `LibraryFormatFilter`, com os estados Todos, .3DS, .CCI, .CIA e .3DSX. A filtragem compara a extensão do nome do arquivo sem diferenciar maiúsculas/minúsculas e funciona em Jogos, Favoritos, Recentes e Homebrew.
- O filtro aparece como controle compacto nas interfaces responsivas em retrato e paisagem; a troca redefine a seleção para evitar índice inválido ou seleção inesperada.
- Estados vazios explicam quando não existem arquivos para a extensão escolhida.
- O filtro só classifica extensões já reconhecidas pelo scanner. Não valida o conteúdo dos contêineres, não altera o loader e não transforma a presença de .CIA em instalação persistente ou gerenciável.
- Testes adicionados para ciclo de filtros, comparação case-insensitive, interação nos dois layouts e manutenção do filtro na página Homebrew.
- Fronteiras: somente `crates/aceris-core/src/video/library.rs` e documentação da experiência/frontends. Nenhum subsistema de emulação, dependência, asset ou conteúdo de jogo foi alterado/adicionado. Trabalho restrito à branch `main`.
- Validação: a primeira CI apontou somente diferenças de Rustfmt, confirmadas nos logs de Linux/macOS; os trechos foram alinhados ao formato emitido pelo próprio verificador. A nova CI precisa executar formatação, builds, testes e Clippy antes de marcar a etapa como aprovada.

## Desempenho — memória rápida e despacho rápido (27/11/2026, Claude)

- O modelo preciso de D-cache continua sendo o padrão. A memória rápida é opcional, via `System::set_fast_memory(true)`, destinada ao preset "Desempenho" do `PERFORMANCE_LOW_END_PLAN.md`. O despacho rápido preserva a semântica e fica ligado por padrão.
- `Arm11Core::run_fast` e `Arm11ProcessSession::run_fast_chunk` implementam o laço rápido; `set_fast_dispatch(false)` força o passo preciso para diagnóstico. A variável `ACERIS_WATCH_PC` também força o passo preciso.
- Sonda: de ~20 para ~36 M instruções/s no padrão e ~41 M com memória rápida. Detalhes e próximos passos em `docs/PERFORMANCE_TESTING.md`.

## Desempenho — despacho em lote com MMU ativa (27/11/2026, ChatGPT)

- A sessão do ARM11 agora usa `Arm11Core::run_fast_translated` quando `SCTLR.M` está ativa. O lote segura um `MmuBus` e a TLB durante várias instruções, evitando reconstruir o adaptador e retirar/restaurar a TLB a cada instrução.
- A tradução virtual→física e a checagem de permissões continuam ocorrendo em cada fetch e acesso de dados. O lote atualiza a condição de privilégio após mudanças de modo; CP15, eventos e acessos ao espaço de registradores GIC/timer/SCU entregam o controle ao caminho preciso.
- `MmuBus` encaminha `take_interrupt_state_touched` ao barramento real para que um acesso de dispositivo encerre o lote no ponto arquitetural correto.
- Testes diferenciais em `crates/aceris-core/src/cpu/mmu.rs`: bloco ARM com `STR/LDR` traduzidos, bloco Thumb, retorno em SVC e handoff antes de MCR CP15. O caso de CP15 inicializa `r1` com ACTLR.SMP para testar a alteração real.
- Arquivos de produto tocados: `crates/aceris-core/src/cpu/mod.rs` e `crates/aceris-core/src/cpu/mmu.rs`. Nenhum código externo, dependência, ROM, firmware, chave ou asset foi adicionado; trabalho feito diretamente em `main`.
- **Desempenho ainda não medido para SCTLR.M ativo.** A sonda `session_loop_throughput` usa `Arm11Core` com `SCTLR.M` no valor de reset desligado; os antigos resultados de −32%/−39% são do caminho sintético sem MMU e não podem ser atribuídos a esta otimização.
- Validação da revisão `5ed21ef`: CI `45935246924` concluída com sucesso em Ubuntu, Windows, macOS e Android arm64-v8a; `cargo fmt`, builds, suíte completa, Clippy e artefatos Android aprovados.
- A CI passou a executar a sonda ignorada explicitamente em release no Ubuntu. Resultados: preciso 6,14 M instruções guest/s em todas as rodadas; lote traduzido 16,71 / 16,89 / 16,90 M instruções guest/s; speedup 2,72× / 2,75× / 2,75× (mediana 2,75×).
- A medição é isolada em `FlatMemory`; não representa o caminho completo `Arm11ProcessSession`, gameplay ou dispositivo Android. Os dados anteriores −32%/−39% continuam ligados à sonda sem MMU e não devem ser misturados com estes valores.
- Próximo passo: montar uma sonda de sessão completa com MMU ativa; só então avaliar cache de blocos ARM/Thumb pré-decodificados com invalidação segura de código auto-modificável e aliases executáveis.

## Sonda de desempenho isolada — MMU ativa (27/11/2026, ChatGPT)

- Adicionado o teste ignorado `cpu::mmu::tests::translated_fast_dispatch_throughput` em `crates/aceris-core/src/cpu/mmu.rs`. Ele compara `step` versus `run_fast` com `SCTLR.M=1`, uma seção traduzida e laço ARM load/store/ALU, validando estado final e memória.
- Comando release: `cargo test --release -p aceris-core --lib translated_fast_dispatch_throughput -- --ignored --nocapture`. Padrão: 1M instruções × 3 rodadas; ajustar com `ACERIS_MMU_PROBE_INSTRUCTIONS` e `ACERIS_MMU_PROBE_ROUNDS`.
- A sonda é deliberadamente isolada em `FlatMemory`: mede o núcleo CPU/MMU, não o caminho completo de `Arm11ProcessSession`, D-cache do processo, periféricos nem GPU. O CI executa este teste ignorado em release apenas no runner Ubuntu, como etapa dedicada.
- Resultado 27/11/2026: 6,14 M guest instr/s no passo preciso; 16,71 / 16,89 / 16,90 M no lote; 2,72× / 2,75× / 2,75×. Runner CI, não hardware local nem aparelho Android.
- Próximo passo: executar uma sonda equivalente através de `Arm11ProcessSession` com MMU ativa e, quando disponível, em Android ARM64 físico; comparar resultados antes de avançar para cache de blocos pré-decodificados.
## Referência Black for Android aplicada ao plano Aceris (27/11/2026, ChatGPT)

- Fonte pública consultada: `https://github.com/RanggaCasper/Black-Android/releases` e README; latest consultada v1.8.1. O port recompila estaticamente o XBE x86 específico de BLACK para C++/ARM64, usa Vulkan, SDL3, escala de saída 20–200% (67% padrão), opções V-Sync/FPS, contador, filtering, configuração persistente e shader cache. Requisitos declarados: Android 10+, ARM64, Vulkan 1.1, 4 GB RAM e 2 GB de storage.
- Não copiar código, shaders, assets, recompiler, estruturas ou dependências. A recompilação estática por título não substitui o intérprete/JIT ARM11/Thumb genérico do Aceris.
- O presenter atual do Aceris é `wgpu` com Vulkan/GL/ES, fallback CPU, `PowerPreference::HighPerformance`, `FIFO` quando disponível e dois pipelines simples de composição. Ele recebe LCDs já renderizados pelo PICA em CPU; não é um rasterizador PICA200 em GPU.
- Adaptação: futura escala apenas da composição host (nunca alterar buffer LCD/temporização guest); perfis Android estruturados/persistentes; FPS/frametime e backend visíveis; cache PICA de pipeline/shader apenas quando o renderer PICA em GPU existir; teste de V-Sync/power preference por aparelho; ajuste térmico somente em qualidade/trabalho host opcional.
- Não copiar o default 67%, V-Sync desligado, FPS ilimitado ou requisitos de 4 GB como política Aceris. O port BLACK tem carga, loop de jogo e arquitetura diferentes.
- Plano e critérios adicionados a `docs/PERFORMANCE_LOW_END_PLAN.md`, `docs/GRAPHICS_BACKENDS.md` e `docs/ROADMAP.md`. Nenhum código de produto foi modificado nesta rodada.
## Android — telemetria host opt-in para FPS e frametime (27/11/2026, ChatGPT)

- Novo módulo `frontends/android/src/performance.rs`: janela fixa de até 300 amostras; relatório periódico a cada cinco segundos; FPS de apresentações concluídas; duração de render host média/p50/p95; p95 de `App::step_into`; p95 da apresentação; intervalo p50/p95 entre apresentações concluídas; proporção de apresentação GPU; contagem de frames não apresentados.
- O monitor é incluído para Android, `aceris_typecheck_native` e builds de teste. Testes cobrem percentis, janela móvel, separação entre app step/presenter e ciclo de relatório.
- `frontends/android/src/native_activity.rs` registra tempos de `App::step_into`, `GraphicsPresenter::present_rgba` ou fallback `ANativeWindow`, além dos intervalos entre apresentações. A coleta é desativada por padrão; ativa-se na inicialização quando `games/.aceris/android_performance_log_v1.txt` contém exatamente `1`.
- Ativação e interpretação: ver `docs/ANDROID_DEVICE_TRIAL.md`. Comando de filtro: `adb logcat | grep "Aceris Android performance"` (PowerShell usa `findstr`). Remover o arquivo e reiniciar para desativar.
- Os valores são host-side: `App::step_into` inclui emulação e composição, não representa tempo puro de CPU guest. `presented_fps` não é velocidade guest. Intervalos entre apresentações acima de dois segundos são ignorados para não misturar pausa/retomada ao ritmo normal.
- Nenhum ajuste altera temporização guest, timers, interrupções, orçamento de instruções, dimensões/stride das LCDs, áudio ou conteúdo de jogos. Sem novas dependências e sem mudanças no `app.rs`/`library.rs` da implementação de memória rápida por Claude.
- CI e compilação ARM64 precisam confirmar esta revisão; nenhum aparelho físico foi testado nesta rodada. O monitor não mede ainda temperatura, bateria, ADPF ou custo de cada estágio PICA.
## Desktop — telemetria host opt-in para FPS e frametime (27/11/2026, ChatGPT)

- Novo módulo `frontends/desktop/src/performance.rs`: janela de até 300 amostras; relatório a cada cinco segundos; modos independentes `intro`, `library` e `game`; FPS de apresentações concluídas; render host média/p50/p95; p95 de frame do sistema, composição/escala, áudio e apresentação; intervalo de apresentação p50/p95; proporção de presenter GPU.
- `frontends/desktop/src/main.rs` mede a execução `run_frame_with_diagnostics`, `AudioOutput::feed`, `compose_bgrx_into` e `present_desktop_frame`. Se o presenter `wgpu` falhar e o caminho CPU assumir a apresentação, a amostra conta como CPU. O monitor recebe métricas de cada quadro bem-sucedido.
- Habilitação desativada por padrão: `ACERIS_PERFORMANCE_LOG=1` ou `games/.aceris/desktop_performance_log_v1.txt` com exatamente `1`. Reiniciar a janela após alterar a configuração. Ver `docs/PERFORMANCE_TESTING.md` para comandos Linux/macOS e PowerShell.
- `presented_fps` é a taxa host de apresentações, não a velocidade guest. `emulation_ms_p95` cronometra a chamada inteira `run_frame_with_diagnostics`; `composition_ms_p95` mede o buffer final CPU, não o rasterizador PICA200. `presentation_ms_p95` inclui a API GPU ou a janela CPU e pode refletir V-Sync/FIFO.
- Ao trocar intro/biblioteca/jogo, o monitor zera a janela para não misturar cargas diferentes. Lacunas entre apresentações acima de dois segundos são descartadas. A coleta não muda relógio guest, timers, orçamento de instruções, sincronização, áudio, frame pacing ou dimensões dos buffers LCD.
- Nenhuma dependência ou recurso externo foi adicionado. Arquivos de produto nesta etapa: `frontends/desktop/src/performance.rs` e `frontends/desktop/src/main.rs`; arquivos de documentação: `docs/PERFORMANCE_TESTING.md`, `docs/PERFORMANCE_LOW_END_PLAN.md`, `docs/ROADMAP.md` e esta nota.
- Pendências: CI do commit final deve confirmar Rustfmt, testes do módulo, builds de Linux/Windows/macOS e Clippy. Nenhum PC físico foi medido; não afirmar ganho de FPS. O Clippy Ubuntu do commit anterior `0f34b83` permanece bloqueado por `needless_range_loop` em `crates/aceris-core/src/cpu/decode_cache.rs`, arquivo do trabalho de Claude; não modificar sem autorização.

## Desempenho — especialização da composição host PC/Android (27/11/2026, ChatGPT)

- Desktop: `frontends/desktop/src/window.rs` ganhou um caminho especializado para escala host `1×`, com conversão RGBA→BGRX por linha. O caminho escalonado genérico permanece para escalas maiores; testes atuais validam posição, cores, gutter e replicação.
- Android: `frontends/android/src/native_activity.rs` usa cópia contígua única quando `ANativeWindow` tem dimensões e stride exatamente iguais ao frame; o fallback linha a linha continua para buffers com padding/recorte.
- A telemetria desktop foi ajustada para agrupar tempos em `DesktopFrameTimings`, removendo o aviso Clippy `too_many_arguments` da função `record`.
- Nenhuma mudança foi feita em `crates/aceris-core/src/cpu/decode_cache.rs`, que pertence à implementação de outro agente. A CI de 27/11/2026 ainda reportava `needless_range_loop` nesse arquivo.
- Estes caminhos não alteram estado guest nem frame pacing. Nenhum ganho foi medido em PC ou Android físico; a próxima validação deve comparar release com telemetria desligada/ligada e confirmar pixels idênticos, backend e stride reais. A compilação Android ARM64 e os testes da janela de composição ainda precisam ser confirmados pela CI do HEAD final.

## Benchmark A/B da sessão ARM11 — despacho rápido versus preciso (27/11/2026, ChatGPT)

- `crates/aceris-core/examples/benchmark.rs` aceita `--precise-dispatch` para desativar `Arm11ProcessSession::fast_dispatch` e forçar o passo preciso. A ausência da opção mantém o caminho rápido padrão. A comparação deve deixar `--fast-memory` desligado, salvo quando a memória rápida for a variável explicitamente avaliada.
- Após o aquecimento, o benchmark imprime o estado `CP15.SCTLR.M`; durante a medição conta quantos limites de quadro observam a MMU ligada. A contagem é uma amostra por quadro, não um registro por instrução.
- Comandos de referência e critérios de comparação foram registrados em `docs/PERFORMANCE_TESTING.md`. Usar a mesma imagem local, configuração, entrada, orçamento, aquecimento e número de quadros, com pelo menos três rodadas por modo.
- Nenhuma medição de jogo foi executada nesta atualização. A mudança oferece um método comparável; não declara o despacho rápido vencedor em sessão completa nem prova MMU ativa em um título até executar a sonda.
- Próximo passo: executar A/B release em PC x86-64 e Android ARM64 físico; comparar throughput, p50/p95/p99, GSP/LCD, estado final e proporção de quadros com `SCTLR.M` ativo. Se a MMU ficar desativada, classificar o resultado como sessão sem MMU.

## Desktop — evitar limpeza duplicada do canvas em escala 1× (27/11/2026, ChatGPT)

- `compose_bgrx_into` calcula o layout antes de inicializar o frame. Em `scale == 1`, inicializa somente o gutter central como preto opaco; os blits por linha das duas LCDs sobrescrevem todos os pixels restantes e forçam alpha `0xFF`.
- Escalas maiores mantêm a inicialização completa e o caminho de replicação anterior. O teste `native_scale_overwrites_reused_buffer_and_keeps_the_gutter_opaque` usa buffer sentinela para detectar pixels antigos não sobrescritos e verifica o gutter.
- Alteração somente no canvas host. Não muda pixels LCD guest nem o layout lógico. Ganho ainda não medido em hardware; o CI do HEAD mais recente continua pendente de confirmação.

## Desempenho — composição host Android e desktop sem limpeza redundante (27/11/2026, ChatGPT)

- Android, commit `61422cdf` (formatação em `6a8f34c8`): `frontends/android/src/app.rs` separa a reserva/redimensão do frame da limpeza. `reset_frame_background` calcula, por linha, os intervalos cobertos por `ScreenLayout.top` e `ScreenLayout.bottom`, mescla os intervalos e limpa somente os pixels restantes em preto opaco. O compositor de jogo usa esse caminho; a introdução continua usando `reset_black_frame` integral.
- O teste `optimized_game_composition_matches_full_clear_for_reused_buffers` compara pixel a pixel o caminho otimizado com uma referência de limpeza integral seguida pelos blits de referência. Abrange `736×240`, `1080×2400`, `2400×1080`, `853×1281`, `1280×720`, largura zero e altura zero, reutilizando um buffer com bytes sentinela.
- Desktop, commit `5b50555d` (formatação em `fa0db091`): `frontends/desktop/src/window.rs` estende o caminho que limpa somente o gutter central às escalas inteiras 1×–4× quando verifica que as dimensões, posições, alturas e strides das LCDs cobrem todo o canvas fora do vão. Se as condições não forem satisfeitas, mantém a limpeza completa anterior.
- O teste `scaled_composition_overwrites_reused_buffer_and_keeps_the_gutter_opaque` valida buffer sentinela, alpha opaco, pixels das duas telas e gutter nas escalas 2×, 3× e 4×. O teste existente cobre escala 1×.
- A equivalência dos buffers foi testada; isso não quantifica ganho de FPS. As duas mudanças atuam somente no trabalho host e não alteram dimensões/pixels guest, relógio, timers, orçamento ARM11, áudio ou frame pacing.
- CI do commit `fa0db09133d49a2b978ef74bb7785753b097ac07`: `cargo fmt`, builds e `cargo test --workspace` passaram em Windows, Ubuntu e macOS; Android ARM64 compilou app, benchmark e ferramenta de perfil de memória. O workflow Build Android APK também concluiu com sucesso.
- A CI completa ainda fica vermelha em Clippy, devido ao aviso preexistente `clippy::needless_range_loop` em `crates/aceris-core/src/cpu/decode_cache.rs:386`. Esse arquivo pertence ao trabalho de outro agente e não foi modificado.
- Pendência: medir Release em PC x86-64 e Android físico, com o mesmo título/entrada e telemetria ligada/desligada. Não declarar ganho de desempenho até obter resultados em hardware real.
- Fronteiras: somente `main`; alterações funcionais limitadas a `frontends/android/src/app.rs` e `frontends/desktop/src/window.rs`, sem dependências, assets, jogos, firmware ou chaves adicionados.

## Composição escalonada desktop em ordem de linhas (27/11/2026, ChatGPT)

- `frontends/desktop/src/window.rs` agora amplia cada linha LCD em ordem contígua: primeiro replica horizontalmente os pixels BGRX na primeira linha ampliada e depois replica a linha pronta nas demais linhas verticais com `copy_within`.
- O caminho anterior escrevia cada pixel fonte como uma pequena coluna vertical. A nova ordem melhora a localidade de memória para escalas inteiras maiores sem alterar a conversão RGBA→BGRX, alpha opaco, dimensões do canvas ou a replicação nearest-neighbour.
- O teste `row_major_scaled_composition_matches_reference_pixel_for_pixel` usa padrões não uniformes e compara o buffer inteiro com uma implementação de referência para escalas 2×, 3× e 4×, incluindo buffer inicializado com sentinela.
- Commit: `8585b41bc5b2c4efb05970ad5c65e4d6080b2156`. CI e medição em hardware físico ainda pendentes; não declarar ganho de FPS.

## Android — reduzir verificações redundantes na sobreposição dos controles (27/11/2026, ChatGPT)

- `crates/aceris-core/src/video/frontend.rs` calcula uma máscara fixa de círculos posteriores que podem geometricamente cobrir cada botão. O loop de pixels só testa esses candidatos, mantendo a verificação exata de distância e a precedência do desenho.
- O cálculo é feito uma vez por composição, sem alocação, e mantém o comportamento correto em superfícies compactas onde os círculos realmente se sobrepõem.
- O teste diferencial `direct_virtual_overlay_matches_full_frame_reference` inclui 64×48 além dos tamanhos portrait/landscape já exercitados e compara o frame resultante pixel a pixel com a composição de overlay em tela inteira.
- Commit: `e7f29c876aed64b3de1ca2a4c101b6f4c47825ef`. A equivalência automatizada precisa ser confirmada pela CI; ainda não há medição em aparelho Android.
- Ambas as otimizações atuam somente na apresentação host. Não alteram instruções, cache, temporização, frame pacing ou estado guest; nenhuma dependência, ROM, firmware, chave ou asset foi adicionado.

## Android — reutilizar buffer de eventos multitouch (27/11/2026, ChatGPT)

- `frontends/android/src/native_activity.rs` mantém um `Vec<Pointer>` de scratch durante toda a thread da aplicação e o reutiliza em `poll_looper`/`drain_input`/`handle_motion`, evitando criar um vetor temporário novo a cada evento de movimento.
- `MOTION_ACTION_POINTER_UP` remove o ponteiro levantado in-place (`Vec::remove`) em vez de coletar um segundo vetor filtrado. `MOTION_ACTION_CANCEL` limpa o scratch e libera os ponteiros da App sem ler coordenadas que seriam descartadas.
- O estado de toque entregue a `App::set_pointers` permanece o mesmo e a ordem dos demais ponteiros é preservada. A App ainda atualiza sua estrutura interna de ponteiros; esta mudança elimina apenas as alocações intermediárias do adaptador Android.
- Commit da reutilização: `1dc201763e5286bfa5fc615693c1f8e28f66122c`; cancelamento antecipado: `cdd4cda2f2591475b7dd1b09b177c3fe1bd12ad6`.
- Sem teste em dispositivo físico ou medição de latência de toque nesta revisão. A compilação Android ARM64/CI ainda precisa confirmar o estado final.

## Presenter host — evitar upload quando a surface não apresenta (27/11/2026, ChatGPT)

- `crates/aceris-graphics/src/lib.rs`: `GraphicsPresenter::present_pixels` agora adquire o `SurfaceTexture` antes de criar/atualizar a textura de frame e chamar `queue.write_texture`.
- Em `Timeout` ou `Occluded`, a função retorna sem enviar o framebuffer inteiro à GPU nem criar a textura de upload naquela iteração. `Outdated`/`Lost` ainda reconfigura a surface e tenta adquirir novamente antes do upload; `Suboptimal` preserva o comportamento de reconfiguração após apresentar.
- A mudança vale para os dois formatos de upload (RGBA Android e BGRX desktop), sem alterar bytes, shader, ordem da apresentação ou estado guest. O presenter `wgpu` foi atribuído ao trabalho do ChatGPT nesta etapa; nenhuma área PICA/CPU/Thumb foi editada.
- Commit `764806637a647e98f9ecb416cf35ffb0809bff5e`. CI, teste em janela minimizada/oculta e medição de tráfego GPU ainda pendentes; não quantificar ganho até medir.

## Desktop — cópia de linhas ampliadas sem memmove no caso disjunto (27/11/2026, ChatGPT)

- A replicação de linhas em `frontends/desktop/src/window.rs` agora usa `split_at_mut` + `copy_from_slice` quando a linha de destino começa depois do fim da primeira linha ampliada, permitindo cópia de fatias comprovadamente disjuntas.
- Mantido `copy_within` como fallback caso uma geometria futura produza regiões sobrepostas em memória. O teste diferencial completo para 2×/3×/4× permanece a referência de pixels.
- Commits `d6e751602ca4cc9d777a627e9c492ff8e3545f01` e `90f2fc5d3041e404eb783cd4470943dbcfc916bd`. Compilação/CI e benchmark release ainda pendentes.


## Experiência do usuário — preferências persistentes e especificação do menu (27/11/2026, ChatGPT)

- crates/aceris-core/src/video/library.rs agora persiste Grade/Lista e ordenação A–Z/Z–A em .aceris/library_preferences_v1.txt.
- O arquivo usa o cabeçalho/versionamento ACERIS_LIBRARY_PREFERENCES_V1; valores desconhecidos, campos duplicados, arquivo ausente ou inválido retornam ao padrão seguro. A configuração é global para a biblioteca e não representa override por título.
- Foram adicionados testes para recuperar as preferências ao recriar Library e para fallback com arquivo inválido.
- A gravação de falha substitui a mensagem de status da alternância para que o usuário não receba confirmação falsa de persistência.
- docs/QUICK_MENU_SETTINGS_SPEC.md registra o desenho do menu rápido, pausa segura em limite de quadro, personalização, categorias de configuração, estado atual e critérios de aceite.
- docs/FRONTENDS.md, docs/USER_EXPERIENCE.md e docs/PRODUCT_IMPLEMENTATION_PLAN.md foram sincronizados com a mudança.
- A implementação do menu rápido, pausa de sessão, layouts avançados, editor de controles, temas alternativos e perfis por jogo continua pendente. Não há controles fictícios nem save state/screenshot anunciados.
- Os testes foram adicionados ao código, mas não foram executados nesta sessão; aguardar CI do HEAD final. Nenhum teste em dispositivo Android físico foi feito.
- Fronteiras preservadas: mudanças funcionais limitadas a crates/aceris-core/src/video/library.rs; documentação de UX/plano/handoff atualizada. Nenhum código CPU/Thumb, GPU/PICA, trabalho de Claude/Thumb, dependência, asset, ROM, firmware ou chave foi alterado. Somente main; nenhuma branch ou PR criada.


## Menu rápido — pausa de sessão e ações iniciais (27/11/2026, ChatGPT)

- Novo módulo compartilhado `crates/aceris-core/src/video/quick_menu.rs`: renderização sem alocação extra do overlay para RGBA8 (Android) e BGRX8 (desktop), hit-test das três ações e botão MENU compacto para hosts touch. Testes cobrem dimensões PC/telefone, correspondência de hit-test, formato de pixel e buffers inválidos.
- `frontends/android/src/app.rs`: novo estado GameMenu. Voltar abre/fecha o menu; o botão MENU visível também abre. O toque do botão é removido do caminho de stylus. Continuar, Reiniciar (recria System do mesmo arquivo) e Voltar à biblioteca estão ligados. Testes cobrem abrir/fechar, não avançar frame guest durante o menu e não encaminhar o toque do botão ao jogo.
- `frontends/desktop/src/main.rs`: Esc abre/fecha o overlay; setas/Enter e mouse navegam; Continuar, Reiniciar e Voltar à biblioteca são ações reais. Enquanto aberto, o loop não chama `run_frame_with_diagnostics` nem alimenta novo áudio. As amostras de telemetria do jogo são suspensas durante o painel.
- `frontends/desktop/src/main.rs` e `frontends/android/src/app.rs` continuam em Rust; o core CPU/Thumb, GPU/PICA, SVC, DSP e CSND não foram modificados. Nenhuma dependência ou asset foi adicionada.
- Limites: atalho PC ainda fixo em Esc; filas de áudio do host não têm flush/pause explícito (podem drenar amostras já enfileiradas); não há confirmação de progresso não salvo. Layout alternativo, troca de telas, save states, rewind, screenshot, cheats e editor de controles continuam indisponíveis.
- Testes foram adicionados, mas não foram executados localmente nesta sessão. O clone local não pôde ser obtido por indisponibilidade de resolução de github.com e os status de checks retornados pelo conector estavam vazios; não declarar CI aprovada. Teste Android físico e desktop com janela/áudio reais permanecem pendentes.
- Fronteiras: somente `main`; arquivos funcionais alterados: `crates/aceris-core/src/video/quick_menu.rs`, `crates/aceris-core/src/video.rs`, `frontends/android/src/app.rs`, `frontends/desktop/src/main.rs`. Documentação de estado/plano/capacidades atualizada. Nenhum trabalho reservado de Claude/Thumb, CPU/Thumb, GPU/PICA ou conteúdo protegido foi alterado.


## Telemetria desktop durante pausa do menu rápido (27/11/2026, ChatGPT)

- `DesktopPerformanceMonitor::reset_after_pause` inicia uma nova janela de amostragem quando o menu rápido abre ou retoma o jogo, para que o tempo de pausa não contamine FPS/frametime de gameplay.
- Foi adicionado teste para confirmar que a janela, contagem de amostras, quadros apresentados e timestamp são reiniciados mantendo o modo Game.
- O reset acontece tanto na tecla Esc quanto na ação Continuar pelo Enter/mouse. Não altera temporização guest nem a cadência do jogo.
- Teste ainda não executado nesta sessão; CI e hardware físico pendentes.

## Limpeza explícita da fila de áudio no menu rápido (27/11/2026, ChatGPT)

- Android: `frontends/android/src/aaudio.rs` agora resolve dinamicamente `AAudioStream_requestPause`, `AAudioStream_requestFlush`, `AAudioStream_requestStart`, `AAudioStream_getState` e `AAudioStream_waitForStateChange`. A pausa aguarda a saída de PAUSING, limpa a fila apenas depois de PAUSED e aguarda a saída de FLUSHING; ao retomar, estados transitórios pendentes são reconciliados antes de reiniciar. `feed` ignora dados enquanto a saída está marcada como pausada.
- `frontends/android/src/app.rs` expõe somente a consulta `is_game_menu_open`; `native_activity.rs` sincroniza pause/flush com o menu e com o estado de segundo plano da NativeActivity, registrando uma mensagem de diagnóstico quando uma transição falha.
- Desktop: `frontends/desktop/src/audio.rs` acrescenta `Sink::flush` e `AudioOutput::flush`, que limpa buffers locais, recria o resampler (descartando histórico de interpolação) e delega ao backend. ALSA usa `snd_pcm_drop` seguido de `snd_pcm_prepare`; WinMM usa `waveOutReset` e desprepara os headers devolvidos; AudioQueue/CoreAudio usa `AudioQueueStop(immediate = true)`, marca os buffers como livres e reinicia a fila. `main.rs` chama a limpeza ao abrir o menu via Esc.
- Teste unitário adicionado para assegurar que `AudioOutput::flush` limpa os buffers locais e chama o sink. Há também teste da classificação de estados silenciosos Android. Os testes não foram executados nesta sessão; não alegar build, Clippy ou testes aprovados sem CI verificável.
- Validação pendente: transições AAudio em aparelhos reais (API 26+), reconexão/desconexão de dispositivos, saída audível do DSP/CSND, e funcionamento em Linux/Windows/macOS com hardware real. A limpeza da fila host não demonstra por si só que o áudio emulado do 3DS está correto.
- M4.1 continua parcial: confirmação de perda de progresso, atalhos configuráveis e validação física não estão concluídos. Nenhuma alteração em CPU/Thumb, GPU/PICA, SVC, DSP/CSND, código reservado de Claude, dependências, assets, ROMs, firmware ou chaves. Todas as alterações seguem diretamente em `main`, sem branch ou PR.
- Commits desta etapa: `50fca2e`, `802218c`, `5141ecb`, `5f648b8`, `62bd875`, `d4074a2`, `b16ee51`, `a9eca78` e `1390684`.
- Revisão adicional `3311d77`: se uma espera anterior terminar em timeout no estado AAudio FLUSHING, a próxima tentativa de pausa agora aguarda a conclusão dessa transição em vez de retornar erro imediatamente.
