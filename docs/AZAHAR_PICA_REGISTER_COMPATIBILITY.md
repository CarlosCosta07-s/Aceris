# Compatibilidade de tabelas PICA200 — Aceris

Atualizado em 26/11/2026. Esta especificação descreve tabelas de registradores e seletores adicionados ou explicitados nesta revisão. Não afirma compatibilidade completa com a GPU física.

## Proveniência

A referência de software foi consultada no repositório [azahar-emu/azahar](https://github.com/azahar-emu/azahar), revisão `c621eafbf316699c55f4c9c0c689d4e49498d360` (25/11/2026; licença geral GPL-3.0). Os cabeçalhos dos arquivos PICA consultados indicam GPLv2-or-later:

- [regs_texturing.h](https://github.com/azahar-emu/azahar/blob/c621eafbf316699c55f4c9c0c689d4e49498d360/src/video_core/pica/regs_texturing.h)
- [sw_texturing.cpp](https://github.com/azahar-emu/azahar/blob/c621eafbf316699c55f4c9c0c689d4e49498d360/src/video_core/renderer_software/sw_texturing.cpp)
- [sw_rasterizer.cpp](https://github.com/azahar-emu/azahar/blob/c621eafbf316699c55f4c9c0c689d4e49498d360/src/video_core/renderer_software/sw_rasterizer.cpp)
- [regs_lighting.h](https://github.com/azahar-emu/azahar/blob/c621eafbf316699c55f4c9c0c689d4e49498d360/src/video_core/pica/regs_lighting.h)

Os comportamentos foram descritos em termos independentes e implementados em Rust com tipos, funções e testes próprios. Nenhuma linha de código, fixture, teste, shader, asset ou implementação foi incorporada. Os valores abaixo são contratos de seleção e precisam de validação em hardware.

## Tipo de textura (GPUREG_TEXUNITn_PARAM, bits 28–30)

| Valor | Nome | Política atual |
|---:|---|---|
| 0 | Texture2D | Aceito |
| 1 | TextureCube | UnsupportedType |
| 2 | Shadow2D | UnsupportedType |
| 3 | Projection2D | Aceito pelo decoder atual |
| 4 | ShadowCube | UnsupportedType |
| 5 | Disabled | UnsupportedType se a unidade estiver marcada como ativa |
| 6–7 | Reservado | Preservado como Reserved(6/7) |

O campo de tipo é consumido apenas pela unidade 0. As unidades 1 e 2 não reutilizam essa interpretação.

## Wrapping S/T (bits 12–14 e 8–10)

| Valor | Nome | Regra |
|---:|---|---|
| 0 | ClampToEdge | Limita pelos limites da textura |
| 1 | ClampToBorder | Coordenadas negativas ou além da extensão retornam border color |
| 2 | Repeat | Coordenada periódica |
| 3 | MirroredRepeat | Período de duas extensões, com segunda metade espelhada |
| 4 | ClampToEdge2 | Coordenada negativa repete; coordenada não negativa limita à borda |
| 5 | ClampToBorder2 | Coordenada negativa repete; coordenada não negativa fora da extensão usa border color |
| 6 | Repeat2 | Alias de Repeat |
| 7 | Repeat3 | Alias de Repeat |

Para os modos 4/5, a decisão é aplicada por eixo antes da leitura do texel. A filtragem linear consulta cada tap após essa classificação, permitindo que taps externos usem a cor de borda. O valor exatamente igual a 1.0 é tratado como borda nos modos 1/5 e como último texel nos modos clamp-to-edge.

**Confiança:** modos 0–3 também aparecem na documentação independente de registradores; modos 4–7 estão documentados na referência de software do Azahar e são evidência E2, não confirmação física. Seletores acima de 7 são rejeitados.

## Configuração de iluminação (GPUREG_LIGHTING_CONFIG0, bits 4–7)

Config7 usa o valor 8, não 7.

| Configuração | D0 | D1 | SP | FR | RR | RG | RB |
|---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| 0 | Sim | Não | Sim | Não | Sim | Não | Não |
| 1 | Não | Não | Sim | Sim | Sim | Não | Não |
| 2 | Sim | Sim | Não | Não | Sim | Não | Não |
| 3 | Sim | Sim | Não | Sim | Não | Não | Não |
| 4 | Sim | Sim | Sim | Não | Sim | Sim | Sim |
| 5 | Sim | Não | Sim | Sim | Sim | Sim | Sim |
| 6 | Sim | Sim | Sim | Sim | Sim | Não | Não |
| 8 | Sim | Sim | Sim | Sim | Sim | Sim | Sim |

Legenda: D0/D1 = Distribution 0/1; SP = Spotlight attenuation; FR = Fresnel; RR/RG/RB = Reflect Red/Green/Blue.

## Banco de 24 LUTs de iluminação

| Índice | Tabela |
|---:|---|
| 0 | Distribution 0 |
| 1 | Distribution 1 |
| 2 | Reservado |
| 3 | Fresnel |
| 4 | Reflect Blue |
| 5 | Reflect Green |
| 6 | Reflect Red |
| 7 | Reservado |
| 8–15 | Spotlight attenuation por luz |
| 16–23 | Distance attenuation por luz |

O Aceris já armazenava as 24 LUTs. Esta revisão nomeia os seletores e os intervalos por luz e testa a matriz de disponibilidade; ela não declara fórmulas de lighting, sombras ou todas as configurações validadas no hardware.

## Implementação Aceris

- Tipos e amostragem de textura: `crates/aceris-core/src/gpu/texture.rs`.
- Configuração de iluminação e matriz de samplers: `crates/aceris-core/src/gpu/lighting.rs`.
- Testes próprios nos mesmos módulos.
- Próximas lacunas: cube maps, shadow compare, geometry shader, gas, clipping, depth offset e comparação com console físico.


## Modo de operação de fragmento e mesclagem

### GPUREG_COLOR_OPERATION, bits 0–1

| Valor | Modo | Situação no Aceris |
|---:|---|---|
| 0 | Default | Implementado |
| 1 | Gas | Identificado; processamento Gas ainda não integrado ao fragment pipeline |
| 2 | Reservado | Rejeitado explicitamente |
| 3 | Shadow | Identificado; processamento Shadow ainda não integrado |

O Aceris expõe `PicaFragmentOperation::from_register` para nomear os quatro resultados, mas só executa `Default`. Isso evita confundir o reconhecimento do seletor com suporte ao algoritmo Gas/Shadow.

### GPUREG_BLEND_FUNC, bits 0–2 e 8–10

| Valor | Equação |
|---:|---|
| 0 | Add |
| 1 | Subtract |
| 2 | ReverseSubtract |
| 3 | Minimum |
| 4 | Maximum |
| 5–7 | Alias observado de Add |

O enum de referência Azahar lista as cinco equações primárias (0–4). A documentação independente 3dbrew observa que 5–7 parecem produzir o mesmo resultado que Add; por isso o Aceris preserva esses aliases e mantém o teste `observed_blend_equation_aliases_decode_as_add`. Essa evidência de alias é observacional, não uma enumeração formal dos valores.

### GPUREG_COLORBUFFER_FORMAT — ponto em aberto

O enum de referência Azahar inclui RGB8 (formato 1), além de RGBA8, RGB5A1, RGB565 e RGBA4. Entretanto, a descrição independente de registradores PICA interno consultada documenta somente pixel-size 0 (16-bit) e 2 (32-bit), e formatos 0, 2, 3 e 4; RGB8 não aparece nessa tabela interna. A página de registradores externos lista RGB8 para o controlador de display, o que não prova que o framebuffer PICA interno use o mesmo seletor.

O Aceris continua aceitando apenas as combinações internas documentadas. Não foi atribuído um valor especulativo de pixel-size para RGB8. A implementação desse formato deve aguardar uma confirmação independente do layout do registrador ou uma captura em hardware.


## Filtros da unidade procedural — GPUREG_TEXUNIT3_PROCTEX4

| Valor | Seletor | Aceris |
|---:|---|---|
| 0 | Nearest | Nearest na LUT do nível 0 |
| 1 | Linear | Linear na LUT do nível 0 |
| 2 | NearestMipmapNearest | Filtro nearest do nível 0; seleção de mip pendente |
| 3 | LinearMipmapNearest | Filtro linear do nível 0; seleção de mip pendente |
| 4 | NearestMipmapLinear | Filtro nearest do nível 0; seleção de mip pendente |
| 5 | LinearMipmapLinear | Filtro linear do nível 0; seleção de mip pendente |
| 6–7 | Reservado | Rejeitado explicitamente |

O código software de referência do Azahar também contém uma observação TODO para implementar mipmapping nessa unidade; no caminho presente, ele agrupa os quatro seletores mipmap pela filtragem nearest/linear da LUT de cor. O Aceris reconhece e executa essa mesma filtragem intranível no nível 0, mas ainda não escolhe níveis por LOD, não aplica os offsets 1–3 e não considera min/max LOD ou bias superior para selecionar mip levels. Portanto, não se deve anunciar mipmapping procedural completo.


## SVCs ARM11 já implementados — identificadores nomeados (26/11/2026)

| ID | SVC | Aceris |
|---:|---|---|
| 0x21 | CreateAddressArbiter | Handler integrado |
| 0x22 | ArbitrateAddress | Handler integrado |
| 0x2A | GetSystemInfo | Handler integrado; valores suportados são limitados ao perfil modelado |
| 0x52 | InvalidateProcessDataCache | Handler integrado |
| 0x53 | StoreProcessDataCache | Handler integrado |
| 0x54 | FlushProcessDataCache | Handler integrado |

Esses handlers já existiam no session dispatcher; esta revisão remove literais numéricos do caminho de dispatch, concentra os IDs em `session/svc.rs` e adiciona regressão para a tabela. O fato de um ID estar nomeado e encaminhado não significa que todas as variantes, tipos de informação ou semânticas de cache estejam verificadas em hardware.


## Fog/Gas mode selector — GPUREG_TEXENV_UPDATE_BUFFER bits 0–2

| Valor | Modo | Estado no Aceris |
|---:|---|---|
| 0 | Disabled | Implementado |
| 1 | Linear fog | Implementado |
| 2–4 | Reservado | Rejeitado explicitamente |
| 5 | Fog por LUT | Implementado |
| 6 | Reservado | Rejeitado explicitamente |
| 7 | Gas | Reconhecido; estágio Gas ainda não executado no fragment pipeline |

PicaFogMode agora nomeia os oito valores possíveis do campo de três bits. A decodificação do modo 7 não implica rasterização Gas. O snapshot PicaGasState já lê os registradores Gas e os seletores de densidade/entrada da LUT, mas a operação de fragmento Gas permanece desativada.

## Formato de framebuffer — tabela de seletores de três bits

| Seletor | Formato nomeado | Estado |
|---:|---|---|
| 0 | RGBA8 | Suportado com pixel-size 2 |
| 1 | RGB8 | Nomeado; combinação interna de pixel-size não confirmada |
| 2 | RGB5A1 | Suportado com pixel-size 0 |
| 3 | RGB565 | Suportado com pixel-size 0 |
| 4 | RGBA4 | Suportado com pixel-size 0 |
| 5–7 | Reservado | Rejeitado explicitamente |

PicaColorBufferFormatSelector representa o campo de formato independentemente do campo pixel-size. Isso permite preservar o código RGB8 observado no enum do Azahar sem habilitar uma combinação que a tabela independente de registradores PICA interno não confirma. PicaColorBufferFormat continua representando somente os formatos que o backend consegue codificar e armazenar corretamente.


## Parâmetros de sombra — GPUREG_FRAGOP_SHADOW (0x130)

| Bits | Campo | Formato |
|---:|---|---|
| 0–15 | Soma da escala de penumbra e do bias | float1.5.10 |
| 16–31 | Escala de penumbra com sinal invertido | float1.5.10 |

O Aceris expõe os dois campos por meio de PicaShadowState e preserva os valores brutos de 16 bits. A captura do estado não equivale ao processamento Shadow: o seletor 3 de GPUREG_COLOR_OPERATION continua sem integração no fragment pipeline.


## Plano de clipping do fragment shader — GPUREG_FRAGOP_CLIP (0x047)

| Registrador | Campo | Codificação |
|---|---|---|
| 0x047 | bit 0 habilita o plano | booleano |
| 0x048 | coeficiente 0 | float1.7.16 |
| 0x049 | coeficiente 1 | float1.7.16 |
| 0x04A | coeficiente 2 | float1.7.16 |
| 0x04B | coeficiente 3 | float1.7.16 |

PicaClipPlaneState expõe a habilitação e os quatro coeficientes como valores brutos de 24 bits. O estado é capturado, mas a equação ainda não é aplicada ao clipping dos triângulos pelo rasterizador.
