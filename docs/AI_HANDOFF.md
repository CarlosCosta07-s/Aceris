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
- Não abrir, ler ou usar código-fonte interno de outros emuladores.
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

- Commit `91b5e5e5bbd8786142b18a78ff83387bbb44ff30` altera somente `crates/aceris-core/src/loader/cia/archive.rs`.
- A verificação SHA-256 dos conteúdos agora repete leituras que retornam `io::ErrorKind::Interrupted`, tanto no corpo do fluxo quanto na leitura final usada para detectar bytes além do tamanho declarado no TMD.
- Foram adicionadas regressões com leitor sintético que alterna interrupções e leituras curtas: uma confirma a validação correta do hash e outra confirma que um byte excedente continua sendo rejeitado mesmo quando a primeira tentativa da checagem final é interrompida.
- A semântica de tamanho exato, SHA-256, limites do leitor e tratamento dos demais erros permanece inalterada. Nenhum arquivo de CPU, GPU/PICA, memória, LCD ou sessão ARM11 foi modificado.
- A CI para este commit ainda precisa ser observada. Não marcar Rustfmt, Clippy, testes do workspace ou build Android como aprovados antes do resultado do workflow.
