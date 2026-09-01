# Dossiê de pesquisa, idealização, execução e evidências

## PS5 Camera — driver e instalador para Windows

> Registro técnico consolidado a partir dos artefatos de `target/`  
> Elaborado em 1º de setembro de 2026  
> Estado do código documentado: commit `7da1ae5b38d287044842d5382ce61c600e816e2b`  
> Versão do workspace: `1.0.1`

## 1. Finalidade deste documento

Este arquivo preserva, em forma pequena e auditável, o conhecimento que estava espalhado pelos 54.554 arquivos e 7.575.172.699 bytes (7,055 GiB) de `target/`. O diretório continha compilações reproduzíveis, cópias de ferramentas e fontes de terceiros, tentativas intermediárias, capturas USB, sondagens do dispositivo, quadros de vídeo, análises de firmware e pacotes de distribuição.

A consolidação privilegia os elementos que explicam e comprovam o trabalho: sequência experimental, medições, hashes, decisões de arquitetura, resultados de testes e limites conhecidos. Código, testes, automação, firmware de referência autorizado e sua proveniência permanecem versionados no repositório.

Este dossiê diferencia deliberadamente:

- **observado**: medido em hardware, arquivo ou comando;
- **comprovado por teste**: comportamento coberto por teste automatizado;
- **idealizado/implementado**: propriedade demonstrável no código, mas que pode depender do ambiente final;
- **não comprovado**: hipótese, limitação ou trabalho futuro.

Nenhuma imagem foi incorporada. Havia centenas de quadros, mas métricas, dimensões e SHA-256 preservam a cadeia probatória com poucos quilobytes. Os hashes das imagens principais são mantidos para conferência contra alguma cópia externa futura.

## 2. Resultado alcançado

O projeto transforma a Sony PlayStation 5 HD Camera CFI-ZEY1 em uma câmera UVC utilizável pelo Windows:

```text
Câmera sem firmware                Câmera operacional
USB 05A9:0580                     USB 05A9:058C
“OmniVision / USB Boot”           “USB Camera-OV580”
        │                                  ▲
        ├─ WinUSB apenas no PID 0580       │
        ├─ serviço valida o firmware       │
        ├─ upload para RAM: 135 blocos     │
        └─ comando de execução ────────────┘
                                           │
                                  driver UVC nativo do Windows
```

O firmware não é gravado permanentemente. Após desconectar a alimentação, o hardware retorna ao PID `0580`; o serviço refaz o upload quando necessário. O WinUSB é associado somente ao modo de boot. O PID UVC `058C` permanece protegido e usa a pilha de câmera nativa do Windows.

## 3. Linha do tempo probatória e sequencial

### 3.1 Origem e evolução anterior

1. **2021 — primeiro loader funcional.** O histórico começa no commit `7244d44` (“Simple firwmare loader works!”), seguido de parametrização do caminho do firmware e correções do uso de libusb.
2. **2022 — implementação Rust.** A sequência `ec0c826` a `db8d58f` registra conexão, claim USB, iteração do firmware, envio USB, correção de warnings e documentação.
3. **2023 — Rust validado e CI inicial.** O commit `dbe98d9` registra a versão Rust funcional; `983473f` inicia integração contínua.
4. **2024–2025 — experimentos WebUSB e consolidação.** Houve protótipo WebUSB; depois o projeto adotou `rusb` e removeu a implementação C++ para manter apenas Rust.
5. **2026 — robustez e produto Windows.** O tratamento de desconexão foi corrigido; entre 29 e 30 de agosto foram publicados o driver Windows V1.0.0, instalador localizado, testes Pester 5, assinatura do catálogo e automação da V1.0.1.

Essa história explica a decisão central da fase atual: transformar um loader manual em uma pilha Windows instalável, segura, observável e reversível.

### 3.2 Reconhecimento inicial do hardware

6. **Ausência inicial.** `probe-connected.json` registrou `status: absent`, mostrando que a ferramenta não inventa dispositivo quando nada compatível está acessível.
7. **Dispositivo presente, sem driver apropriado.** `probe-connected-pnp.json` encontrou `USB\VID_05A9&PID_0580`, mas apenas via PnP, com `problem_code: 28`. O Windows reconhecia fisicamente o dispositivo, porém não possuía binding funcional para acesso libusb.
8. **Hipótese de solução.** Como o modo de boot expõe interface vendor-specific (`class 0xFF`) e endpoints bulk, foi idealizado um INF declarativo que associa WinUSB exclusivamente a `USB\VID_05A9&PID_0580`.
9. **Binding confirmado.** Após WinUSB, `probe-winusb.json` registrou `problem_code: 0`, barramento 1, porta 3, USB 2.0/high-speed, endpoint OUT `0x01` e IN `0x82`, ambos bulk de 512 bytes. Isso comprovou acesso programático ao bootloader.
10. **Condição operacional corrigida.** Em teste posterior na porta 13, `post-reboot-probe/report.json` comprovou SuperSpeed/USB 3.0, endpoints bulk de 1.024 bytes, fabricante “OmniVision Technologies, Inc.”, produto “USB Boot ”, `problem_code: 0` e descritores crus sem issues. O dump de 114 bytes tinha SHA-256 `7a7b5ee59ca07cc7483183f34f5e08a32d494250443735af6e2e87fa4eca2e80`.

