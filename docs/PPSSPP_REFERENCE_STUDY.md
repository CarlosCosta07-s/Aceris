# Estudo de referência — PPSSPP

**Revisão:** 27/11/2026  
**Escopo:** ideias de produto e comportamento, não reutilização de implementação.

Este estudo registra quais recursos observados no PPSSPP fazem sentido para o Aceris (Nintendo 3DS), o que já existe no projeto, o que deve ser adaptado e quais dependências impedem uma implementação segura. O PPSSPP é uma referência funcional; não é uma dependência técnica do Aceris.

## Regras de adoção

- Todo código, UI, formatos de configuração e assets do Aceris continuam originais e em Rust.
- Não copiar nem adaptar código-fonte, shaders, imagens, ícones, traduções ou componentes do PPSSPP.
- Usar o PPSSPP somente para estudar comportamentos e opções que podem ser reproduzidos independentemente.
- Não expor configurações que não tenham efeito real no core ou no backend ativo.
- Toda opção de precisão reduzida deve ser explícita, reversível e identificada como experimental.
- Telemetria e relatórios ficam locais e desativados por padrão; qualquer exportação deve apresentar uma prévia e permitir sanitização.
- Nenhum recurso deve incluir ROMs, firmware, chaves, conteúdo extraído ou dados de jogo no repositório, CI ou releases.

## Inventário de adoção

| Área | Ideia de referência | Estado atual do Aceris | Decisão |
|---|---|---|---|
| Controles virtuais | Mover, redimensionar, opacidade, presets, ocultar e layouts por jogo | Android já tem editor por arraste/pinça, opacidade, escala, presets, orientação e override por Title ID | **Manter e completar**; validar em aparelho e melhorar descoberta/acessibilidade |
| Controles físicos | Remapeamento de botões/eixos, deadzone, sensibilidade, inversão e perfis por dispositivo | Editor visual compartilhado para 14 botões e captura física, além da tela EIXOS nos dois hosts. Ela permite inverter X/Y, ajustar sensibilidade de 50%–200% em passos de 10%, restaurar padrões e alternar escopo global/jogo; no desktop também seleciona o perfil por modelo quando existe UUID válido. O núcleo persiste `ACERIS_GAMEPAD_AXIS_SETTINGS_V1` separadamente, com precedência jogo > modelo > global; o processamento radial é compartilhado. O perfil por modelo dos botões ainda não é editável, e o Android não persiste perfil por modelo. Faltam remapeamento das origens X/Y e calibração de centro/deadzone por controle | **Remapear e calibrar eixos** |
| Atalhos | Ações configuráveis de pausa, menu, screenshot e progresso | Menu rápido existe; atalhos continuam fixos | **Adotar** com detecção de conflitos e padrões restauráveis |
| Perfis por jogo | Overrides com herança global | Layout de telas e controles virtuais Android possuem overrides por jogo; schema geral ainda não existe | **Expandir** para controles físicos, performance e opções realmente disponíveis |
| Overlay de diagnóstico | FPS/frametime, estado dos subsistemas, falhas e informações do backend | Há overlay durante o jogo em PC e Android com PC guest, solicitações não tratadas e métricas de desempenho do host; ainda faltam velocidade guest e estados detalhados por subsistema | **Expandir**, sem confundir FPS apresentado com velocidade guest |
| Relatório de diagnóstico | Exportar informações úteis para resolver incompatibilidades | Não existe fluxo completo com prévia/sanitização | **Adotar**; opt-in, local, sanitizado e sem ROM/chaves/saves |
| Captura de tela | Captura do quadro do jogo com destino escolhido | Ação de screenshot já existe no menu rápido de PC e Android, salva PNG em `.aceris/screenshots` e apresenta retorno de sucesso/erro; ainda não há seletor de destino ou galeria | **Completar** com destino configurável e prévia/galeria |
| Save states | Slots, quick save/load, backups, identificação do jogo | Envelope versionado, seções, identidade SHA-256 e armazenamento com gerações imutáveis já existem; não há snapshot/restauração completa do `System` | **Completar com prioridade**, mas bloquear UI até cobrir todo estado determinante |
| Rewind | Voltar alguns segundos com snapshots diferenciais | Não disponível; depende de snapshot completo, memória e custo de compressão/cópia | **Adotar futuramente**, desligado por padrão e com orçamento configurável |
| Fast Forward / velocidade alternativa | Acelerar ou limitar a velocidade de execução | Não há controle de velocidade de usuário integrado | **Adotar** após definir clock guest, áudio, sincronização e limites de frame pacing |
| Perfis de desempenho | Compatibilidade, equilibrado, desempenho e personalizado | Existe modo Fast Memory opt-in; métricas host já são coletadas em janelas locais | **Adotar com cautela**; nunca prometer ganhos ou mudar precisão silenciosamente |
| VSync / pacing / contador | Ajustar apresentação e observar estabilidade | Presenters e medições host existem; opção integrada e overlay de usuário ainda não existem | **Adotar** quando o backend realmente expuser e validar a opção |
| Resolução interna / filtros / AA | Melhorar o render 3D e apresentação | O Aceris compõe LCDs e possui partes do PICA200; não há conjunto de opções gráficas de usuário validado | **Adotar gradualmente** somente após renderer PICA200 completo e comparado |
| Substituição de texturas | Pacotes locais de texturas por jogo | Não existe pipeline de substituição de assets | **Adotar futuramente**, com cache limitado e sem alterar dados originais do jogo |
| Gerenciador de saves | Visualizar, copiar, importar/exportar e fazer backup de save data/extdata | Armazenamento HLE por jogo é parcial; não há gerenciador de UI | **Adotar** depois de definir a cobertura e formato real de cada serviço |
| Cheats locais | Códigos por jogo com origem, descrição e desligamento geral | Não disponível | **Adotar futuramente** com validação, escopo por título e desativado por padrão |
| Multiplayer local | Comunicação local entre instâncias/aparelhos | Serviços de rede 3DS ainda são parciais; não há multiplayer funcional | **Estudar depois** de fechar serviços de rede e validar protocolo 3DS |
| Conquistas | Integração opcional com serviço externo | Não disponível | **Estudar depois** de revisar API, autenticação, regras, privacidade e integridade |
| Áudio | Volume, mute, dispositivo, latência e visualização de estado | Sink host existe, mas a inicialização e a saída DSP/CSND ainda precisam de validação funcional | **Adotar após corrigir áudio**; não expor controles que não controlam o caminho real |

