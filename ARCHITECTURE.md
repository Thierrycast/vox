# Como o Vox é desenhado

Este documento é o "porquê" por trás das decisões do código — para o "o quê"
e o "como instalar", ver o [README](README.md). Cada seção aqui existe porque
alguma escolha não era óbvia, ou porque o caminho errado já foi tentado e
falhou de um jeito que vale registrar.

## O que acontece ao apertar o atalho de ditado

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

## Por que a leitura é fatiada

A speech-api devolve o arquivo pronto, sem streaming. Medido num servidor de
referência:

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

Um limite de concorrência no servidor (`tts_gate: limit 1`, no servidor de
referência) significa que pedir em paralelo não adianta — os pedidos
enfileiram. A geração aqui é sequencial de propósito.

## O modelo de transcrição

Dois caminhos possíveis, e a escolha é de privacidade — qual dos dois existe
depende de como o servidor está montado:

| Modelo | Onde roda (referência) | O áudio sai? | Qualidade |
|---|---|---|---|
| `vosk` | local ao servidor | não | sem pontuação, sem maiúscula |
| `groq/whisper-large-v3-turbo` | via gateway externo | **sim, sai da rede local** | pronto para colar |

Teste controlado com o Vosk: *"O rato roeu a roupa do rei de Roma."* voltou
como `o rato ruiu a roupa do rei de roma`. Por isso o padrão é o Whisper —
quem quiser privacidade total troca nas preferências e aceita o
pós-processamento.

## Os sons

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

## As quatro (e agora cinco) janelas

O front não é uma janela com modos: são várias, cada uma com o seu próprio
ciclo de vida. Foi uma decisão de desenho, não acidente de implementação.

| Janela | Onde fica | Quando aparece |
|---|---|---|
| `hud` | centro inferior no ditado, borda direita na leitura | durante o ditado e como player da leitura |
| `reader` | janela normal | quando a leitura precisa de espaço: transporte, linha do tempo, destaque por palavra |
| `live` | canto superior direito | só se `show_live_transcription` estiver ligada |
| `settings` | janela normal, translúcida | pela bandeja — clique no ícone ou *Preferências…* |
| `onboarding` | centrada, translúcida | uma vez, no primeiro uso — ver "Primeiro uso", abaixo |

A janela de preferências é um aplicativo, e não um formulário: navegação lateral
com várias seções, controles próprios (interruptor, seletor, fichas de vocabulário)
e cor de destaque escolhível. Os `<select>` nativos continuam no DOM guardando o
valor e respondendo ao teclado — o que se vê é desenhado por cima deles, porque
a lista cinza-clara do Windows no meio de uma janela escura é de outro
aplicativo.

O texto reconhecido ao vivo mora fora do HUD porque as duas coisas têm ritmos
incompatíveis: a onda responde à voz em tempo real e precisa ficar parada no
lugar, enquanto o texto cresce e reflui a cada palavra. Dentro do mesmo card, o
HUD pulava de tamanho no meio da fala. E por padrão ele nem aparece — texto
correndo embaixo da onda disputa a atenção justamente enquanto a pessoa está
formulando a frase.

A transcrição ao vivo continua sendo **capturada** mesmo com o popup desligado:
ela é a rede de segurança se a transcrição final falhar. `live_transcription`
liga a captura; `show_live_transcription` liga a exibição.

## Janelas de vidro, sem moldura do Windows

`hud`, `live`, `settings` e `onboarding` não usam a moldura nativa
(`decorations: false`, `transparent: true`) — a pílula flutuante é a cara do
Vox, e uma janela com barra branca do Windows no meio do resto pareceria outro
aplicativo. Isso significa arrastar, minimizar, maximizar e fechar deixam de
ser de graça: cada uma dessas janelas tem sua própria barra de título
(`-webkit-app-region: drag`, com os botões marcados `no-drag`) e seus próprios
botões, escritos em `settings.css`/`onboarding.css`.

