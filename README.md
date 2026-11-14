<p align="center">
  <img src="assets/icon/png/aceris-256.png" alt="Ícone do Aceris: uma acerola" width="160">
</p>

<h1 align="center">Aceris</h1>

<p align="center">
  Um emulador de um certo console de 2 telas, escrito em Rust, para PC e Android.
</p>

> **Status: fase inicial (v0.0.1).** O projeto ainda não executa jogos. O núcleo Rust já reúne uma base inicial de CPU ARM11, MMU, loader, kernel e GPU PICA200. No PC Linux há uma janela experimental X11 com abertura e biblioteca animada, incluindo fundos personalizados. O crate Android compila como biblioteca nativa e já expõe contratos de renderização, orientação vertical/horizontal e controles virtuais, mas ainda não existe um aplicativo Android instalável completo. Veja o [roadmap](docs/ROADMAP.md), a [arquitetura](docs/ARCHITECTURE.md) e os [frontends](docs/FRONTENDS.md).

O nome vem da **acerola**, a pequena fruta vermelha cheia de energia.

## Princípios

- **Código 100% original.** O Aceris é escrito do zero. Nenhum código, interface ou
  recurso de outros emuladores é usado ou traduzido. Veja as [regras obrigatórias para IAs](docs/REGRAS_PARA_IA.md)
  e a [política de contribuição](CONTRIBUTING.md).
- **Rust de ponta a ponta**, com um núcleo independente de plataforma.
- **Sem crates Rust externos no produto:** somente biblioteca padrão e crates internos do workspace.
- **PC e Android** compartilhando o mesmo núcleo.
- **Sem conteúdo protegido.** O projeto não inclui nem distribui jogos, firmware ou
  chaves. Você precisa usar arquivos obtidos legalmente dos seus próprios dispositivos.

## Estrutura

```
aceris/
├── crates/
│   ├── aceris-common/    erros e utilitários compartilhados
│   └── aceris-core/      núcleo de emulação (CPU, memória, loader, sistema)
├── frontends/
│   ├── desktop/          binário `aceris` (janela experimental X11 no Linux)
│   └── android/          biblioteca Rust nativa (cdylib) + recursos de identidade
├── assets/icon/          ícone (SVG, PNGs, .ico)
├── tools/                scripts de desenvolvimento
└── docs/                 arquitetura e roadmap
```

Mais detalhes em [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/ROADMAP.md](docs/ROADMAP.md) e no guia de [personalização e frontends](docs/FRONTENDS.md).

## Compilando

Requer o [Rust](https://rustup.rs) estável (1.85 ou mais recente).

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

É necessário ter o Android NDK instalado (`ANDROID_NDK_HOME`). O crate atual gera apenas a biblioteca nativa; o host/Activity Android instalável ainda precisa ser implementado em Rust. A biblioteca já expõe os contratos de UI descritos em [docs/FRONTENDS.md](docs/FRONTENDS.md).

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