## Modelo de remapeamento físico implementado no núcleo

O núcleo define o schema `ACERIS_GAMEPAD_MAPPING_V1`, com entradas padronizadas (`south`, `east`, `north`, `west`, D-pad, ombros, gatilhos, Start e Select). Os destinos são botões 3DS (`a`, `b`, `x`, `y`, `l`, `r`, `zl`, `zr`, direções, Start e Select) ou `none` para desativar uma entrada. O parser rejeita versões desconhecidas, campos ausentes, fora de ordem ou inválidos.

O núcleo também define o schema independente `ACERIS_GAMEPAD_AXIS_SETTINGS_V1`: `invert_x` e `invert_y` (0/1) e `sensitivity_percent` (50–200). Os arquivos são `.aceris/gamepad_axis_settings_v1.txt` (global), `.aceris/gamepad_axis_devices/<uuid>.v1` (modelo) e `.aceris/gamepad_axis_games/<title-id>.v1` (jogo), com precedência jogo > modelo > global. O desktop carrega esses perfis pelo UUID do GilRs; o Android atualmente carrega perfil global ou por jogo. A sensibilidade atua após a zona morta radial e mantém o limite máximo do Circle Pad. A tela EIXOS permite inverter X/Y, ajustar sensibilidade em passos de 10%, restaurar padrões e escolher escopo global/jogo; no desktop, também permite escolher MODELO quando há UUID válido. O perfil por modelo das entradas de botão ainda não é editável, e o Android ainda não persiste perfil por modelo. Remapeamento das origens dos eixos e calibração individual de centro/deadzone seguem pendentes.

