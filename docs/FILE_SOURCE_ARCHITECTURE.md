# Arquitetura de origem de arquivos — desktop e Android SAF

> Decisão de arquitetura proposta — 27/11/2026. Preparação do marco M1.1 do plano de implementação. Este documento define a fronteira técnica antes de alterações no loader ou na biblioteca. Não declara que o seletor de pastas ou a integração SAF já existam.

## 1. Problema observado

A implementação atual representa cada jogo como Entry { title, path: PathBuf }. O scanner usa std::fs::read_dir, canonicalize e metadados do filesystem. A configuração de pastas persiste caminhos absolutos em library_scan_v1.txt. O loader de identificação abre File::open(path), e o fluxo de execução do host também entrega caminhos ao núcleo.

Esse modelo funciona para diretórios comuns acessíveis ao processo. Ele não representa uma árvore de documentos Android selecionada pelo Storage Access Framework (SAF), que é identificada por URI e depende de permissões concedidas pelo usuário. Uma URI SAF não é um caminho POSIX e não deve ser convertida artificialmente em PathBuf.

Há também uma base reutilizável no core: os leitores NCSD/NCCH usam generics Read + Seek e NcsdPartitionReader restringe leituras ao intervalo da partição. fs.rs define um trait object-safe ReadSeek para RomFsRegion. O principal acoplamento a remover é a abertura do arquivo pelo caminho, não a necessidade legítima de leitura aleatória do formato.

## 2. Decisões

### 2.1 Separar identificação da origem e identidade do jogo

Não usar um único PathBuf como identidade, local de exibição e mecanismo de abertura.

O modelo futuro deve distinguir:

- **SourceId:** identidade estável da origem/entrada escolhida pelo usuário. Representa de onde abrir o arquivo, não o conteúdo do jogo.
- **GameIdentity:** identidade de conteúdo obtida apenas depois de validar metadados do cabeçalho, como Title ID, região e revisão quando disponíveis.
- **DisplayName:** nome legível apresentado na biblioteca.
- **SourceDescriptor:** referência opaca e serializável da origem, sem pressupor caminho local.
- **OpenedGameSource:** leitor aberto, somente leitura, com tamanho conhecido e sem expor ao core detalhes de Android, JNI ou diálogo de desktop.

Favoritos e Recentes devem migrar do atual path-v1 para um identificador de origem versionado. Não associar jogos diferentes apenas pelo nome. Uma cópia do mesmo título em dois locais pode ter origens diferentes, mas compartilhar a GameIdentity validada quando isso for comprovado. A política de configuração por jogo deve usar GameIdentity; antes dessa validação, não inventar um Title ID.

### 2.1.1 Implementação parcial da identidade de título (27/11/2026)

O core agora expõe o tipo `GameIdentity` e `System::game_identity()`. A identidade atual é deliberadamente restrita ao Title ID de 64 bits:

- NCSD/CCI: leitura do program ID do NCCH primário, após validar a estrutura do container e antes do loader de execução; o leitor é rebobinado para offset zero antes de continuar.
- CIA: leitura do Title ID do TMD, somente depois de conferir que o NCCH de boot apresenta o mesmo program ID.
- Um Title ID zero ou um formato sem identificador validado (atualmente 3DSX) produz identidade desconhecida; não é fabricado um identificador baseado no nome do arquivo.
- A chave persistente é `title-v1:<16 dígitos hexadecimais>`. Ela identifica o título lógico e deliberadamente não separa revisões/versões de lançamento. Região, product code, revisão e identidade formal da origem ainda precisam ser modelados antes da página de metadados.
- Essa verificação estrutural não equivale a validar assinatura/autenticidade do título. O estado da origem continua separado: Favoritos/Recentes ainda usam IDs derivados do caminho.

A primeira consumidora é a preferência de layout de duas telas. O formato v2 mantém padrões globais por orientação e overrides opcionais por Title ID; configurações para títulos sem identidade validada herdam o padrão global. O arquivo v1 global-only continua sendo lido e é preservado; a próxima gravação cria `.aceris/screen_layout_preferences_v2.txt`. O restante do schema de configurações por jogo e a formalização de SourceId permanecem planejados.

### 2.2 Manter o core independente de plataforma

O core não deve chamar ContentResolver, DocumentsContract, JNI, Win32, Cocoa ou APIs X11 para localizar arquivos.

A fronteira deve ser semelhante a:

