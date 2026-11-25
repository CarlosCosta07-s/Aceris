# Estudo de referências externas — Aceris

> Revisão em 25/11/2026. Este documento transforma referências externas em perguntas e critérios para uma implementação independente. Não autoriza copiar, adaptar, traduzir ou portar código, shaders, testes, assets ou organização expressiva de terceiros.

## Regras que não mudam

- O produto Aceris permanece integralmente em Rust: núcleo, frontends e lógica da aplicação.
- Todo trabalho continua exclusivamente em `main`; não criar branches nem usar pull requests como fluxo de desenvolvimento.
- Nenhuma IA pode modificar trabalho de outra IA sem autorização específica do mantenedor. Esta pesquisa não altera módulos de CPU, MMU, loader, PICA, frontend Android ou presenter.
- Antes de consultar código de terceiros, identificar a licença do projeto e do arquivo específico. A existência de um repositório público não torna todo o conteúdo reutilizável.
- Não incluir jogos, ROMs, CIA/CCI/3DS, firmware, chaves, saves ou dados extraídos no Git, CI, issues, releases ou artefatos.
- As referências produzem hipóteses e especificações independentes; a confirmação exige testes próprios e, quando possível, hardware legítimo.

## Matriz de referências

| Projeto | O que estudar | Limite de uso |
|---|---|---|
| [Azahar](https://github.com/azahar-emu/azahar) | Compatibilidade 3DS, PICA200, GSP, serviços HLE, sincronização, renderizadores e Android | GPL-3.0; estudo técnico e comportamental. Nenhum código, shader, teste ou asset deve ser incorporado |
| [PPSSPP](https://github.com/hrydgard/ppsspp) | JIT/dynarec ARM64, intérprete rápido, caches de blocos, precisão versus velocidade, renderização e perfis móveis | GPL-2.0-or-later; somente referência. Implementação do Aceris deve ser original em Rust |
| [DraStic](https://drasticemulator.com/frequently-asked-questions-drastic-ds-emulator/) | Experiência Android, duas telas, controles táteis, suspensão/retomada, desempenho e configurações | Projeto fechado; não há código-fonte público para consulta. Apenas comportamento observável e documentação oficial |
| [Eden](https://github.com/eden-emulator/mirror) | Vulkan móvel, seleção de GPU/driver, perfis por dispositivo e diagnóstico gráfico | GPL-3.0-or-later; Switch é hardware diferente de 3DS. Usar só como referência de backend e operação Android |
| [PCSX2](https://github.com/PCSX2/pcsx2) | Separação entre CPU e GS, intérpretes/recompiladores, OpenGL/Vulkan, sincronização e testes de renderizador | GPL-3.0; projeto desktop/PS2. Não transportar código ou pressupor semântica PICA200 |
| [Black for Android](https://github.com/RanggaCasper/Black-Android) | Fluxo de importação, verificação local, pacote ARM64, Vulkan, controles na tela, logs e configurações por dispositivo | Licença MIT no código próprio do repositório; conteúdo de jogo e arquivos de terceiros mantêm direitos/licenças próprios. A recompilação de Xbox não é emulação 3DS |
| [Dragon Rage / Tenkaichi3DecompAndroid](https://github.com/dfdx047/Tenkaichi3DecompAndroid) | Empacotamento Android, importação local, suporte a GPU, configuração, controles táteis e distribuição de APK | Port/recompilação de jogo PS2, não emulador. O repositório declara AGPL-3.0 para o port e MIT para partes do app; código do jogo continua pertencendo aos detentores dos direitos |
| [Gen1Recomp](https://github.com/bryanthaboi/gen1recomp) | Validação de arquivo fornecido pelo usuário, importação, cache privado versionado, progresso, saves e recuperação de falhas | É uma recriação em Lua/LÖVE, não um emulador Game Boy nem um recompilador estático. Não foi encontrada licença na raiz consultada; não usar seu código até esclarecer a licença |

## Lições técnicas que se aplicam ao Aceris

### CPU ARM11 e desempenho

1. Separar custo de interpretação, MMU/TLB, scheduler, serviços HLE, memória e rasterização por quadro.
2. Preservar uma implementação de referência precisa antes de criar caminhos rápidos.
3. Investigar dispatch de instruções, redução de alocações, acesso rápido a regiões de código imutáveis e caches de tradução.
4. Considerar um dynarec ARMv6/ARMv6K → AArch64 escrito em Rust apenas depois de ter contratos de exceção, MMU, self-modifying code, invalidação e sincronização bem testados. Um JIT não deve ser iniciado como um atalho sem esses invariantes.
5. Comparar throughput com a mesma sequência de quadros e entradas. Não comparar apenas tempos de boot ou quadros ociosos.

### PICA200 e backend gráfico

1. Manter o rasterizador PICA em CPU como oráculo de compatibilidade.
2. Definir um pacote de draw imutável, independente de Vulkan/OpenGL, contendo estado de vértices, shaders, texturas, TEV, framebuffer, depth/stencil, blend e scissor.
3. Introduzir aceleração por subconjuntos de funcionalidades, com fallback explícito para CPU quando uma operação não estiver suportada.
4. Preservar semântica de memória guest, DMA, render targets, readback, ordenação e sincronização. Uma swapchain que mostra um frame já composto não prova aceleração PICA.
5. Comparar imagens determinísticas CPU versus backend host com entradas próprias e registrar diferenças por pixel.

### Android e entrega testável

1. Produzir APK ARM64 repetível a partir da branch `main`, sem incluir conteúdo de jogo.
2. Validar assinatura, ABI, manifesto, dependências nativas e instalação antes de testar gameplay.
3. No aparelho, registrar SoC, GPU/driver, Android, memória disponível, backend selecionado, tempo por quadro, temperatura e logs.
4. Testar primeiro inicialização, duas telas, toque, pausa/retomada e retorno ao launcher; depois carregar homebrew local sem criptografia externa.
5. Não declarar compatibilidade jogável a partir de compilação, tela estática ou teste headless.

## Estado observado no Aceris em 25/11/2026

- A branch `main` estava no commit `c1b39086ccd86d6753af33b1335672ca1bb701e9` quando este documento foi preparado.
- O histórico de trabalho registra Cookie Clicker executando gameplay headless com injeção de toques e HUD em 8 cookies.
- Um perfil anterior registrou cerca de 3,5 quadros/s no desktop; isso é evidência de gargalo de desempenho, não uma medição Android.
- O presenter `aceris-graphics` apresenta frames LCD já compostos; ainda não rasteriza diretamente os draws PICA200.
- Android ARM64 compila na CI, mas o projeto ainda não tinha APK instalado ou validado em aparelho físico.
- A CI global recente falhou no Clippy do `aceris-core` com ocorrências de `manual_is_multiple_of`. A correção desses módulos deve ser coordenada com o agente responsável; esta etapa não os modifica.

## Próximos experimentos próprios

1. Obter um APK de teste assinado automaticamente para instalar a aplicação sem depender de consertar primeiro o Clippy global.
2. Fazer um smoke test no aparelho sem jogo e confirmar ciclo de vida, superfície, toque e logs.
3. Carregar um homebrew 3DS legal do usuário pela pasta local e guardar diagnóstico de serviços HLE.
4. Repetir no Android o benchmark por quadro com a mesma sequência de toque usada no desktop.
5. Só então priorizar o maior gargalo observado: CPU/interpreter, rasterização PICA, GPU host, áudio ou acesso a arquivos.

## Referências públicas consultadas

- Azahar README e changelog: https://github.com/azahar-emu/azahar e https://github.com/azahar-emu/azahar/releases
- PPSSPP introdução de desenvolvimento e licença: https://www.ppsspp.org/docs/development/getting-started/
- DraStic FAQ (código fechado): https://drasticemulator.com/frequently-asked-questions-drastic-ds-emulator/
- Eden repository e handbook: https://github.com/eden-emulator/mirror e https://github.com/eden-emulator/mirror/tree/master/docs/user
- PCSX2 repository e GS renderer: https://github.com/PCSX2/pcsx2 e https://github.com/PCSX2/pcsx2/blob/master/pcsx2/GS/GS.cpp
- Black for Android: https://github.com/RanggaCasper/Black-Android
- Dragon Rage: https://github.com/dfdx047/Tenkaichi3DecompAndroid
- Gen1Recomp: https://github.com/bryanthaboi/gen1recomp
