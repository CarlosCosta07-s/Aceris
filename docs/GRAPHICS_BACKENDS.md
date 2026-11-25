# Backends gráficos OpenGL e Vulkan do Aceris

**Estado verificado em 25/11/2026.** Este documento separa o pipeline guest PICA200, o apresentador das LCDs e os futuros backends acelerados. São camadas diferentes e precisam de critérios de validação diferentes.

## Estado atual

### PICA200 e referência por software

O núcleo já contém um caminho PICA200 em Rust:

- banco de registradores e decodificação de command lists;
- fila GX/GSP e parte dos comandos de DMA, memória, cópia e transferência;
- fetch de índices/atributos, execução de vertex shader, output map, clipping e montagem de triângulos;
- `PicaFragmentRenderSession`, conectada ao estado capturado no draw;
- unidades de textura 2D, TEV, unidade procedural 3, fog, alpha test, depth/stencil, blending, logic ops e máscaras;
- rasterização de referência por software e codecs dos render targets físicos 8×8/32×32;
- transferência e scanout das superfícies LCD.

Esse caminho é a implementação de referência que deve continuar funcionando mesmo quando um backend acelerado estiver disponível. Ele também é o oráculo de comparação para testes de equivalência visual.

### Apresentação host atual — primeira etapa implementada em 25/11/2026

O núcleo continua produzindo as três superfícies LCD em memória CPU/RGBA. A camada de apresentação host agora também tem um caminho GPU próprio:

- crate Rust `aceris-graphics`, separado do núcleo e do estado guest;
- `wgpu 30.0.2` com backends Vulkan e GL (OpenGL/OpenGL ES), e Metal para a seleção automática no macOS;
- criação de surface/swapchain a partir de handles nativos: Xlib, Win32, AppKit e Android Native Window;
- upload de frames RGBA/BGRX para textura host, pipeline WGSL de apresentação e submissão do frame;
- seleção por `ACERIS_GRAPHICS_BACKEND=auto|opengl|vulkan|software`; aliases `gl`, `gles`, `vk` e `cpu` também são aceitos;
- o desktop e o Android mantêm o caminho CPU nativo como fallback quando a inicialização, aquisição de frame ou apresentação GPU falha.

A camada usa uma abstração comum para acionar os backends nativos de `wgpu`; ela não é uma implementação manual de GLX/WGL/EGL/Vulkan. No modo `auto`, macOS usa Metal e as demais plataformas habilitam Vulkan/GL para seleção do adapter compatível.

**Limite decisivo:** este presenter recebe pixels LCD já compostos pelo caminho CPU. Ele não recebe DRAWARRAYS/DRAWELEMENTS, não executa vertex/fragment shaders PICA, não gerencia texturas PICA nem grava render targets guest. Portanto a apresentação OpenGL/Vulkan está integrada, mas o rasterizador PICA acelerado ainda não está implementado.

## Referência técnica Azahar — somente arquitetura