Conclusão da etapa: o PID `0580` é o bootloader OV580 e requer WinUSB para o loader; USB 3/SuperSpeed é tratado pelo produto como requisito para a operação pretendida.

### 3.3 Captura USB e determinismo

11. **Captura de enumeração.** Uma captura USBPcap/TShark isolou os frames 13 a 28 da enumeração do alvo `05A9:0580`: 16 linhas, duas linhas de identidade e endereços observados 1 e 64.
12. **Minimização.** A captura bruta, de 26.904 bytes e SHA-256 `042cbd41a45815e3c9657b162dd2526a4e367c5ddd4725ca70c19526dfc0b93e`, foi classificada como privada. Foram derivados somente CSV, transcrição, resumo e filtro.
13. **Reprodutibilidade.** Duas publicações independentes (`capture-publish-determinism-a` e `-b`) produziram hashes idênticos:

| Derivado | Bytes | SHA-256 |
|---|---:|---|
| CSV USB | 1.481 | `157a5fb4ecdd9107ec3812ec406b6bca12929db1363d1166374c4e9bb93ee8ff` |
| Transcrição JSON | 7.731 | `75124147c3a154c3def329e200b93a01e5445ad856ede22b4052b49451833e96` |
| Resumo JSON | 1.065 | `4921ad6f36d9bf2dfcffb6a0fa1a26e9a4c1128e9b1369a561d2698dfad3954d` |
| Filtro | 44 | `1ecc9353f88c91675fdbb0ce40ec3aa46b55274f5f83a93b35637969b7dccc3f` |

Conclusão da etapa: o pipeline de redução de captura era determinístico e mantinha a identidade do alvo sem exigir a publicação da captura bruta.

### 3.4 Firmware, autorização e análise

14. **Firmware V1 escolhido.** O arquivo `21.01-03.20.00.04-00.00.00.bin` possui 68.948 bytes e SHA-256 `10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54`.
15. **Proveniência fixada.** A origem registrada é `prosperodev/hdcamera`, commit `8773610978d5a4d91a6a6d8063d48a4f3afcfe5b`, caminho `firmware/21.01-03.20.00.04-00.00.00.bin`, sob a licença MIT declarada pelo publicador.
16. **Limite jurídico/técnico explicitado.** A V1 não afirma autoria Sony, origem clean-room do firmware ou verificação independente da cadeia de titularidade. Ela redistribui uma referência de terceiro com base na licença MIT preservada. A pilha Windows desenvolvida neste projeto é independente; o firmware V1 não é.
17. **Comparação de variante.** Foi pesquisado um segundo binário chamado `firmware_discord_and_gamma_fix.bin`, também de 68.948 bytes, SHA-256 `42ce5f7033a1115988cada3ebb8e0a143e8b241661f203a12275354dd2ba0bf6`. Apenas oito bytes diferiam, em quatro intervalos nos offsets 61.110–61.112, 61.115–61.117, 66.563 e 66.567. A análise marcou `execution: none`: essa variante **não foi executada** e não integra o produto.
18. **Ferramenta de análise.** Foi criado `ov580-fw-analyzer`, capaz de produzir relatório estruturado, extrair candidatos a strings/descritores e comparar blocos sem incorporar os bytes do firmware.
19. **Pesquisa de arquitetura.** Para investigação privada, GNU binutils 2.36.1 foi obtido de `ftp.gnu.org`, SHA-256 `e81d9edf373f193af428a0f256674aea62a9d74dfe93f65192d4eae030b0f3b0`, compilado para `nds32be-elf` e usado como `objdump -D -b binary -m nds32 -EB --adjust-vma=0`.
20. **Resultado da desmontagem.** A desmontagem de 838.400 bytes tinha SHA-256 `512b265607b2f8b020a49c89f6827e01ee84a4f3c3f6c59b87c5c5f920bc8ba8`; a análise contou 19.436 instruções, 244 branches, 39 internos e 205 externos. Esse material era somente pesquisa privada e não foi incluído no produto.

Conclusão da etapa: para a V1, a opção responsável foi fixar e verificar um firmware de referência já funcional, registrar sua licença/proveniência e deixar firmware totalmente independente como trabalho futuro.

### 3.5 Loader seguro e upload real

21. **Protocolo implementado.** O firmware é dividido em blocos de 512 bytes. O endereço é derivado de offset/banco, cada transferência verifica escrita completa e há cancelamento e timeout limitados.
22. **Guardas antes da mutação.** O loader exige exatamente um dispositivo boot acessível e SuperSpeed, confirmação literal do hardware ID, SHA-256 esperado, proveniência, referência de autorização e aceite explícito. Mudança de identidade/localização entre preflight e envio faz o processo falhar fechado.
23. **Guarda final.** Após o último bloco e antes do comando de execução, cancelamento e deadline são verificados novamente, fechando a janela TOCTOU mais sensível.
24. **Primeira execução real autorizada.** O resumo `private-upload-20260826/execution-summary.json` registrou:

| Medida | Resultado observado |
|---|---|
| Status | `ok` |
| Firmware | hash V1 esperado |
| Bytes enviados | 68.948 |
| Blocos enviados | 135 |
| Alvo | barramento 1, porta física 13, SuperSpeed |
| Comando execute | aceito |
| Reenumeração | `camera_ready` |
| Tempo | 554 ms |