O painel e o assistente compartilham a mesma receita visual: um painel `.janela`
com fundo escuro semi-transparente (duas declarações de `background-color` — uma
hex simples, outra com `rgb(from ...)` — para o WebView2 mais antigo não ficar
sem nada), `border-radius`, e um destaque de borda desenhado num pseudo-elemento
`::after` com `box-shadow: inset` em vez de uma `border` de verdade (que somaria
largura e empurraria o conteúdo). `backdrop-filter` não é usado: numa janela
`transparent: true` o WebView2 não tem o que amostrar atrás, e o resultado é uma
lavagem cinza opaca em vez de desfoque — confirmado testando, não por teoria.

Fechar essas janelas não as destrói: o backend intercepta `CloseRequested` e
troca por `hide()` (`settings`, `reader`) — reabrir depois não paga o custo de
recarregar a página inteira. `onboarding` é a exceção: ela só existe uma vez,
então fechar de fato (ou concluir o assistente) a destrói mesmo.

## O painel de preferências

Abre pela bandeja: clique no ícone, ou *Preferências…* no menu. Cobre servidor,
som, ditado, leitura, gravações, atalhos e a ponte da extensão — o token fica
ali, a um clique de ser copiado.

Não tem botão de salvar (a maioria dos campos). Cada mudança grava sozinha, e
o painel se redesenha com o que voltou do disco: o backend corrige valores fora
de faixa, e a tela precisa mostrar o que ficou valendo, não o que foi digitado.
Um formulário com "Aplicar" convida a fechar sem clicar, e aí a preferência não
valeu sem ninguém avisar. As exceções que **exigem reiniciar** (endereço do
servidor, porta da ponte) dizem isso explicitamente, com um botão que já
reinicia.

## Primeiro uso: o assistente de onboarding

Sem um servidor de voz configurado, o Vox não faz nada — e uma bandeja muda,
sem nenhum aviso, é o pior jeito de descobrir isso. `onboarding.html` é uma
janela que aparece uma vez (controlada por `settings.onboarding_completed`) e
recolhe só o que exige uma decisão: endereço do servidor (com teste de conexão
ao vivo, sem precisar terminar o assistente), microfone, e uma vista rápida dos
atalhos padrão. Tudo o resto (voz, tema, intensidade do ajuste por IA) já tem
um padrão sensato e fica para as preferências.

Concluir salva e reinicia — pela mesma razão que mudar o servidor nas
preferências reinicia: o cliente da API é montado uma vez, na partida.

Quem já tinha um `settings.json` de antes desse campo existir não é jogado no
assistente à força. `Settings::load` confere se a chave
`onboarding_completed` estava mesmo **ausente** do JSON — e não apenas
preenchida pelo padrão do serde, que não distingue "ausente" de "presente e
falso" — antes de decidir se é instalação nova ou upgrade de quem já usa o
Vox.

## O servidor não é embutido

O Vox é um **cliente**: ele não tem nenhum modelo de fala embarcado, e depende
de um servidor compatível pra transcrever e sintetizar voz. O contrato que esse
servidor precisa cumprir, hoje, é este:

| Rota | Método | Para quê |
|---|---|---|
| `/health` | GET | usada pelo painel para "Testar conexão" |
| `/voices/names` | GET | catálogo de vozes disponíveis |
| `/v1/audio/transcriptions` | POST, multipart | transcrição em lote (`file`, `model`, `prompt`) |
| `/text/prepare` | POST | limpa marcação antes de sintetizar |
| `/text/rewrite` | POST | o ajuste por IA do que foi ditado |
| `/text/presets` | GET | os moldes que `/text/rewrite` aceita |
| `/tts` | POST | sintetiza um trecho, devolve onde buscar o áudio |
| `/stt/stream` | WebSocket | transcrição incremental (texto ao vivo) |

