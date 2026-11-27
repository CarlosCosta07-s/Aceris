# Menu rápido, personalização e configurações

> Especificação de produto e implementação — 27/11/2026. Este documento define como o Aceris deve organizar estas funções. Somente as capacidades identificadas como disponíveis no estado atual devem aparecer como ações utilizáveis na interface.

## 1. Objetivo

O menu rápido deve permitir que o jogador controle a sessão sem voltar à biblioteca. As configurações devem ser fáceis de encontrar, explicar o efeito de cada opção e separar preferências globais das alterações específicas de um jogo.

PPSSPP, Azahar, Citra, PCSX2 e outros emuladores são referências de requisitos e expectativas de uso. O Aceris manterá código, componentes, identidade visual e fluxos próprios, desenvolvidos em Rust.

## 2. Menu rápido durante o jogo

### Abertura e comportamento

**Android**
- Botão de menu discreto e sempre acessível durante o jogo, posicionado em uma área segura que não cubra o touchscreen do jogo nem os controles virtuais.
- A tecla/gesto Voltar pode abrir o menu enquanto a partida está ativa. Uma segunda ação Voltar fecha o menu e retoma o jogo; a saída para a biblioteca deve ser uma ação explícita.
- Painel compacto, com alvos de toque grandes e rolagem, sem interromper o loop nativo de eventos.

**PC**
- Atalho de teclado configurável, com um padrão inicial documentado e sem conflito com o sistema operacional.
- Esc abre o menu durante a partida; dentro do menu, Esc retorna ao jogo. Fechar a janela continua sendo uma ação separada.
- Painel lateral ou sobreposição central, navegável por teclado, mouse e controle.

### Estado da primeira implementação (27/11/2026)

A primeira camada do menu está conectada aos dois frontends:
- Android: botão MENU visível durante o jogo e tecla/gesto Voltar; toque no botão é excluído do stylus guest.
- PC: Esc abre/fecha; setas/Enter e mouse navegam.
- Continuar, Reiniciar e Voltar à biblioteca executam ações reais.
- Quando aberto, o loop não executa um novo quadro guest e continua redesenhando o último scanout.
- Ao abrir o menu, o Android pausa e limpa a fila AAudio após aguardar os estados assíncronos; no PC, o host limpa as filas ALSA, WinMM ou AudioQueue e reinicia o histórico do resampler. O Android também limpa a fila quando a NativeActivity vai para segundo plano.
- Reiniciar e Voltar à biblioteca mostram uma confirmação com aviso de possível perda de progresso não salvo; o aviso é preventivo e não depende de uma detecção automática de estado sujo. O atalho PC ainda não é configurável, e a validação física dos fluxos permanece pendente.
- As ações **Alterar layout**, **Trocar telas** e **Padrão global** estão conectadas nos dois hosts. Modos disponíveis: Lado a lado, Vertical, Superior ampliada, Inferior ampliada, Somente superior e Somente inferior. O desktop ajusta o tamanho da janela ao canvas do modo; Android usa rotação do sensor e encaixe proporcional. O toque continua mapeado à tela inferior física após a troca. Perfis globais separados para retrato/paisagem e overrides por Title ID são persistidos em `.aceris/screen_layout_preferences_v2.txt`; o formato v1 é migrado ao carregar. 3DSX sem identidade validada herda o perfil global.
- O menu rápido agora mostra **PERFIL GLOBAL** ou **PERFIL DESTE JOGO**, consultando somente o override da orientação ativa. Assim, uma personalização salva para paisagem não é apresentada incorretamente como ativa no modo retrato.
- No Android, as Configurações da biblioteca permitem mostrar/ocultar os controles virtuais. A preferência global é persistida em `.aceris/virtual_controls_v1.txt`; quando ocultos, os botões deixam de ser desenhados e deixam de capturar toques, enquanto entradas físicas continuam funcionando. O padrão continua visível para instalações existentes. A opacidade pode ser alternada entre 100%, 80%, 60% e 40%, permanece em 100% por padrão e é persistida separadamente em `.aceris/virtual_controls_opacity_v1.txt`. O tamanho pode ser alternado entre 80%, 100% e 120%, com padrão de 100%, e é persistido em `.aceris/virtual_controls_size_v1.txt`. Os presets Padrão, Compacto, Canhoto e Destro são persistidos em `.aceris/virtual_controls_preset_v1.txt`. Os três seletores ficam lado a lado em retrato e paisagem. O preset Canhoto troca os grupos do direcional e dos botões de ação; Destro desloca o grupo de ação para mais perto da borda; Compacto reduz o tamanho relativo do conjunto. O ajuste manual de tamanho continua disponível e é aplicado sobre o preset. O tamanho altera em conjunto o desenho e a área de toque; a opacidade afeta preenchimento, borda e rótulo sem alterar a geometria.

### Pausa real

O menu não pode apenas desenhar uma camada sobre uma emulação que continua avançando. O fluxo obrigatório é:

