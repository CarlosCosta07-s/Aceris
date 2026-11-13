<p align="center">
  <img src="assets/icon/png/aceris-256.png" alt="Ícone do Aceris: uma acerola" width="160">
</p>

<h1 align="center">Aceris</h1>

<p align="center">
  Um emulador de um certo console de 2 telas, escrito em Rust, para PC e Android.
</p>

> **Status: fase inicial (v0.0.1).** O projeto ainda não executa jogos. O núcleo Rust já reúne um interpretador ARM11 A32/Thumb com modos privilegiados, SRS/RFE, CP15/MMU com formatos XP=0/XP=1, FCSE, TLB de software com lockdown e WFI/IRQ/FIQ, VFPv2 escalar e short vectors parciais e uma base inicial de GPU PICA200 com fila GX e relay de interrupções GSP parcial. Os frontends de PC e Android ainda não oferecem uma experiência gráfica jogável. Veja o [roadmap](docs/ROADMAP.md) e a [arquitetura](docs/ARCHITECTURE.md).

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
│   ├── desktop/          binário `aceris` (Windows, Linux, macOS)
│   └── android/          biblioteca nativa (cdylib) + recursos do app (ícones)
├── assets/icon/          ícone (SVG, PNGs, .ico)
├── tools/                scripts de desenvolvimento
└── docs/                 arquitetura e roadmap
```

Mais detalhes em [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) e
[docs/ROADMAP.md](docs/ROADMAP.md).

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

É necessário ter o Android NDK instalado (`ANDROID_NDK_HOME`). A aplicação Android e sua interface também deverão ser implementadas em Rust;
o crate atual gera apenas a biblioteca nativa.

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
