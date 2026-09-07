# Vox

Ditado e leitura por voz no Windows, sobre a `speech-api` do lab.

Dois comandos, um pacote, um provider:

- **Ditado** (`Ctrl+Shift+D`) — fala vira texto e é colado no app em foco.
- **Leitura** (`Ctrl+Alt+L`) — o texto selecionado é lido em voz alta.

Os dois atalhos são configuráveis. `Ctrl+Shift+S` era o padrão da leitura e
foi trocado: abre o DevTools no Chrome, e o navegador ganha a disputa.

O mesmo atalho serve para os três momentos da leitura: começa, pausa e retoma.

---

## Antes de rodar

### 1. O nome da API precisa resolver

`speech-api.lab.home` só existe na LAN e não está em DNS nenhum. Acrescente ao
`C:\Windows\System32\drivers\etc\hosts` (como administrador):

```
192.168.1.36  speech-api.lab.home
```

O certificado é do **Thierry Lab CA**, que esta máquina já confia — não precisa
de `-k` nem de exceção.

### 2. A credencial

A rota está atrás do middleware `panel-auth@file` do Traefik, então sem
credencial vem `401`. Crie um usuário só para o app, para poder revogá-lo sem
mexer na sua senha pessoal:

```bash
docker run --rm httpd:alpine htpasswd -nbB voice-client '<senha>'
```

Acrescente a linha resultante ao `panel-auth` em
`/DATA/AppData/traefik-v3/config/dynamic/lab-routes.yml`, no argos.

### 3. O ambiente

```bash
cp .env.example .env
```

e preencha `VOX_API_USER` / `VOX_API_PASSWORD`. O `.env` está no `.gitignore`.

---

## Rodar

```powershell
cd src-tauri
cargo run                 # desenvolvimento
cargo build --release     # binário
cargo test                # os testes de lógica pura
```

---

## Como está desenhado

### O que acontece ao apertar o atalho de ditado

A ordem não é arbitrária — cada passo está onde está por causa de latência
percebida:

1. **O som toca.** Retorno em poucos milissegundos, antes de qualquer coisa que
   possa falhar ou demorar.
2. **O microfone abre.** O primeiro fonema não pode se perder.
3. **O HUD aparece.** A janela já existe escondida desde a partida do app;
   criar uma webview custa 100–200 ms, e pagar isso no atalho apareceria como
   atraso. Mostrar uma janela pronta custa cerca de um quadro.
4. **A conexão é pré-aquecida**, em segundo plano, enquanto o usuário fala. O
   handshake TLS custa 100–300 ms e é de graça agora; no fim, seria demora.

A conversão para 16 kHz / mono / 16-bit acontece **dentro** do callback de
captura, não num passe depois. Quando o atalho é solto, o buffer já está no
formato de envio.

### Por que a leitura é fatiada

A `speech-api` devolve o arquivo pronto, sem streaming. Medido no argos:

| Texto | Geração | Áudio | Razão |
|---|---|---|---|
| 3 chars | 740 ms | — | — |
| 134 chars | 7939 ms | 7,50 s | **1,06×** |
| repetido | 1 ms | — | cache |

Duas consequências mandam no desenho:

- Pedir o texto inteiro faria esperar dezenas de segundos pelo primeiro som. Por
  isso o texto é quebrado, e o **primeiro trecho sai curto de propósito** —
  ~740 ms até começar a falar, em vez de minutos.
- Como gerar é **mais lento que falar**, a fila esvazia sozinha num texto longo:
  o déficit de 6% acumula e nunca se recupera. Por isso o player acumula uma
  dianteira antes de começar (`prebuffer_ratio`, 10% por padrão). Sem ela, a
  leitura engasga no meio.

`tts_gate: limit 1` no servidor significa que pedir em paralelo não adianta — os
pedidos enfileiram. A geração aqui é sequencial de propósito.

### O modelo de transcrição

Dois disponíveis, e a escolha é de privacidade:

| Modelo | Onde roda | O áudio sai? | Qualidade |
|---|---|---|---|
| `vosk` | argos | não | sem pontuação, sem maiúscula |
| `groq/whisper-large-v3-turbo` | via omniroute | **sim, sai da LAN** | pronto para colar |

Teste controlado com o Vosk: *"O rato roeu a roupa do rei de Roma."* voltou como
`o rato ruiu a roupa do rei de roma`. Por isso o padrão é o Whisper — quem
quiser privacidade total troca nas preferências e aceita o pós-processamento.

### Os sons

Retorno sonoro é o canal principal quando não se está olhando para a tela, então
o conjunto se separa por **contorno**, não por altura:

| Evento | Notas | Gesto | Duração |
|---|---|---|---|
| Leitura começou | D5 → A5 | **sobe** | 260 ms |
| Leitura terminou | A5 resolve em D5 | **desce** | 535 ms |
| Leitura pausou | D5 | blip curto | 140 ms |
| Leitura falhou | D5 + C#5 | **bate** | 340 ms |

