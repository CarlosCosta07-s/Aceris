# Roadmap

Ordem sugerida; os marcos podem ser reordenados conforme o que for aprendido.

## 0. Fundação (concluído)

- [x] Workspace Rust com núcleo, frontend desktop e biblioteca Android
- [x] CI (fmt, clippy, testes em Linux/Windows/macOS e build Android)
- [x] Política de sala limpa, licença e ícone

## 1. Núcleo de CPU

- [ ] Decodificação e execução do conjunto de instruções ARM
- [ ] Instruções Thumb
- [ ] Coprocessador e unidade de ponto flutuante vetorial
- [ ] Testes por instrução

## 2. Memória e carregamento

- [ ] Mapa de memória real e MMU
- [ ] Parser dos formatos de imagem identificados em `loader`
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

- [ ] Projeto Gradle/Kotlin com ponte JNI para `aceris-android`
- [ ] Gerenciamento de superfície, entrada e ciclo de vida