Não existe hoje um adaptador para outro provedor (Whisper.cpp local, Azure
Speech, ElevenLabs, etc.) — o cliente foi escrito contra uma API própria
(chamada aqui de "speech-api"), pensada para servir este e outros clientes do
mesmo autor. Generalizar isso — um adaptador por provedor, atrás de uma
interface comum — é um caminho conhecido e deliberadamente não percorrido
ainda: o ganho não paga o custo enquanto só existe um consumidor real do
contrato.

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

## O texto é preparado antes de ser fatiado

Quem lê um `.md`, uma resposta de chat ou documentação encontra marcação a cada
parágrafo — e um sintetizador lê o que recebe: `**MCP**` vira "asterisco
asterisco MCP asterisco asterisco".

A limpeza é do servidor, não daqui, porque são vários consumidores e cada um
faria uma versão pior da mesma coisa. O Vox chama `/text/prepare` **uma vez,
com o texto inteiro**, antes de dividir em frases.

Fatiar antes de limpar seria pior do que parece: um título de Markdown ou uma
linha de tabela viram fronteira de frase falsa, os trechos saem cortados no lugar
errado, e o destaque na tela deixa de bater com o que se ouve.

Se o preparo falhar, a leitura segue com o texto original — a API limpa de novo
na síntese, então o que se perde é o corte bom, não a marcação falada.

**Corrigir o texto** (acentuação, ortografia, pontuação) é outra coisa, feita por
um modelo, e fica desligada por padrão: custa de 4 a 7 segundos por parágrafo
antes do primeiro som. Vale para texto mal escrito; é pura espera para texto que
já está certo, que é a maioria do que se lê. A chave está no painel.

## O ajuste por IA do que foi ditado

A transcrição é fiel ao que foi **dito**: hesitação, repetição, frase recomeçada
no meio, "né" e "tipo assim". Fiel e quase nunca é o que a pessoa queria ter
escrito — e limpar isso à mão anula o ganho de ter ditado.

Ligado nas preferências, o texto passa por um modelo antes de ser colado. Cinco
moldes, que o servidor lista em `GET /text/presets`:

| Molde | O que sai |
|---|---|
| Fala limpa | o mesmo conteúdo sem os tropeços, na sua voz |
| Prompt | uma instrução direta para um agente |
| Prompt detalhado | objetivo, contexto, restrições e critério de pronto |
| Mensagem profissional | registro de trabalho |
| Traduzir para inglês | limpa e traduz |

A **intensidade** (mínima, média, alta) não é um botão de qualidade: é a escolha
entre fidelidade e fluência. Em mínima o modelo tira hesitação e mantém as frases
como foram ditas; em alta ele reorganiza — e às vezes inventa contexto. Numa
medição, "quando tá pausado" virou "quando o vídeo está pausado", sem vídeo
nenhum na frase. Por isso a escala é explícita na interface e vem em média.

O custo é de segundos, e a escolha do modelo é o que decide se são dois ou dez.
O painel tem uma prévia para experimentar antes de confiar no molde para o dia
a dia.

Falha nunca custa o que foi ditado: modelo fora do ar, resposta vazia ou com
tamanho fora da faixa do molde devolvem o texto original, com o motivo no log.

## Tabela não se lê, se explica

Documento com tabela era o pior caso da leitura. Mesmo sem as barras, o que saía
era *"app, versão, o que entrou, toolbox inventory, zero ponto dois ponto três,
direção visual"* — uma fila de palavras sem a grade que as fazia significar
alguma coisa. A estrutura de uma tabela é visual; no áudio ela não existe.

Agora um modelo a transforma em explicação antes de falar:

> *"Esta tabela mostra as atualizações recentes de quatro aplicativos, listando
> suas versões e as principais novidades de cada um. O toolbox-inventory recebeu
> a direção visual da spec aplicada, o video-analyzer incorporou entradas mistas
> no dossiê…"*

Custa alguns segundos numa tabela de quatro linhas, e **zero em texto sem
tabela** — por isso vem ligado, diferente da correção de acentuação, que
custaria em todo texto.