Só um sobe, só um bate, só um é curtíssimo — dá para distinguir de olhos
fechados. A gramática (senoide com harmônico de oitava, ataque de 12–55 ms,
decaimento exponencial puro, o significado no intervalo) veio de medir os sons
de ditado do Raycast; os arquivos em `assets/sounds/` são nossos.

Os três sons de **ditado** não acompanham o binário: se houver uma instalação do
Raycast na máquina, o app aponta para os `.aif` dela; se não houver, usa os
equivalentes da leitura.

---

## As três janelas

O front não é uma janela com modos: são três, cada uma com o seu próprio ciclo
de vida. Foi uma decisão de desenho, não acidente de implementação.

| Janela | Onde fica | Quando aparece |
|---|---|---|
| `hud` | centro inferior, com folga da barra de tarefas | durante o ditado e como player compacto na leitura |
| `reader` | janela normal | quando a leitura precisa de espaço: transporte, linha do tempo, destaque por palavra |
| `live` | canto superior direito | só se `show_live_transcription` estiver ligada |

O texto reconhecido ao vivo mora fora do HUD porque as duas coisas têm ritmos
incompatíveis: a onda responde à voz em tempo real e precisa ficar parada no
lugar, enquanto o texto cresce e reflui a cada palavra. Dentro do mesmo card, o
HUD pulava de tamanho no meio da fala. E por padrão ele nem aparece — texto
correndo embaixo da onda disputa a atenção justamente enquanto a pessoa está
formulando a frase.

A transcrição ao vivo continua sendo **capturada** mesmo com o popup desligado:
ela é a rede de segurança se a transcrição final falhar. `live_transcription`
liga a captura; `show_live_transcription` liga a exibição.

## A onda

A referência do Raycast faz poll a 50 ms e desenha o valor cru com uma transição
de 20 ms. Copiado ao pé da letra, o resultado é uma onda nervosa: a barra assenta
em 20 ms e fica parada os outros 30, o que se vê é um degrau piscando.

Aqui a altura passa por uma média móvel exponencial antes de ir para a tela, com
ataque mais rápido que a queda — 0,55 subindo, 0,82 descendo. Subir devagar faria
a onda parecer atrasada em relação à voz; descer devagar é o que dá o decaimento
natural. A transição da barra cobre o intervalo inteiro do poll (130 ms), então o
movimento nunca para.

## Formas de acionar

| Caminho | Onde funciona |
|---|---|
| `Ctrl+Shift+D` / `Ctrl+Alt+L` | qualquer app |
| Bandeja → *Ler a área de transferência* | qualquer app |
| Extensão: botão direito → *Ler com o Vox*, ou `Alt+Shift+L` | navegadores Chromium |

A extensão está em `extension/` e é a única que **destaca o texto na própria
página**, sem abrir janela. Ela fala com o Vox por um servidor em
`127.0.0.1:8765` — ver `extension/README.md` para instalação e para o modelo de
ameaça, que não é decorativo: loopback é alcançável por qualquer página aberta,
e por isso a checagem de origem e o token acontecem no servidor, antes de agir.

## Estrutura

```
vox/
├── assets/sounds/       os quatro .wav, 48 kHz 24-bit
├── src/                 o front — três janelas independentes
│   ├── hud.html         a barra do ditado e a coluna da leitura
│   ├── reader.html      a janela de leitura, com transporte e destaque
│   ├── live.html        o texto reconhecido ao vivo (opcional)
│   ├── css/             uma folha por janela
│   └── js/              um módulo por janela
└── src-tauri/src/
    ├── main.rs          atalhos, comandos, partida
    ├── audio.rs         captura, reamostragem, níveis
    ├── api.rs           cliente da speech-api, fatiador
    ├── dictation.rs     o ciclo do ditado
    ├── reading.rs       a fila de leitura e a dianteira
    ├── paste.rs         colagem, capitalização, palavra-chave
    ├── sounds.rs        retorno sonoro
    ├── stt_stream.rs    transcrição incremental por WebSocket
    ├── tray.rs          ícone da bandeja e estado dos atalhos
    └── config.rs        preferências e ambiente
```

## Limites herdados

Da API e do desenho de referência:

| | |
|---|---|
| Vocabulário | 50 itens, 50 caracteres cada, únicos |
| Velocidade da voz | 0,5× a 2,0× |
| Vozes | 26 — 22 do Kokoro e 4 do Piper; padrão `piper:pt_BR-cadu-medium` |
| Poll dos níveis | 50 ms, com transição de 130 ms na barra |
| HUD após sucesso | 2000 ms |
| Espera antes do envio | 50 ms |

## Backlog do backend

As limitações medidas e as oportunidades de evolução estão em
`argos:~/dev/lab-standards/SPEECH-API-EVOLUCOES.md`.
