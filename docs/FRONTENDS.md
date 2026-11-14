# Frontends PC e Android

Este documento descreve os contratos de interface compartilhados pelo Aceris. O visual continua seguindo integralmente `docs/IDENTIDADE_VISUAL.md`: fundo roxo profundo, acerola vermelha, verde apenas como apoio e texto claro. Os fundos personalizados são escurecidos na composição para que nomes, estados e seleção continuem legíveis.

## Biblioteca do PC

A biblioteca é renderizada pelo núcleo em uma tela lógica de 400x480 RGBA8. A animação usa quadros determinísticos e elementos sutis da paleta Aceris; não adiciona marcas, ícones ou cores de outro emulador.

O backend X11 redesenha a biblioteca a 60 Hz. Um fundo animado pessoal pode ser carregado com:

```sh
aceris --window --background-dir ~/Aceris/Backgrounds/Atual --background-fps 24
```

A pasta deve conter de 1 a 120 arquivos `.rgba`. Cada arquivo é um quadro 400x480, RGBA8 sem compressão, exatamente 768.000 bytes. Os nomes são ordenados lexicograficamente; use nomes como `frame_000.rgba`, `frame_001.rgba` e assim por diante. A taxa aceita vai de 1 a 60 quadros por segundo e o padrão é 24. A sequência repete em loop.

Exemplo para converter um vídeo pessoal usando FFmpeg instalado pelo usuário:

```sh
mkdir -p ~/Aceris/Backgrounds/Atual
ffmpeg -i fundo.mp4 \
  -vf "fps=24,scale=400:480:force_original_aspect_ratio=increase,crop=400:480" \
  -frames:v 120 -f rawvideo -pix_fmt rgba \
  ~/Aceris/Backgrounds/Atual/frame_%03d.rgba
```

O conversor não é incorporado ao Aceris. O emulador apenas lê os quadros que o usuário escolheu. O formato cru evita adicionar dependências de decodificação de mídia ao núcleo.

### Duas telas durante o jogo

No PC, a orientação é fixa: as duas telas nunca são empilhadas. Quando um jogo é aberto, a janela X11 muda para um canvas de 736x240: a tela superior (400x240) fica à esquerda e a tela inferior (320x240) à direita, separadas por uma margem de 16 pixels. O fator `--scale` amplia as duas telas juntas. Ao retornar à biblioteca, a janela volta ao canvas de interface 400x480. O formato da janela não altera a disposição horizontal das telas durante o jogo.

## Android: orientação e telas

O crate nativo expõe um contrato C ABI para o host Android:

- `aceris_library_set_orientation(handle, mode)`: guarda a preferência `0=automático`, `1=vertical` ou `2=horizontal`.
- `aceris_library_get_orientation(handle)`: recupera a preferência.
- `aceris_library_get_screen_layout(handle, width, height, out)`: calcula os retângulos das duas telas no tamanho real da superfície.

No Android, o usuário pode alternar entre dois modos: no horizontal, as telas ficam lado a lado (superior à esquerda e inferior à direita); no vertical, a tela superior fica acima da inferior, que permanece centralizada. Ambos os modos preservam a proporção, centralizam o conjunto e evitam recorte. O modo automático acompanha a proporção atual da superfície. Essa rotação é exclusiva do Android: no PC as telas permanecem sempre lado a lado.

A ABI informa o modo e a geometria; o host Android é responsável por solicitar a orientação correspondente à Activity e recalcular o layout após mudanças de tamanho. O repositório ainda não possui uma Activity/projeto Android instalável, portanto essa chamada de orientação do sistema operacional e a tela de configurações ainda precisam ser conectadas ao host.

## Android: fundos animados

`aceris_library_render_animated` recebe um quadro RGBA8 opcional do host junto com o número do quadro da animação. O buffer deve ter exatamente 400x480 pixels. O Aceris compõe o quadro durante a renderização e não retém ponteiros do host. Isso permite que um host Android em Rust decodifique uma animação escolhida pelo usuário e envie os quadros sucessivos, sem alterar a identidade visual do emulador.

A composição mistura o fundo personalizado com a base roxa a uma opacidade máxima de 38%. A biblioteca, os textos, o indicador de seleção e o rodapé continuam desenhados por cima.

A ABI também oferece `aceris_library_render_animated_with_backgrounds`, que recebe dois quadros distintos no mesmo ciclo: um para vertical e outro para horizontal. O host escolhe os dois arquivos/animações na sua tela de personalização e envia os quadros atuais; a ABI usa apenas o quadro correspondente à orientação da superfície. Assim o usuário pode manter, por exemplo, um tema animado vertical e outro completamente diferente para horizontal.

## Android: controles virtuais

A ABI também disponibiliza:

- `aceris_ui_render_virtual_controls`: desenha uma camada RGBA transparente com direcional, A/B/X/Y, L/R, Start e Select.
- `aceris_ui_hit_test_virtual_control`: informa qual botão foi tocado usando coordenadas da mesma superfície.

Os bits são estáveis: Up `1<<0`, Down `1<<1`, Left `1<<2`, Right `1<<3`, A `1<<4`, B `1<<5`, X `1<<6`, Y `1<<7`, L `1<<8`, R `1<<9`, Start `1<<10`, Select `1<<11`. O host deve manter o estado dos toques, combinar dedos simultâneos e encaminhar o estado resultante ao dispositivo de entrada do núcleo.

A camada gráfica e o hit-test estão disponíveis. A ligação final desses bits a um dispositivo HID do sistema convidado ainda depende da implementação do dispositivo de entrada no núcleo; não se deve considerar os botões jogáveis até essa integração ser concluída.

## Estado e limites atuais

- PC: biblioteca animada e fundos em sequência RGBA estão integrados ao frontend X11. Durante o jogo, o canvas é 736x240 com as telas lado a lado; a janela redimensiona ao entrar e sair do jogo.
- Android: ABI nativa para renderização animada, fundos fornecidos pelo host, orientação, geometria de telas e controles virtuais.
- Android: ainda falta o host/Activity instalável que apresente as telas, solicite rotação do sistema, faça seleção de mídia e mantenha o ciclo de vida da superfície.
- Windows/macOS: os backends de janela ainda não foram implementados.
- Áudio e integração dos botões virtuais com o HID do núcleo continuam pendentes.

Essas limitações são explícitas: os contratos nativos não são apresentados como um aplicativo Android completo nem como compatibilidade de entrada/áudio já funcional.
