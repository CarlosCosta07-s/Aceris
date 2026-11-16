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
