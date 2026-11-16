# Testes com homebrew real

Este guia descreve como validar um arquivo de homebrew fornecido pelo mantenedor
sem copiar ROMs, CIA, executáveis, firmware ou chaves para o repositório, CI ou
releases.

## Super Mario 64 – 3DS Port (Luigi Version)

O arquivo CIA fornecido para esta rodada foi inspecionado localmente. Os dados
estruturais observados foram:

| Campo | Valor |
|---|---:|
| Title ID (TMD e ticket) | `0x000400000FAA5C00` |
| Índice de conteúdo de boot | `0` |
| Índice de chave comum do ticket | `0` |
| Início do conteúdo | `0x3900` |
| Tamanho do conteúdo | `0x770C00` |
| Tipo do conteúdo no TMD | `0x0000` (sem AES-CBC externo) |
| NCCH program ID | `0x000400000FAA5C00` |
| Flag NCCH NoCrypto | desligada |

O SHA-256 do conteúdo armazenado corresponde ao digest declarado no TMD. As
tabelas de conteúdo e os hashes agregados do TMD também foram conferidos. O
ticket e o TMD declaram o mesmo Title ID.

O ponto que ainda impede a inicialização é a criptografia **interna do NCCH**.
O conteúdo CIA não usa a camada AES-CBC externa, mas o NCCH não declara
`NoCrypto`; portanto, o carregador sem chaves deve recusar a inicialização.
Isso não é evidência de incompatibilidade do código ARM11 ou da GPU: o guest
ainda não chegou a executar.

## Repetir a validação com o arquivo original

O teste de integração dedicado é intencionalmente ignorado por padrão: ele
depende do arquivo local e não deve transferi-lo ao CI.

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

Esse teste abre o CIA com `CiaArchiveReader`, verifica os metadados do ticket e
do TMD, confere o hash SHA-256 do conteúdo completo, interpreta o cabeçalho
NCCH, compara o program ID e confirma que a inicialização sem chaves termina
no erro explícito de criptografia, sem marcar um processo como carregado.

O teste é compilado pelo CI, mas fica ignorado durante a execução automática,
pois o arquivo do mantenedor não faz parte do repositório nem dos runners.

## Próxima etapa para executar o jogo

O caminho de sistema `System::load_cia_with_keys` já existe. Ele recebe um
`TicketCommonKeyStore` fornecido pelo chamador, além das chaves NCCH de menu
info e conteúdo. O projeto não descobre, extrai, baixa, incorpora ou persiste
essas chaves automaticamente.

Para prosseguir com este título, o ambiente local precisa configurar as chaves
legítimas obtidas pelo próprio mantenedor. O material deve ficar fora do
repositório, do CI, de logs e de releases. Não coloque chaves em issues ou
mensagens públicas. Depois disso, o teste real poderá avançar da validação do
contêiner para a preparação ARM11 e, então, registrar os serviços ausentes
durante a execução.

## Limites do resultado

- A validação do CIA e do hash TMD não equivale à verificação RSA das
  assinaturas do ticket/TMD; o parser estrutural atual não valida essas
  assinaturas.
- A validação do cabeçalho NCCH não equivale à descriptografia dos segmentos.
- O teste manual não declara que o jogo inicializa, produz imagem, áudio ou
  responde a controles.
- Não use uma fixture sintética como substituta da execução deste homebrew real.