A pesquisa foi feita na revisão pública `71915de9e326da0bfe76902bf9269406a3dc3adc` do [Azahar](https://github.com/azahar-emu/azahar), sem copiar ou adaptar código, shaders, testes ou assets.

O README dessa revisão indica, como requisitos de GPU do próprio Azahar, desktop OpenGL 4.3 ou Vulkan 1.1 e Android OpenGL ES 3.2 ou Vulkan 1.1. Isso é uma referência de requisitos, não uma garantia de que todas as GPUs que executem Aceris suportarão as mesmas APIs.

A arquitetura de alto nível observada é:

- conversores PICA→OpenGL e PICA→Vulkan para enums e estados fixos (filtros, wrap, topologia, culling, blend, depth/stencil e logic ops);
- rasterizador OpenGL que sincroniza estado do draw, recupera superfícies, configura viewport/scissor, liga texturas, seleciona shaders e envia LUTs/uniforms antes do draw;
- rasterizador Vulkan que monta/cacheia pipelines gráficos e gerencia descritores, recursos, submissões e sincronização.

Essas observações servem como lista de responsabilidades, não como implementação transplantável. O repositório Azahar declara GPL-3.0; os arquivos consultados incluem cabeçalhos GPL-3.0-or-later e GPL-2.0-or-later. O Aceris continua MIT e deve manter implementação original em Rust.

Referências públicas consultadas:

- [Azahar README — revisão consultada](https://github.com/azahar-emu/azahar/blob/71915de9e326da0bfe76902bf9269406a3dc3adc/README.md)
- [Azahar LICENSE — revisão consultada](https://github.com/azahar-emu/azahar/blob/71915de9e326da0bfe76902bf9269406a3dc3adc/LICENSE.txt)
- [OpenGL rasterizer — revisão consultada](https://github.com/azahar-emu/azahar/blob/71915de9e326da0bfe76902bf9269406a3dc3adc/src/video_core/renderer_opengl/gl_rasterizer.cpp)
- [PICA→OpenGL mappings — revisão consultada](https://github.com/azahar-emu/azahar/blob/71915de9e326da0bfe76902bf9269406a3dc3adc/src/video_core/renderer_opengl/pica_to_gl.h)
- [Vulkan rasterizer — revisão consultada](https://github.com/azahar-emu/azahar/blob/71915de9e326da0bfe76902bf9269406a3dc3adc/src/video_core/renderer_vulkan/vk_rasterizer.cpp)
- [PICA→Vulkan mappings — revisão consultada](https://github.com/azahar-emu/azahar/blob/71915de9e326da0bfe76902bf9269406a3dc3adc/src/video_core/renderer_vulkan/pica_to_vk.h)
- [PICA200 framebuffer registers — 3dbrew](https://www.3dbrew.org/wiki/GPU/Internal_Registers)

## Contrato que os backends devem respeitar

O backend host não deve substituir o modelo de memória guest nem executar APIs gráficas a partir do caminho ARM11. A fronteira deve ficar no pipeline GPU:

1. O parser PICA captura os registradores e os parâmetros do draw no instante em que o guest dispara DRAW.
2. O core resolve endereços guest/virtuais/físicos, valida memória, obtém atributos/índices e mantém a semântica de cache/DMA.
3. Um pacote de draw imutável contém estado PICA já decodificado: topologia, viewport, scissor, culling, depth/stencil, blend, formato e endereços dos recursos, saída do vertex shader e bindings de textura/LUT.
4. O backend escolhido traduz esse pacote para o estado e os recursos da API host.
5. Operações que o backend não suporta precisam cair explicitamente no renderizador de referência ou retornar diagnóstico. Nunca descartar um draw silenciosamente.
6. A coerência com CPU/DMA, mudanças de render target, readback e sincronização entre filas precisam ter semântica definida. Uma fence de apresentação não substitui o modelo de visibilidade de memória do guest.

O pacote de draw deve ser independente de OpenGL/Vulkan; nenhuma estrutura `Vk*`, `GLenum` ou handle de janela deve entrar em `aceris-core` como parte do estado guest.

## Diferença entre apresentar e acelerar PICA

**Presenter OpenGL/Vulkan:** recebe os três buffers LCD RGBA, faz upload e compõe as telas em uma surface/swapchain. Isso pode reduzir o custo de composição e apresentação, mas continua pagando pelo rasterizador PICA em CPU. É uma primeira entrega útil, porém não é um rasterizador PICA acelerado.

**Rasterizador PICA acelerado:** recebe draws e estado PICA, traduz vertex/fragment shaders e estados fixos, gerencia render targets/texturas/depth, submete draws à GPU host e preserva o contrato de memória/readback. Essa é a entrega completa que o pedido de backend PICA exige.

Os dois caminhos podem coexistir: software de referência para compatibilidade/diagnóstico, GL e Vulkan como opções aceleradas e fallback quando um recurso não for suportado.

## Sequência de implementação e critérios de aceite

### 1. Fechar a semântica PICA independente da API

- manter o renderizador CPU como oráculo;
- normalizar snapshots de draw e estado de framebuffer;
- fechar viewport, scissor, face culling, formatos de render target, depth/stencil, TEV, fog e blending;
- terminar os modos de fragmento e textura ainda não suportados (GAS, shadow, cubemap/projection, filtros/mipmaps procedurais e layout 32×32);
- cada item exige testes determinísticos, além de validação com homebrew/jogo real quando possível.

### 2. OpenGL

- criar superfície e contexto por plataforma (desktop e Android usam ciclos de vida diferentes);
- testar capacidades e extensões na inicialização;
- implementar buffers, texturas, framebuffer objects, shaders, upload de LUTs e cache de recursos;
- traduzir o estado PICA para OpenGL/ES em uma camada separada;
- incluir fallback para o renderizador CPU quando a GPU não satisfizer os requisitos.

### 3. Vulkan

- criar instance, surface, physical/logical device, queues, swapchain e command pools;
- implementar allocator de memória, image/buffer resources, descriptors, pipeline cache e compilação de SPIR-V;
- definir fences/semaphores, sincronização de acesso a imagens e readback;
- traduzir os mesmos snapshots PICA, sem criar uma segunda interpretação do protocolo guest.

### 4. Verificação e comparação

- testes sem GPU para os conversores de estado e pacote de draw;
- smoke tests GL/Vulkan com drivers de software nos runners compatíveis;
- comparação de imagens determinísticas contra o renderizador CPU, com tolerâncias documentadas para precisão de ponto flutuante;
- testes em hardware real Android e desktop para superfície, troca de frame, resize, perda/recriação de contexto e suspensão/retomada;
- perfis por quadro só depois de equivalência visual e coerência de memória.

## Dependências e bloqueios

O mantenedor autorizou explicitamente a inclusão de dependências para implementar a apresentação host OpenGL/OpenGL ES/Vulkan. O crate `aceris-graphics` usa `wgpu`, `raw-window-handle` e `pollster`; a autorização cobre essa camada de API e os bindings de superfície, sem autorizar código de outros emuladores.

A abstração reduz a quantidade de FFI por plataforma, mas não elimina as diferenças de ciclo de vida dos handles nativos. O frontend continua responsável por manter display/window vivos enquanto a surface existir, e o Android retém uma referência extra a `ANativeWindow` até destruir a surface.

A inclusão de `wgpu` não transforma o presenter em rasterizador PICA. O próximo marco exige um pacote de draw PICA independente de API, a tradução de shaders/estados e o contrato de sincronização/readback guest.

## Alteração PICA desta etapa

O bit 24 de `GPUREG_FRAMEBUFFER_DIM` já era decodificado em `PicaRenderTargetConfig.vertical_flip_enabled`, mas o renderizador de fragmentos ignorava o valor. O caminho PICA de referência agora espelha viewport e scissor no eixo Y e ajusta o sinal de winding usado pelo face culling. Isso implementa a semântica de orientação documentada para o framebuffer e dá ao futuro pacote de draw um estado de viewport coerente para GL/Vulkan.

A alteração não cria nem declara prontos os backends OpenGL/Vulkan e ainda precisa passar a matriz completa de CI. A convenção do bit e o efeito sobre o viewport foram comparados com 3dbrew e com a organização de estado do Azahar; a validação final em hardware PICA físico ainda não existe.


## Upload BGRA direto para o presenter desktop — 25/11/2026

- **Gargalo removido:** o desktop compõe a imagem como BGRX8, mas o presenter criava um segundo buffer e percorria todos os pixels na CPU para reorganizar BGRX em RGBA antes de cada upload.
- **Mudança independente em Rust:** `present_bgrx` agora envia os bytes diretamente para uma textura `Bgra8Unorm` ou `Bgra8UnormSrgb`, conforme a superfície. Um entry point próprio do shader ignora o quarto byte (padding X) e escreve alpha 1.0. O caminho Android `present_rgba` continua usando `Rgba8Unorm`/`Rgba8UnormSrgb`.
- **Efeito esperado:** elimina uma varredura e uma escrita temporária de 4 bytes por pixel no desktop. O upload de todos os pixels para a GPU continua necessário; nenhuma redução de FPS/latência foi medida e não se declara ganho numérico.
- **Base da API:** a documentação de `wgpu::TextureFormat` descreve os formatos BGRA8 e sua conversão sRGB para valores float no shader: https://docs.rs/wgpu/latest/wgpu/enum.TextureFormat.html
- **Testes adicionados:** seleção dos quatro formatos RGBA/BGRA nos modos linear/sRGB e verificação de que o shader BGRX força alpha opaco. A CI também executa Clippy isolado de `aceris-graphics`, antes do Clippy do workspace.
- **Limite preservado:** essa é uma otimização do presenter host. Não altera draw calls, shader PICA, render targets guest, memória física ou temporização da PICA200.
