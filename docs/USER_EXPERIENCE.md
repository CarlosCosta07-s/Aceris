# Experiência do usuário e funções planejadas do Aceris

> Proposta de produto e prioridades — 26/11/2026. Este documento define a experiência desejada para os usuários do Aceris. Não declara que os recursos planejados já estejam implementados. O estado real do núcleo e dos frontends continua registrado em `README.md`, `docs/ROADMAP.md` e `docs/FRONTENDS.md`.

## 1. Objetivo

Construir uma experiência de emulador moderna, simples para quem só quer abrir um jogo e suficientemente configurável para usuários avançados. PPSSPP, PCSX2, Azahar e Citra são referências de produto: devemos aprender com os problemas que resolveram e com as funções que seus usuários valorizam, mas criar identidade visual, arquitetura, código, componentes e fluxos próprios para o Aceris.

A implementação do produto continua integralmente em Rust, compartilhando o mesmo `aceris-core` entre PC e Android. Não copiar código, assets, layouts, ícones, textos, shaders, testes ou a organização interna de outros projetos. As referências deste documento servem para identificar requisitos de usuário, não para reutilização de implementação.

A pesquisa é qualitativa: documentação oficial, recursos publicados pelos projetos, discussões comunitárias e pedidos recorrentes. Não é uma pesquisa estatística representativa de todos os usuários.

## 2. O que as referências ensinam

### PPSSPP

O ponto forte a aproveitar é a flexibilidade sem obrigar o usuário a entender todos os detalhes técnicos. O PPSSPP oferece layouts de interface e controles diferentes entre retrato e paisagem, personalização do controle virtual, configurações específicas por jogo, ajustes gráficos, estados de salvamento, rewind e documentação de ajuda por categoria. A opção de rewind é desativada por padrão porque consome memória; essa é uma boa prática para o Aceris: recursos caros precisam explicar o custo e permitir ativação por jogo.

Sinais recorrentes de interesse da comunidade incluem controles virtuais ajustáveis, feedback tátil, ocultar botões que não são usados, perfis por jogo, orientação retrato, atalhos de salvamento e continuidade entre dispositivos. São pedidos e discussões de usuários, não prova de que todos prefiram as mesmas opções.

**O que aproveitar:** controles que se adaptam ao aparelho e ao jogo, configurações por jogo, explicações curtas nas opções, atalhos rápidos e recursos avançados opcionais.

### PCSX2

A interface Qt moderna popularizou uma biblioteca visual com capas, modos de grade e lista, propriedades independentes por jogo, configuração de controles simplificada com mapeamento automático e suporte nativo a controles populares. As propriedades por jogo são especialmente importantes: uma correção ou ajuste para um título não deve alterar a experiência de todos os outros.

A apresentação da nova interface Qt e seus recursos de configuração por jogo e controle receberam grande atenção em discussões comunitárias. A biblioteca visual também facilita descobrir, organizar e iniciar títulos sem navegar por pastas toda vez.

**O que aproveitar:** biblioteca com capas e metadados, grade/lista, painel de propriedades por jogo, perfis de controle e uma interface principal sem excesso de opções técnicas.

### Azahar e Citra

A referência mais específica para o Aceris é a organização de duas telas. Os projetos oferecem variações de tela única, tela ampliada, telas lado a lado, híbrida e personalizada, além de troca de telas e atalhos. As propriedades por jogo separam opções gerais, sistema, melhorias, layout, gráficos, áudio, depuração e cheats.

No Android, os pedidos mais visíveis incluem controle virtual confortável, botões redimensionáveis e ajustáveis, transparência, vibração, quick save/load e acesso rápido ao menu. Uma discussão popular sobre uma versão personalizada do Azahar destaca justamente redesenho dos controles, atalhos nas bordas e salvamento automático ao sair. Esses recursos devem ser implementados de forma nativa e coerente com a identidade do Aceris, não reproduzidos visualmente.

**O que aproveitar:** layouts de duas telas flexíveis, menu durante a partida, configuração por jogo, atalhos, controles virtuais personalizáveis e ferramentas de compatibilidade.

## 3. Estado atual do Aceris: o que já existe