25. **Transformação USB comprovada.** Depois do upload, a câmera apareceu como `05A9:058C`, `problem_code: 0`, SuperSpeed, configuração UVC de 619 bytes e endpoint isócrono `0x81`.
26. **Volatilidade comprovada.** Após power cycle, o mesmo caminho físico voltou ao modo boot `05A9:0580`, SuperSpeed e sem problema PnP. Isso confirma que o upload é para RAM e precisa ser repetido após perda de energia.
27. **Limitação da primeira captura.** USBPcap/TShark falhou antes de gerar pcapng válido porque a sessão inicial não estava elevada. O firmware foi enviado e a câmera reenumerou, mas essa primeira tentativa não capturou os pacotes.
28. **Segunda captura.** Com preflight elevado, foi gerado pcapng de 892 bytes, SHA-256 `324628fda6503559f6a1cfc5f1eb7aaa7b9a08bf37d9c891d50ed126cb47491e`, e CSV de 835 bytes, SHA-256 `694e696af542c5965bafec61b73fbb643a4ecfb42b6d0d538636590dd509ebb3`.
29. **Limitação da segunda captura.** A captura registrou oito linhas de descritores/controle, mas não os payloads bulk do upload. Portanto, o sucesso do envio é sustentado pelo contador do loader e pela reenumeração funcional, não por uma transcrição completa dos 68.948 bytes no barramento.

### 3.6 Validação UVC e vídeo

30. **Descritor UVC.** A sonda no PID `058C` registrou USB 3.2, classe `0xEF`, duas interfaces, configuração de 619 bytes e interface VideoStreaming com endpoint isócrono IN `0x81`, 1.024 bytes, intervalo 1.
31. **Primeiro quadro 1080p.** Um quadro PNG RGB24, originado de YUYV422 em `1920×1080 @30`, foi capturado com 128.318 bytes e SHA-256 `9ceef4f332a9c00bf7f4dea95918e199292159215946815aad16de585c35963b`. Embora escuro, tinha valores 0–255 e 2.639 cores RGB distintas: não era um quadro vazio.
32. **Exposição.** Uma sequência de dez quadros tinha dados não zero em todos eles e médias de canal amostradas entre 60,7701 e 63,6914. Isso evidenciou convergência de exposição. A execução curta relatou buffer DirectShow quase cheio e frames descartados.
33. **Buffer corrigido.** Com buffer DirectShow de 128 MiB, 30/30 quadros foram escritos sem alerta de descarte; fração não zero mínima de 0,9900 e máxima de 0,9987.
34. **Estabilidade 1080p.** Uma captura nominal de dez segundos escreveu 300/300 quadros em `YUYV422 1920×1080 @30`, sem alerta de descarte. A média amostrada variou de 33,6055 a 37,2444 e a fração mínima de pixels não zero foi 0,7457.
35. **Estéreo aberto.** Um quadro `3840×1080 @30` abriu as duas metades, mas a iluminação inicial deixou a direita quase preta.
36. **Estéreo iluminado.** Com contraluz, as duas metades ficaram visíveis e comparáveis. O PNG de 2.286.831 bytes tem SHA-256 `63a231283321e276c17feaaa74a18565f1ec25f95ecfd3c708405dd713ebc6ae`; médias amostradas 30,2219 (esquerda) e 35,3908 (direita), com 1.204 e 1.297 valores distintos.
37. **Sequência estéreo.** Foram escritos 60/60 quadros `YUYV422 3840×1080 @30`. A baixa correlação espacial é coerente com pontos de vista distintos. **Sincronismo temporal entre os dois sensores não foi provado.**
38. **Benchmark bruto.** `mono-3s.yuyv` preservava 377.395.200 bytes, SHA-256 `bbb78a08411074275d9bc2994e92944669babee4fc7851196192fc228d38be12`. Uma captura visual adicional `current-1920x1080.png` tinha 1.001.395 bytes, SHA-256 `89422497f7b48e57cc829546901c2c90e4fde0626cea55705c4a15d00fdbc6eb`.
39. **Controles UVC pesquisados.** Uma captura de 15 segundos na interface `USBPcap1` gerou seis linhas; pcapng de 1.324 bytes, SHA-256 `246fa5b2317c5c5dd0dfd1197dffa4433d626ab1d9db6c285d7585701791865d`, e CSV de 686 bytes, SHA-256 `830a05a313c0cc86f4817b5100dd7d2f345fd0fb9e42975fcfd2fe0d37cf43b9`. As notas não chegaram a classificar fatos ou inferências por frame; portanto, nenhum suporte adicional a controles é alegado.

Conclusão da etapa: funcionamento mono 1080p e abertura do modo estéreo 3840×1080 a 30 fps foram observados em hardware. A pilha atual não deve alegar sincronismo estéreo comprovado nem validação completa de todos os modos anunciados no descritor.

### 3.7 Serviço automático

40. **Objetivo.** Eliminar a necessidade de executar manualmente o loader após cada conexão ou power cycle.
41. **Implementação.** `PS5CameraService` integra o Service Control Manager, notificações de chegada/remoção USB e uma máquina de estados determinística.
42. **Correlação física.** A câmera `058C` só é aceita como resultado do upload quando reaparece no mesmo controlador e caminho de porta do boot `0580`, evitando atribuir a transição a outro dispositivo.
43. **Resiliência.** Desconexões durante upload entram em retry/backoff limitado; duplicatas não criam loop; remoção habilita novo ciclo; stop/shutdown cancelam trabalho em andamento.
44. **Integridade.** O firmware incorporado ao serviço é limitado, validado estruturalmente e comparado ao hash V1 fixado antes do upload.
45. **Auditoria.** Eventos são estruturados e serializáveis sem vazar os bytes do firmware. Há self-test independente do SCM e limites para payload do Windows Event Log.

