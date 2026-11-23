# Plano de paridade funcional com emuladores 3DS maduros

> Revisão inicial em 23/11/2026. Este documento usa o Azahar como referência de comportamento e de cobertura funcional, não como fonte de implementação. Todo código do Aceris continua original e escrito em Rust.

## Objetivo

Reduzir o tempo até o primeiro jogo comercial jogável, usando documentação pública independente, recursos visíveis do Azahar, sua lista pública de compatibilidade e testes próprios do Aceris para identificar lacunas. Não é necessário reproduzir toda a interface ou todos os recursos avançados antes de conseguir executar um jogo.

## Limites de referência

Permitido:
- Comparar recursos visíveis: formatos aceitos, inicialização, controles, áudio, vídeo, salvamento, configurações e plataformas.
- Consultar documentação pública independente de hardware, formatos, serviços e APIs.
- Usar relatos públicos de compatibilidade como seleção de cenários de teste, sem tratá-los como prova do hardware.

Não permitido:
- Copiar, traduzir, adaptar ou reproduzir código, testes, fixtures, shaders, patches, assets ou arquitetura interna de outro emulador.
- Incorporar firmware, chaves, jogos ou dados protegidos de terceiros.
- Marcar compatibilidade com base apenas em semelhança visual ou em um boot isolado.

## Situação observada no Aceris

Base: código e documentação do repositório em `main`, snapshot funcional e handoff de 22–23/11/2026.

- O carregador já reconhece 3DSX e caminhos NCSD/CCI e CIA em condições limitadas, incluindo conteúdo NCCH NoCrypto. Instalação persistente e gestão de títulos CIA não estão concluídas.
- Um homebrew CIA real (Lua Player Plus Cookie Clicker) já executa código ARM11, lê RomFS e submete listas GPU/trocas de buffer. O teste chegou a centenas de quadros sem erro, mas as capturas permanecem pretas e o desempenho observado é de aproximadamente 2 quadros/s com orçamento de 4 milhões de instruções por quadro. Isso não é gameplay validado.
- O kernel, SVC, sessões, IPC, memória compartilhada, GSP e vários serviços têm implementações parciais. Ainda faltam fluxos completos de scheduler, bloqueio/despertar, IPC e serviços exigidos por jogos.
- A CPU ARM11 tem ampla cobertura A32/Thumb, MMU/TLB e VFP, mas a auditoria de ISA, coprocessadores e temporização não está encerrada.
- O PICA200 tem caminhos de geometria e fragmentos em progresso, mas ainda faltam partes importantes: textura procedural ativa (unidade 3), operações gas/shadow, cubemap/projeção, clipping/precisão e validação com cenas de referência.
- O caminho de áudio/DSP é inicial; o HLE de csnd já passou pela inicialização e reinicialização observadas, mas áudio audível e compatibilidade de jogos não foram comprovados.
- O estado de entrada do host existe, mas ainda precisa chegar ao guest pelo serviço HID e sua memória compartilhada. Gamepad físico, sensores e microfone também precisam de trabalho.
- A apresentação de vídeo e os frontends desktop/Android estão incompletos. Build Android ARM64 no CI não significa APK validado em aparelho.
- Saves, armazenamento de usuário, NAND/SD, instalação de títulos e gestão de conteúdo ainda precisam de integração e testes reais.

## Matriz de lacunas por prioridade

### P0 — tornar um jogo visível e controlável

1. **Desempenho do ARM11 e do renderizador**
   - Perfilar separadamente fetch/decodificação/execução ARM, MMU/D-cache, scheduler, PICA200 e cópias de framebuffer.
   - Usar o mesmo homebrew e o mesmo orçamento de instruções como baseline reproduzível.
   - Meta: remover gargalos dominantes sem alterar a semântica; medir instruções por segundo e quadros por segundo.
   - Respeitar o isolamento entre agentes: não modificar caminhos de código atribuídos a outro agente sem autorização explícita.

2. **HID de ponta a ponta**
   - Completar a memória compartilhada HID e o serviço `hid:USER`.
   - Conectar botões, circle pad e touchscreen já recebidos pelo frontend ao estado que o jogo realmente lê.
   - Validar press/release, coordenadas, dead zones e polling com homebrew de teste.

3. **GPU PICA200 de ponta a ponta**
   - Primeiro corrigir a tela preta: provar que command lists, vertex fetch, shader, rasterização, color target, DisplayTransfer e scanout estão ligados ao mesmo backing e à ordem de sincronização correta.
   - Criar cenas determinísticas próprias para triângulos, texturas, alpha, depth/stencil, blending, scissor, culling e troca de buffers.
   - Só depois avançar para os recursos de maior complexidade: textura procedural, gas/shadow, cubemap/projeção e layouts tiled especiais.
   - Não ativar um recurso cujo resultado ainda seja especulativo; manter falha explícita até existir teste confiável.