A inspeção de `crates/aceris-core/src/video/library.rs`, `crates/aceris-core/src/video/frontend.rs`, `frontends/android/src/app.rs`, `frontends/android/src/ui.rs` e `docs/FRONTENDS.md` identifica estes recursos existentes:

- Tela inicial, início da biblioteca e páginas de Jogos, Favoritos, Recentes, Homebrew e Configurações.
- Biblioteca com visualização de grade e lista selecionável no modo paisagem; o fallback em retrato continua usando lista adaptável.
- Pesquisa funcional por título, nome do arquivo e caminho, sem diferenciar maiúsculas/minúsculas ou acentos. Desktop usa texto do teclado do sistema; Android oferece teclado virtual desenhado em Rust e aceita caracteres Unicode de teclados físicos.
- Favoritos e Recentes persistidos localmente com identidade por caminho completo (inclui migração dos antigos registros por nome); ordenação A–Z/Z–A e atualização manual da biblioteca.
- Plano de fundo personalizado na biblioteca, com suporte a quadros animados fornecidos pelo host.
- O scanner compartilhado em Rust aceita múltiplas raízes e recursão opcional, sem seguir links simbólicos e deduplicando diretórios/arquivos sobrepostos. `LibraryScanConfig` também oferece gravação/carregamento versionado das pastas e da opção de recursão; os frontends ainda não conectam essa configuração à interface.
- Apresentação das duas telas do 3DS com ajuste proporcional ao espaço disponível.
- Camada básica de controle virtual e identificação de toque na tela inferior.
- Entrada básica por teclado, toque e alguns eventos de controle físico encaminhados pelo host.
- Pastas de dados de usuário por jogo para arquivos de save/extdata nos caminhos HLE atualmente implementados.
- Diagnósticos de carregamento/execução em evolução.

**Limites atuais que a nova experiência precisa resolver:** os frontends ainda chamam o scanner de uma única pasta e mantêm a recursão desativada, embora o core já ofereça API de varredura e persistência de configuração para múltiplas raízes e recursão opcional. Ainda não existe interface para adicionar/remover pastas nem seleção SAF no Android. O scanner lista `.3ds`, `.cci`, `.cia` e `.3dsx`; CIA só pode ser carregado nas condições suportadas pelo núcleo (conteúdo de boot em claro e NCCH NoCrypto), sem instalação persistente ou gerenciamento de atualizações. A pesquisa atua somente nos arquivos já encontrados: não consulta Title ID/metadados externos. A página de configurações da biblioteca ainda é básica. Não há editor completo de controles, perfis por jogo, painel de propriedades por jogo ou menu de pausa completo. Não há sistema de save states/rewind disponível ao usuário. O armazenamento de save data do jogo não deve ser confundido com save state do emulador.

O layout do jogo atualmente compõe as telas conforme a orientação disponível, mas ainda não oferece todas as opções avançadas de disposição descritas abaixo.

## 4. Experiência proposta

### 4.1 Primeiro início

Um assistente curto deve explicar o que o Aceris faz e orientar o usuário a:

1. Escolher uma ou mais pastas onde mantém seus próprios jogos.
2. Conceder acesso às pastas necessárias no Android pelo seletor de documentos adequado à plataforma.
3. Verificar os arquivos de sistema que o jogo ou recurso realmente exigir, sem baixar nem fornecer firmware, chaves ou jogos.
4. Executar uma varredura e mostrar quantos arquivos foram encontrados, reconhecidos, duplicados ou rejeitados, com o motivo.
5. Escolher idioma, tema e perfil de controles inicial.

O assistente deve poder ser reaberto nas Configurações. Nunca deve exigir que o usuário mova os arquivos para uma única pasta privada se a plataforma permitir acesso seguro a pastas escolhidas por ele.

### 4.2 Início

A tela inicial deve ser um painel de retomada, não apenas uma lista de arquivos:

- **Continuar:** abre o último jogo usado, quando ele ainda está disponível.
- **Continuar jogando:** mostra os últimos títulos com capa, nome e última sessão.
- **Favoritos:** acesso imediato aos títulos marcados.
- **Adicionados recentemente:** itens descobertos na última varredura.
- **Estado do sistema:** avisos de arquivos ausentes, atualização da biblioteca ou configuração incompleta.
- **Ações rápidas:** adicionar pasta, atualizar biblioteca e abrir configurações.