Os arquivos ficam em `.aceris/gamepad_mapping_v1.txt` (global), `.aceris/gamepad_devices/<uuid>.v1` (modelo de controle, atualmente desktop) e `.aceris/gamepad_games/<title-id>.v1` (jogo). A precedência é jogo, modelo de controle e global. O UUID de 16 bytes oferecido por GilRs identifica o modelo do controle, não uma unidade física individual; UUID zero não gera perfil para evitar colisões. Desktop e Android aplicam o schema: Android carrega perfis global e por jogo, enquanto desktop também carrega perfil por modelo. O editor visual V1 está no menu rápido dos dois hosts: permite selecionar uma das 14 entradas, percorrer destinos 3DS (incluindo desligado), alternar escopo e salvar automaticamente. Para capturar, a linha selecionada precisa ter um destino 3DS; o usuário aciona CAPTURAR e pressiona o controle físico. A origem detectada recebe aquele destino e o vínculo anterior é liberado. Desktop usa eventos de botão do GilRs; Android usa key events físicos e os eixos analógicos L2/R2 com limiar de 50%. A captura pode ser cancelada por Escape/Voltar ou pelo botão Cancelar. A edição do perfil por modelo das entradas de botão ainda não está exposta no editor. X/Y continuam ligados ao analógico esquerdo e ainda não têm remapeamento de origem ou calibração de centro/deadzone; inversão e sensibilidade são configuráveis na tela EIXOS descrita acima.

Exemplo:

```text
ACERIS_GAMEPAD_MAPPING_V1
south=a
east=b
north=x
west=y
dpad_up=up
dpad_down=down
dpad_left=left
dpad_right=right
left_shoulder=l
right_shoulder=r
left_trigger=zl
right_trigger=zr
start=start
select=select
```

## O que não deve ser copiado literalmente

- Particularidades do PSP, como configurações específicas do hardware, resoluções e interfaces de sistema.
- Protocolos e serviços PSP Ad Hoc como se fossem equivalentes à rede local do 3DS.
- JIT, IR, cache de CPU ou implementação de GPU do PPSSPP. A arquitetura do PSP é diferente; Aceris precisa de implementação própria para ARM11/PICA200.
- Speedhacks que pulam efeitos, sincronização, renderização ou operações guest para obter velocidade aparente.
- Opções que prometam resolução interna aumentada quando só estiverem ampliando o LCD composto.
- Achievements, sincronização em nuvem, telemetria ou recursos online ativados por padrão.

## Ordem de implementação proposta

### Fase A — Controles e perfis (P0)

1. Definir um modelo comum de ações lógicas: botões, gatilhos, eixos, stylus e atalhos.
2. Adicionar detecção, conexão/desconexão e nomes de gamepads em Windows, Linux, macOS e Android.
3. Criar mapeamento editável para teclado, gamepad e controles virtuais, com calibração de deadzone, sensibilidade e inversão.
4. Persistir perfis globais, por dispositivo e por jogo com herança explícita e migração versionada.
5. Permitir ocultar o overlay virtual quando um controle físico for conectado, mantendo um override manual.
6. Criar editor de atalhos com detecção de conflitos e restauração dos padrões.

**Aceite:** testes para botões/eixos, reconexão, dispositivos múltiplos, foco perdido, multi-touch + stylus, conflito de atalhos, migração e isolamento dos perfis.

### Fase B — Diagnóstico e captura (P0/P1)

1. Overlay opcional durante o jogo com FPS apresentado, frametime e tempos de CPU/render/composição/apresentação.
2. Exibir velocidade guest separadamente; não inferir a velocidade guest a partir dos frames host.
3. Indicadores reais de estado para ARM11/MMU, PICA200/GSP, DSP/CSND, HID e serviços IPC, sem transformar ausência de instrumentação em estado saudável.
4. Captura de tela das duas LCDs e dos layouts de tela disponíveis, com caminho de destino escolhido pelo usuário.
5. Relatório de diagnóstico com versão, OS, CPU/GPU/driver quando disponíveis, identidade do título, último erro e logs selecionados.
6. Prévia antes da exportação, com remoção de caminhos privados e sem incluir jogos, chaves, saves ou dados pessoais por padrão.

