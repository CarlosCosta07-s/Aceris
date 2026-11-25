# Testes com homebrew real

Este guia registra a validação do arquivo Super Mario 64 – 3DS Port (Luigi
Version) fornecido pelo mantenedor e explica como repetir o teste de boot
localmente. O CIA não é copiado para o repositório, CI ou releases.

## Super Mario 64 – 3DS Port (Luigi Version)

O arquivo local foi inspecionado estruturalmente. Os valores observados são:

| Campo | Valor |
|---|---:|
| Title ID (ticket e TMD) | `0x000400000FAA5C00` |
| Índice de conteúdo de boot | `0` |
| Índice de chave comum do ticket | `0` |
| Início do conteúdo | `0x3900` |
| Tamanho do conteúdo | `0x770C00` |
| Tipo do conteúdo no TMD | `0x0000` (sem AES-CBC externo) |
| NCCH program ID | `0x000400000FAA5C00` |
| NCCH flags | `00 00 00 00 01 02 00 07` |
| NCCH NoCrypto | ativado |

As verificações SHA-256 também foram repetidas diretamente sobre o arquivo:
o conteúdo confere com o digest do TMD; o hash da tabela de informações e o
grupo de registros do TMD conferem; o hash do extended header NCCH confere; o
hash da superblock ExeFS confere; e os digests dos quatro arquivos ExeFS
(`.code`, `banner`, `icon` e `logo`) conferem.

**Correção importante:** a flag NoCrypto está ativa no NCCH. Este homebrew não
precisa de chaves de console para ser carregado pelo caminho CIA sem
criptografia do Aceris. A conclusão anterior de que o NCCH deste arquivo
exigia chaves estava incorreta.

O extended header marca o ExeFS/.code como comprimido (flag bit 0). O teste
manual abaixo força a preparação ARM11, que inclui validação do extended
header, ExeFS, digest do `.code` e caminho de descompressão BLZ.

## Repetir o teste com o arquivo original

O teste de integração é intencionalmente ignorado por padrão: ele depende do
arquivo local e não deve transferi-lo ao CI.

Linux/macOS:

```sh
export ACERIS_REAL_CIA="/caminho/local/para/Super Mario 64 Luigi.cia"
cargo test -p aceris-core --test cia_real_homebrew -- --ignored --nocapture
```

PowerShell:

```powershell
$env:ACERIS_REAL_CIA = "C:\caminho\para\Super Mario 64 Luigi.cia"
cargo test -p aceris-core --test cia_real_homebrew -- --ignored --nocapture
```

O teste executa as seguintes etapas:

1. Abre o CIA e valida ticket, TMD, bitmap e faixa do conteúdo.
2. Confere o hash SHA-256 de todo o conteúdo e valida o cabeçalho NCCH.
3. Confirma Title ID e flag NoCrypto.
4. Prepara o espaço ARM11 e exige validação do extended header, da superblock
   ExeFS, do digest do `.code` e da preparação BLZ.
5. Carrega o arquivo pelo mesmo `System::load_executable_file` usado pelo
   launcher, sem fornecer chaves.
6. Executa um frame com limite de 50.000 instruções e registra instruções
   executadas, motivo de parada da CPU, listas GSP e atualizações LCD.

O teste é compilado pelo CI, mas permanece ignorado durante a execução
automática porque o arquivo do mantenedor não faz parte do repositório nem dos
runners.

## Limites do resultado

- A validação estrutural do ticket/TMD e seus hashes não equivale à verificação
  RSA/ECDSA das assinaturas; o parser atual não valida a cadeia de certificados.
- O teste manual ainda precisa ser executado no ambiente que possui o arquivo
  CIA. A inspeção binária isolada não prova que o Aceris conseguiu executar o
  guest.
- Passar o teste de um frame confirma que a sessão ARM11 iniciou e avançou;
  não prova compatibilidade completa, áudio, controles, rendering PICA200
  correto ou gameplay contínuo.
- O arquivo original permanece fora do repositório, CI, issues e releases.


## Atualização: Lua Player Plus Cookie Clicker (CIA) — 22/11/2026