### 3.8 Driver, empacotamento e instalador

46. **INF mínimo.** `windows/package/ps5cam-boot.inf` aceita amd64 e arm64, inclui `winusb.inf`, usa `WINUSB.NT`/`.Services` e declara a interface `{ABB9454F-E674-4620-8C6E-49A5777EB078}`.
47. **Proteção do UVC.** O único hardware ID instalável no INF e no manifesto é `USB\VID_05A9&PID_0580`. Os testes rejeitam inclusão do PID `058C`, inclusive quando escondido em seções adicionais.
48. **Pipeline.** A automação planeja/realiza Inf2Cat, assinatura SHA-256, instalação via PnPUtil e rollback. A montagem exige INF, catálogo, firmware autorizado, serviço, diagnóstico, instalador, engine e licença, todos verificados por hash.
49. **Iteração.** `target/` guardava pelo menos 15 diretórios sucessivos `ps5cam-v1-development*`, além de V1.0.0, catálogos WDK e três gerações do executável de setup. Essa sequência documenta a evolução incremental até a composição final da release.
50. **Release V1.0.1.** O manifesto local de release registrou a revisão-fonte `fb41770caf50d62519e59cc5e1bf87d3d7e61783`, o único hardware ID boot e os componentes finais. Principais hashes:

| Componente | Bytes | SHA-256 |
|---|---:|---|
| `ps5cam-boot.inf` | 1.061 | `a68c852bce4412e4815649381711124023b36ae0e38dea48c720666c7f3c0ac2` |
| `ps5cam-boot.cat` | 2.436 | `9cdf9edd11cab430118580538b0db5d5bfba04e292a4cdabd38372694a077efb` |
| firmware V1 | 68.948 | `10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54` |
| `ps5cam-service.exe` | 565.760 | `ae8ab5622e7dd2d6bee8a95516b208664f1a5b0f609ce7aed1e018367c3236d4` |
| `ps5cam-diagnostics.exe` | 1.084.928 | `3651bbe4fa41d72907c4e3c77a9f602a9400b44b6a408f047fc8a2f32d0971d8` |
| manifesto de release | 2.553 | `26eddb05f344ef3747f701a0207c17738e2f17c6f0588089e9dd051c3a7b077d` |
| assinatura destacada do manifesto | 1.608 | `5637201afc8ad97827ad7cf6ae84c3153159cd5ae2c151bcc493fe26e35cb364` |
| setup único | 2.178.560 | `53a3ffa3a799e110f8c7ec8f34ac3155c7613706bd831f2f3277d173b4fe6099` |

51. **Assinatura verificada.** `Get-AuthenticodeSignature` retornou `Valid` para `ps5cam-boot.cat`, assinado por `CN=PS5 Camera Development Signing`, thumbprint `EDAF55A1E4AE0C8C197988F7286626BD51228CA2`.
52. **Limite de distribuição.** Essa é assinatura de desenvolvimento, não Microsoft/WHQL/Windows Update. O certificado precisa de confiança explícita do administrador. O `PS5-Camera-Setup.exe` local retornou `NotSigned`; a validade do catálogo não deve ser confundida com assinatura Authenticode do invólucro do setup.
53. **Instalador único.** A UI nativa detecta estado e oferece instalação, reparo/reinstalação ou remoção. O engine usa staging, locks, hashes, estado autenticado, rollback e só remove driver/arquivos que reconhece como próprios.
54. **Localização.** Idiomas implementados: inglês/fallback, português, francês CA/FR, espanhol, alemão, japonês e chinês simplificado.
55. **Automação de release.** O workflow usa runner efêmero `windows-2022`, segredos para PFX/senha/token e remove o certificado do ambiente ao final. O histórico registra correções sucessivas até publicação por GitHub CLI no commit documentado.

## 4. Arquitetura final e rastreabilidade no repositório

| Responsabilidade | Fonte preservada | Evidência/testes preservados |
|---|---|---|
| Protocolo de blocos e execução OV580 | [`crates/ov580-loader`](crates/ov580-loader) | `src/tests.rs` |
| CLI auditável de upload | [`crates/ps5cam-loader-cli`](crates/ps5cam-loader-cli) | testes no próprio `main.rs` |
| Descoberta USB/PnP e descritores | [`crates/ps5cam-usb`](crates/ps5cam-usb) | fixtures e testes de merge/parser |
| Sonda de hardware | [`crates/ps5cam-probe`](crates/ps5cam-probe) | parser de CLI |
| Diagnóstico de ambiente | [`crates/ps5cam-diagnostics`](crates/ps5cam-diagnostics) | gates USB, WDK e USBPcap |
| Serviço automático | [`crates/ps5cam-service`](crates/ps5cam-service) | 51 testes de estados, SCM e falhas |
| Captura UVC/Media Foundation | [`crates/ps5cam-uvc`](crates/ps5cam-uvc) | estatísticas, estéreo e gaps |
| Análise de firmware | [`crates/ov580-fw-analyzer`](crates/ov580-fw-analyzer) | fixtures sintéticas e CLI |
| Setup nativo | [`crates/ps5cam-setup`](crates/ps5cam-setup) | localização, estado e assets |
| INF e pacote | [`windows/package`](windows/package) | testes de guards e assembler |
| Engine do instalador | [`windows/installer`](windows/installer) | 24 testes Pester |
| Firmware/licença/proveniência | [`firmware/reference`](firmware/reference) | hash e origem fixados |
| CI e publicação | [`.github/workflows`](.github/workflows) | verificação Windows e release por tag |

