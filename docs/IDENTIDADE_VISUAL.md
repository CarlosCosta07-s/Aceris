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

## Abertura animada
`aceris_core::video::intro` gera a abertura (2,5 s a 60 FPS, 150 quadros) como função pura
do número do quadro, então PC e Android veem exatamente a mesma sequência:
queda da acerola, dois quicadas com achatamento, brilho diagonal, nome "ACERIS" letra a letra
e fade para preto. Feita só com a paleta acima, sem recursos de terceiros.
- PC: toca antes do jogo em `--window`; `--skip-intro` pula; `--dump-intro <DIR>` grava quadros-chave em PNG.
- Android: a UI futura deve chamar `render_frame` e `rgba_to_scaled_bgrx`/RGBA direto.
- Áudio da abertura: pendente (ainda não há backend de áudio).

## Pendências
- Ícone na janela X11 (`_NET_WM_ICON`) e backends Win32/Cocoa.
- Tela inicial/splash e tema da interface Android (a UI ainda não existe).
- Versão monocromática do ícone (Android 13 themed icon).