- Um contrato compartilhado para identificar uma origem, obter metadados e abrir um fluxo de leitura.
- Um provedor de biblioteca implementado pelo host, responsável por enumerar as raízes que o usuário selecionou.
- Um leitor seekable entregue ao loader por Read + Seek, mantendo as validações e limites de intervalo já usados pelos parsers.
- Erros tipados para permissão revogada, origem indisponível, arquivo removido, formato inválido, falha de leitura e espaço insuficiente.

O contrato exato deve ser definido após o inventário de todos os pontos que ainda abrem Path diretamente. Não criar uma abstração paralela que deixe parte do loader usando URI e outra parte usando caminho sem uma regra clara.

### 2.3 Separar provedor de biblioteca de leitor de conteúdo

O provedor do host enumera arquivos e diretórios dentro das raízes autorizadas e produz descritores de origem. Ele não valida conteúdo por extensão.

O scanner compartilhado continua responsável pelas regras de produto: extensões candidatas, profundidade máxima, prevenção de duplicatas e ordenação determinística. A plataforma fornece as operações de enumeração e acesso. A varredura deve conseguir reportar progresso/cancelamento sem travar o thread da interface.

A enumeração SAF precisa respeitar as capacidades reais de cada DocumentProvider. Nem todo provedor permite enumerar descendentes, entregar tamanho, data de modificação ou acesso aleatório. Falta de metadado não deve ser tratada como arquivo inválido.

### 2.4 Leitura aleatória e cache de preparação

NCSD, NCCH, CIA e 3DSX podem exigir seek e acesso a diferentes regiões do arquivo. Não carregar jogos inteiros na RAM para simular seek.

A estratégia de abertura será:

1. Tentar obter um descritor somente leitura que ofereça leitura e seek.
2. Verificar o seek real, o tamanho e os limites; não confiar somente na extensão ou no MIME type.
3. Se a origem SAF não oferecer seek confiável, preparar uma cópia no cache privado do Aceris em blocos limitados, mantendo a origem original intacta.
4. Validar o tamanho copiado e concluir a preparação com gravação atômica: arquivo temporário, flush/sync quando disponível e rename para o objeto final.
5. Abrir o arquivo preparado como leitor seekable e executar o mesmo loader/validador usado para caminho local.

O cache deve ser versionado, ter identidade ligada à origem e aos metadados disponíveis (tamanho, modificação e identificador do documento) e detectar entradas parciais. Não recalcular hash completo de arquivos grandes em toda inicialização sem necessidade. Hash pode ser usado quando necessário para integridade, colisões ou verificação de mudança, com custo informado.

A política de cache deve prever:

- Espaço disponível e erro explícito antes de cópias grandes quando o host puder consultar essa informação.
- Cancelamento de cópia e limpeza apenas do arquivo temporário incompleto.
- Preservação da última cópia íntegra até que a nova esteja validada.
- Remoção manual de cache e política de limite/evicção que nunca apague saves nem arquivos originais.
- Nenhum upload, cópia para repositório, ou inclusão de conteúdo do jogo em CI/releases.

O caminho de cache é fallback para compatibilidade de leitura; não é uma instalação de título e não deve ser apresentado como tal.

## 3. Comportamento por plataforma

### Desktop (Windows, Linux e macOS)

- Adicionar ação "Selecionar pasta" por diálogo nativo de diretório.
- Manter suporte a várias raízes, recursão opcional e persistência das escolhas.
- Armazenar caminho normalizado e exibir o local real de forma legível.
- Se o caminho desaparecer ou as permissões mudarem, marcar a raiz como indisponível e permitir localizar novamente; não remover automaticamente favoritos, recentes ou configurações.
- Não seguir links simbólicos na recursão, preservando a proteção já existente.
- Manter a digitação de caminho como alternativa avançada/diagnóstico, não como único método de inclusão.

A implementação deve continuar em Rust e evitar adicionar dependências de interface/diálogo sem aprovação. A escolha entre APIs nativas e uma abstração interna deve ser decidida para cada backend suportado, sem quebrar hosts que não tenham janela de diálogo disponível.

### Android

- Ação "Selecionar pasta" deve abrir o seletor de documentos do sistema para uma árvore, com acesso somente às pastas que o usuário concedeu.
- Persistir o URI e a permissão concedida, verificando no retorno que a permissão pode ser mantida após reiniciar o aplicativo.
- Não converter content:// em caminho de filesystem.
- Enumerar filhos via APIs SAF e respeitar sinalizações do provider sobre diretório, tamanho, data e MIME.
- Ao iniciar um jogo, confirmar que a permissão ainda existe. Se tiver sido revogada, informar qual origem perdeu acesso e oferecer nova seleção.
- Para leitura seekable, usar descritor somente leitura quando o provider suportar; testar seek. Se não suportar, copiar em blocos para cache privado como descrito em 2.4.
- Não pedir permissão ampla de armazenamento quando o seletor de documentos resolve o caso.
- Não depender de armazenamento externo montado em um caminho fixo.
- A UI deve mostrar nome/caminho legível, não uma URI extensa como texto primário.