## 5. Verificação atual, refeita para este dossiê

Em 1º de setembro de 2026, no commit `7da1ae5b38d287044842d5382ce61c600e816e2b`, foram executados novamente:

```powershell
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
.\windows\package\test-package.ps1
.\windows\installer\test-installer.ps1
```

Resultados:

| Verificação | Resultado |
|---|---|
| Rustfmt | passou, sem alteração |
| Testes Rust | **133 passaram; 0 falharam** |
| Clippy com warnings como erro | passou |
| Guards do pacote | **4 cenários passaram** (`status: ok`) |
| Instalador/Pester | **24 passaram; 0 falharam** |

O primeiro lançamento do Pester dentro do sandbox falhou antes de executar testes porque o ambiente bloqueou a criação da chave temporária `HKCU\Software\Pester`. A repetição com acesso normal ao registro executou os 24 testes com sucesso. Logo, esse primeiro erro foi de infraestrutura de teste, não uma regressão do instalador.

Total diretamente revalidado neste fechamento: **161 testes/cenários aprovados** (133 Rust + 4 de pacote + 24 do instalador), além de formatação e lint sem falhas.

## 6. Alcance e limites dos resultados

Os resultados obtidos são suficientes para demonstrar o objetivo principal do projeto: identificar a câmera em boot, carregar o firmware de forma controlada, acompanhar sua reenumeração e obter vídeo por meio da pilha UVC nativa do Windows. Ainda assim, algumas fronteiras precisam permanecer claras para que o trabalho seja compreendido pelo que realmente demonstrou.

O firmware utilizado na V1 veio do projeto `prosperodev/hdcamera` e foi preservado conforme a licença MIT declarada na origem. A implementação Windows — loader, serviço, diagnóstico, empacotamento e instalador — foi desenvolvida neste projeto, mas o firmware não deve ser apresentado como resultado de um processo clean-room. A variante que diferia em oito bytes foi apenas comparada; ela nunca foi executada nem incorporada ao produto.

Também existe uma diferença importante entre um pacote funcional para desenvolvimento e um driver pronto para distribuição ampla. O catálogo foi validamente assinado com o certificado `PS5 Camera Development Signing`, enquanto o executável local do setup não possuía assinatura Authenticode. Não houve assinatura Microsoft, certificação WHQL ou publicação pelo Windows Update.

As capturas USB confirmaram enumeração e tráfego de controle, mas não registraram integralmente os payloads bulk do firmware. A comprovação do upload resulta da contagem auditada de 68.948 bytes, da aceitação do comando de execução e, sobretudo, da reenumeração funcional do mesmo dispositivo físico como `05A9:058C`.

No vídeo, os ensaios comprovaram operação em `1920×1080 @30` e abertura do modo `3840×1080 @30` em YUYV422, com conteúdo real nas duas metades da imagem estéreo. Eles não mediram sincronismo temporal entre os sensores e não cobriram exaustivamente todos os formatos anunciados no descritor UVC. Essas questões permanecem como continuidade natural da pesquisa, não como falhas do resultado já alcançado.

## 7. Reprodutibilidade técnica

O estado documentado pode ser reconstruído e verificado a partir das fontes versionadas. Esta seção registra o ambiente, os programas instalados e a sequência necessária para repetir as sondagens, o upload, as capturas de vídeo, a análise de firmware e o empacotamento.

### 7.1 Ambiente de referência

O host registrado nas sondagens era Windows x64, build `10.0.26200`. A verificação final foi executada com:

| Componente | Versão observada | Função |
|---|---|---|
| Rust | `rustc 1.97.0`, host `x86_64-pc-windows-msvc`, LLVM 22.1.6 | compilar e testar o workspace |
| Cargo | `1.97.0` | dependências e builds reproduzíveis com `Cargo.lock` |
| Rust mínimo declarado | `1.85` | piso de compatibilidade do projeto |
| Visual Studio Community 2022 | `17.12.4`, workload C++/MSVC | linker e toolchain nativa do Rust MSVC |
| PowerShell | `7.6.5` x64 | testes, pacote, assinatura e instalação |
| Pester | `5.7.1` | 24 testes do instalador; a suíte exige Pester 5.x |
| libusb via `rusb` | libusb `1.0.27.11882`; crate `rusb = 0.9.4` | descoberta, descritores e transferências do bootloader |
| Windows SDK | `10.1.26100.2454`, tools em `10.0.26100.0` | SignTool e ferramentas do SDK |
| Windows Driver Kit | `10.1.26100.6584` | Inf2Cat, InfVerif e catálogo do driver |
| Wireshark/TShark/Dumpcap | `4.6.7` x64 | inspeção e exportação das capturas USB |
| USBPcap | `1.5.4.0` | driver e captura do root hub USB |
| FFmpeg | `8.1.1-full_build-www.gyan.dev`, GCC 15.2.0 | captura DirectShow, PNG e YUYV bruto |
| Zadig | `2.9` portátil | binding WinUSB temporário usado na investigação inicial |
| GNU binutils | `2.36.1`, alvo `nds32be-elf` | desmontagem privada do firmware |
| 7-Zip | `26.02` | abertura do arquivo de ferramenta OV580; não participa do produto |
| GitHub CLI | `2.97.0` | configuração de Secrets e publicação; não participa das capturas |

