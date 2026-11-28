# Identidade visual do Aceris

Base: o ícone original (uma acerola) em `assets/icon/`. Nada aqui copia a marca
de outro emulador. A fonte única de verdade é `tools/icongen/src/scene.rs`.

## Conceito
Fruta vermelha brilhante e folha verde sobre fundo roxo profundo: vibrante,
legível em tamanhos pequenos e fácil de distinguir em listas de apps.

## Paleta
Definida em `crates/aceris-common/src/brand.rs` (PC e lógica compartilhada) e em
`frontends/android/res/values/colors.xml` (Android). Testes garantem que as cores
continuam presentes no SVG do ícone e no fundo do ícone adaptativo.

| Papel | Cor | Uso |
|---|---|---|
| Fundo | `#2A1240` | fundo de janelas, splash, ícone adaptativo |
| Roxo claro / escuro | `#4A1D5E` / `#1A0B2E` | gradientes e painéis |
| Vermelho (destaque) | `#E5173F` | ações principais, seleção |
| Vermelho claro / escuro | `#FF6B6B` / `#8E0A2B` | brilho e sombra |
| Verde (secundário) | `#3E8E3A` | estados positivos, detalhes |
| Verde claro | `#86D95F` | realces |
| Texto | `#F5EEF8` | texto sobre fundo escuro |

## Tokens de interface

A biblioteca, as configurações e os menus durante a partida consomem os mesmos
tokens semânticos definidos em `crates/aceris-common/src/brand.rs`. O valor
visual pode evoluir sem espalhar cores literais por cada tela.

| Token | Cor | Aplicação |
|---|---|---|
| `SURFACE_SHADOW` | `#10071D` | sombra de painéis e cartões elevados |
| `SURFACE_BASE` | `#1A0B2E` | fundo principal |
| `SURFACE_HEADER` | `#211032` | cabeçalhos e barras |
| `SURFACE_PANEL` | `#2A1240` | painéis e superfícies internas |
| `SURFACE_CARD` | `#351A4A` | cartões e navegação inativa |
| `SURFACE_RAISED` | `#402054` | controles elevados |
| `SURFACE_SELECTED` | `#4A1D5E` | item selecionado |
| `SURFACE_BUTTON` | `#3A214B` | botões secundários |
| `BORDER` | `#644078` | contornos discretos |
| `TEXT_MUTED` | `#C7B8D3` | texto auxiliar |
| `TEXT_SUBTLE` | `#A998B8` | metadados e dicas |
| `SUCCESS_SURFACE` | `#23432D` | estados positivos |

O vermelho da acerola identifica ação primária e foco; o verde fica reservado
a estados positivos. Os tokens de texto auxiliar mantêm contraste superior a
4,5:1 nas superfícies escuras documentadas.

## Regras de uso
- Nome sempre escrito **Aceris** (A maiúscula); título de janela: `Aceris <versão>`.
- Tema escuro por padrão; texto claro sobre `#2A1240`/`#1A0B2E`.
- Um único vermelho de destaque por tela; verde só como apoio.
- Não distorcer, recolorir nem alterar o ícone; manter a área segura do ícone adaptativo.
- Contraste mínimo 4,5:1 para texto (o `#F5EEF8` sobre `#2A1240` cumpre).

## Tipografia

A família **Aceris UI** é desenhada e rasterizada pelo próprio módulo
`crates/aceris-core/src/video/font.rs`. Ela inclui letras minúsculas próprias,
acentos do português e glifos latinos usados com frequência em nomes de jogos
em espanhol e idiomas da Europa Ocidental, além de ligaturas, pontuação e
símbolos da interface. Os pesos Regular e Semibold compartilham os mesmos
glifos e são rasterizados pelo mesmo código; o Semibold dá hierarquia a
títulos, navegação e ações prioritárias. Desktop, Android e o menu durante a
partida usam esse renderer compartilhado.

Nenhuma fonte de terceiros ou fonte do sistema operacional é carregada. A
fonte bitmap 5×7 ainda não cobre alfabetos não latinos completos; esses
caracteres usam o glifo de substituição até que exista uma cobertura dedicada.
Isso mantém a aparência consistente entre plataformas e respeita a regra de
assets originais do projeto.

## Ícones
- PC: `assets/icon/aceris.ico` (16–256 px) e PNGs em `assets/icon/png/`.
- Android: `frontends/android/res/mipmap-*` (ícone adaptativo, fundo `#2A1240`).
- Regenerar tudo: `cargo run --release -p aceris-icongen`.

## Launcher horizontal, abertura e áudio externo

O launcher Android inicia em paisagem (`sensorLandscape`). A home partilha o renderer Rust com o desktop e apresenta Início, Jogos, Favoritos, Recentes, Homebrew e Configurações.

- A barra lateral permanece fixa; a home destaca Continuar jogando, contagens de Favoritos/Homebrew e o indicador de mídia externa.
- Jogos, Favoritos, Recentes e Homebrew usam cartões em grade de três colunas, com ordenação A–Z/Z–A e ações por toque/clique. Favoritos e Recentes persistem na pasta interna `.aceris` da pasta de jogos.
- Android: toque único, D-pad, arraste/scroll. Desktop: teclado, mouse e navegação horizontal/vertical.
- A abertura original foi reamostrada de 150 quadros (2,5 s) para 84 quadros (1,4 s). Android limita a resolução interna a cerca de 600×360; RGBA e BGRX são reutilizados entre quadros.
- A biblioteca mostra o estado de mídia externa, mas não informa faixa/artista: no Android API 28+ a camada Rust consulta sessões ativas anônimas e somente a categoria de uso de áudio. Com mídia `USAGE_MEDIA`, o AAudio marcado como `USAGE_GAME` é silenciado até a sessão externa terminar. Abaixo da API 28, essa detecção fica desativada.
- Nenhuma permissão de notificações é solicitada. Título, artista, capa e nome de Spotify/YouTube Music exigem autorização de Notification Listener/MediaSession e uma integração Android adicional; não fazem parte desta versão Rust-only.

### Recursos ainda não concluídos

- Capas e metadados reais por título, pesquisa textual e filtros avançados.
- Perfis e overrides por jogo.
- Save states, quick save/load, backups e rewind; aguardam serialização estável do estado completo do sistema.
- Editor de controles virtuais, transparência/auto-hide, gestos e calibração analógica.
- Indicador musical com metadados exatos. O indicador disponível mostra somente se há mídia externa ativa.

Layout, integração JNI e desempenho de abertura ainda precisam ser avaliados em aparelho físico. CI verde comprova build/testes automatizados, não o comportamento em todos os fabricantes, versões de Android, serviços de música ou drivers.

## Estado de integração visual — 22/11/2026

A identidade e os assets Aceris permanecem próprios. O frontend desktop possui backends X11, Win32 e Cocoa; o Android inclui NativeActivity nativa em Rust e ícone adaptativo. A existência desses backends não significa validação visual em hardware/dispositivo. CI confirmou compilação Android ARM64 e verificações em Linux, Windows e macOS.

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.