A implementação pode chamar APIs Android por JNI/NDK a partir do frontend Rust, mantendo todo o comportamento e estado da aplicação em Rust. A camada Java do sistema é uma ponte de plataforma, não uma segunda implementação da UI ou do emulador.

#### Limite do host NativeActivity atual

O app declara `android.app.NativeActivity` no manifesto e implementa diretamente
`ANativeActivityCallbacks` em Rust. O contrato nativo disponibiliza callbacks de
ciclo de vida, janela, foco, memória e fila de entrada, mas não oferece um
callback `onActivityResult`. Portanto, iniciar `ACTION_OPEN_DOCUMENT_TREE`
por JNI não basta: o host ainda precisa receber o resultado da Activity para
obter a URI selecionada e os flags de permissão.

A estrutura `ANativeActivity` expõe a referência Java da Activity e o JNI para
chamar APIs do framework; isso não adiciona o callback de retorno ausente. Uma
solução comum exigiria uma Activity derivada que encaminhasse o resultado a
Rust por JNI, ou outro host Android que fornecesse esse canal. Como o host
atual e a regra de implementação do Aceris são Rust-only, não introduzir uma
classe Java/Kotlin silenciosamente para contornar o limite. Até existir uma
solução compatível e autorizada, manter a entrada de caminho como fallback e
não persistir uma URI que a biblioteca ainda não consegue enumerar nem abrir.

#### Acesso ao armazenamento compartilhado (implementado)

Enquanto o seletor SAF não tiver canal de retorno, o app pede acesso amplo ao
armazenamento, por decisão do mantenedor, usando apenas JNI a partir de Rust:

- Android 11+: `MANAGE_EXTERNAL_STORAGE`. O botão PERMITIR abre a tela do
  sistema "Acesso a todos os arquivos" para o pacote (com fallback para a lista
  geral), e o estado é lido com `Environment.isExternalStorageManager()`.
- Android 6–10: `requestPermissions` com `READ/WRITE_EXTERNAL_STORAGE`, lido
  com `checkSelfPermission`; `requestLegacyExternalStorage` cobre o Android 10.
- O resultado não depende de callback: o host relê a permissão quando a
  Activity volta ao primeiro plano e a cada segundo enquanto o aviso está na
  tela. O JNI só é anexado depois do primeiro quadro.
- Fluxo: abertura → aviso ACESSO AOS ARQUIVOS (DEPOIS / PERMITIR). DEPOIS fica
  salvo em `.aceris/android_storage_access_v1.txt` e o aviso não volta sozinho,
  mas Importar Pasta pede o acesso de novo antes de abrir a digitação do
  caminho.
- Na primeira concessão, `<armazenamento>/Aceris/games` é criada e adicionada
  à biblioteca uma única vez; se o usuário a remover, ela não volta. A pasta
  interna do app continua sendo lida.

Esse limite é somente sobre o retorno do seletor. A futura integração SAF
continua dependendo também do provedor de documentos, da persistência da
permissão e da preparação de uma origem seekable descrita nesta arquitetura.

## 4. Persistência e migração

O formato atual library_scan_v1.txt guarda caminhos absolutos. O novo formato deve ter versão explícita, tipos de origem e dados mínimos necessários para reabrir a raiz. A migração deve ser conservadora:

- Ler v1 sem alterar a semântica de raízes locais existentes.
- Escrever v2 somente depois de validar que todos os descritores foram serializados corretamente.
- Preservar o arquivo v1 até que v2 tenha sido escrito e lido de volta com sucesso.
- Não remover raízes quando a permissão SAF estiver temporariamente indisponível.
- Tratar URI malformada, versão futura e campo desconhecido sem travar a inicialização; apresentar diagnóstico e manter cópia do arquivo anterior.
- Manter favoritos e recentes legados recuperáveis; migração para SourceId deve ocorrer de maneira transacional e sem perder o arquivo original até validação.
- Não inventar GameIdentity quando o cabeçalho não fornecer metadados suficientes.

## 5. Segurança e confiança

