# Frontends PC e Android

Para consultar a distinção entre suporte do core, recursos parcialmente integrados e opções realmente expostas ao usuário, veja a [Matriz de capacidades reais](CAPABILITY_MATRIX.md). A ordem de implementação está no [Plano de implementação do produto](PRODUCT_IMPLEMENTATION_PLAN.md). A arquitetura planejada para pastas locais e Android SAF está em [FILE_SOURCE_ARCHITECTURE.md](FILE_SOURCE_ARCHITECTURE.md).


Este documento descreve o estado atual dos hosts do Aceris. O núcleo de emulação continua independente de plataforma e é compartilhado entre desktop e Android.

## Desktop

- O frontend desktop possui backends de janela X11 (Linux), Win32 (Windows) e Cocoa (macOS), além de sinks de áudio ALSA, WinMM e AudioQueue/CoreAudio, respectivamente.
- A biblioteca usa canvas próprio. Durante a execução de um jogo, as telas 400×240 e 320×240 são apresentadas lado a lado; a janela volta ao layout da biblioteca ao encerrar o jogo.
- O presenter `aceris-graphics` usa `wgpu` para enviar os frames LCD já compostos ao host GPU. Ele acelera upload/apresentação, não substitui o rasterizador PICA200 do core nem executa DRAWARRAYS/DRAWELEMENTS.
- O crate `aceris-gl` mantém um contexto EGL/OpenGL offscreen independente. Ele não está integrado ao `System` como rasterizador PICA.

## Android NativeActivity

O aplicativo Android usa `frontends/android`, com NativeActivity e lógica do host em Rust. A interface compartilhada mostra a biblioteca, navegação, Favoritos, Recentes e Configurações. A atividade aceita rotação do sensor (`sensor`); por padrão, usa perfil Vertical em portrait e Lado a lado em landscape, com preferências separadas. Quando o modo Auto é selecionado, a geometria escolhe lado a lado em landscape e vertical em portrait; o menu permite selecionar outros layouts.

O workflow **Build Android APK** compila e empacota um APK debug ARM64. Isso confirma geração do artefato, não valida instalação, renderização, toque, áudio ou desempenho em um aparelho real. O estado de teste e os passos de instalação estão em [ANDROID_DEVICE_TRIAL.md](ANDROID_DEVICE_TRIAL.md).

### Biblioteca e arquivos aceitos

O core compartilha `System::load_executable_file` entre os hosts. O scanner da biblioteca lista:

- `.3ds` e `.cci` para imagens NCSD descriptografadas (NCCH NoCrypto);
- `.cia` para contêineres com conteúdo de boot em claro e NCCH NoCrypto; o Aceris inicia diretamente esse conteúdo, mas ainda não instala títulos de forma persistente;
- `.3dsx` para homebrew.

A presença de um CIA na biblioteca **não significa instalação persistente, gerenciamento de atualizações ou suporte a conteúdo criptografado**. O carregador continua validando o arquivo ao iniciar e apresenta um diagnóstico quando o conteúdo não é compatível. Desktop e Android agora inicializam a biblioteca com `Library::from_scan_settings`, carregando as raízes e a opção de recursão gravadas em `.aceris/library_scan_v1.txt`, dentro da pasta de dados da biblioteca; se a configuração estiver ausente ou inválida, usam a pasta de jogos original sem recursão. A página Configurações permite adicionar uma pasta digitando seu caminho, remover raízes existentes e alternar a busca em subpastas. A pasta precisa existir e estar acessível ao processo antes de ser cadastrada. No Android, o teclado virtual inclui `/` e `.` para caminhos comuns; essa entrada por caminho não substitui a seleção SAF, que continua pendente para armazenamento compartilhado.

### Política de criptografia

O fluxo normal de lançamento, usado pela biblioteca tanto no desktop quanto no Android, aceita somente imagens descriptografadas. NCSD (`.3ds`/`.cci`) precisa apresentar NCCH NoCrypto; CIA precisa ter o conteúdo de boot sem criptografia externa e o NCCH com NoCrypto. Arquivos criptografados são recusados antes de iniciar o processo, com uma mensagem que informa a política. Isso segue o comportamento atual documentado pelo Azahar: o usuário prepara uma cópia descriptografada a partir do próprio console.

O núcleo ainda mantém rotinas de baixo nível com chaves explícitas para testes e pesquisa de formatos; elas não fazem parte do fluxo de lançamento da biblioteca nem representam suporte de produto a jogos criptografados. O Aceris não descobre, incorpora ou persiste chaves. O carregamento direto de CIA também não equivale à instalação persistente de títulos do Azahar.

### Apresentação e gráficos

O frontend mantém uma referência adicional de `ANativeWindow` enquanto a surface está ativa. O presenter host pode usar Vulkan ou OpenGL ES quando disponível e conserva o caminho CPU como fallback. A apresentação recebe o framebuffer LCD já composto pelo core.