Em telefones, cartões maiores e alvos de toque generosos. Em PC, aproveitar a área horizontal e permitir teclado e controle sem exigir mouse.

### 4.3 Biblioteca

A biblioteca deve oferecer duas visualizações selecionáveis:

- **Grade:** capas, título e indicadores compactos; adequada para descoberta visual.
- **Lista:** título e metadados em colunas; melhor para bibliotecas grandes e telas menores.

Funções necessárias:

- Pesquisa por nome, Title ID quando disponível e caminho.
- Filtros por favoritos, recentes, homebrew, formato, região e estado de compatibilidade quando houver dados confiáveis.
- Ordenação por nome, última execução, data de adição e tamanho.
- Seleção de várias entradas para marcar favoritos, atualizar metadados ou remover referências da biblioteca sem apagar os arquivos originais.
- Atualização manual e automática ao iniciar, com opção de desativar.
- Gerenciamento de várias pastas e varredura recursiva opcional.
- Identificação de duplicatas sem apagar arquivos automaticamente.
- Capas e metadados opcionais. Primeiro usar dados locais fornecidos pelo usuário; qualquer fonte externa deve ser opcional, identificada e limitada a metadados/imagens autorizados. Nunca baixar jogos, firmware ou chaves.
- Estado visível para formatos não suportados: explicar o motivo em vez de esconder o arquivo ou falhar silenciosamente.

Os formatos mostrados na interface devem refletir exatamente o suporte real do loader. `.cia` só deve aparecer como instalável/gerenciável depois de existir um fluxo persistente de instalação, atualização e remoção testado. Não apresentar apenas a leitura de CIA NoCrypto como suporte completo a instalação de títulos.

### 4.4 Página de um jogo

Ao selecionar um título, abrir uma página própria com:

- Capa ou arte local, título, formato, caminho e identificador/região quando conhecidos.
- Estado de compatibilidade e limitações verificadas, sem inventar classificação.
- Última execução e tempo de jogo somente quando a medição estiver disponível e for confiável.
- Botão principal **Jogar**.
- Favoritar/desfavoritar.
- **Configurações deste jogo**.
- Gerenciamento de saves e dados do usuário.
- Abrir pasta do jogo ou dos dados do jogo, respeitando permissões da plataforma.
- Exportar relatório de diagnóstico para esse jogo.
- Remover da biblioteca sem excluir o arquivo original.

### 4.5 Menu rápido durante a partida

O menu deve ser acessível por um botão visível no Android e por atalho configurável no PC. Deve pausar a emulação antes de exibir ações que alterem o estado.

Organização sugerida:

**Jogo:** Continuar, Reiniciar, Encerrar.

**Progresso:** Salvar estado, Carregar estado, slots, backup e, futuramente, Rewind.

**Telas:** Layout, trocar telas, tela superior apenas, tela inferior apenas, telas lado a lado, tela principal ampliada com secundária em janela flutuante e layout personalizado.

**Controles:** Editar controles de toque, escolher perfil de controle físico, testar entradas e ajustar sensibilidade.

**Imagem e som:** ajustes específicos daquele jogo, sem perder as configurações globais.

**Ferramentas:** screenshot, estatísticas de desempenho, relatório de compatibilidade e cheats quando suportados.

O menu deve ser compacto no celular e uma barra/painel no desktop. As ações mais usadas (continuar, quick save, quick load, trocar telas e screenshot) precisam ficar a um toque/atalho de distância.

### 4.6 Duas telas: recurso central do Aceris

A apresentação deve oferecer estes modos, quando cada um estiver validado:

1. **Original:** telas na disposição padrão, mantendo proporções.
2. **Vertical:** tela superior acima da inferior.
3. **Lado a lado:** duas telas alinhadas horizontalmente.
4. **Tela superior ampliada:** tela superior maior e inferior em janela menor.
5. **Tela inferior ampliada:** o inverso, útil para jogos que usam muito o toque.
6. **Tela única:** escolher superior ou inferior.
7. **Janela separada no desktop:** opcional, quando o backend de janela permitir.
8. **Personalizado:** tamanho, posição e espaçamento ajustáveis, com botão para restaurar o padrão.

