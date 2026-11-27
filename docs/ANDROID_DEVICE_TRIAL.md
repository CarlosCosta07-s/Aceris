# Teste do Aceris em aparelho Android

> Guia de teste inicial. Estado do repositório consultado em 25/11/2026. O APK é de desenvolvimento, arm64-v8a, assinado com chave descartável gerada no CI. A compilação/assinatura não equivale a um teste em dispositivo físico.

## APK de teste

O workflow `.github/workflows/android-apk.yml` compila o frontend nativo Rust e empacota um APK debug ARM64. Ele roda em pushes à branch `main` e também pode ser iniciado manualmente.

Quando a execução terminar com sucesso:

1. Abra GitHub → Aceris → Actions.
2. Entre no workflow **Build Android APK** mais recente com resultado verde.
3. Baixe o artefato `aceris-android-arm64-debug`.
4. Extraia o ZIP e instale `aceris-debug.apk`.

O pacote utiliza o application ID provisório `org.aceris.emulator`, definido no manifesto atual. A assinatura é regenerada em cada execução do CI; para instalar uma nova compilação sobre a anterior será necessário desinstalar a versão antiga primeiro. Isso também remove os dados locais do app. Não use esta chave descartável para distribuição pública.

## Primeira abertura

1. Instale e abra Aceris.
2. Confirme que a introdução aparece e que o app chega à biblioteca.
3. Confirme que não fecha sozinho, que a tela se adapta à orientação/tamanho e que os toques são reconhecidos.
4. Abra e feche o app; teste pausa e retomada pelo alternador de aplicativos.
5. Saia de um jogo de teste pelo botão Voltar e registre a mensagem de diagnóstico HLE exibida na biblioteca.

## Onde colocar um arquivo de teste

O frontend Android atual procura os jogos na pasta `games` dentro do diretório de arquivos externos específico do app. Com o application ID atual, o caminho esperado é:

```
/storage/emulated/0/Android/data/org.aceris.emulator/files/games/
```

Crie a pasta `games` e coloque diretamente nela um arquivo de jogo/homebrew que você tenha direito de usar, com extensão `.3ds`, `.cci` ou `.3dsx`. A biblioteca Android atual só lista esses três formatos e não pesquisa subpastas. Embora o núcleo tenha testes headless com CIA NoCrypto, a biblioteca visual ainda não lista arquivos `.cia`; portanto, não use CIA para este primeiro teste da interface. O pacote não inclui jogos, firmware, chaves, ROMs ou dados extraídos.

O frontend atual ainda não oferece um seletor SAF de arquivos dentro do app. Se o gerenciador de arquivos do Android bloquear o acesso a `Android/data`, conecte o telefone a um computador com ADB e use:

```sh
adb shell mkdir -p /sdcard/Android/data/org.aceris.emulator/files/games
adb push "SEU_ARQUIVO.cci" /sdcard/Android/data/org.aceris.emulator/files/games/
```

Substitua `SEU_ARQUIVO.cci` pelo caminho de um CCI/3DS compatível no computador (ou use `.3dsx`). Não use `.cia` neste primeiro teste: o loader central aceita CIA NoCrypto, mas o scanner da biblioteca ainda não lista essa extensão. Não renomeie a extensão para contornar a lista. Não envie o arquivo de jogo ao GitHub ou ao workflow.

## Coleta de diagnóstico

Com ADB habilitado:

```sh
adb logcat -c
adb logcat
```

Inicie Aceris, reproduza o problema e capture as linhas do momento da falha. Registre também:

- modelo do telefone e SoC;
- versão Android;
- GPU e versão do driver, se disponíveis;
- RAM total e espaço livre;
- se a janela abriu, se a biblioteca apareceu e se o arquivo foi listado;
- se o backend Vulkan/OpenGL ES iniciou ou se houve fallback para CPU;
- tempo por quadro/FPS observado, aquecimento, travamentos e mensagem de erro.

## Limites conhecidos

- A CI compila a biblioteca ARM64; não prova que o app instala/abre no seu aparelho.
- O presenter Vulkan/OpenGL ES atual recebe os frames LCD já compostos pelo núcleo; ele não substitui o rasterizador PICA200 em CPU.
- Um perfil de desktop anterior registrou aproximadamente 3,5 FPS com o workload Cookie Clicker. O desempenho Android ainda é desconhecido e precisa ser medido.
- Áudio real, compatibilidade de jogos e funcionamento de todos os gestos/teclas continuam sem confirmação em aparelho.
- O APK gerado é um build experimental de desenvolvimento, não um lançamento.

## Critérios de aceite do primeiro teste físico

- APK instala e abre sem crash.
- Introdução e biblioteca aparecem.
- Retorno/pausa/retomada não deixam uma superfície ou thread nativa pendurada.
- Biblioteca detecta um arquivo compatível presente na pasta `games`.
- O primeiro diagnóstico do loader/HLE fica legível quando a execução falha.
- São guardados logs e dados do aparelho para reproduzir o resultado.

## Medição de FPS e frametime no Android (opt-in)

O render thread pode emitir um resumo de desempenho a cada cinco segundos. A coleta fica desativada por padrão. Para habilitá-la, crie o arquivo de configuração abaixo com o conteúdo `1`:

```sh
adb shell mkdir -p /sdcard/Android/data/org.aceris.emulator/files/games/.aceris
adb shell "echo 1 > /sdcard/Android/data/org.aceris.emulator/files/games/.aceris/android_performance_log_v1.txt"
```

Reinicie o Aceris e filtre o Logcat:

```sh
adb logcat -c
adb logcat | grep "Aceris Android performance"
```

No PowerShell, substitua `grep` por `findstr`.

Para desativar, remova o arquivo e reinicie o Aceris:

```sh
adb shell rm /sdcard/Android/data/org.aceris.emulator/files/games/.aceris/android_performance_log_v1.txt
```

Cada relatório cobre cinco segundos e usa até 300 amostras recentes. Os campos são:

- `presented_fps`: apresentações concluídas por segundo, não FPS guest nem velocidade de emulação.
- `render_ms_avg/p50/p95`: duração da função de renderização no host, incluindo bloqueio da janela, `App::step_into` e apresentação.
- `app_step_ms_p95`: tempo de `App::step_into`; inclui execução do jogo e composição do frame, não é tempo isolado da CPU guest.
- `present_ms_p95`: tempo gasto pelo presenter Vulkan/OpenGL ES ou pelo fallback `ANativeWindow`.
- `present_interval_ms_p50/p95`: intervalo entre apresentações concluídas; lacunas acima de dois segundos são descartadas para não confundir pausa/retomada com o ritmo normal.
- `gpu_present`: porcentagem das apresentações concluídas pelo presenter `wgpu`; o restante foi apresentado pelo fallback CPU.
- `frames_not_presented`: tentativas de renderização sem apresentação concluída.

A primeira janela pode incluir a inicialização fria do presenter e do driver. Compare relatórios subsequentes e anote aparelho, SoC, GPU/driver, perfil e duração do ensaio. Essa telemetria mede o caminho host; ela não altera clock, timers, orçamento de instruções, resolução guest ou temporização do 3DS.