A apresentação Android ainda precisa de validação física em diferentes GPUs/drivers. Um build ARM64 ou APK concluído não demonstra que a surface, o driver ou o ritmo de quadros funcionam corretamente no dispositivo do usuário.

### Entrada e controles

A NativeActivity recebe eventos nativos de tecla, movimento/toque e eventos de joystick e encaminha o estado de entrada ao core. O mapeamento-base de teclas cobre D-pad, A/B/X/Y, L1/R1, Start e Select. L2/R2 ainda não são encaminhados como ZL/ZR: esses botões New 3DS usam a memória adicional `ir:rst`, ainda não implementada. Para eventos de joystick, o eixo primário X/Y é convertido para o Circle Pad (±156), com deadzone padrão de 16% e inversão de Y. O core publica botões e touchscreen pelo HID guest quando o título requisita os handles/memória compartilhada. Um ensaio headless com o homebrew Cookie Clicker demonstrou que toques injetados podem percorrer HID → guest → gameplay → PICA/LCD; isso não equivale à validação física de toque ou gamepad.

A C ABI também oferece renderização e hit-test para uma camada de controles virtuais. Remapeamento, seleção/perfis de dispositivos, eixos adicionais, gatilhos e a combinação completa de múltiplos dedos, gamepad virtual e gamepad físico ainda precisam de uma rodada específica de validação no aparelho.

### Menu rápido durante a partida (PC e Android)

- **Android:** a tela do jogo mostra um botão discreto **MENU** no canto superior direito; a tecla/gesto Voltar também abre o painel. Tocar no botão MENU é consumido pela interface e não é encaminhado como toque da caneta ao jogo.
- **PC:** Esc abre e fecha o painel rápido. A escolha é navegável por setas/Enter e mouse. O atalho ainda é fixo; personalização de atalhos permanece pendente.
- O painel apresenta Continuar, Reiniciar, Voltar à biblioteca, Alterar layout, Trocar telas, Padrão global e Capturar tela no desktop; no Android também inclui Restaurar controles, Escopo dos controles, Editar controles e Diagnóstico. Reiniciar recria a sessão do mesmo arquivo; Voltar à biblioteca encerra a sessão atual. Ambas as ações exibem confirmação preventiva de que o progresso não salvo pelo próprio jogo pode ser perdido; o Aceris ainda não detecta automaticamente um estado de progresso não salvo.
- **Alterar layout** e **Trocar telas** oferecem seis modos: Lado a lado, Vertical, Superior ampliada, Inferior ampliada, Somente superior e Somente inferior. No desktop, a janela acompanha o canvas do modo; no Android, a disposição é ajustada à surface e à orientação atual. A troca preserva a identidade física do LCD inferior para o stylus. As preferências globais por orientação e os overrides por título são salvos em `.aceris/screen_layout_preferences_v2.txt`; o formato v1 é lido como migração automática. NCSD/CCI e CIA usam identidade Title ID validada; 3DSX sem identidade herda o perfil global. A ação **Padrão global** remove os overrides do título atual sem alterar os padrões globais.
- Enquanto o painel está aberto, o frontend continua processando eventos e redesenhando o último scanout; não chama o próximo passo de emulação guest. Ao abrir o painel, o Android solicita pausa, aguarda a transição e limpa a fila AAudio antes de retomá-la com `requestStart`; o fluxo também é pausado e limpo quando a NativeActivity vai para segundo plano. No PC, a fila host é descartada pelo backend (ALSA `drop`/`prepare`, WinMM `waveOutReset` e AudioQueue `Stop` imediato), e o resampler desktop reinicia seu histórico.
- **Capturar tela:** salva a composição atual dos LCDs como PNG, sem incluir o painel do menu nem os controles virtuais. O menu apresenta confirmação de sucesso/erro. No Android grava em `<pasta de dados do app>/.aceris/screenshots`; no desktop grava em `<pasta de configurações>/screenshots` e imprime o caminho no terminal. Ainda não há diálogo para escolher destino, publicação na galeria/SAF ou validação física.
- **Diagnóstico:** ativa um painel durante a partida com PC do ARM11, número e primeira requisição IPC não tratada e, quando a coleta opt-in está ativa, resumo de FPS apresentado e tempos do host. Esses dados não representam velocidade guest; métricas de guest ainda não estão disponíveis.
- Save states, Rewind, cheats, remapeamento de controles físicos e atalhos configuráveis ainda não são ações utilizáveis. O Android já lê o eixo primário X/Y de controles USB/Bluetooth e normaliza para o Circle Pad com deadzone de 16%, mas essa entrada ainda precisa de testes físicos e perfis/remapeamento.
- Esta primeira implementação ainda precisa passar pela CI e por testes visuais/entrada/áudio em PC e aparelho Android físico.