Se o modelo devolver um número de parágrafos diferente do número de tabelas,
nenhuma é usada: sem correspondência segura, trocar a tabela errada seria dizer
ao ouvinte um conteúdo que não está ali. Falhando, volta a leitura célula a
célula — ruim de ouvir, mas é o conteúdo.

## O vocabulário chegou ao reconhecedor

`vocabulary` e `custom_instructions` eram guardados e nunca enviados — não havia
onde. O `/v1/audio/transcriptions` aceita `prompt`, e é ele que ensina nome
próprio ao modelo: sem isso "Traefik" volta como "trafic" toda vez.

Os termos vão primeiro e as instruções depois, porque o campo tem teto de tamanho
no servidor e o que for cortado deve ser a prosa, que ajuda menos que a lista de
palavras. Só vale para os modelos remotos: o Vosk local não tem onde encaixar
contexto.

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

### Cada papel lembra o próprio lugar

A janela é uma só, mas o ditado (barra) e a leitura (coluna) guardam posições
separadas: `hud_position_dictation` pelo canto superior esquerdo e
`hud_position_reading` pela **borda direita**, que não muda quando a legenda
abre. Antes havia um campo só, e três coisas o misturavam:

- a posição atual passava de um papel para o outro quando a janela já estava
  visível — uma leitura na tela entregava o lugar dela ao ditado seguinte;
- o `Moved` que o próprio app causa com `set_position` era gravado como se fosse
  arraste. Agora `shape_hud` marca uma janela de 450 ms em que esses eventos são
  ignorados;
- o painel gravava o `Settings` que tinha carregado ao abrir, com a posição
  velha. O `save_settings` passou a manter as posições vigentes.

Posição guardada num monitor que não está mais ligado é descartada na hora de
mostrar, e o widget volta ao padrão do papel em vez de abrir fora da tela. E se
a própria consulta ao monitor atual falhar — o caso comum quando a topologia de
telas mudou desde a última vez que a janela apareceu, notebook desligado,
monitor trocado de porta — cair para o monitor principal, e depois para o
primeiro disponível, evita o mesmo defeito por outro caminho: o widget preso nas
coordenadas de uma tela que não existe mais.

### O widget que parava de aparecer

Duas causas diferentes produziam o mesmo sintoma — o ditado colava, a leitura
falava, e nenhum widget aparecia — e as duas moram em `presence.rs`.

**A primeira: o WebView2 parava de pintar.** Medido com o widget sumido, a
janela estava visível, por cima de tudo, no lugar e no tamanho certos, e mesmo
assim a captura daquele retângulo vinha vazia. A página estava viva; só não
pintava. O Tauri esconde a janela nativa sem avisar o controlador do WebView2,
e quem decide se a página desenha é a detecção de oclusão do Chromium — que no
bloqueio de tela e na suspensão marca tudo como encoberto, e uma janela que
estava **escondida** nesse momento pode voltar sem ser reavaliada.

A saída, em três partes: `CalculateNativeWinOcclusion` desligado em todas as
janelas (precisa ser idêntico em todas — elas dividem o mesmo ambiente do
WebView2); `reveal`/`conceal` avisam o controlador diretamente
(`SetIsVisible`); e a cada vez que o HUD aparece, a página confirma que pintou
de dentro de um `requestAnimationFrame` — sem resposta a tempo, a visibilidade é
sacudida de novo (duas tentativas, e por fim a página é recarregada de
verdade, que reconstrói a composição do zero em vez de só reafirmar uma flag).

**A segunda: a confirmação só rodava na hora de aparecer.** Um relato em uso
real mostrou o widget sumindo **depois** de já estar visível há um tempo —
sem transição de escondido pra visível, nada disparava a checagem de novo, e o
widget ficava "visível e vazio" até a pessoa reiniciar o app na mão. Um vigia
(`watch_hud`) passou a rodar a mesma checagem a cada 15 segundos enquanto o HUD
estiver visível, e não só no instante em que `reveal()` é chamado.

