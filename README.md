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

O Vox precisa de uma `speech-api` alcançável. Há dois caminhos, e o padrão não
exige configuração nenhuma.

### O caminho padrão — tailnet

```bash
cp .env.example .env
```

O `.env.example` já vem com `VOX_API_URL=http://100.122.39.56:8010`. Esse
endereço é do tailnet: funciona de qualquer lugar, não passa pelo Traefik e
**não pede credencial** — qualquer dispositivo do tailnet alcança a API. Não há
mais nada a fazer.

### O caminho da LAN — autenticado

Só faz sentido em casa, e é o único que passa pelo `panel-auth` do Traefik.

Primeiro, o nome precisa resolver. `speech-api.lab.home` não está em DNS nenhum;
acrescente ao `C:\Windows\System32\drivers\etc\hosts`, como administrador:

```
192.168.1.36  speech-api.lab.home
```

O certificado é do **Thierry Lab CA**, que esta máquina já confia — não precisa
de `-k` nem de exceção.

Depois, a credencial. Crie um usuário só para o app, para poder revogá-lo sem
mexer na sua senha pessoal:

```bash
docker run --rm httpd:alpine htpasswd -nbB voice-client '<senha>'
```

Acrescente a linha resultante ao `panel-auth` em
`/DATA/AppData/traefik-v3/config/dynamic/lab-routes.yml`, no argos, e preencha
`VOX_API_USER` / `VOX_API_PASSWORD` no `.env`. O `.env` está no `.gitignore`.

Por fim, aponte `VOX_API_URL` para `https://speech-api.lab.home`.

---

## Rodar

```powershell
cd src-tauri
cargo run                 # desenvolvimento
cargo build --release     # binário
cargo test                # os testes de lógica pura
```

### O atalho da área de trabalho

Enquanto o projeto está em desenvolvimento não há instalador nem partida
automática no boot — mas também não precisa abrir o terminal para subir o app:

```powershell
pwsh -File scripts\instalar-atalho.ps1     # cria "Vox (dev)" na área de trabalho e no Iniciar
pwsh -File scripts\instalar-atalho.ps1 -Remove
```

O atalho **não aponta para o executável**. Aponta para `scripts\vox-dev.ps1`, que
a cada clique:

1. **encerra o Vox que estiver aberto** — dois processos não convivem (o atalho
   global é exclusivo do primeiro que registrar, e o segundo fica aberto sem
   responder a nada), e o Windows tranca o `.exe` em execução: compilar antes de
   encerrar termina em `Acesso negado` depois de compilar tudo;
2. compara a data das fontes com a do binário e **recompila se algo mudou** —
   por isso o atalho não envelhece junto com o build de ontem;
3. carrega o `.env` no ambiente do processo e sobe o app.

Se a compilação falhar, o binário anterior ainda está no disco e é ele que sobe:
um erro de compilação não deixa a máquina sem Vox nenhum. A janela da compilação
fica aberta com o erro, e o `launcher.log` diz `compilacao falhou - subindo o
binario anterior`, para não se confundir *"está rodando"* com *"está rodando o
que eu acabei de escrever"*.

Clicar no atalho, portanto, significa sempre *"põe a última versão no ar"*.

O executável de `target\debug` abre um console porque o `windows_subsystem =
"windows"` só vale em release — é intencional, é ali que se lê o log ao vivo. O
atalho usa o binário de release, sem console; o log vai para arquivo (abaixo).

Não há partida automática no boot **de propósito**: em desenvolvimento, subir
sozinho significaria encontrar um bug antes de pedir por ele. Quando o app
estabilizar, um `.lnk` na pasta `shell:startup` resolve.

### Onde ficam os logs

```
%APPDATA%\vox\config\vox.log         o app — gira em 2 MB, guarda um .log.old
%APPDATA%\vox\config\launcher.log    o atalho — o que compilou, matou e subiu
```

É a mesma pasta do `settings.json`, que a bandeja abre em *"Abrir o arquivo de
preferências"*. O nível vem de `VOX_LOG` (`info` por padrão); em debug a saída
continua indo também para o terminal.

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