### Áudio e ciclo de vida

O host Android usa AAudio. Há lógica para classificar a saída do Aceris como áudio de jogo e reduzir a própria saída enquanto outra mídia externa está ativa, sem identificar o player ou ler metadados da faixa. O host também pausa e limpa explicitamente a fila ao abrir o menu rápido ou ao ir para segundo plano; ao retomar, inicia o stream sem as amostras antigas. A existência do sink não significa que o áudio do console esteja correto: a inicialização de áudio do homebrew de teste ainda apresenta falha e não há confirmação de saída audível.

O ciclo de vida de NativeActivity, pause/resume, destruição e recriação de surface, rotação e monitoramento de mídia devem ser testados em aparelho físico. A CI não substitui esse teste.

## Contratos compartilhados de UI

- A UI e a biblioteca são desenhadas em Rust e usam a mesma identidade visual no desktop e Android.
- Nas páginas Jogos, Favoritos, Recentes e Homebrew, a biblioteca horizontal oferece alternância funcional entre Grade e Lista. A navegação vertical acompanha o modo escolhido e, na Lista, os alvos de toque/clique correspondem às linhas renderizadas. A visualização e a ordenação A–Z/Z–A são persistidas em `.aceris/library_preferences_v1.txt`, com formato versionado e retorno aos padrões seguros quando o arquivo está ausente ou inválido; essas preferências são globais da biblioteca, não perfis por jogo.
- A pesquisa filtra o catálogo em memória por título, nome do arquivo e caminho, com comparação sem distinção de caixa/acentos. Android recebe Unicode do NDK para teclados físicos e oferece teclado virtual Rust básico para toque. Desktop usa os eventos de texto nativos de X11, Win32 e AppKit. O teclado virtual tem letras latinas, espaço, apagar, limpar e OK; não é um IME completo.
- Fundos personalizados podem ser fornecidos ao renderer como quadros RGBA8 400×480. A ABI não decodifica vídeos nem retém ponteiros do host após a chamada.
- A ABI preserva helpers de orientação/geometria e de controles virtuais para hosts que optem por usá-los. O aplicativo Android atual é horizontal; não se deve inferir que todos os modos de orientação oferecidos pela ABI estejam expostos na interface.
- Favoritos e Recentes são persistidos em arquivos da pasta de dados do Aceris usando identificadores derivados do caminho completo, evitando colisões entre arquivos com o mesmo nome em pastas diferentes. Registros antigos, baseados somente no nome, são migrados para a pasta de jogos original. Metadados/capas reais, seleção SAF, perfis por jogo, editor de controles e temas alternativos permanecem como trabalho futuro.

- As páginas Jogos, Favoritos, Recentes e Homebrew oferecem filtro por extensão (.3DS, .CCI, .CIA, .3DSX), além da opção Todos. O filtro é compartilhado pela biblioteca Rust e funciona nos layouts responsivos em retrato e paisagem. Ele classifica pelo nome do arquivo, não valida conteúdo; em particular, filtrar .CIA não significa que o Aceris ofereça instalação persistente de títulos.

## Modo Desempenho (opcional)

Configurações da biblioteca → **MODO DESEMPENHO**. A escolha é salva em
`<pasta de configurações>/performance_mode.txt` (`1`/`0`) e aplicada com
`System::set_fast_memory(true)` antes de carregar o próximo jogo, tanto no
Android quanto no desktop. No desktop, `ACERIS_FAST_MEMORY=1` também ativa o
modo, e o benchmark aceita `--fast-memory`. O padrão continua sendo o modelo
preciso de D-cache (docs/PERFORMANCE_LOW_END_PLAN.md). O modo é mais rápido,
mas pode quebrar jogos que dependem da incoerência do cache.

## Estado de validação

| Área | Evidência atual | O que falta |
|---|---|---|
| Desktop | Backends X11, Win32 e Cocoa; build/testes na CI | Execução visual/áudio em cada SO e GPU física |
| Android ARM64 | Biblioteca nativa e APK debug empacotado pela CI | Instalação e teste em aparelho real |
| Input guest | HID para botões/toque; ensaio headless Cookie Clicker | Touchscreen físico, multi-touch e gamepads |
| Áudio | Sink AAudio e motor DSP/CSND em evolução | Corrigir Sound.init e confirmar áudio audível |
| GPU host | Presenter wgpu com fallback CPU | Validar drivers/surfaces em hardware real |
| PICA200 | Rasterização CPU e cadeia guest-driven demonstrada | Fechar recursos pendentes e validar imagens contra hardware |

O resultado headless do Cookie Clicker demonstra gameplay básico em um cenário controlado, mas o desempenho medido anteriormente ficou em aproximadamente 3,5 quadros/s. Não é desempenho jogável em tempo real e não comprova compatibilidade com jogos comerciais.
