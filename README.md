# PS5 Camera Windows Driver

Driver Windows em Rust para a **Sony PlayStation 5 HD Camera CFI-ZEY1**. A instalação entrega uma webcam UVC para Camera, OBS e aplicativos que usam o driver nativo do Windows.

## Instalar

1. Baixe `PS5-Camera-Setup.exe` na [última release](https://github.com/valtoni/PS5-Camera/releases/latest).
2. Execute o arquivo e siga o assistente de uma janela.
3. Autorize o UAC quando o Windows solicitar.
4. Conecte ou reconecte a câmera.

O instalador detecta o estado atual e oferece apenas as ações adequadas: instalar, reparar/reinstalar ou remover. Ele instala WinUSB exclusivamente para o bootloader `USB\\VID_05A9&PID_0580`, o serviço de upload e o diagnóstico. O dispositivo UVC final (`USB\\VID_05A9&PID_058C`) continua no driver de câmera já incluído no Windows.

## Como funciona

```text
PS5 HD Camera em boot (05A9:0580)
              │
              ├─ WinUSB + serviço PS5 Camera
              │          │
              │          └─ carrega o firmware V1 na RAM
              │
              └─ USB Camera-OV580 (05A9:058C)
                           │
                           └─ UVC nativo do Windows
```

O firmware não é gravado na câmera. Após desligar ou remover o cabo, ela retorna ao modo boot; o serviço faz o upload novamente na próxima conexão.

## Estado da v1.0.0

- upload automático do firmware na conexão e reconexão;
- vídeo UVC validado em `1920×1080 @30` e estéreo `3840×1080 @30`;
- instalador único, com interface nativa, progresso e desinstalação;
- driver WinUSB restrito ao modo boot — não substitui o driver UVC do Windows;
- firmware de referência fixado por SHA-256 e acompanhado da licença MIT declarada pelo publicador.

O firmware V1 distribuído é `21.01-03.20.00.04-00.00.00.bin`, SHA-256 `10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54`, proveniente do commit `8773610978d5a4d91a6a6d8063d48a4f3afcfe5b` de [prosperodev/hdcamera](https://github.com/prosperodev/hdcamera). A V1 usa essa referência MIT; firmware independente é trabalho futuro.

## Limites de distribuição

O pacote atual usa assinatura de desenvolvimento para o catálogo do WinUSB. Por isso, a instalação pede autorização administrativa explícita para confiar no certificado do projeto. Não é uma assinatura de distribuição Microsoft nem um pacote do Windows Update.

## Desenvolvimento e validação

```powershell
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
.\windows\package\test-package.ps1
.\windows\installer\test-installer.ps1
```

O workflow de validação roda esses testes em Windows. Releases são montadas e publicadas automaticamente para tags `v*` por um runner Windows/WDK dedicado (`self-hosted`, `Windows`, `X64`, `ps5cam-signing`), que mantém a chave de assinatura fora do GitHub.

## Projeto de origem

https://github.com/raleighlittles/PS5-Camera-Firmware-Loader

## Apoie o projeto

<a href="bitcoin:bc1qw22nzhyrrk3eq45n4c06tje2q37a8fjtslrwrm"><img src="assets/bitcoin-donation-qr.svg" width="180" alt="QR code para doação em Bitcoin" /></a>

Bitcoin: [`bc1qw22nzhyrrk3eq45n4c06tje2q37a8fjtslrwrm`](bitcoin:bc1qw22nzhyrrk3eq45n4c06tje2q37a8fjtslrwrm)
