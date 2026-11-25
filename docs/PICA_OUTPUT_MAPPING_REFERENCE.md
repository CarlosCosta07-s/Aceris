# PICA200 — máscara e contagem do mapa de saída do vertex shader

**Data:** 24/11/2026  
**Objetivo:** impedir que linhas do mapa semântico sejam associadas ao registrador shader errado quando a máscara de saída tem lacunas.

## Evidência técnica

A documentação pública de GPU descreve três campos que precisam ser lidos em conjunto:

- `GPUREG_SH_OUTMAP_TOTAL` em `0x04F` limita o número de linhas semânticas `O0..O6` consumidas.
- `GPUREG_SH_OUTMAP_O0..O6` em `0x050..0x056` associam lanes a semânticas.
- `GPUREG_VSH_OUTMAP_MASK` em `0x2BD` habilita os registradores de saída do vertex shader `o0..o15`.

A página de pitfalls da Mikage/3dbrew registra um caso esparso: `TOTAL=2`, `MASK=0x11` habilita `o0` e `o4`; a primeira linha semântica descreve `o0`, e a segunda descreve `o4`. Uma saída desabilitada não consome uma posição do mapa.

Fontes independentes:
- https://docs.mikage.app/GPU/Pitfalls/
- https://3dbrew.org/wiki/GPU/Internal_Registers

## Referência de implementação open source consultada

- Repositório: https://github.com/azahar-emu/azahar
- Revisão consultada: branch `master`, em 24/11/2026.
- Arquivos usados somente para confirmar os nomes/arranjo dos campos e o fluxo de saída: `src/video_core/pica/regs_shader.h`, `src/video_core/pica/regs_rasterizer.h` e `src/video_core/pica/output_vertex.cpp`.
- Os cabeçalhos desses arquivos indicam GPL-2.0-or-later. Nenhum trecho, teste ou estrutura desses arquivos foi copiado, traduzido ou adaptado para o Aceris.

## Implementação independente no Aceris

- `PicaVertexOutputMap::from_registers_masked` associa cada linha semântica ao próximo bit habilitado, em ordem crescente.
- Linhas após `SH_OUTMAP_TOTAL` são ignoradas, mesmo que seus registradores contenham dados antigos.
- Máscara com menos bits habilitados que o total solicitado e contagem acima das sete linhas disponíveis retornam erros explícitos.
- O caminho `Pica200::vertex_output_map` lê a máscara e a contagem atuais do convidado; o construtor denso anterior permanece disponível para testes isolados.

## Testes adicionados

- Máscara esparsa `0x0011`: a segunda linha semântica resolve para shader output `o4`, não `o1`.
- Linhas de mapa além da contagem informada não participam da resolução.
- Máscara insuficiente e contagem acima do limite são rejeitadas.
- Leitura integrada dos registradores vivos do PICA200.

**Limite:** isso corrige uma omissão comprovada no decodificador do Aceris, mas não prova que todos os jogos passarão a renderizar. O resultado ainda precisa ser conferido com a captura do jogo-alvo e com a CI multiplataforma.
