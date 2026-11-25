# Estudo de referências para o Aceris — foco Android

**Data da pesquisa:** 25/11/2026  
**HEAD do Aceris observado:** `8be7ae77f40187fe52a2210e0f5d0780d004a1c1`  
**Objetivo:** acelerar a estabilização do Aceris e chegar a testes reproduzíveis em aparelho Android, preservando a implementação inteiramente original em Rust.

## Regras que não mudam

- O produto Aceris continua integralmente em Rust: núcleo, frontends, aplicação, UI e lógica Android.
- O desenvolvimento permanece exclusivamente em `main`.
- Não copiar, colar, traduzir, portar, adaptar ou reproduzir estruturalmente código, shaders, testes, fixtures, assets ou dependências de outros projetos.
- Consultar código aberto é permitido somente após verificar licença do repositório e do arquivo consultado, conforme `docs/REGRAS_PARA_IA.md` e `docs/INDEPENDENT_RESEARCH.md`.
- DraStic é fechado. Não consultar, obter ou tentar reconstruir seu código interno; limitar a referência a comportamento observável, opções e experiência do usuário.
- Não incluir ROMs, CIA/CCI/3DS, firmware, chaves, saves, dumps ou dados extraídos no Git, CI, issues ou releases.
- Não modificar arquivos reservados a outro agente sem autorização específica do mantenedor.

## Matriz de referências

| Projeto | Licença / natureza | Uso permitido como referência |
|---|---|---|
| Azahar | GPL-3.0; emulador 3DS aberto | Comportamento 3DS, contratos PICA/GSP/IPC, compatibilidade, requisitos gráficos e escolhas Android |
| PPSSPP | GPL-2.0-or-later; emulador PSP aberto | Estratégia multi-backend, fallback OpenGL/Vulkan, profiling ARM64, pacing, responsividade e hints de desempenho Android |
| DraStic | Código fechado | Apenas observação externa de UX, duas telas, controles, biblioteca e comportamento em Android |
| My Boy! | Emulador GBA proprietário | Editor de layout de tela, perfis de controles, multitoque, mapeamento de gamepad, atalhos de save/load, fast-forward e preferências de vídeo/áudio; somente UX e comportamento observável |
| Eden | GPL-3.0-or-later; emulador Switch aberto | Gestão de backend host e GPU em Android, diagnóstico por dispositivo e separação do núcleo |
| PCSX2 | GPL-3.0-or-later; emulador PS2 aberto, focado em desktop | Separação entre estado virtual, memória, renderizador de referência, renderer GPU, caches e sincronização/readback |
| BLACK-Android / BLACK NX PORT | BLACK-Android MIT; componentes do port Switch têm licenças próprias | Fluxo de arquivos do usuário, ciclo de vida, controles, logs, perfis gráficos e separação da camada de plataforma |
| Dragon Rage / Tenkaichi3DecompAndroid | Repositório com AGPL-3.0 e arquivos/licenças de terceiros; o código do jogo tem direitos próprios | Instalação, importação local, configurações gráficas por dispositivo, entrada touch e diagnóstico/logs |
| Gen1Recomp | GPLv3 com termos adicionais | Verificação de identidade/hash, importação local, cache privado versionado, separação entre arquivo original e dados derivados |

### Revisões e fontes consultadas

As revisões abaixo servem para tornar a pesquisa reproduzível; não indicam que código foi incorporado.