1. Receber a solicitação de pausa.
2. Concluir o quadro guest já iniciado e parar no limite seguro entre quadros.
3. Parar a produção de novos quadros e limpar explicitamente as filas de áudio do host; no Android, aguardar as transições AAudio antes de pausar/limpar e iniciar novamente ao retomar.
4. Liberar entradas mantidas ou evitar que teclas/botões pressionados antes da pausa fiquem presos.
5. Exibir o menu mantendo a última imagem do jogo.
6. Ao continuar, restaurar o fluxo de áudio/entrada e retomar o agendamento guest.

A interface continua processando eventos enquanto a sessão está pausada. A pausa não deve bloquear a thread de UI, congelar a janela ou depender de um segundo loop de emulação.

### Organização do menu

O menu terá categorias curtas:

| Categoria | Ações previstas |
|---|---|
| Jogo | Continuar, Reiniciar, Voltar à biblioteca |
| Telas | Layout, trocar telas, tela superior/inferior, restaurar disposição |
| Controles | Editar controles de toque, perfis físicos, testar entrada |
| Imagem e som | Opções reais de vídeo, desempenho e áudio |
| Progresso | Save states, slots, backup e recuperação, quando disponíveis |
| Ferramentas | Screenshot, estatísticas e diagnóstico, quando disponíveis |

A primeira versão deve expor somente as ações que tenham implementação completa. Continuar/Retomar é a saída principal; Reiniciar e Voltar à biblioteca precisam respeitar a política de salvamento e confirmar ações quando houver risco conhecido de perda de progresso.

Não mostrar botões inativos para save state, rewind, screenshot, cheats, layouts avançados ou controles configuráveis antes de os respectivos subsistemas estarem prontos.

### Atalhos rápidos

Após cada função ser implementada, ela poderá ganhar um atalho configurável:
- Abrir/fechar menu rápido.
- Pausar/retomar.
- Trocar a tela principal.
- Salvar/carregar estado rápido, somente depois de save states validados.
- Capturar screenshot, somente depois de captura e seleção de destino implementadas.

A tela de atalhos deve indicar conflitos e oferecer restauração dos padrões.

## 3. Personalização

### Biblioteca

Preferências de apresentação da biblioteca:
- Grade ou lista.
- Ordenação A–Z ou Z–A.
- Filtros de formato já disponíveis.
- Favoritos e Recentes já persistidos.

**Implementado nesta revisão:** Grade/Lista e ordenação agora são persistidas globalmente em `.aceris/library_preferences_v1.txt`, formato versionado. Arquivo ausente, versão desconhecida, campos duplicados ou valores inválidos retornam aos padrões seguros. Essas escolhas não são ainda perfis independentes por jogo.

### Controles

O editor futuro deve permitir:
- Mover, redimensionar, ocultar e ajustar opacidade de cada controle virtual.
- Presets Padrão, Compacto, Canhoto, Destro e Personalizado.
- Ajustar posição do Circle Pad e área de toque/stylus.
- Preferências independentes para retrato e paisagem quando as duas orientações forem suportadas.
- Ocultar controles virtuais ao conectar um controle físico, com opção para mantê-los visíveis.
- Feedback tátil opcional, quando o host e o dispositivo oferecerem suporte.

Não cobrir a tela inferior com botões nem capturar toques que deveriam chegar ao jogo.

### Aparência

Categorias futuras:
- Tema Escuro, Claro e Alto Contraste.
- Cor de destaque e densidade da interface.
- Escala de UI e tamanho de texto independentes da escala da imagem do jogo.
- Reduzir/desativar animações.
- Fundo personalizado da biblioteca, com fallback para o fundo padrão.

Temas alternativos completos ainda não estão implementados. A existência de suporte do renderer a quadros de fundo não significa que o usuário já tenha um seletor de tema disponível.

### Perfis

A arquitetura deve distinguir:
- **Global:** padrão utilizado por todos os jogos.
- **Por jogo:** somente os campos alterados para um título.
- **Por dispositivo:** mapeamentos de teclado/controle físico.
- **Por orientação:** disposição e controles quando retrato/paisagem estiverem disponíveis.

Um jogo herda o padrão global até que um override explícito seja criado. A interface mostrará “Usando padrão global” ou “Personalizado para este jogo”. Restaurar padrão do jogo remove apenas os overrides daquele título. Não usar somente nome do arquivo como identidade definitiva; depender de identidade estável do título e região quando o M2 estiver pronto.

## 4. Organização das configurações

A tela de configurações deve começar com categorias simples e deixar detalhes técnicos em “Avançado”.

| Categoria | Conteúdo |
|---|---|
| Geral | Idioma, inicialização, comportamento ao sair e preferências de interface |
| Biblioteca | Pastas, busca recursiva, modo de visualização, ordenação e filtros |
| Aparência | Tema, contraste, tamanho de UI, animações e fundo |
| Controles | Dispositivos, mapeamento, perfis e controles virtuais |
| Telas e layout | Orientação, disposição, escala e troca de telas |
| Gráficos e desempenho | Backend realmente disponível, V-Sync, limitador e opções de precisão validadas |
| Áudio | Volume, mute, dispositivo e latência, somente quando o caminho de áudio permitir |
| Sistema | Relógio, região e modelo apenas quando implementados |
| Diagnóstico | Avisos, logs locais, teste de compatibilidade e exportação revisável |
| Avançado | Opções experimentais com explicação de riscos e restauração fácil |

