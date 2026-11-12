# Notas de transferência entre IAs — Aceris

> **Leitura obrigatória ao retomar o trabalho.** Esta nota registra alterações autorizadas pelo mantenedor e evita que uma IA restaure ou sobrescreva trabalho de outra.

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