As versões exatas registram o ambiente que funcionou; não significam que todas sejam requisitos rígidos. Para reproduzir hashes de binários e comportamento de ferramentas, prefira as mesmas versões. Para desenvolver e testar o código, versões compatíveis podem funcionar, respeitando Rust 1.85+, PowerShell 7+ e Pester 5.x.

### 7.2 Instalações necessárias

Em uma estação Windows limpa, preparar:

1. Visual Studio 2022 ou Build Tools com “Desktop development with C++”, MSVC x64/x86 e um Windows 11 SDK compatível.
2. Rust pelo `rustup`, usando o target `x86_64-pc-windows-msvc` e os componentes `rustfmt` e `clippy`. O `Cargo.lock` deve ser respeitado com `--locked`.
3. PowerShell 7 x64. Instalar Pester 5.7.1 no escopo do usuário:

   ```powershell
   Install-Module Pester -RequiredVersion 5.7.1 -Scope CurrentUser
   ```

4. Windows SDK e WDK. Confirmar que `signtool.exe`, `infverif.exe` e `inf2cat.exe` são encontrados. No host de referência, SignTool estava em `Windows Kits\10\bin\10.0.26100.0\x64` e Inf2Cat em `...\x86`.
5. Wireshark 4.6.7 com TShark, Dumpcap e o componente USBPcap 1.5.4.0. Confirmar o serviço `USBPcap` e executar as capturas em terminal elevado.
6. FFmpeg com entrada DirectShow e saída PNG/rawvideo; o build 8.1.1 usado tinha `dshow` habilitado.
7. Um driver WinUSB somente para `USB\VID_05A9&PID_0580`. Pode ser o INF produzido pelo projeto ou, durante a investigação inicial, Zadig 2.9. **Nunca substituir o driver do PID `058C`**, pois esse é o dispositivo UVC final.

O Windows já fornece PnPUtil, CertUtil, a pilha UVC e Media Foundation. Não é necessário instalar driver de vídeo próprio para o PID `058C`.

Ferramentas opcionais de contexto histórico:

- `Camera_tool_OV580.7z`: 346.433 bytes, SHA-256 `c87b1905a3b86b131de916f3afaf9682a674f98b917dea7b44ec5d42c24cff87`. Continha um aplicativo .NET Framework 4.5 e DLLs de câmera; sua proveniência não ficou suficientemente documentada e ele não foi necessário para comprovar o produto.
- `danyu9394/linux_camera_tool`, snapshot que se identifica em `includes/gitversion.h` como `v0.4.9 - 2020-01-30`, GPLv3: usado como referência sobre câmeras Leopard/OV580, V4L2, controles, exposição e separação estéreo. O ZIP pesquisado tinha 253.076.913 bytes e SHA-256 `ecdf4a1093ed54b50c8de1a0034715615a8dcfd9d4014ab790638a0644f82063`. É uma ferramenta Linux e não integra o fluxo Windows.

### 7.3 Compilação e verificação do projeto

Na raiz do repositório:

```powershell
# obter exatamente as versões de crates fixadas no Cargo.lock
cargo fetch --locked

# validação do código Rust
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

# validação do pacote Windows e do instalador
.\windows\package\test-package.ps1
.\windows\installer\test-installer.ps1

# compilação nos perfis de desenvolvimento e release
cargo build --workspace --locked
cargo build --workspace --release --locked
```

Os executáveis relevantes serão gerados em `target/debug` ou `target/release`: `ps5cam-probe`, `ps5cam-loader`, `ps5cam-diagnostics`, `ps5cam-service`, `ps5cam-uvc-capture`, `ov580-fw-analyzer` e `PS5-Camera-Setup` quando houver payload de release.

### 7.4 Sequência para repetir a pesquisa em hardware

Usar a câmera CFI-ZEY1 em uma porta USB 3.x direta ou hub SuperSpeed confiável. A sequência abaixo separa observação de mutação.

#### A. Estado inicial e descritores

Com a câmera conectada, antes e depois do binding WinUSB:

```powershell
New-Item -ItemType Directory -Force artifacts/reconstruction/probe | Out-Null
cargo run --locked -p ps5cam-probe -- `
  artifacts/reconstruction/probe/boot.json `
  --dump-dir artifacts/reconstruction/probe/descriptors
cargo run --locked -p ps5cam-diagnostics -- --timeout-ms 1000
```

Resultados esperados:

- sem binding: o PnP pode encontrar `05A9:0580` com problema 28, sem acesso libusb;
- com WinUSB: exatamente um boot device, `problem_code: 0`, SuperSpeed, porta física identificada e endpoints bulk `0x01`/`0x82`;
- se aparecer somente high-speed/USB 2, trocar porta/cabo antes de executar upload.

#### B. Validar o firmware sem acessar USB

```powershell
$firmwarePath = 'firmware/reference/21.01-03.20.00.04-00.00.00.bin'
$firmwareHash = '10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54'

cargo run --locked -p ps5cam-loader-cli --bin ps5cam-loader -- inspect `
  $firmwarePath `
  --expected-sha256 $firmwareHash `
  --provenance 'prosperodev/hdcamera@8773610978d5a4d91a6a6d8063d48a4f3afcfe5b' `
  --authorization-reference 'upstream-mit-license-2021-prosperodev'
```

