# Arquitetura

## Visão geral

```
 frontends/desktop ─┐
                    ├──►  aceris-core  ──►  aceris-common
 frontends/android ─┘
```

O **núcleo** (`aceris-core`) não conhece janelas, áudio de saída ou interface.
Ele expõe um `System` que o frontend carrega com um jogo e avança quadro a quadro
(`run_frame`). Cada frontend é responsável por:

- criar a superfície de vídeo e apresentar os quadros das duas telas;
- capturar entrada (teclado, controle, toque);
- reproduzir o áudio gerado pelo núcleo;
- gerenciar arquivos, configurações e ciclo de vida do app.

## Crates

| Crate | Papel |
|-------|-------|
| `aceris-common` | Tipo de erro (`Error`/`Result`), nome e versão do projeto. |
| `aceris-core` | `cpu` (núcleo ARM11), `memory` (trait `Bus`), `loader` (identificação de imagens), `system`. |
| `aceris-desktop` | Binário `aceris`. Hoje é só linha de comando (`--selftest`). |
| `aceris-android` | `cdylib` carregada pelo app Android. Exporta `aceris_version()` e `aceris_selftest()`. |

## Estado atual do núcleo

- **CPU (ARM11, estado ARM):** interpretador com banco único de registradores
  (modo usuário). `step()` busca, decodifica e executa uma instrução e devolve um
  `StepEvent` (`Executed` ou `Svc(n)`, que será tratado pelo kernel emulado).
  Já implementado:
  - processamento de dados (as 16 operações, shifter completo, flags N/Z/C/V);
  - `LDR/STR/LDRB/STRB` (pré/pós-indexado, writeback, offset escalado);
  - `LDRH/STRH/LDRSB/LDRSH/LDRD/STRD`, `LDM/STM`, `SWP`;
  - `MUL/MLA` e multiplicações longas (`UMULL/SMULL/UMLAL/SMLAL`);
  - `B/BL/BX/BLX`, `CLZ`, `MRS/MSR` (apenas flags) e `SWI`.
  Ainda faltam: Thumb, modos privilegiados e registradores com banco, instruções
  de mídia/exclusivas do ARMv6, coprocessadores e VFP.
- **Autoteste (`selftest`):** programa ARM minúsculo executado pelo interpretador.
  O desktop o expõe em `aceris --selftest` e o Android em `aceris_selftest()`,
  provando que as duas plataformas rodam o mesmo núcleo.
- **Memória:** trait `Bus` (little-endian) e `FlatMemory` para testes e bring-up.
- **Loader:** identifica ELF, 3DSX, NCSD, NCCH e CIA pelos bytes mágicos.
- **Sistema:** `System::load` identifica a imagem; `run_frame` ainda não emula.

## Decisões em aberto

Estas escolhas ainda não foram feitas e merecem discussão antes de serem
implementadas:

- **Janela e entrada no desktop:** `winit` é o candidato natural.
- **Renderização:** `wgpu` cobre Vulkan, Metal, DX12 e GL/GLES no mesmo código,
  o que ajuda a manter PC e Android juntos.
- **Interface (menus, biblioteca de jogos, configurações):** `egui` para o desktop;
  no Android, UI nativa (Kotlin/Compose) conversando com o núcleo via JNI.
- **Áudio:** `cpal` é uma opção multiplataforma.
- **Estratégia de CPU:** começar com um interpretador simples e correto; considerar
  um JIT/recompilador dinâmico só depois que a compatibilidade estiver estável.
- **Distribuição do app Android:** Gradle + Kotlin com `cargo-ndk` gerando as `.so`
  em `jniLibs/`.

## Build para Android

O frontend Android é um crate `cdylib`. O fluxo previsto é:

1. `cargo ndk` compila `aceris-android` para cada ABI (`arm64-v8a` primeiro).
2. As `.so` são copiadas para `frontends/android/app/src/main/jniLibs/`.
3. O projeto Gradle empacota o APK usando os recursos em `frontends/android/res/`
   (ícones adaptativos já gerados).
