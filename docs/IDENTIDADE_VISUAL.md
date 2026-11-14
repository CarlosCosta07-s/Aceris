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

## Regras de uso
- Nome sempre escrito **Aceris** (A maiúscula); título de janela: `Aceris <versão>`.
- Tema escuro por padrão; texto claro sobre `#2A1240`/`#1A0B2E`.
- Um único vermelho de destaque por tela; verde só como apoio.
- Não distorcer, recolorir nem alterar o ícone; manter a área segura do ícone adaptativo.
- Contraste mínimo 4,5:1 para texto (o `#F5EEF8` sobre `#2A1240` cumpre).

## Ícones
- PC: `assets/icon/aceris.ico` (16–256 px) e PNGs em `assets/icon/png/`.
- Android: `frontends/android/res/mipmap-*` (ícone adaptativo, fundo `#2A1240`).
- Regenerar tudo: `cargo run --release -p aceris-icongen`.

## Abertura e biblioteca animadas
`aceris_core::video::intro` gera a abertura (2,5 s a 60 FPS, 150 quadros) como função pura
do número do quadro, então PC e Android compartilham a mesma sequência original. A biblioteca
também é renderizada pelo núcleo em RGBA8 e recebe animação sutil da própria paleta.

- PC: a biblioteca redesenha a 60 FPS no X11 e aceita fundos personalizados por sequência de quadros
  RGBA8 400x480 usando `--background-dir` e `--background-fps`.
- Android: a ABI nativa expõe renderização da biblioteca com quadro de fundo fornecido pelo host,
  orientação automática/vertical/horizontal, geometria das telas e camada/hit-test dos controles
  virtuais. O host ainda precisa aplicar a orientação à Activity e encaminhar o estado dos botões.
- Fluxo no PC: abrir o ícone → abertura → **biblioteca** (lista de jogos `.3ds`/`.cci` da pasta
  `--games`, `$ACERIS_GAMES` ou `~/Aceris/Games`). Setas/PgUp/PgDn/roda do mouse navegam, Enter ou
  segundo clique jogam, Esc volta à biblioteca durante o jogo (e sai dela). Telas em
  `aceris_core::video::library` (reutilizável no Android).
- Atalho de Linux: `packaging/linux/aceris.desktop` (`Exec=aceris --window`).
- Fundos personalizados são escurecidos na composição para preservar contraste e a identidade Aceris.

## Pendências
- Ícone na janela X11 (`_NET_WM_ICON`) e backends Win32/Cocoa.
- Tela inicial/splash e tema da interface Android (a UI ainda não existe).
- Versão monocromática do ícone (Android 13 themed icon).