Conferir 68.948 bytes, 135 blocos de 512 bytes (o último é curto) e o hash esperado.

#### C. Capturar a enumeração USB

Abrir PowerShell elevado. Identificar o root hub correto executando `USBPcapCMD.exe` sem iniciar upload e selecionar a interface que contém a porta da câmera; no ensaio original foi `\\.\USBPcap1`.

```powershell
New-Item -ItemType Directory -Force artifacts/reconstruction/usb | Out-Null
& 'C:\Program Files\USBPcap\USBPcapCMD.exe' `
  -d '\\.\USBPcap1' `
  -o artifacts/reconstruction/usb/boot-enumeration.pcapng
```

Reconectar a câmera durante a janela de captura e encerrar com `Ctrl+C`. No ensaio original a janela foi de 60 segundos. Para o upload, iniciar nova captura da mesma forma, executar a etapa D em outro terminal elevado e encerrar após a reenumeração; a janela original foi de 45 segundos.

Filtros de exibição úteis no Wireshark/TShark:

```text
usb.idVendor == 0x05a9 && usb.idProduct == 0x0580
usb.idVendor == 0x05a9 && usb.idProduct == 0x058c
usb.bmRequestType == 0x40 && usb.setup.bRequest == 0x00
usb.setup.wValue == 0x2200 && usb.setup.wIndex == 0x8018
usb.transfer_type == 0x02 && usb.bmRequestType.type == 0x01
```

Exportação tabular equivalente à usada na pesquisa:

```powershell
$tshark = 'C:\Program Files\Wireshark\tshark.exe'
& $tshark -r artifacts/reconstruction/usb/boot-enumeration.pcapng `
  -Y 'usb' -T fields -E header=y -E separator=, -E quote=d `
  -e frame.number -e frame.time_relative -e frame.len `
  -e usb.bus_id -e usb.device_address -e usb.idVendor -e usb.idProduct `
  -e usb.endpoint_address -e usb.transfer_type -e usb.urb_type `
  -e usb.bmRequestType -e usb.setup.bRequest -e usb.setup.wValue `
  -e usb.setup.wIndex -e usb.data_len |
  Set-Content -Encoding utf8 artifacts/reconstruction/usb/boot-enumeration.usb.csv
```

Na publicação original, os frames 13–28 foram a janela minimizada. Em uma nova captura, números, endereços e timestamps podem mudar; deve-se selecionar novamente a transação pela identidade `05A9:0580`, não presumir os mesmos frames. A transformação de uma mesma entrada deve ser determinística, mas uma nova captura não terá os hashes antigos.

#### D. Upload autorizado e reenumeração

Esta é uma operação real no dispositivo e exige terminal elevado, firmware autorizado e exatamente um boot device SuperSpeed:

```powershell
cargo run --release --locked -p ps5cam-loader-cli --bin ps5cam-loader -- upload `
  $firmwarePath `
  --expected-sha256 $firmwareHash `
  --provenance 'prosperodev/hdcamera@8773610978d5a4d91a6a6d8063d48a4f3afcfe5b' `
  --authorization-reference 'upstream-mit-license-2021-prosperodev' `
  --acknowledge-authorized-firmware `
  --confirm-device 'USB\VID_05A9&PID_0580' `
  --preflight-timeout-ms 1000 `
  --transfer-timeout-ms 1000 `
  --upload-deadline-ms 120000 `
  --reenumeration-timeout-ms 10000
```

Salvar o JSON emitido. O resultado esperado é 68.948 bytes/135 blocos, `execute: command_accepted` e o mesmo caminho físico reaparecendo como `05A9:058C`. Repetir `ps5cam-probe` no estado UVC. Desconectar a alimentação e sondar novamente para comprovar o retorno ao PID `0580`.

#### E. Captura e medição UVC

Primeiro enumerar os nomes DirectShow e confirmar `USB Camera-OV580`:

```powershell
ffmpeg -hide_banner -list_devices true -f dshow -i dummy
```

Captura mono de 300 quadros, equivalente ao ensaio longo de dez segundos:

```powershell
New-Item -ItemType Directory -Force artifacts/reconstruction/mono | Out-Null
ffmpeg -hide_banner -loglevel info -rtbufsize 128M -f dshow `
  -video_size 1920x1080 -framerate 30 -pixel_format yuyv422 `
  -i 'video=USB Camera-OV580' -frames:v 300 -vf format=gray -y `
  artifacts/reconstruction/mono/frame-%04d.png
```

Captura estéreo de 60 quadros:

```powershell
New-Item -ItemType Directory -Force artifacts/reconstruction/stereo | Out-Null
ffmpeg -hide_banner -loglevel info -rtbufsize 128M -f dshow `
  -video_size 3840x1080 -framerate 30 -pixel_format yuyv422 `
  -i 'video=USB Camera-OV580' -frames:v 60 -vf format=gray -y `
  artifacts/reconstruction/stereo/frame-%04d.png
```

O utilitário do projeto mede os quadros sem gravá-los:

```powershell
cargo run --release --locked -p ps5cam-uvc --bin ps5cam-uvc-capture -- `
  --backend media-foundation --mode mono --frames 300

