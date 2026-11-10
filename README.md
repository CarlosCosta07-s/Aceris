<p align="center">
  <img src="assets/icon/png/aceris-256.png" alt="Ícone do Aceris: uma acerola" width="160">
</p>

<h1 align="center">Aceris</h1>

<p align="center">
  Um emulador de um certo console de 2 telas, escrito em Rust, para PC e Android.
</p>

> **Status: fase inicial (v0.0.1).** O projeto ainda não executa jogos. Hoje existe
> apenas a estrutura do workspace, o esqueleto do núcleo e os frontends mínimos.

O nome vem da **acerola**, a pequena fruta vermelha cheia de energia.

## Princípios

- **Código 100% original.** O Aceris é escrito do zero. Nenhum código, interface ou
  recurso de outros emuladores é usado ou traduzido. Veja a
  [política de sala limpa](CONTRIBUTING.md#política-de-sala-limpa).
- **Rust de ponta a ponta**, com um núcleo independente de plataforma.
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
```

### Android

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk
cargo ndk -t arm64-v8a build -p aceris-android --release
```

É necessário ter o Android NDK instalado (`ANDROID_NDK_HOME`). O app Android
(Gradle/Kotlin) ainda será criado; por enquanto o crate gera apenas a biblioteca
nativa.

## Ícone

O ícone é um desenho original, gerado por [`tools/generate_icons.py`](tools/generate_icons.py)
a partir de um único SVG (precisa de `rsvg-convert` e ImageMagick). Ele cria o SVG
mestre, os PNGs, o `.ico` do Windows e os ícones adaptativos do Android.

## Contribuidores

- [Carlos Costa](https://github.com/CarlosCosta07-s): criador e mantenedor
- ChatGPT (plano gratuito): assistente de IA
- Claude (Anthropic): assistente de IA

## Licença

[MIT](LICENSE).