**De quebra**, `alwaysOnTop` tinha o mesmo defeito de fundo: nasce ligado, mas
o Windows tira o topo do z-order de quem tinha quando outra janela também
topmost sobe (um vídeo em tela cheia numa aba, um instalador, um UAC), e só
reafirmar ao mudar de forma deixava o mesmo buraco. O vigia reafirma isso
também, a cada 3 segundos — é uma chamada síncrona do Win32, quase de graça,
então não precisa esperar a checagem de pintura, mais cara.

Havia também uma corrida: o ditado escondia o widget 2 s depois do "Copiado"
sem olhar se ele ainda era dele, e derrubava uma leitura ou um ditado que
tivesse começado nesse intervalo. Agora ele só esconde se ninguém pediu o widget
de novo (`hud_epoch`), e esconder respeita o papel: cancelar um ditado não tira a
leitura da tela, e parar a leitura não tira o ditado.

## A onda

A referência do Raycast faz poll a 50 ms e desenha o valor cru com uma transição
de 20 ms. Copiado ao pé da letra, o resultado é uma onda nervosa: a barra assenta
em 20 ms e fica parada os outros 30, o que se vê é um degrau piscando.

Aqui a altura passa por uma média móvel exponencial antes de ir para a tela, com
ataque mais rápido que a queda — 0,55 subindo, 0,82 descendo. Subir devagar faria
a onda parecer atrasada em relação à voz; descer devagar é o que dá o decaimento
natural. A transição da barra cobre o intervalo inteiro do poll (130 ms), então o
movimento nunca para.

## A identidade do microfone

`cpal`, a biblioteca de captura, só expõe o **nome amigável** de cada
dispositivo — não um ID estável. Isso é péssimo para guardar como preferência:
o Windows embute um índice de desambiguação no nome
(`"Fifine microfone (3- fifine Microphone)"`) que muda sozinho entre sessões, e
a pessoa também pode renomear o aparelho nas configurações de som. As duas
coisas quebram uma comparação por string — e quebravam **em silêncio**: o
código caía para o microfone padrão do sistema sem avisar nada, a forma de onda
continuava se movendo (o padrão também capta som), e a transcrição vinha vazia
porque a fala de verdade ia para outro aparelho.

A saída (`src-tauri/src/mic_id.rs`) fala direto com o WASAPI — COM cru, o mesmo
caminho que o `Device::name()` do `cpal` usa por baixo — para pegar o
`IMMDevice::GetId()` de cada microfone: um identificador que não muda enquanto
o aparelho continuar na mesma porta USB, não importa como o driver decida
nomeá-lo hoje. `settings.json` guarda esse ID, não o nome cru. Uma configuração
salva antes dessa mudança ainda funciona, pelo casamento por nome como reserva
— e é migrada para o ID na primeira subida depois da atualização.

## O backup de áudio

Cada passo entre gravar e colar o texto pode falhar sem culpa de quem ditou:
rede caiu, o back-end não respondeu, o reconhecedor local errou o julgamento de
"não teve fala", a transcrição voltou vazia ou errada. `recordings.rs` grava
cada WAV em disco **antes** de qualquer coisa que possa dar errado — inclusive
antes do próprio julgamento de "sem fala", que é o caso mais fácil de reabrir na
mão depois.

A pasta se limpa sozinha: uma gravação que ninguém marcar como "salva" expira em
24h. É uma rede de segurança contra falha, não um histórico de ditados — quem
decide que uma gravação importa é quem ditou, não o app.

## A bandeja manda no aplicativo

Dois interruptores no menu do botão direito, e os mesmos no painel:

**Vox ativo.** Desligado, ele solta os atalhos globais, para o que estiver
tocando e faz a ponte recusar os pedidos da extensão — sem essa última parte, o
navegador continuaria fazendo o computador falar com o Vox de folga.