Um homebrew real fornecido localmente pelo mantenedor foi usado para avançar além da preparação NCSD. O CIA tem conteúdo NCCH NoCrypto e Title ID `0004000000F23F00`; o arquivo não foi adicionado ao repositório nem à CI.

### Resultado observado

- O Aceris carregou o CIA, preparou o código ARM11 e executou o runtime Lua Player Plus.
- O aplicativo leu seu RomFS, publicou framebuffers e desenhou sua própria tela de erro.
- A falha observada foi `cannot initialize Sound module`.
- Após HLE mínimo de `csnd:SND`, `csndInit` passou, mas `Sound.init` continuou falhando em rotina interna do LPP. Nenhuma requisição DSP apareceu no trace observado; a causa precisa ser rastreada.
- Ensaio headless de 300 quadros, orçamento de 4.000.000 instruções por quadro, levou aproximadamente 100 segundos de CPU host. Não é jogável em tempo real.

Esse avanço chega a uma tela produzida pelo próprio aplicativo, mas não é boot funcional nem gameplay. O teste anterior de Super Mario 64 – 3DS Port não deve ser descrito como execução real concluída sem evidência adicional.

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.


## Atualização: primeiro quadro gráfico do Cookie Clicker (24–25/11/2026)

- Com a renderização PICA200 guest habilitada nos frontends e as correções de stride/formato/layout de atributos, o homebrew Lua Player Plus Cookie Clicker deixou de produzir apenas uma tela preta.
- No ensaio manual registrado em 24/11, após 450 quadros com orçamento de 4.000.000 instruções por quadro, a tela superior exibiu a abertura do port (texto "PORT BY" e "CRECKERYOP" na orientação esperada). Cada lista observada escreveu 192.000 pixels.
- A tela inferior continuou preta. O resultado não demonstra gameplay, entrada funcional ou áudio; a falha de inicialização `Sound.init` ainda precisa ser rastreada.
- Em 25/11, a unidade PICA200 procedural foi conectada ao TEV e ganhou um modelo de ruído de referência em software. Isso ainda não foi validado em console físico nem confirmado como a causa de qualquer resultado visual do Cookie Clicker.
- A CI mais recente do HEAD `d5cdc15b13e454a8c580bb8dbb50b522c05d7693` passou em Linux, Windows, macOS e Android ARM64 no workflow [45600245040](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45600245040). Nos desktops, formatação, build, Clippy, testes completos e benchmark release passaram; no Android, o app e o benchmark ARM64 release foram compilados. A CI não contém o CIA e não executa este teste manual ignorado.

### Repetir a validação com o CIA Cookie Clicker fornecido

O teste `validates_and_runs_real_cookie_clicker_cia_without_console_keys` foi adicionado como teste ignorado. Ele valida Title ID, ticket, seleção do conteúdo de boot, ausência de criptografia externa, flag NoCrypto do NCCH, preparação ARM11, carregamento pelo caminho padrão e execução de um quadro. O arquivo permanece no computador do mantenedor.

Linux/macOS:

```sh
export ACERIS_COOKIE_CLICKER_CIA="/caminho/local/para/0004000000F23F00 Cookie Clicker.cia"
cargo test -p aceris-core --test cia_real_homebrew validates_and_runs_real_cookie_clicker_cia_without_console_keys -- --ignored --nocapture
unset ACERIS_COOKIE_CLICKER_CIA
```

PowerShell:

```powershell
$env:ACERIS_COOKIE_CLICKER_CIA = "C:\caminho\para\0004000000F23F00 Cookie Clicker.cia"
cargo test -p aceris-core --test cia_real_homebrew validates_and_runs_real_cookie_clicker_cia_without_console_keys -- --ignored --nocapture
Remove-Item Env:ACERIS_COOKIE_CLICKER_CIA
```

O arquivo anexado nesta sessão foi mantido fora do repositório. Seu SHA-256 local é `d047701f8e9b4127436046f5a650dede06e03bc1b5f1b271981d4ea27db58093`. **Esse fingerprint identifica o arquivo, mas não significa que o teste acima tenha sido executado nesta sessão.**
