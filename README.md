# Vox

Ditado e leitura por voz para Windows: fala vira texto no app em foco, e texto
selecionado vira voz. Pensado para ser mais leve e mais rápido de acionar do
que qualquer app nativo equivalente — atalho global, pílula flutuante,
zero cliques além do atalho.

![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)

---

## ⚠️ Isto precisa de um servidor

**O Vox não transcreve nem sintetiza voz por conta própria.** Ele é um
**cliente**: manda áudio para um servidor de voz compatível e recebe texto ou
áudio de volta. Sem esse servidor configurado, ditado e leitura simplesmente
não funcionam — o resto do app (atalhos, painel, bandeja) funciona normalmente
de qualquer forma.

Hoje esse servidor é uma API própria do autor (chamada de "speech-api" no
código), não um produto publicado. Se você tem uma instância dela, é só
apontar o endereço no assistente de primeiro uso ou nas preferências. Se não
tem, mas quer construir uma compatível, o contrato que ela precisa cumprir
está documentado em [ARCHITECTURE.md → "O servidor não é embutido"](ARCHITECTURE.md#o-servidor-não-é-embutido).
Generalizar isso para outros provedores (Whisper local, Azure, ElevenLabs...)
é trabalho futuro, ainda não feito.

---

## O que ele faz

- **Ditado** (`Ctrl+Shift+D`) — fala vira texto e é colado no app em foco.
  Uma vez grava, de novo transcreve e entrega.
- **Leitura** (`Ctrl+Alt+L`) — o texto selecionado é lido em voz alta, com
  legenda guiada opcional (destaque palavra a palavra) e transporte
  (pausar, avançar, voltar).
- **Ajuste por IA** — a fala crua ("é... tipo assim, o que eu queria dizer")
  pode ser reescrita antes de colar, com presets (fala limpa, prompt de
  agente, mensagem profissional, tradução) e um controle de intensidade.
- **Backup de áudio** — toda gravação é guardada localmente antes de ir para
  o servidor. Se a rede cair, a transcrição vier vazia ou o reconhecedor errar
  o julgamento de "sem fala", o áudio continua disponível para reenviar — some
  sozinho em 24h, a menos que você marque para guardar.
- **Legenda com clique** — clicar numa frase da legenda volta o áudio para lá.
- **Painel de preferências** — translúcido, com navegação lateral, sem botão
  de salvar (cada mudança grava sozinha).
- **Widget flutuante** — arrastável, lembra a posição por papel (ditado vs.
  leitura), sobrevive a troca de monitor e a bloqueio de tela.
- **Extensão de navegador** (Chromium) — lê com destaque na própria página,
  sem abrir janela nenhuma. Ver `extension/README.md`.

Tudo isso é configurável — atalhos, vozes, modelo de transcrição, tema,
comportamento — pelo painel (`Ctrl+Alt+O` ou pela bandeja).

## Instalação

Ainda não há instalador publicado (MSI/NSIS) — a distribuição é compilar do
código-fonte:

```powershell
git clone https://github.com/Thierrycast/vox.git
cd vox/src-tauri
cargo build --release
./target/release/vox.exe
```

Requer o [Rust](https://rustup.rs/) (edição 2021, `rust-version` 1.82+) e as
dependências de build do Tauri 2 para Windows (WebView2 já vem com o Windows
10/11 atualizado).

Na primeira execução, um assistente pede o endereço do servidor, o microfone
e mostra os atalhos padrão. Pode ser pulado e configurado depois, pelo painel.

### Desenvolvimento

```powershell
cd src-tauri
cargo run                 # sobe com console e log ao vivo
cargo test                # a suíte de testes de lógica pura
cargo clippy               # lint
```

`scripts/instalar-atalho.ps1` cria um atalho "Vox (dev)" que recompila se as
fontes mudaram, mata a instância anterior e sobe a mais nova — útil para não
abrir terminal a cada iteração. `scripts/atalhos-livres.ps1` mede quais
combinações de atalho global estão livres nesta máquina (rode com o Vox
fechado, senão ele aparece "ocupando" os próprios atalhos).

## Configuração

A maior parte é pelo painel (`Ctrl+Alt+O`):

| Seção | O que tem |
|---|---|
| Geral | som, cor de destaque, reiniciar o app |
| **Servidor** | endereço, credencial, teste de conexão |
| Ditado | microfone, modelo de transcrição, como o texto é entregue |
| Ajuste por IA | reescrita da fala crua, presets, intensidade |
| Leitura | voz, velocidade, legenda guiada |
| Vocabulário | termos e instruções que o reconhecedor recebe como contexto |
| Gravações | o backup de áudio: reenviar, copiar, salvar, excluir |
| Atalhos | toda combinação global, com detecção de conflito |
| Widget | onde a pílula flutuante fica |
| Extensão | a ponte local que o navegador usa |

Para automação/deploy, as mesmas três coisas do servidor aceitam variável de
ambiente (`VOX_API_URL`, `VOX_API_USER`, `VOX_API_PASSWORD` — ver
`.env.example`) como caminho alternativo; o painel vence quando os dois
estão preenchidos.

## Atalhos padrão

| Comando | Atalho |
|---|---|
| Ditar | `Ctrl+Shift+D` |
| Cancelar o ditado | `Ctrl+Shift+X` |
| Ler a seleção / pausar / retomar | `Ctrl+Alt+L` |
| Pausar e retomar (sem começar leitura nova) | `Ctrl+Alt+P` |
| Parar a leitura | `Ctrl+Alt+S` |
| Legenda guiada | `Ctrl+Alt+G` |
| Mostrar/esconder o widget | `Ctrl+Alt+V` |
| Preferências | `Ctrl+Alt+O` |

Todos reconfiguráveis pelo painel, com aviso de conflito. As combinações
padrão foram escolhidas para não tomar nada do Windows nem do teclado ABNT2
(onde `AltGr` produz `Ctrl+Alt`) — ver
[ARCHITECTURE.md](ARCHITECTURE.md#o-catálogo-de-comandos) para o porquê de
cada letra.

## Por que confiar nisto

- **46 testes automatizados** de lógica pura (`cargo test`), sem depender de
  microfone, rede ou servidor.
- **`cargo clippy` limpo**, sem avisos.
- Falhas de rede, de transcrição ou de reconhecimento nunca perdem o áudio —
  ver "Backup de áudio", acima.
- Cada decisão de design não-óbvia está documentada onde a decisão foi
  tomada — no código, e consolidada em [ARCHITECTURE.md](ARCHITECTURE.md).

## Limites conhecidos

- Sem instalador publicado ainda (MSI/NSIS existem como target do Tauri, não
  publicados como release).
- Só fala o contrato de uma API própria — sem adaptador para outros
  provedores de voz ainda.
- Destaque de texto na própria página só funciona pela extensão de
  navegador; em outros apps, a leitura abre uma janela própria.

Ver [ARCHITECTURE.md → "O que ainda não existe"](ARCHITECTURE.md#o-que-ainda-não-existe)
para a lista completa e o porquê de cada item.

## Estrutura, arquitetura e decisões de design

O "como" e o "por quê" de cada peça — janelas translúcidas sem moldura do
Windows, por que a leitura é fatiada, como o widget deixou de sumir, a
identidade estável do microfone via WASAPI, e mais — estão em
[ARCHITECTURE.md](ARCHITECTURE.md).

## Licença

[MIT](LICENSE).
