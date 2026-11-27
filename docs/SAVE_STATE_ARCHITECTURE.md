# Arquitetura de save states

> Estado em 27/11/2026: o formato de contêiner versionado está implementado. A captura/restauração integral de System, a gravação em slots e os controles de interface ainda não estão disponíveis. Não apresentar o subsistema como concluído.

## Save de jogo não é save state

O Aceris já possui persistência de arquivos de save do próprio jogo por meio de fs:USER e do armazenamento host. Isso permite ao guest gravar seus próprios arquivos, mas não congela a execução do emulador.

Um save state deve retomar a máquina no mesmo ponto da execução: registradores, memória, threads, handles, serviços, GPU, áudio e operações pendentes precisam permanecer coerentes.

## Inventário de estado que precisa ser capturado

O agregado atual System mantém, entre outros componentes:

- Arm11Core e Arm11ProcessSession: contexto ARM11, execução, threads, memória do processo, estado de kernel, handles, sessões IPC, eventos e chamadas suspensas.
- InputState, HidService, AptService, FsState e operações pendentes: entrada publicada ao guest, estado de applet, sessões de filesystem e operações que aguardam conclusão.
- DspService, Dsp, CsndService e CsndAudio: RAM DSP, pipes, vozes, buffers em reprodução, semáforos, eventos, canais CSND, temporização e filas emuladas.
- GspServiceRegistry, estados de busy/AcquireRight, filas GSP e MpcoreScu: sincronização entre guest, GPU e interrupções.
- Pica200, memória física/acesso DMA, sessões de renderização, LcdController e DualScreenDisplay: registradores, buffers, mapeamentos e scanout.
- Contadores de quadro e relógio virtual, inclusive o restante de uma fatia de tempo quando existir.

A lista acima é o inventário inicial baseado nos campos atuais de System; cada tipo precisa de uma política explícita de serialização e de versão própria.

## Recursos host que não devem ser copiados cegamente

Surface de janela, dispositivo de áudio, handles do sistema operacional, objetos GPU do host e pipeline caches compilados não são estado guest. Após a restauração, devem ser recriados ou invalidados com segurança.

ROM/NCCH/ExeFS/RomFS e diretórios de save do usuário são fontes externas. O estado deve guardar identidade e posições lógicas necessárias para reabrir os mesmos recursos, não serializar ponteiros ou caminhos temporários do processo. A gravação normal de save data do jogo deve permanecer fora do arquivo do save state.

GameIdentity atualmente contém apenas Title ID e omite revisão; homebrew 3DSX pode não ter Title ID. O save state exige identidade de imagem mais forte: SHA-256 de uma representação canônica do conteúdo de boot, com Title ID adicional quando disponível. Caminho e nome do arquivo não são identidade confiável.

## Envelope binário v1

O módulo aceris_core::save_state implementa um envelope little-endian:

| Campo | Tamanho |
|---|---:|
| Magic ACRSTATE | 8 bytes |
| Versão do formato | 2 bytes |
| Tamanho do cabeçalho | 2 bytes |
| Flags reservadas | 4 bytes |
| Title ID opcional (zero quando ausente) | 8 bytes |
| SHA-256 do conteúdo de boot | 32 bytes |
| Número do quadro | 8 bytes |
| Tempo virtual em ns | 8 bytes |
| Comprimento do payload | 8 bytes |
| CRC-32 do payload | 4 bytes |
| Palavra reservada | 4 bytes |

O payload é opaco nesta primeira etapa. O decodificador valida assinatura, versão, cabeçalho, flags, identidade exata, limites, tamanho total e checksum antes de aceitar o conteúdo. O limite defensivo inicial do payload é 512 MiB; qualquer revisão desse limite deve ser baseada no tamanho máximo real dos mapas de memória e no orçamento das plataformas.

O CRC-32 detecta corrupção acidental; não autentica o arquivo e não substitui uma política de integridade/segurança se os estados forem compartilhados entre usuários.

### Tabela interna de seções — ACRPAYLD v1

O módulo também implementa um segundo contêiner, usado como payload do arquivo externo:

| Campo | Tamanho |
|---|---:|
| Magic ACRPAYLD | 8 bytes |
| Quantidade de seções | 4 bytes |
| Flags reservadas | 4 bytes |

Cada seção contém um cabeçalho de 24 bytes: ID (u32), versão do componente (u16), flags (u16), comprimento (u64), CRC-32 (u32) e palavra reservada (u32), seguido pelos bytes do componente.

Os IDs devem ser não-zero, versões devem ser não-zero e IDs duplicados são rejeitados. A ordem de inserção é preservada para produzir arquivos reproduzíveis. O limite atual é 32 seções e o tamanho total continua sujeito ao limite de 512 MiB. Flags ainda não definidas são rejeitadas. Seções desconhecidas podem ser preservadas pelo contêiner, mas um futuro restaurador deve rejeitar IDs desconhecidos quando forem obrigatórios para a versão da máquina.

O contêiner não define ainda serializers para CPU, memória ou serviços; ele fornece a estrutura de enquadramento e validação para que esses serializers sejam adicionados individualmente.

Os IDs estáveis reservados em código são: 1 metadados do sistema, 2 CPU ARM11, 3 memória do processo, 4 kernel, 5 PICA200, 6 GSP/LCD, 7 DSP, 8 CSND, 9 HID/entrada/APT e 10 filesystem. Reservar um ID não significa que seu serializer já esteja implementado.

O caminho de API também está conectado: SaveStateFile::from_sections cria o envelope externo a partir de uma tabela validada, e decode_sections recupera a tabela interna depois que o arquivo externo foi validado para a identidade do jogo.

## Próximas etapas obrigatórias

1. Calcular e persistir a identidade canônica do conteúdo de boot em todos os loaders suportados, inclusive 3DSX. Recusar estados quando a identidade não puder ser confirmada.
2. Definir seções de payload com ID, versão, tamanho e checksum próprios. Implementar serializers pequenos por componente e testes de round-trip.
3. Implementar captura apenas em fronteira segura de quadro, com DSP/CSND, GPU, IPC e scheduler pausados em um ponto consistente.
4. Restaurar em estado temporário: validar todas as seções e referências primeiro; só substituir a máquina ativa depois que a reconstrução completa for bem-sucedida. Uma falha não pode deixar metade do estado antigo e metade do novo.
5. Adicionar escrita em arquivo temporário, flush e substituição atômica multiplataforma. Falha de gravação não pode destruir o slot anterior.
6. Testar continuação determinística: salvar, executar N quadros, restaurar e comparar registradores, memória guest, eventos, imagem e sequência de áudio de uma fixture sintética.
7. Só então integrar slots, nomes, timestamps e Save/Load ao menu rápido em desktop e Android.

## Critérios de aceite

- Estado incompatível, truncado, corrompido, excessivamente grande ou de outro jogo é rejeitado antes de alterar a máquina ativa.
- Save e load preservam processos, memória, interrupções, sincronização GSP/PICA, estado interno DSP/CSND e relógio virtual.
- Buffers de saída do host são limpos/recriados sem perder o estado interno da emulação nem duplicar áudio.
- Falha ao salvar preserva o slot válido anterior.
- Testes automatizados, boot homebrew contínuo e validação em desktop e Android são registrados.
- Nenhuma ROM, firmware, chave ou dado extraído é armazenado no repositório.