## As quatro janelas

O front não é uma janela com modos: são quatro, cada uma com o seu próprio ciclo
de vida. Foi uma decisão de desenho, não acidente de implementação.

| Janela | Onde fica | Quando aparece |
|---|---|---|
| `hud` | centro inferior no ditado, borda direita na leitura | durante o ditado e como player da leitura |
| `reader` | janela normal | quando a leitura precisa de espaço: transporte, linha do tempo, destaque por palavra |
| `live` | canto superior direito | só se `show_live_transcription` estiver ligada |
| `settings` | janela normal | pela bandeja — clique no ícone ou *Preferências…* |

O texto reconhecido ao vivo mora fora do HUD porque as duas coisas têm ritmos
incompatíveis: a onda responde à voz em tempo real e precisa ficar parada no
lugar, enquanto o texto cresce e reflui a cada palavra. Dentro do mesmo card, o
HUD pulava de tamanho no meio da fala. E por padrão ele nem aparece — texto
correndo embaixo da onda disputa a atenção justamente enquanto a pessoa está
formulando a frase.

A transcrição ao vivo continua sendo **capturada** mesmo com o popup desligado:
ela é a rede de segurança se a transcrição final falhar. `live_transcription`
liga a captura; `show_live_transcription` liga a exibição.

## O painel de preferências

Abre pela bandeja: clique no ícone, ou *Preferências…* no menu. Cobre som (com o
**volume dos avisos** e um botão para ouvir), ditado, leitura, atalhos e a ponte
da extensão — o token fica ali, a um clique de ser copiado.

Não tem botão de salvar. Cada mudança grava sozinha, e o painel se redesenha com
o que voltou do disco: o backend corrige valores fora de faixa, e a tela precisa
mostrar o que ficou valendo, não o que foi digitado. Um formulário com "Aplicar"
convida a fechar sem clicar, e aí a preferência não valeu sem ninguém avisar.

Porta da ponte e atalhos globais só valem depois de reiniciar o Vox; o resto vale
na hora. O `settings.json` continua acessível pela bandeja para o que o painel
não expõe.

## A legenda guiada

Durante a leitura, o botão de linhas na pílula abre a legenda: ela **cresce para
a esquerda** e passa a mostrar o texto que está sendo falado, com a frase atual
em destaque e a palavra corrente acesa. Fechada, a pílula é a coluna estreita de
sempre — quem só quer ouvir não paga espaço nenhum por isso.

O estado fica guardado: a próxima leitura já começa como a última terminou, e por
isso a pílula nasce do tamanho certo em vez de crescer depois de aparecer.

Duas coisas crescem juntas, e a ordem entre elas importa. A janela do sistema é o
recipiente; o card é o desenho. Ao abrir, a janela vai primeiro — se ficasse do
tamanho antigo, o texto nasceria cortado na borda. Ao fechar, ela vai por último,
depois da animação. E o tamanho é sempre justo: uma janela transparente maior que
o desenho continua capturando o clique na área vazia, e a página atrás pararia de
responder num retângulo invisível.

A posição da fala vem da janela de leitura, que é quem tem o elemento de áudio.
As duas mostram o mesmo texto ao mesmo tempo; se cada uma estimasse por conta
própria onde a voz está, o olho perceberia a divergência na hora.

## Como o realce sabe onde a voz está

Nenhuma das vozes devolve marcação de tempo por palavra, então a posição é
estimada — e a estimativa é o que separa um destaque que acompanha de um que
atrapalha.

O tempo da frase é repartido por **peso**, não por caractere: letras, mais um
custo fixo por palavra, mais a pausa que a pontuação impõe. A conta por caractere
puro assume que toda palavra é falada na mesma velocidade por letra, e não é —
começar a falar custa tempo que não tem letra nenhuma, e uma vírgula segura a voz
sem gastar caractere. O erro típico daquela conta era chegar cedo no começo da
frase e tarde no fim.