- Toda origem selecionada é inicialmente somente leitura para o carregamento do jogo.
- O loader não deve escrever na origem do jogo.
- Acesso a uma pasta significa somente a árvore concedida pelo usuário; não ampliar silenciosamente o escopo.
- Validar todos os tamanhos e offsets declarados pelos contêineres antes de alocar ou ler regiões.
- Não seguir links simbólicos na enumeração recursiva.
- Tratar URIs, nomes de documentos e caminhos como dados não confiáveis.
- Não registrar tokens ou detalhes privados de URI em logs exportados.
- Erros de permissão devem orientar nova autorização; nunca cair silenciosamente para outra pasta.
- Remover uma origem da biblioteca não apaga o arquivo do usuário.

## 6. Testes de contrato necessários

### Core e biblioteca (sem Android)

- Leitor baseado em Cursor<Vec<u8>> percorre identify/parse/loader com os mesmos resultados de uma imagem temporária em disco.
- Leitor com seek que falha devolve erro explícito e permite ao host optar pelo cache.
- Leitor limitado impede ultrapassar o tamanho declarado da imagem ou da partição.
- Leitura curta, EOF prematuro, tamanho incorreto, seek inválido e erro de I/O produzem diagnóstico previsível.
- Mesma imagem sob nomes distintos não é confundida por DisplayName; identificador de origem e identidade de jogo permanecem separados.
- Caminho v1 migra para v2 sem perder favoritos e recentes.
- Origens removidas ou indisponíveis não removem saves ou configurações.

### Desktop

- Seleção de pasta com espaços, Unicode, links e várias raízes.
- Cancelar diálogo não altera a configuração anterior.
- Pasta removida, sem permissão ou em disco desconectado produz estado recuperável.
- Revarredura de raízes sobrepostas não duplica entradas.

### Android SAF

- Selecionar árvore e manter permissão após fechar e reabrir o app.
- Provider local com seekable descriptor.
- Provider sem seek, exigindo staging para cache.
- Permissão revogada, documento removido, provider indisponível e erro de rede.
- Arquivo grande com progresso, cancelamento, pouco espaço livre e retomada por nova tentativa.
- Nome Unicode, nomes repetidos em pastas diferentes e providers com metadados incompletos.
- Confirmar em aparelho físico; build ARM64/APK na CI sozinho não aprova o fluxo.

## 7. Critérios de aceite do M1.1/M1.2

O marco só pode ser considerado concluído quando:

1. O core consegue identificar e carregar um jogo a partir de leitor seekable sem exigir PathBuf em cada estágio do loader.
2. O desktop mantém o comportamento de caminhos locais e oferece diálogo de seleção em cada backend alvo suportado.
3. O Android seleciona uma árvore via SAF, persiste a permissão e enumera seus arquivos sem converter URI em caminho.
4. Providers sem seek funcionam através do cache privado, sem ler o arquivo inteiro para RAM.
5. O usuário entende progresso, cancelamento, permissão revogada, falta de espaço e erro de leitura.
6. Testes de contrato passam no CI; SAF e diálogo nativo têm validação física/manual documentada.
7. Biblioteca, favoritos, recentes, saves e dados preexistentes são preservados durante a migração.

## 8. Ordem técnica recomendada

1. Auditar todos os usos de Path/File::open no loader e nos pontos de entrada de System; identificar APIs que já aceitam Read + Seek.
2. Criar testes de contrato usando Cursor antes de alterar a lógica de carregamento.
3. Generalizar identificação e parsers de topo para leitores seekable e manter wrappers de caminho para compatibilidade.
4. Introduzir SourceId/SourceDescriptor sem ainda mudar os hosts; testar migração dos IDs path-v1.
5. Implementar o provedor local desktop sobre std::fs como primeiro adaptador.
6. Implementar o seletor de pasta e o provedor SAF no host Android.
7. Adicionar staging/cache apenas para providers não seekable e documentar limites de espaço.
8. Integrar varredura assíncrona com progresso e cancelamento depois que as fontes estiverem estáveis.

## 9. Relação com outros documentos

- Estado atual auditado: [CAPABILITY_MATRIX.md](CAPABILITY_MATRIX.md).
- Ordem e critérios gerais: [PRODUCT_IMPLEMENTATION_PLAN.md](PRODUCT_IMPLEMENTATION_PLAN.md).
- Experiência funcional desejada: [USER_EXPERIENCE.md](USER_EXPERIENCE.md).
- Contratos dos hosts: [FRONTENDS.md](FRONTENDS.md).
