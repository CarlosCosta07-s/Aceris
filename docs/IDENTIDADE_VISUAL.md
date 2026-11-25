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

## Abertura e launcher responsivo

aceris_core::video::intro gera a abertura original em Rust como função do número
do quadro. Os frontends Android e desktop ampliam um buffer interno reduzido para
ocupar a superfície completa, evitando multiplicar o trabalho da animação pela
resolução física do aparelho.

A biblioteca usa o renderer responsivo de aceris_core::video::library, partilhado
entre Android e desktop. Texto, painéis, ícones e áreas de toque usam a mesma
transformação lógica/física. A tipografia bitmap própria oferece letras minúsculas
e acentos portugueses, com rótulos em caixa normal para melhorar a leitura.

- **Android:** renderiza diretamente na superfície da NativeActivity, em retrato
  ou paisagem. As seções são **Jogos**, **Homebrew** e **Configurações**. Um toque
  num item tenta iniciá-lo; setas/botões físicos também podem navegar. A interface
  continua em Rust, sem camada Kotlin ou Java.
- **Desktop:** abre a biblioteca num canvas widescreen de 960×600; teclado,
  rolagem do mouse e cliques navegam. Um clique seleciona e um segundo clique
  ou Enter inicia. Esc durante o jogo volta à biblioteca.
- **Formatos atualmente listados:** .3ds e .cci na seção Jogos; .3dsx
  na seção Homebrew. A pasta padrão pode ser escolhida por --games,
  $ACERIS_GAMES ou ~/Aceris/Games.
- **Fundos personalizados no desktop:** --background-dir aceita sequências
  RGBA8 400×480 e --background-fps define a frequência de origem. O renderer
  responsivo faz cover-crop e mistura o fundo abaixo dos controles, mantendo
  contraste. A ausência de um fundo personalizado usa o tema Aceris.
- **Princípio de honestidade da interface:** não expor opções de backend,
  histórico, capas, perfis ou controles configuráveis antes de haver uma ação
  real que as persista e aplique.

A composição atual é um primeiro launcher funcional e responsivo, não a interface
final. Capa e metadados reais dependem de extrair e validar informações dos títulos
fornecidos pelo usuário. Busca, favoritos, histórico de recentes, ordenação,
seletor de diretórios, configurações por jogo, editor de controles virtuais e temas
alternativos continuam planejados, não concluídos.


## Estado de integração visual — 22/11/2026

A identidade e os assets Aceris permanecem próprios. O frontend desktop possui backends X11, Win32 e Cocoa; o Android inclui NativeActivity nativa em Rust e ícone adaptativo. A existência desses backends não significa validação visual em hardware/dispositivo. CI confirmou compilação Android ARM64 e verificações em Linux, Windows e macOS.

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.