Também haverá ação de **Trocar telas**, ajuste de escala sem distorcer a proporção, opção de manter molduras/bordas e perfis separados para retrato e paisagem. O toque deve continuar correspondendo à tela inferior mesmo quando o usuário altera o layout.

### 4.7 Controles

**Controles físicos e teclado**

- Detectar dispositivos conectados e exibir o nome do dispositivo.
- Mapeamento automático para controles conhecidos.
- Mapeamento manual de cada botão e eixo.
- Perfis globais e por jogo.
- Ajuste de deadzone, sensibilidade e inversão dos eixos.
- Atalhos configuráveis para menu, pause, salvar/carregar estado, turbo, trocar telas e screenshot.
- Teste visual de botões, eixos e toque.
- Vibração/rumble quando o dispositivo, sistema operacional e jogo forem suportados.

**Controles virtuais no Android**

- Mover, redimensionar, ocultar e alterar opacidade de cada botão.
- Presets: padrão, compacto, canhoto, destro e personalizado.
- Ajustar posição do Circle Pad e tamanho da área de toque/stylus.
- Feedback tátil opcional.
- Ajustes independentes para retrato e paisagem.
- Ocultar o overlay automaticamente quando um controle físico for conectado, com opção de manter.
- Gestos/atalhos nas bordas para abrir o menu, pausar, trocar telas ou acionar turbo; todos configuráveis e desativáveis.

Não colocar botões sobre a área de toque de forma que impeçam desenhar ou selecionar objetos na tela inferior.

### 4.8 Configurações por jogo

Este é um requisito de arquitetura, não apenas uma tela. Cada jogo deve poder herdar as opções globais e gravar somente as alterações feitas para ele.

Categorias:

- Geral: nome interno, região, idioma e propriedades disponíveis.
- Sistema: opções de relógio/modelo/região apenas quando realmente implementadas.
- Gráficos: backend, resolução interna, filtros, sincronização e opções de precisão.
- Layout: disposição das duas telas e escala.
- Áudio: volume, saída, latência e modo de som, conforme o suporte real.
- Controles: perfil e remapeamentos.
- Cheats: lista, ativação individual e origem do arquivo.
- Diagnóstico: avisos, logs e testes disponíveis para aquele título.

Cada opção deve dizer o que faz, se pode reduzir desempenho e se pode causar falhas gráficas ou incompatibilidade. Um botão **Restaurar padrão deste jogo** remove apenas os overrides daquele título. Nunca alterar silenciosamente o perfil global ao corrigir um único jogo.

A identidade do perfil deve usar informações estáveis do título quando disponíveis (por exemplo, Title ID e região), com fallback seguro para arquivo sem metadados. Mudança de caminho não deve apagar o perfil; cópias com versões/regiões diferentes não devem compartilhar configurações por engano.

### 4.9 Gráficos e desempenho

**Perfis simples:** Compatibilidade, Equilibrado e Qualidade. O perfil escolhe apenas opções existentes e testadas no Aceris; não ativa hacks desconhecidos.

**Ajustes avançados futuros:**

- Resolução interna nativa e multiplicadores quando o caminho PICA200 suportar renderização ampliada real.
- VSync e limitador de quadros.
- Filtros de textura e antialiasing somente quando implementados pelo backend e validados.
- Cache de shaders e opções de compilação quando existirem.
- Estatísticas opcionais: FPS, velocidade relativa, frametime, uso de CPU/GPU, backend e avisos.

Não apresentar escala de resolução como melhoria real se apenas ampliar a imagem LCD já renderizada. Diferenciar claramente resolução interna do emulador e escala de apresentação.

Frameskip, sincronização relaxada e hacks de precisão devem vir desativados por padrão, com aviso sobre os possíveis defeitos. O objetivo principal é desempenho estável sem sacrificar fidelidade nem criar opções que mascarem bugs do emulador.

### 4.10 Áudio

- Volume geral e mute acessíveis durante o jogo.
- Seleção de dispositivo de saída quando disponível.
- Latência/buffer com presets simples e opção avançada.
- Modo estéreo/mono somente quando o sistema emulado e o caminho de áudio suportarem.
- Indicador de áudio ativo e diagnóstico de inicialização.
- Opção de reduzir o áudio do jogo quando outra mídia externa estiver tocando, sem ler o conteúdo da mídia.
- Captura/diagnóstico de áudio para relatórios, sem gravar conversas ou microfone sem ação explícita.