4. **Ciclo de vida guest/kernel**
   - Completar scheduler de threads, estados de espera, timeouts, sincronização e entrega de IRQ/timers de dispositivos.
   - Fechar o caminho IPC de processos e a transferência de handles, preservando os contratos já testados de memória compartilhada e GSP.
   - Substituir stubs de serviços por implementações mínimas orientadas por traces reais e documentação independente.

### P1 — transformar o primeiro boot em sessão jogável

5. **Áudio e DSP**
   - Fechar a sequência csnd/DSP usada pelos títulos, buffers, sincronização e saída audível.
   - Validar init/exit/reinit, reprodução, volume, underrun e latência em desktop e Android.

6. **Filesystem, saves e conteúdo**
   - Completar leitura/escrita de arquivos guest, save data, SD/NAND modelados e serviços FS/AM necessários.
   - Completar CIA install/launch e validação de NCCH criptografado usando apenas chaves e conteúdo fornecidos legalmente pelo usuário.
   - Testar persistência entre encerramento e reabertura; nunca colocar jogos, firmware, chaves ou saves no repositório/CI.

7. **Compatibilidade de CPU e memória**
   - Fechar lacunas relevantes de ARM11/Thumb/VFP/CP15, exceções, cache, write buffer, temporização e SMP.
   - Priorizar instruções e estados realmente alcançados pelos títulos de teste; manter testes independentes por comportamento.

8. **Vídeo e plataformas**
   - Completar apresentação das duas telas, seleção estéreo, escala, orientação e sincronização.
   - Validar teclado, controles, toque, ciclo de vida, áudio e armazenamento em Windows, Linux, macOS e dispositivos Android reais.

### P2 — paridade de produto e compatibilidade ampla

9. **Recursos de conveniência**
   - Configurações por jogo, remapeamento de controles, shaders/cache, escalas de resolução, limites de velocidade, screenshots e ferramentas de diagnóstico.
   - Save states somente depois que o estado completo de CPU, GPU, DSP, serviços, timers e armazenamento puder ser serializado com segurança.

10. **Matriz pública de compatibilidade**
    - Selecionar títulos de teste por variedade de subsistemas e registrar versão/região, formato, requisitos de sistema, renderer, configuração, resultado e logs.
    - Usar uma escala interna simples: não inicia, inicia sem imagem, imagem parcial, menu, gameplay, gameplay com áudio, conclusão.
    - Só anunciar um jogo como jogável após sessão reproduzível com entrada, vídeo, áudio e salvamento testados.

## Sequência de execução recomendada

1. Perfil de CPU/GPU no cenário Cookie Clicker, sem mexer em arquivos de outro agente.
2. Diagnóstico da tela preta com cenas PICA200 próprias e captura de cada estágio do pipeline.
3. HID guest-side e primeiro controle real de um homebrew.
4. Scheduler/IPC/serviços que o primeiro título comercial exigir, guiados por traces.
5. Áudio/DSP e save data.
6. Teste de um jogo comercial escolhido pelo mantenedor, com dump próprio e requisitos do sistema fornecidos pelo usuário.
7. Expandir a matriz de jogos e só então priorizar recursos avançados.

## Critério de sucesso do primeiro jogo jogável

Um título comercial só será considerado jogável quando:
- carregar a partir de um arquivo suportado e de dados de sistema fornecidos pelo usuário;
- chegar ao menu e permitir navegação por controles guest-side;
- renderizar corretamente as duas telas durante uma sessão contínua;
- produzir áudio audível sem travamentos relevantes;
- permitir salvar e carregar progresso pelo fluxo normal do jogo;
- manter desempenho suficiente para jogar, sem depender de limites artificiais de quadros;
- passar por uma sessão reproduzível documentada em PC e, posteriormente, Android.

## Referências públicas consultadas

- Azahar — visão geral, plataformas, recursos e requisitos: https://github.com/azahar-emu/azahar
- Azahar — página oficial: https://www.azahar-emu.org/
- Azahar — releases e changelogs públicos (incluindo melhorias de interpreter, Android e configurações por aplicativo): https://github.com/azahar-emu/azahar/releases
- Azahar — lista pública de compatibilidade: https://github.com/azahar-emu/compatibility-list

A lista de compatibilidade do Azahar usa uma escala de 0 a 5 e distingue títulos não testados; ela serve como referência para escolher cenários, não como promessa de compatibilidade do Aceris.

## Regra de implementação

Toda implementação do Aceris permanece 100% original e em Rust, somente na branch `main`. Referências externas orientam o que testar e qual comportamento buscar; não fornecem código para reutilização. Preservar o isolamento de trabalho entre agentes e consultar o mantenedor antes de qualquer sobreposição.