Cada configuração deve apresentar:
- Nome claro e valor atual.
- Explicação curta e ajuda contextual opcional.
- Escopo: global, por jogo ou por dispositivo.
- Efeito: imediato, próximo jogo ou reinício necessário.
- Impactos conhecidos em precisão, compatibilidade, desempenho, memória e bateria.
- Ação “Restaurar padrão”.

Configurações experimentais começam desativadas e nunca são descritas como ganho garantido.

## 5. Estado real e fases

| Recurso | Estado em 27/11/2026 |
|---|---|
| Biblioteca Grade/Lista | Disponível e agora persistida |
| Ordenação A–Z/Z–A | Disponível e agora persistida |
| Favoritos/Recentes | Disponíveis e persistidos |
| Pastas da biblioteca e busca recursiva | Configuração básica disponível por caminho digitado |
| Modo Desempenho | Disponível como opção global; efeito aplicado no próximo jogo e com ressalvas de compatibilidade |
| Fundo personalizado | Suporte de renderer; não confundir com seletor completo de tema |
| Overlay de controles virtuais | Parcial | Visibilidade global, opacidade em quatro níveis, escala global de 80%/100%/120% e presets Padrão/Compacto/Canhoto/Destro persistidos no Android; hit testing acompanha a mesma geometria do renderer. Posição individual, tamanho independente por botão e perfis por orientação ainda não estão implementados. |
| Menu rápido com pausa | Parcial | Continuar, Reiniciar, Voltar à biblioteca, Alterar layout, Trocar telas e Padrão global funcionam no PC e Android; pausa entre quadros, limpeza explícita das filas host de áudio, confirmação preventiva das ações destrutivas e botão Android MENU. Atalho editável e validação física pendentes. |
| Layouts básicos/troca de telas | Parcial | Seis modos, troca física de LCD, perfis globais por orientação, overrides por Title ID com herança/reset e indicador de escopo no menu rápido disponíveis nos dois hosts. Ajuste fino, layouts personalizados e validação física pendentes. |
| Perfis de controles | Planejados |
| Temas alternativos e alto contraste | Planejados |
| Configurações por jogo | Parciais; somente layout por Title ID está disponível. Demais categorias dependem de schema e metadados adicionais. |

## 6. Sequência de implementação

1. **Preferências da biblioteca:** concluída nesta revisão — persistir Grade/Lista e ordenação com formato versionado e testes.
2. **Pausa e menu mínimo:** primeira versão implementada no desktop e Android — estado pausado no limite de quadro, janela responsiva, scanout mantido, ações Continuar/Reiniciar/Voltar à biblioteca, limpeza explícita das filas host de áudio e confirmação preventiva das ações destrutivas. Próximo: atalho PC configurável e validação física dos backends; a detecção automática de progresso não salvo ainda não existe.
3. **Atalhos e navegação:** unificar ações de teclado, controle e toque, com detecção de conflitos e acessibilidade.
4. **Layouts de telas:** geometria compartilhada, seis modos básicos, troca física, mapeamento do stylus, perfis globais e overrides por Title ID com restauração do padrão global implementados; indicador visual de herança/override adicionado ao menu rápido; próximo: ajuste fino de posição/espaçamento e validação física.
5. **Editor de controles:** primeira etapa concluída parcialmente — visibilidade global, opacidade, escala global em três níveis e presets Padrão/Compacto/Canhoto/Destro persistidos no Android, com hit testing coerente; próximos: posicionamento individual, escala independente por botão, presets e preferências por orientação e detecção de controle físico. Depois, separar dispositivo físico, mapeamento lógico e perfil por jogo.
6. **Configurações versionadas:** categorias globais, overrides por jogo, migrações e restauração seletiva.
7. **Aparência e acessibilidade:** temas, contraste, tamanho de UI e fundos selecionáveis, sempre com fallback seguro.

## 7. Critérios de aceite

- Abrir o menu não avança o estado do jogo enquanto pausado.
- O usuário consegue fechar o menu e continuar sem reiniciar a sessão.
- O jogo não recebe toques destinados aos controles do menu.
- As mesmas ações e conceitos existem no PC e Android, com disposição adaptada.
- A navegação funciona por toque, mouse, teclado e controle conforme o host.
- Configurações persistem após reiniciar e arquivos inválidos não corrompem o perfil.
- Uma alteração por jogo nunca muda silenciosamente o padrão global.
- Opções indisponíveis ou planejadas não aparecem como funcionais.
- Testes de pausa, áudio, entrada, foco e ciclo de vida são executados em PC e Android físico antes de declarar o marco concluído.