O ícone **não sai da bandeja** em nenhum caso. Pausar é dizer "agora não", e quem
some do sistema quando se pede um intervalo obriga a ir procurar o app para
voltar. A dica do cursor passa a dizer o estado, porque é o único lugar onde ele
aparece sem abrir nada.

**Iniciar com o Windows.** Uma entrada na chave `Run` do usuário, escrita pelo
`reg.exe`. A chave em vez da pasta Inicializar porque criar um `.lnk` exigiria
COM; o `reg.exe` em vez da API do registro porque falar com ele em Rust pede o
crate `windows` inteiro para três operações de texto.

A entrada aponta para o **executável**, não para o atalho de desenvolvimento: no
boot se quer o aplicativo, não uma compilação.

A preferência é a intenção e a chave é o estado. A partida alinha as duas — elas
divergem quando alguém limpa a inicialização com um utilitário por fora.

**Reiniciar.** Um bug sem tempo de investigar resolve boa parte das vezes com
um reinício — e nem sempre vale caçar o processo no Gerenciador de Tarefas só
para isso. O item no menu e o botão no painel chamam `AppHandle::restart()` do
Tauri.

## O catálogo de comandos

Comandos globais, declarados uma vez em `src-tauri/src/commands.rs`. O
registro, a preferência e a linha no menu da bandeja derivam dessa tabela — antes
cada atalho custava as três coisas escritas à mão, e um comando novo seria o
que alguém esqueceria em uma delas.

| Comando | Padrão | |
|---|---|---|
| Ditar | `Ctrl+Shift+D` | uma vez grava, de novo entrega |
| Cancelar o ditado | `Ctrl+Shift+X` | descarta sem transcrever |
| Ler a seleção | `Ctrl+Alt+L` | e pausa/retoma durante a leitura |
| Pausar e retomar | `Ctrl+Alt+P` | nunca começa uma leitura nova |
| Parar a leitura | `Ctrl+Alt+S` | |
| Legenda guiada | `Ctrl+Alt+G` | abre e fecha o texto na pílula |
| Mostrar/esconder o widget | `Ctrl+Alt+V` | alterna; nunca começa nada |
| Preferências | `Ctrl+Alt+O` | |

Duas famílias, separadas pelo que a mão está fazendo: `Ctrl+Shift` para o ditado,
que se usa enquanto se escreve, e `Ctrl+Alt` para a leitura e a janela, que se
usam enquanto se lê.

**As letras não são estéticas.** No teclado ABNT2, `AltGr` é `Ctrl+Alt`: um
atalho global em `Ctrl+Alt+Q` rouba o `/` de quem digita, e `Ctrl+Alt+E` rouba o
`€`. As letras do catálogo não produzem caractere nenhum com AltGr, e um teste
falha se alguém escolher uma que produz.

Atalho global é recurso disputado e quem registra primeiro leva — o Vox sobe
depois do navegador. Uma combinação tomada falha em silêncio, então o que não
registrar aparece marcado no painel, ao lado do campo que a causou, e no menu da
bandeja. Campo vazio é escolha: significa "sem atalho".

**Quais estão livres se responde medindo**, não supondo:

```powershell
pwsh -File scripts\atalhos-livres.ps1
```

Ele pergunta ao Windows com a mesma `RegisterHotKey` que o plugin usa por baixo:
registra, anota e desregistra na hora, sem deixar nada preso e sem disparar a
ação de quem já tem a tecla.

Um aviso sobre esse tipo de medição: **rode com o Vox fechado.** Com ele no ar,
o próprio Vox aparece como "ocupando" as combinações que ele mesmo registrou —
o medidor contaminando a medida.

O botão **Aplicar**, na seção de atalhos, solta tudo e registra de novo. Vale
para a combinação nova passar a valer sem reiniciar, e para tentar outra vez uma
que estava tomada: a disputa muda no minuto em que o outro programa fecha.

