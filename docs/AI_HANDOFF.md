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