cargo run --release --locked -p ps5cam-uvc --bin ps5cam-uvc-capture -- `
  --backend directshow --mode stereo --frames 60 --ffmpeg ffmpeg
```

Guardar a saída JSON, o log do FFmpeg, contagem de arquivos, dimensões, médias de luma, fração não zero e hashes SHA-256. Para o benchmark bruto mono de três segundos:

```powershell
ffmpeg -hide_banner -rtbufsize 128M -f dshow `
  -video_size 1920x1080 -framerate 30 -pixel_format yuyv422 `
  -i 'video=USB Camera-OV580' -t 3 -c:v copy -f rawvideo -y `
  artifacts/reconstruction/mono-3s.yuyv
```

Iluminar a cena de modo que as duas metades do quadro estéreo recebam sinal. Conteúdo bilateral pode ser medido; sincronismo temporal exige um estímulo óptico comum e instrumentado, ensaio que ainda não foi realizado.

#### F. Análise e comparação de firmware

O firmware de referência pode ser analisado sem hardware:

```powershell
cargo run --release --locked -p ov580-fw-analyzer -- analyze `
  firmware/reference/21.01-03.20.00.04-00.00.00.bin

cargo run --release --locked -p ov580-fw-analyzer -- diff `
  caminho/firmware.bin caminho/variante.bin
```

Para reconstruir a desmontagem privada, obter `binutils-2.36.1.tar.xz` do arquivo oficial GNU, verificar SHA-256 `e81d9edf373f193af428a0f256674aea62a9d74dfe93f65192d4eae030b0f3b0`, compilar `binutils` para o target `nds32be-elf` e executar:

```text
objdump -D -b binary -m nds32 -EB --adjust-vma=0 21.01-03.20.00.04-00.00.00.bin
```

O resultado original tinha 838.400 bytes e SHA-256 `512b265607b2f8b020a49c89f6827e01ee84a4f3c3f6c59b87c5c5f920bc8ba8`. Diferenças de versão/formatação do objdump podem alterar o hash textual sem alterar o firmware.

### 7.5 Reconstrução do pacote e da release

Os scripts de [`windows/package`](windows/package) são a especificação executável do empacotamento. Com WDK disponível:

```powershell
$catalogDir = 'artifacts/reconstruction/catalog'
$releaseDir = 'artifacts/reconstruction/release'
$setupDir = 'artifacts/reconstruction/setup'
$sourceRevision = (git rev-parse HEAD).Trim()
$sourceEpoch = [long](git log -1 --format=%ct)

.\windows\package\validate-package.ps1 -RequireWdk
.\windows\package\package-pipeline.ps1 -Action Catalog -Execute `
  -StagingDirectory $catalogDir `
  -ConfirmHardwareId 'USB\VID_05A9&PID_0580'

cargo build --release --locked -p ps5cam-service -p ps5cam-diagnostics
```

Para assinar o catálogo é obrigatório fornecer um certificado de desenvolvimento autorizado com chave privada e passar seu thumbprint ao `package-pipeline.ps1 -Action TestSign`. Depois:

```powershell
.\windows\package\build-development-release.ps1 `
  -OutputDirectory $releaseDir `
  -ReleaseVersion 1.0.1 `
  -SourceRevision $sourceRevision `
  -SourceDateEpoch $sourceEpoch `
  -CatalogDirectory $catalogDir `
  -ManifestCertificateThumbprint '<THUMBPRINT_DO_CERTIFICADO>'

.\windows\package\build-single-file-setup.ps1 `
  -ReleaseDirectory $releaseDir `
  -OutputDirectory $setupDir
```

O certificado usado na V1 tinha thumbprint `EDAF55A1E4AE0C8C197988F7286626BD51228CA2`. Sua chave privada **não está no repositório nem neste dossiê**; o PFX e a senha são mantidos como `PS5CAM_SIGNING_PFX_BASE64` e `PS5CAM_SIGNING_PFX_PASSWORD` nos Secrets do GitHub. `PS5CAM_RELEASE_TOKEN` é necessário apenas para publicar. Sem o PFX original é possível reconstruir e testar o código, gerar um catálogo com outro certificado de desenvolvimento e repetir as capturas, mas não reproduzir a identidade criptográfica nem os hashes exatos da release assinada V1.0.1.

Operações de catálogo, assinatura, instalação, certificado, serviço e upload devem ocorrer em PowerShell elevado. Os testes unitários, análise de firmware, validação estática e planejamento do pacote permanecem separados e podem ser repetidos sem câmera nem alterações no sistema.

## 8. Conclusão

A sequência de evidências sustenta que o projeto deixou de ser apenas um experimento de upload e se tornou uma solução Windows coerente:

1. identificou corretamente bootloader e câmera pelos PIDs;
2. limitou WinUSB ao modo boot;
3. validou e enviou firmware autorizado para RAM;
4. correlacionou a reenumeração ao mesmo caminho físico;
5. obteve vídeo UVC 1080p e estéreo largo a 30 fps;
6. automatizou reconexão por serviço;
7. criou instalação, reparo, rollback e remoção com integridade por hashes;
8. adicionou localização, diagnóstico, pacote e CI;
9. registrou honestamente as limitações de firmware, assinatura, captura USB e sincronismo;
10. encerrou com 161 testes/cenários revalidados, formatação e lint aprovados.

O conjunto preservado permite compreender a idealização, repetir a compilação, auditar as decisões técnicas e conferir os resultados experimentais que demonstraram o funcionamento do projeto.