**Aceite:** overlay desligado sem custo relevante, métricas com nomes e unidades corretos, screenshot verificável nos dois hosts e relatório revisável antes de compartilhar.

### Fase C — Velocidade e apresentação (P1)

1. Fast Forward com velocidades configuráveis, limite seguro e ação momentânea/alternável.
2. Velocidade alternativa abaixo/acima de 100% sem alterar a frequência nominal do hardware guest sem modelo explícito.
3. Perfil Compatibilidade, Equilibrado, Desempenho e Personalizado formado exclusivamente por opções já validadas.
4. Opções VSync, limitador, pacing, escala de apresentação e filtros somente quando o backend realmente as suportar.
5. Benchmarks repetíveis separados por jogo, host, backend, driver e configuração.

**Aceite:** relógio guest, áudio, timers, input e sincronização permanecem coerentes; usuário pode interromper Fast Forward imediatamente; todos os perfis são reversíveis.

### Fase D — Progresso e dados (P1/P2)

1. Completar captura/restauração de todos os subsistemas determinantes do `System`.
2. Só então habilitar slots, quick save/load, confirmação de sobrescrita, prévias e backups rotativos.
3. Adicionar recuperação de arquivo incompleto/corrompido e compatibilidade de versão.
4. Rewind apenas depois dos save states, com orçamento máximo de RAM/armazenamento, janela temporal configurável e compressão medida.
5. Gerenciador de save data/extdata com backup antes de importação/exportação e aviso claro sobre compatibilidade.

**Aceite:** testes de fechar/reabrir, restauração repetida, arquivo inválido, jogo diferente, versão antiga e falha durante escrita; save normal do jogo nunca é confundido com save state.

### Fase E — Gráficos e expansão (P2)

1. Completar e validar pipeline PICA200 antes de expor filtros de renderização 3D.
2. Implementar perfis gráficos por jogo e cache limitado quando houver shaders/pipelines PICA compilados pelo próprio Aceris.
3. Estudar substituição local de texturas por Title ID e hash, com cache invalidável.
4. Cheats locais somente com escopo por jogo, origem, validação e desligamento geral.
5. Multiplayer local e conquistas somente depois de revisão dos serviços/protocolos 3DS, segurança e privacidade.

**Aceite:** testes visuais de regressão, renderer de referência/fallback, limpeza de cache, compatibilidade por jogo e nenhum recurso online sem escolha explícita.

## Dependências que bloqueiam a interface prematura

- **Save states/Rewind:** o formato de arquivo e o armazenamento já existem, mas não existe captura/restauração completa e atômica da máquina.
- **Filtros/resolução interna/texturas:** presenter LCD host não substitui renderer PICA200; as opções precisam afetar o caminho 3D real.
- **Métricas guest:** FPS apresentado pelo sistema host não mede velocidade do jogo emulado.
- **Áudio configurável:** precisa de saída DSP/CSND funcional antes de expor dispositivo, volume ou latência como controles prontos.
- **Multiplayer/conquistas:** dependem de serviços de rede 3DS e integrações explicitamente opt-in.

## Referências públicas

- [PPSSPP — Settings](https://www.ppsspp.org/docs/settings/)
- [PPSSPP — Developer tools](https://www.ppsspp.org/docs/development/developer-tools/)
- [PPSSPP — Rewind](https://www.ppsspp.org/docs/reference/rewind/)
- [PPSSPP — código-fonte público (referência comportamental; não copiar)](https://github.com/hrydgard/ppsspp)

Este documento é uma especificação de adoção de recursos. Uma linha descrita como “adotar” não significa que ela já tenha sido implementada.