### 4.11 Progresso e segurança dos saves

Separar claramente dois conceitos:

- **Save do jogo:** dados que o próprio jogo grava no sistema emulado.
- **Save state:** fotografia do estado completo do emulador, usada para retomar exatamente aquele momento.

Quando save states forem implementados, oferecer vários slots nomeados, quick save/load, data/hora, screenshot de prévia, confirmação ao sobrescrever/carregar, backup e recuperação. O estado precisa possuir versão de formato, identificação do jogo e validação antes de substituir uma sessão.

Auto-save ao sair deve ser um recurso opcional separado dos slots manuais, com rotação de backups; nunca sobrescrever silenciosamente o único save manual. Rewind deve ser opcional e informar o consumo de memória. Atualizações do Aceris não devem apagar saves ou estados incompatíveis: devem preservar o arquivo e mostrar uma mensagem explicativa.

### 4.12 Cheats, ferramentas e recursos avançados

- Gerenciador de cheats por jogo, com habilitação por código, descrição, origem e botão para desativar tudo.
- Importação de arquivos de cheats locais; nunca distribuir códigos/arquivos protegidos de terceiros.
- Ferramentas de compatibilidade com teste de entrada, vídeo, áudio, serviço e diagnóstico.
- Exportação de logs sanitizados, mostrando caminho e dados técnicos necessários, sem chaves, conteúdo do jogo ou dados pessoais por padrão.
- Substituição de texturas locais, somente quando o pipeline de texturas estiver pronto e o pacote for fornecido pelo usuário.
- Recursos de Amiibo/NFC, câmera, microfone, giroscópio, acelerômetro e multiplayer somente quando o HLE e a plataforma correspondente estiverem implementados e testados.
- Integração com RetroAchievements é uma possibilidade posterior, sujeita à compatibilidade, autenticação, regras do serviço e privacidade do usuário.

### 4.13 Preferências gerais

- Português do Brasil como idioma inicial, com estrutura preparada para traduções.
- Temas escuro e claro, mais escala de interface e opção de alto contraste.
- Navegação por mouse, teclado, controle e toque.
- Pastas da biblioteca e armazenamento com acesso explícito.
- Backup/exportação das configurações.
- Canal de atualização estável e canal de testes somente quando houver processo de release confiável; mostrar changelog e permitir adiar.
- Privacidade: sem telemetria invasiva, sem upload automático de ROMs/saves/logs e sem contas obrigatórias para usar o emulador.

## 5. Ordem recomendada de implementação

A ordem abaixo é uma proposta de produto. O desempenho, a fidelidade da PICA200 e o áudio continuam sendo prioridades técnicas do núcleo e devem avançar em paralelo; uma interface mais completa não substitui essas correções.

### Etapa A — Tornar a biblioteca realmente utilizável

1. Pesquisa, filtros e troca entre grade/lista.
2. Gerenciador de múltiplas pastas, atualização incremental e varredura recursiva opcional.
3. Melhorar os cartões e a página de detalhes; capas locais opcionais.
4. Corrigir a exibição de formatos suportados e distinguir carregar de instalar CIA.
5. Página de configurações gerais com idioma, tema, armazenamento e diretórios.

**Critério de conclusão:** usuário adiciona uma pasta, vê progresso, encontra seu jogo, filtra/ordena, favoritar uma entrada e abre os detalhes sem precisar navegar pelo gerenciador de arquivos a cada execução.

### Etapa B — Controles e menu rápido

1. Menu de pausa comum ao PC e Android.
2. Atalhos globais e configuração de atalhos.
3. Troca entre os layouts de duas telas e tela única.
4. Editor dos controles virtuais Android.
5. Mapeamento e perfis de teclado/gamepad.

**Critério de conclusão:** usuário consegue pausar, continuar, trocar as telas e configurar controles sem sair do jogo; perfil de um jogo não afeta os demais.

### Etapa C — Configurações por jogo e progresso

1. Modelo de configuração global + overrides por título.
2. Tela de propriedades por jogo.
3. Save-state com formato versionado, slots, backup e validação.
4. Screenshots e estatísticas durante a partida.
5. Diagnóstico por jogo e exportação de relatório sanitizado.