- **Azahar:** [repositório](https://github.com/azahar-emu/azahar), revisão `f1cbaa02f84d316dd1c2b0e72894c8025b5c3949`, licença GPL-3.0. Fonte adicional: [README e requisitos Android](https://github.com/azahar-emu/azahar/blob/master/README.md). O README informa Android 10+, GPU OpenGL ES 3.2 ou Vulkan 1.1 e 2 GB mínimos / 4 GB recomendados para o próprio Azahar. Esses números não são requisitos automaticamente transferíveis ao Aceris.
- **PPSSPP:** [repositório](https://github.com/hrydgard/ppsspp), revisão `8cfbdb3e8239f78eb784e8cc804932f70cf20e2f`, GPL-2.0-or-later. Fontes: [introdução de desenvolvimento](https://www.ppsspp.org/docs/development/getting-started/) e [opções gráficas](https://www.ppsspp.org/docs/settings/graphics/). A documentação recomenda Vulkan na maioria dos dispositivos compatíveis e mantém OpenGL como alternativa de compatibilidade no Android.
- **DraStic:** [FAQ oficial](https://drasticemulator.com/frequently-asked-questions-drastic-ds-emulator/). A própria FAQ declara o projeto fechado; nenhuma fonte interna foi consultada.
- **My Boy!:** [listagem do Google Play](https://play.google.com/store/apps/details?id=com.fastemulator.gba). A descrição pública destaca editor de layout de tela, perfis de controles e mapeamento de teclas, gamepad externo, atalhos para save/load, fast-forward e sincronização de saves. Esses itens orientam possíveis funcionalidades de frontend; não justificam importar código, recursos de GBA ou lógica específica desse emulador.
- **Eden:** [repositório oficial](https://git.eden-emu.dev/eden-emu/eden/), licença GPL-3.0-or-later. O README público descreve um emulador Switch multiplataforma, incluindo Android. A consulta aqui limita-se à documentação e às características publicadas, sem reutilizar código.
- **PCSX2:** [repositório](https://github.com/PCSX2/pcsx2), revisão `4e56d430595164106eab9606cb46c94c902802cd`, GPL-3.0-or-later. Fonte: [FAQ de arquitetura e licença](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/Docs/PCSX2_FAQ.md). O projeto documenta intérpretes/recompiladores MIPS e uma VM que administra estado e memória PS2; serve como referência conceitual para fronteiras de subsistemas, não como implementação Android.
- **BLACK-Android:** [repositório](https://github.com/RanggaCasper/Black-Android), revisão `8e9c6fc7d570503ea68d039cff16c09339ee6d30`, licença MIT no repositório. **BLACK NX PORT:** [README](https://github.com/gerisson1996/BLACK-NX-PORT), revisão `d7611d14f89179cbbf8113edee0f52b2b5f52bf5`; o README descreve a divisão entre camada de plataforma, renderização Vulkan, áudio, entrada e acesso aos arquivos. Componentes internos mantêm suas licenças próprias.
- **Dragon Rage:** [Tenkaichi3DecompAndroid](https://github.com/dfdx047/Tenkaichi3DecompAndroid), revisão `2caa07a69ed4576704730e27a286df7eb4c2c3a2`. O repositório contém licença AGPL-3.0 e licenças separadas; código de jogo decompilado não é material livre para incorporação. A pesquisa aproveita somente o fluxo de uso e documentação do port Android.
- **Gen1Recomp:** [repositório](https://github.com/bryanthaboi/gen1recomp), revisão `53ad85ca5480cb6d52a578e01c24854ae21ae242`, GPLv3 com termos adicionais em `LICENSE.MD`. A documentação descreve um mecanismo próprio em Lua/LÖVE que verifica o arquivo do usuário, importa dados para cache privado e não mantém a ROM original no cache. Apesar do nome, não é um emulador nem uma recompilação estática convencional.

## O que isso significa para o Aceris

### 1. Manter quatro camadas diferentes

1. **Hardware guest 3DS:** ARM11, MMU, memória, SVC, IPC, GSP, PICA200, DSP e LCD. Seus contratos precisam refletir o console.
2. **Renderizador PICA de referência:** continua em Rust/CPU e é a referência determinística de correção.
3. **Backend host:** Vulkan/OpenGL/OpenGL ES apresenta a imagem e, futuramente, poderá rasterizar draws PICA através de um pacote de draw independente de API.
4. **Frontend Android:** NativeActivity em Rust, ciclo de vida, armazenamento do usuário, entrada multi-touch/gamepad, áudio, logs, superfície e preferências por aparelho.

Não misturar estado guest com handles Vulkan/OpenGL, nem tratar a apresentação de uma LCD já pronta como aceleração PICA.

### 2. Estratégia gráfica para celular

- Manter seleção automática de backend e fallback. Não assumir que Vulkan é sempre superior: comparar Vulkan e OpenGL ES no mesmo aparelho, mesma cena e mesmos parâmetros.
- Registrar GPU, driver, backend efetivamente escolhido, dimensões das superfícies, perdas/recriações de surface, tempo de quadro e motivo de fallback.
- Primeiro validar a superfície Android, apresentação, pausa/retomada e rotação. Só depois atribuir falhas visuais à PICA.
- Preservar renderização CPU como fallback/oráculo enquanto o caminho PICA em GPU não demonstrar equivalência visual e coerência de memória.
- Considerar APIs de desempenho adaptativo do Android (como ADPF) somente após obter baseline no aparelho e confirmar a disponibilidade da API; não adicionar dependência externa nem mudar o escalonador guest por suposição.
- Comparar uma sequência reproduzível, e não somente FPS médio: boot, menu, idle, entrada, gameplay, troca de tela e suspensão/retomada.

### 3. Fluxo seguro de conteúdo e biblioteca

- Validar identidade, cabeçalhos, hashes e limites antes de preparar uma imagem guest.
- Manter conteúdo fornecido pelo usuário separado do repositório e do cache de compilação.
- Usar cache privado por identidade/versão, com verificação de integridade e diagnóstico de falha; não copiar arquivos originais para GitHub.
- A biblioteca Android atual lista diretamente na pasta `games` somente `.3ds`, `.cci` e `.3dsx`. O loader central também tem caminho para CIA claro/NoCrypto, mas `scan_games` ainda não lista `.cia`. Portanto o primeiro teste da biblioteca deve usar um dos três formatos listados. A extensão não deve ser falsificada para contornar a interface.
- O frontend ainda não oferece seletor SAF. Para o teste atual, o caminho suportado é a pasta privada `Android/data/org.aceris.emulator/files/games` ou cópia via ADB.

### 4. O que não deve ser copiado

- Código C/C++ de CPU, GPU, Vulkan, OpenGL, DSP ou HLE de Azahar, PPSSPP, Eden ou PCSX2.
- Código Android em Kotlin/Java, SDL, JNI wrappers, layouts ou scripts de BLACK/Dragon Rage.
- Código Lua/LÖVE, dados, nomes internos, assets, cache formatado ou interface do Gen1Recomp.
- Qualquer código ou estrutura interna do DraStic.
- Código, assets, shaders, testes, ROMs, imagens de jogos ou dados extraídos de qualquer um desses projetos.

A pesquisa deve produzir hipóteses, especificações, testes independentes e medições próprias do Aceris. Toda mudança continua sujeita à verificação de autoria e licença por arquivo.


### 5. Experiência do launcher e controles

O estudo público do My Boy! reforça que uma interface móvel competitiva precisa ir além de uma lista: editor de posição/tamanho dos controles, perfis de layout e mapeamento, suporte a controles externos e atalhos rápidos são funcionalidades de UX valiosas. PPSSPP e Azahar também orientam a separar opções globais de opções por jogo e tornar backend/diagnósticos compreensíveis. Para o Aceris, essas funcionalidades devem ser reimplementadas em Rust e só adicionadas quando o núcleo e os contratos do frontend as suportarem; não apresentar uma opção como funcional antes de existir um caminho real de aplicação.

## Baseline Android já disponível

No HEAD `8be7ae77f40187fe52a2210e0f5d0780d004a1c1`, o workflow [Build Android APK](https://github.com/CarlosCosta07-s/Aceris/actions/runs/45693520694) foi concluído com sucesso e gerou o artefato `aceris-android-arm64-debug` (arm64-v8a). O APK foi assinado com chave de desenvolvimento descartável e verificado pelo workflow; isso comprova empacotamento/assinatura, não instalação ou execução em hardware físico.

- Application ID provisório: `org.aceris.emulator`.
- Diretório de jogos: `/storage/emulated/0/Android/data/org.aceris.emulator/files/games/`.
- Formatos listados pela biblioteca: `.3ds`, `.cci`, `.3dsx`.
- Para trocar uma instalação por outro build do CI, desinstale o anterior: a chave de debug é regenerada e os dados locais também são removidos.
- O presenter host já tenta Vulkan ou OpenGL ES e mantém fallback CPU; ele apresenta quadros LCD que o núcleo já compôs. Ainda não executa a rasterização PICA200 na GPU host.
- O teste físico precisa registrar modelo/SoC, Android, GPU/driver, backend, logs, estabilidade de surface, resposta touch e tempo por quadro.

## Próxima sequência

1. Instalar o APK ARM64 e confirmar intro, biblioteca, toque, rotação, pause/resume e ausência de crash.
2. Testar um arquivo próprio `.3ds`, `.cci` ou `.3dsx` que a biblioteca lista e registrar exatamente a mensagem/resultados.
3. Capturar logcat e identificar backend/adapter/fallback. Executar a mesma sequência com Vulkan e OpenGL ES quando ambos estiverem disponíveis.
4. Medir o caminho real em aparelho antes de escolher otimizações CPU/GPU. Não transferir para o telefone a estimativa histórica do benchmark desktop.
5. Após baseline físico, priorizar o primeiro bloqueio guest que impeça gameplay real; só então avançar em renderer PICA acelerado e áudio.

**Resultado desta pesquisa:** nenhuma implementação de terceiros foi incorporada. O produto continua Rust original e as referências servem exclusivamente para arquitetura, comportamento, critérios de teste e prioridades.
