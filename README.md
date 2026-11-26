<p align="center">
  <img src="assets/icon/png/aceris-256.png" alt="Ícone do Aceris: uma acerola" width="160">
</p>

<h1 align="center">Aceris</h1>

<p align="center">
  Um emulador de um certo console de 2 telas, escrito em Rust, para PC e Android.
</p>

> **Estado de desenvolvimento em 26/11/2026.** O homebrew Lua Player Plus Cookie Clicker (CIA NoCrypto, fornecido localmente pelo mantenedor) já passou da abertura, exibiu o menu nas duas telas e entrou em gameplay básico em ensaio headless reproduzível: oito toques produziram “8 COOKIES” no HUD. O ensaio de 1.000 quadros não representa velocidade em tempo real; medições anteriores ficaram em aproximadamente 3,5 quadros/s. Áudio audível, execução em Android físico e compatibilidade de jogos comerciais permanecem sem validação. O CIA não é incluído no repositório ou na CI.

O Aceris possui núcleo ARM11/Thumb, MMU/TLB, VFPv2, memória/cache esparsa, loaders CIA/NCSD/NCCH/3DSX, kernel/SVC/IPC parcial, serviços HLE, GPU PICA200 em evolução (incluindo a unidade procedural conectada ao TEV), DSP HLE e frontends nativos. O homebrew Cookie Clicker já percorreu a cadeia guest → HID/GSP → PICA200 → LCD e demonstrou gameplay básico headless (oito toques produziram oito cookies), mas ainda está muito abaixo de tempo real; áudio audível e execução em Android físico não foram validados, e não há compatibilidade comercial comprovada. Consulte o [roadmap](docs/ROADMAP.md), [arquitetura](docs/ARCHITECTURE.md), [frontends](docs/FRONTENDS.md), [pesquisa técnica](docs/INDEPENDENT_RESEARCH.md), [tabelas PICA200](docs/AZAHAR_PICA_REGISTER_COMPATIBILITY.md) e [benchmark](docs/PERFORMANCE_TESTING.md).

O nome vem da **acerola**, a pequena fruta vermelha cheia de energia.

## Princípios

- **Implementação original.** Todo código do produto é escrito independentemente em Rust. Fontes open source de outros emuladores podem ser consultadas como referência técnica após verificação da licença do projeto e do arquivo; código, assets e testes não são copiados, traduzidos, portados nem reproduzidos estruturalmente. Veja as [regras obrigatórias para IAs](docs/REGRAS_PARA_IA.md), a [pesquisa técnica independente](docs/INDEPENDENT_RESEARCH.md) e a [política de contribuição](CONTRIBUTING.md).
- **Rust de ponta a ponta**, com um núcleo independente de plataforma.
- **Núcleo independente:** `aceris-core` usa Rust e crates internos. A apresentação host em `aceris-graphics` utiliza `wgpu`, `pollster` e `raw-window-handle`; essas dependências não fornecem a lógica de emulação PICA200.
- **PC e Android** compartilhando o mesmo núcleo.
- **Sem conteúdo protegido.** O projeto não inclui nem distribui jogos, firmware ou
  chaves. Você precisa usar arquivos obtidos legalmente dos seus próprios dispositivos.

## Estrutura

```
aceris/
├── crates/
│   ├── aceris-common/    erros e utilitários compartilhados
│   ├── aceris-core/      núcleo de emulação (CPU, memória, loader, sistema)
│   ├── aceris-graphics/  apresentação host Vulkan/OpenGL via wgpu
│   └── aceris-gl/        contexto EGL/OpenGL offscreen por carga dinâmica
├── frontends/
│   ├── desktop/          binário `aceris` com backends X11, Win32 e Cocoa
│   └── android/          NativeActivity e biblioteca Rust nativa (cdylib)
├── assets/icon/          ícone (SVG, PNGs, .ico)
├── tools/                scripts de desenvolvimento
└── docs/                 arquitetura, roadmap, pesquisa e benchmark
```

Mais detalhes em [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/ROADMAP.md](docs/ROADMAP.md), [docs/INDEPENDENT_RESEARCH.md](docs/INDEPENDENT_RESEARCH.md),
no [estudo de referências de desempenho](docs/PERFORMANCE_REFERENCE_STUDY.md)
e no guia de [personalização e frontends](docs/FRONTENDS.md).

## Compilando

Requer o [Rust](https://rustup.rs) estável **1.87 ou mais recente**, conforme `rust-version` no workspace.

```sh
cargo build            # compila núcleo e frontend desktop
cargo test --workspace # roda os testes
cargo run -- --help    # executa o frontend desktop
cargo run -- --selftest # roda o autoteste da CPU
```

### Android

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk
cargo ndk -t arm64-v8a build -p aceris-android --release
```

É necessário ter o Android NDK instalado (`ANDROID_NDK_HOME`). O projeto inclui NativeActivity em Rust, manifesto e lógica nativa. A CI compila ARM64 e já produz um APK de desenvolvimento; instalação, ciclo de vida, áudio, desempenho e gameplay ainda precisam ser validados em aparelho físico. Consulte [docs/ANDROID_BUILD.md](docs/ANDROID_BUILD.md) e [docs/ANDROID_DEVICE_TRIAL.md](docs/ANDROID_DEVICE_TRIAL.md).

## Ícone

O ícone é um desenho original, gerado por uma ferramenta própria em Rust,
[`tools/icongen`](tools/icongen), sem dependências externas. Uma única descrição
vetorial produz o SVG, os PNGs, o `.ico` do Windows e os ícones adaptativos do Android:

```sh
cargo run --release -p aceris-icongen
```

Paleta, regras de uso e pendências: [`docs/IDENTIDADE_VISUAL.md`](docs/IDENTIDADE_VISUAL.md).


## Licença

[MIT](LICENSE).