## Formas de acionar

| Caminho | Onde funciona |
|---|---|
| `Ctrl+Shift+D` / `Ctrl+Alt+L` | qualquer app |
| Bandeja → *Ler a área de transferência* | qualquer app |
| Extensão: botão direito → *Ler com o Vox*, ou `Alt+Shift+L` | navegadores Chromium |

A extensão está em `extension/` e é a única que **destaca o texto na própria
página**, sem abrir janela. Ela fala com o Vox por um servidor local em
`127.0.0.1:8765` — ver `extension/README.md` para instalação e para o modelo de
ameaça, que não é decorativo: loopback é alcançável por qualquer página aberta,
e por isso a checagem de origem e o token acontecem no servidor, antes de agir.

## Estrutura

```
vox/
├── assets/sounds/       os quatro .wav, 48 kHz 24-bit
├── scripts/             atalho de desenvolvimento e diagnóstico
│   ├── vox-dev.ps1      compila se preciso, encerra o antigo e sobe
│   ├── vox-dev.vbs      abre o launcher sem piscar console
│   ├── instalar-atalho.ps1
│   └── atalhos-livres.ps1  quais combinações globais estão livres
├── src/                 o front — uma janela por arquivo .html
│   ├── hud.html         a barra do ditado e a coluna da leitura
│   ├── reader.html      a janela de leitura, com transporte e destaque
│   ├── live.html        o texto reconhecido ao vivo (opcional)
│   ├── settings.html    o painel de preferências
│   ├── onboarding.html  o assistente de primeiro uso
│   ├── css/             uma folha por janela
│   └── js/              um módulo por janela
└── src-tauri/src/
    ├── main.rs          atalhos, comandos, partida
    ├── audio.rs         captura, reamostragem, níveis, downmix
    ├── mic_id.rs         identidade estável do microfone (WASAPI)
    ├── api.rs           cliente da speech-api, fatiador
    ├── dictation.rs     o ciclo do ditado, o HUD, a posição do widget
    ├── reading.rs       a fila de leitura e a dianteira
    ├── recordings.rs    backup local de áudio, com expurgo de 24h
    ├── presence.rs      visibilidade e "sempre por cima" do widget
    ├── paste.rs         colagem, capitalização, palavra-chave
    ├── sounds.rs        retorno sonoro
    ├── stt_stream.rs    transcrição incremental por WebSocket
    ├── tray.rs          ícone da bandeja e estado dos atalhos
    ├── autostart.rs     subir junto com o Windows
    └── config.rs        preferências, migração, ambiente
```

## Limites herdados

Da API e do desenho de referência:

| | |
|---|---|
| Vocabulário | 50 itens, 50 caracteres cada, únicos |
| Velocidade da voz | 0,5× a 2,0× |
| Poll dos níveis | 50 ms, com transição de 130 ms na barra |
| HUD após sucesso | 2000 ms |
| Espera antes do envio | 50 ms |
| Backup de áudio | expira em 24h, a menos que marcado como salvo |

## O que ainda não existe

Está em uso diário e o que está aqui funciona, mas nem tudo o que a configuração
promete está ligado:

| | |
|---|---|
| Instalador | não há MSI publicado; a distribuição ainda é o binário compilado no lugar |
| Destaque fora do navegador | só a extensão acompanha o texto na tela; em outros apps abre-se a janela de leitura |
| Outros provedores de voz | o cliente só fala o contrato de uma API própria — ver "O servidor não é embutido" |

Nada disso quebra o uso: ditado e leitura funcionam ponta a ponta.

**Duas preferências foram removidas**, pelo mesmo critério. `mute_while_recording`
nunca teve uma linha de código atrás dela. `context_aware_paste` ligava e
desligava um ajuste que depende de saber o que está antes do cursor — e ler isso
exige UI Automation, que ainda não existe aqui; o ajuste continua no código e
passa a valer sozinho quando essa leitura chegar.