O destaque anda num laço de quadro, e não a cada evento de tempo do áudio: o
`timeupdate` dispara uma média de quatro vezes por segundo e sem ritmo garantido,
o que dá degraus de até 250 ms — quase uma palavra inteira de atraso numa fala
normal.

E o que viaja entre as janelas é o **índice da palavra**, não a fração da frase.
Com a fração, cada janela reconstruía a divisão do texto por conta própria e
bastava um espaço a mais para as duas discordarem sobre qual palavra é a atual.

## Quem manda no estado da leitura

O áudio toca na janela de leitura, então é ela que sabe se está tocando. O estado
vem dos eventos do próprio elemento de áudio — `play` e `pause` — e não de quem
clicou no botão.

A diferença não é acadêmica: enquanto cada botão anunciava o estado que
*pretendia* causar, bastava um caminho esquecido para a etiqueta mentir. Avançar
15 s numa leitura terminada voltava a tocar de verdade, mas ninguém dizia
"playing", e a pílula seguia mostrando o ícone de play com a voz falando.

O backend guarda uma cópia desse estado, porque é ela que decide o que o atalho
global faz: pausar, retomar ou começar uma leitura nova. A cópia é atualizada
pelo front a cada mudança, e o backend **não** reemite o evento ao recebê-la —
senão os dois ficariam se avisando em círculo. Parado continua parado: depois de
um `stop`, o evento de pausa que o elemento de áudio ainda dispara não ressuscita
a leitura.

## Um widget, vários conteúdos

Gravando, processando, "Copiado", aviso, erro — é a mesma janela, no mesmo lugar,
com a **mesma largura**. Antes o "Copiado" era `width: auto` e nascia bem menor
que a barra da onda: o card encolhia de repente e lia como se outro aplicativo
tivesse aparecido no meio do caminho.

A única exceção é a altura do erro quando ele traz um motivo — o texto precisa da
segunda linha, e cortá-lo custaria justamente a informação que decide o que
fazer.

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
├── scripts/             atalho de desenvolvimento
│   ├── vox-dev.ps1      compila se preciso, encerra o antigo e sobe
│   ├── vox-dev.vbs      abre o launcher sem piscar console
│   └── instalar-atalho.ps1
├── src/                 o front — quatro janelas independentes
│   ├── hud.html         a barra do ditado e a coluna da leitura
│   ├── reader.html      a janela de leitura, com transporte e destaque
│   ├── live.html        o texto reconhecido ao vivo (opcional)
│   ├── settings.html    o painel de preferências
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

## O que ainda não existe

Está em uso diário e o que está aqui funciona, mas nem tudo o que a configuração
promete está ligado:

| | |
|---|---|
| Instalador e partida no boot | só o atalho de desenvolvimento; não há MSI nem entrada de inicialização |
| `vocabulary` e `custom_instructions` | guardados e **nunca enviados** — ver abaixo |
| Destaque fora do navegador | só a extensão acompanha o texto na tela; em outros apps abre-se a janela de leitura |

Nada disso quebra o uso: ditado e leitura funcionam ponta a ponta.

**Sobre o vocabulário e as instruções.** Eles só teriam efeito como *prompt* da
transcrição — é assim que se ensina um nome próprio ao Whisper. O
`/v1/audio/transcriptions` da speech-api hoje aceita `file`, `model` e
`response_format`, e ignora qualquer outro campo: mandar daqui não faria nada.
Ligar isso é uma mudança no servidor, não no app, e por isso os dois campos
continuam fora do painel de preferências — uma opção que não faz nada é pior que
uma opção que não existe.

**Duas preferências foram removidas**, pelo mesmo critério. `mute_while_recording`
nunca teve uma linha de código atrás dela. `context_aware_paste` ligava e
desligava um ajuste que depende de saber o que está antes do cursor — e ler isso
exige UI Automation, que ainda não existe aqui; o ajuste continua no código e
passa a valer sozinho quando essa leitura chegar.

## Backlog do backend

As limitações medidas e as oportunidades de evolução estão em
`argos:~/dev/lab-standards/SPEECH-API-EVOLUCOES.md`.