**Critério de conclusão:** configurações isoladas e saves sobrevivem a reinícios, arquivos movidos e falhas de carregamento sem perda silenciosa de dados.

### Etapa D — Melhorias gráficas e sonoras

1. Expor as opções gráficas e de áudio que já tenham implementação real.
2. Perfis Compatibilidade/Equilibrado/Qualidade.
3. Estatísticas de desempenho confiáveis.
4. Resolução interna ampliada, filtros e rewind apenas após suporte técnico e testes próprios.

**Critério de conclusão:** cada opção exibida possui efeito demonstrável, explicação, teste e possibilidade de voltar ao padrão.

### Etapa E — Recursos avançados

Cheats, substituição de texturas, Amiibo/NFC, sensores, câmera, microfone, multiplayer e RetroAchievements. Cada recurso deve ser condicionado a suporte real do núcleo, testes e permissões da plataforma.

## 6. Requisitos transversais de qualidade

- A versão PC e a Android devem compartilhar conceitos, configurações e lógica Rust, mas cada uma deve usar ergonomia adequada à plataforma.
- O usuário sempre deve poder cancelar uma varredura ou operação demorada.
- Nenhuma operação deve apagar jogos, saves ou estados sem confirmação explícita.
- Configuração avançada não deve aparecer como opção funcional antes de existir implementação.
- Toda opção avançada deve explicar impacto em desempenho e compatibilidade.
- Erros devem indicar o que aconteceu, qual arquivo/jogo foi afetado e o próximo passo possível.
- A interface deve continuar responsiva enquanto varre a biblioteca, cria backups ou prepara jogos.
- Recursos experimentais devem ter rótulo e descrição de limitações.
- Nenhuma conclusão de compatibilidade pode ser inferida apenas porque o arquivo apareceu na biblioteca ou a CI compilou.

## 7. Referências consultadas

### Documentação e projetos

- PPSSPP — configurações de sistema, UI, estados e controles: https://www.ppsspp.org/docs/settings/system/
- PPSSPP — índice de configurações: https://www.ppsspp.org/docs/settings/
- PPSSPP — configurações gráficas e recomendações: https://www.ppsspp.org/docs/settings/graphics/
- PPSSPP — Rewind e impacto de memória: https://www.ppsspp.org/docs/reference/rewind/
- PCSX2 — configurações gerais e propriedades por jogo: https://pcsx2.net/docs/configuration/general/
- PCSX2 — repositório e descrição da biblioteca, capas, configurações por jogo, controles e RetroAchievements: https://github.com/PCSX2/pcsx2
- Azahar — projeto, plataformas e objetivo: https://github.com/azahar-emu/azahar
- Azahar — opções de disposição de telas: https://github.com/azahar-emu/azahar/blob/master/src/common/settings.h
- Azahar — categorias das propriedades por jogo: https://github.com/azahar-emu/azahar/blob/master/src/citra_qt/configuration/configure_per_game.cpp

### Discussões e feedback comunitário

- Discussão muito votada da atualização da interface Qt do PCSX2, citando grade/lista, capas, configuração por jogo e mapeamento automático de controles: https://www.reddit.com/r/emulation/comments/uvf7cd/
- Discussão da atualização do PCSX2 com configuração por jogo e suporte nativo a controles DualShock 4/DualSense: https://www.reddit.com/r/Games/comments/uvplsx/
- Discussão sobre redesenho de controles, hot corners e quick save/load em uma versão personalizada do Azahar: https://www.reddit.com/r/EmulationOnAndroid/comments/1n96nxc/
- Discussão das melhorias do Citra Enhanced, incluindo opacidade, reposicionamento e feedback tátil dos controles: https://www.reddit.com/r/EmulationOnAndroid/comments/1chiwde/
- Pedidos de usuários do PPSSPP por controles ocultáveis, perfis por jogo, orientação retrato, vibração e integração com armazenamento em nuvem: https://forums.ppsspp.org/showthread.php?pid=43320
- Lista histórica de pedidos de funcionalidades do PPSSPP, incluindo textura personalizada, configurações por jogo, rotação e atalhos de save: https://forums.ppsspp.org/showthread.php?pid=20824

As discussões comunitárias refletem experiências e preferências de quem participou delas; não devem ser interpretadas como votação oficial nem como consenso universal.
