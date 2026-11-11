# Roadmap

Ordem sugerida; os marcos podem ser reordenados conforme o que for aprendido.

## 0. Fundação (concluído)

- [x] Workspace Rust com núcleo, frontend desktop e biblioteca Android
- [x] CI (fmt, clippy, testes em Linux/Windows/macOS e build Android)
- [x] Política de sala limpa, licença e ícone

## 1. Núcleo de CPU

- [x] Decodificação e execução do conjunto de instruções ARM (modo usuário)
- [x] Autoteste da CPU compartilhado por PC e Android
- [x] Instruções Thumb (formas clássicas, BL/BLX, REV/SXT/UXT; faltam BKPT, CPS e SETEND)
- [ ] Modos privilegiados e registradores com banco
- [ ] Instruções do ARMv6 (mídia, exclusivas, saturação, multiplicações DSP)
- [ ] Coprocessador e unidade de ponto flutuante vetorial
- [ ] Testes por instrução

## 2. Memória e carregamento

- [ ] Mapa de memória real e MMU
- [x] Parser estrutural de NCSD/CCI (.3ds/.cci), com validação das oito entradas e dos limites das partições
- [ ] Parser NCCH (cabeçalho, ExeFS e RomFS)
- [ ] Parser CIA e 3DSX
- [ ] Carregamento de executáveis homebrew como primeiro alvo

## 3. Sistema operacional emulado

- [ ] Chamadas de sistema e objetos do kernel
- [ ] Serviços básicos

## 4. Vídeo e áudio

- [ ] Janela e superfície de vídeo no desktop
- [ ] Renderizador da GPU
- [ ] Saída das duas telas
- [ ] Áudio

## 5. Entrada e interface

- [ ] Teclado, controle e toque (desktop e Android)
- [ ] Biblioteca de jogos e configurações

## 6. App Android

- [ ] Projeto Gradle/Kotlin com ponte JNI para aceris-android
- [ ] Gerenciamento de superfície, entrada e ciclo de vida
