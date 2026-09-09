/* Controle do HUD.
 *
 * O front não decide nada sozinho: o backend manda o estado por evento e este
 * módulo desenha. A única coisa que ele faz por conta própria é o poll dos
 * níveis a 50 ms enquanto grava — puxar em vez de receber evita inundar a ponte
 * do Tauri com 20 mensagens por segundo.
 */

/* Sem console nesta janela: um erro de JavaScript some sem deixar rastro, e a
   janela fica com a forma desenhada e nada dentro — foi exatamente o que
   aconteceu quando o CSP bloqueou o bootstrap do Tauri. Aqui a falha vira
   texto na própria tela. */
function mostrarFalha(motivo) {
  const alvo = document.body;
  if (!alvo || alvo.dataset.falhou === "true") return;
  alvo.dataset.falhou = "true";
  const caixa = document.createElement("div");
  caixa.setAttribute("style",
    "position:fixed;inset:0;z-index:9999;padding:10px;overflow:auto;" +
    "background:#2a1113;color:#ffb4b4;font:11px/1.4 ui-monospace,monospace;" +
    "white-space:pre-wrap;word-break:break-word");
  caixa.textContent = "Vox — falha no front:\n\n" + motivo;
  alvo.appendChild(caixa);
}

window.addEventListener("error", (evento) => {
  mostrarFalha(String(evento.message) + "\n" + (evento.filename || "") + ":" + evento.lineno);
});
window.addEventListener("unhandledrejection", (evento) => {
  mostrarFalha("promessa rejeitada: " + String(evento.reason));
});

if (!window.__TAURI__) {
  mostrarFalha(
    "window.__TAURI__ não existe.\n\n" +
    "O bootstrap de IPC não carregou. Causa mais comum: a CSP em " +
    "tauri.conf.json sem `script-src 'self'` — sem essa diretiva o Tauri não " +
    "consegue injetar o nonce do próprio script."
  );
  throw new Error("bootstrap do Tauri ausente");
}

const { invoke } = window.__TAURI__.core;
const { listen, emit } = window.__TAURI__.event;

/* Os tempos vêm do backend: duplicá-los aqui seria dois lugares para mudar o
   mesmo número, e divergir sem ninguém perceber. Os valores abaixo são só o
   fallback para o instante entre carregar a página e a resposta chegar. */
const timings = { pollMs: 50, successHoldMs: 2000, barsNormal: 37, barsPushToTalk: 37 };

invoke("hud_timings")
  .then((values) => {
    Object.assign(timings, values);
    desenharRepouso();
  })
  .catch((error) => {
    // Antes isto era um `console.warn` — e numa janela sem console a falha
    // sumia. O HUD ficava com a forma desenhada e nada dentro, sem nenhuma
    // pista de que a ponte com o backend estava quebrada.
    mostrarFalha("invoke('hud_timings') falhou:\n" + String(error));
  });

const hud = document.getElementById("hud");
const content = document.getElementById("content");
const variant = document.getElementById("variant");
const sparkles = document.getElementById("sparkles");
const player = document.getElementById("player");

const icons = {
  cancel: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="M6 6l8 8M14 6l-8 8"/></svg>',
  accept: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M5 10.5l3.5 3.5L15 7"/></svg>',
  play:   '<svg viewBox="0 0 20 20" fill="currentColor"><path d="M6 4l10 6-10 6z"/></svg>',
  pause:  '<svg viewBox="0 0 20 20" fill="currentColor"><rect x="6" y="4.5" width="3" height="11" rx="1"/><rect x="11" y="4.5" width="3" height="11" rx="1"/></svg>',
  stop:   '<svg viewBox="0 0 20 20" fill="currentColor"><rect x="5.5" y="5.5" width="9" height="9" rx="1.6"/></svg>',
};

let pollHandle = null;
let timerHandle = null;
let elapsedSeconds = 0;
/* Alturas no instante em que a gravação para — a animação de processar parte
   delas, e é isso que faz a onda "derreter" em vez de saltar. */
let lastHeights = [];
function formatTime(totalSeconds) {
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = Math.floor(totalSeconds % 60);
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

function clearHudFlags() {
  hud.removeAttribute("data-stopping");
  hud.removeAttribute("data-success");
  hud.removeAttribute("data-error");
  hud.removeAttribute("data-push-to-talk");
  hud.removeAttribute("data-playing");
  variant.removeAttribute("data-variant");
  sparkles.hidden = true;
}

function stopTimers() {
  clearInterval(pollHandle);
  clearInterval(timerHandle);
  pollHandle = null;
  timerHandle = null;
}

function fillSparkles() {
  const seeds = [
    { x: 12, y: 24, size: 5, opacity: 0.75, duration: 2.4, delay: 0 },
    { x: 31, y: 70, size: 4, opacity: 0.55, duration: 3.1, delay: 0.45 },
    { x: 50, y: 18, size: 6, opacity: 0.85, duration: 2.7, delay: 0.9 },
    { x: 70, y: 72, size: 4, opacity: 0.5,  duration: 3.4, delay: 0.3 },
    { x: 88, y: 32, size: 5, opacity: 0.7,  duration: 2.9, delay: 1.15 },
  ];
  sparkles.innerHTML = "";
  for (const seed of seeds) {
    const node = document.createElement("span");
    node.className = "sparkle";
    node.style.setProperty("--sparkle-x", `${seed.x}%`);
    node.style.setProperty("--sparkle-y", `${seed.y}%`);
    node.style.setProperty("--sparkle-size", `${seed.size}px`);
    node.style.setProperty("--sparkle-opacity", seed.opacity);
    node.style.setProperty("--sparkle-duration", `${seed.duration}s`);
    node.style.setProperty("--sparkle-delay", `${seed.delay}s`);
    sparkles.appendChild(node);
  }
}

function renderRecording(barCount, pushToTalk) {
  const bars = Array.from(
    { length: barCount },
    (unused, index) => `<span class="wf-bar" style="--i:${index}; height:2px"></span>`,
  ).join("");

  if (pushToTalk) {
    content.innerHTML =
      `<div class="hud-lane"><div class="hud-waveform"><div class="wf">${bars}</div></div></div>`;
    return;
  }

  content.innerHTML =
    `<button class="hud-btn" type="button" data-action="cancel" aria-label="Cancelar">${icons.cancel}</button>` +
    `<div class="hud-lane">` +
      `<div class="hud-waveform"><div class="wf">${bars}</div></div>` +
      `<div class="hud-timer">${formatTime(elapsedSeconds)}</div>` +
    `</div>` +
    `<button class="hud-btn" type="button" data-action="finish" aria-label="Finalizar">${icons.accept}</button>`;
}

function renderStopping(barCount) {
  const bars = Array.from({ length: barCount }, (unused, index) => {
    const height = Math.max(2, lastHeights[index] ?? 2);
    return `<span class="wf-bar" style="--i:${index}; --initial-h:${height.toFixed(2)}; --initial-opacity:1"></span>`;
  }).join("");
  content.innerHTML = `<div class="hud-lane"><div class="hud-stopping">${bars}</div></div>`;
}

function renderSuccess(title) {
  content.innerHTML =
    `<div class="hud-success">${icons.accept}<span>${title ?? "Pronto"}</span></div>`;
}

function renderMessage(title, message, color) {
  content.innerHTML =
    `<div class="hud-err">` +
      `<span class="dot" style="background:${color}"></span>` +
      `<div class="hud-err-details">` +
        `<span class="hud-err-title">${title}</span>` +
        (message ? `<span class="hud-err-msg">${message}</span>` : "") +
      `</div>` +
    `</div>`;
}

/* Altura útil da barra, em pixels acima do mínimo de 2. A faixa de gravação
   tem 28px; 22 deixa a onda respirar sem encostar nas bordas. */
const ALTURA_MAXIMA = 22;
const SUAVIZACAO_SUBIDA = 0.55;
const SUAVIZACAO_DESCIDA = 0.82;

async function pollLevels() {
  let snapshot;
  try {
    snapshot = await invoke("audio_levels");
  } catch {
    return; // uma leitura perdida não vale interromper o desenho
  }

  if (snapshot.phase !== "recording") {
    return;
  }

  const bars = content.querySelectorAll(".hud-waveform .wf-bar");
  if (bars.length !== snapshot.levels.length) {
    return; // o desenho ainda não acompanhou a mudança de barCount
  }

  /* Suavização temporal.
   *
   * O nível cru salta muito entre uma leitura e outra, e desenhado direto a
   * onda fica nervosa — pisca em vez de ondular. A média móvel exponencial
   * deixa cada barra perseguir o valor novo em vez de pular para ele: o
   * traçado ganha inércia e lê como respiração, não como ruído.
   *
   * SUAVIZACAO é quanto do valor antigo permanece. Mais alto, mais lento e
   * mais macio; mais baixo, mais reativo. Com 0,72 e poll de 50 ms a onda leva
   * ~150 ms para acompanhar uma mudança brusca de volume, que é perto do que o
   * ouvido espera ver. */
  bars.forEach((bar, index) => {
    const alvo = 2 + (snapshot.levels[index] ?? 0) * ALTURA_MAXIMA;
    const anterior = lastHeights[index] ?? 2;
    // Sobe mais rápido do que desce: ataque preguiçoso faz a onda parecer
    // atrasada em relação à voz, enquanto a queda lenta é o que dá o efeito de
    // decaimento natural.
    const fator = alvo > anterior ? SUAVIZACAO_SUBIDA : SUAVIZACAO_DESCIDA;
    const suave = anterior * fator + alvo * (1 - fator);
    lastHeights[index] = suave;
    bar.style.height = `${suave.toFixed(1)}px`;
  });
}

function startRecording(payload) {
  stopTimers();
  clearHudFlags();
  player.hidden = true;
  hud.hidden = false;

  const pushToTalk = Boolean(payload.pushToTalk);
  const barCount = pushToTalk ? timings.barsPushToTalk : timings.barsNormal;

  hud.setAttribute("data-dictation", "");
  hud.removeAttribute("data-reading");
  if (pushToTalk) hud.setAttribute("data-push-to-talk", "");

  elapsedSeconds = 0;
  lastHeights = new Array(barCount).fill(2);
  renderRecording(barCount, pushToTalk);

  timerHandle = setInterval(() => {
    elapsedSeconds += 1;
    const node = content.querySelector(".hud-timer");
    if (node) node.textContent = formatTime(elapsedSeconds);
  }, 1000);

  pollHandle = setInterval(pollLevels, timings.pollMs);
}

function startTranscribing() {
  stopTimers();
  clearHudFlags();
  hud.setAttribute("data-stopping", "");
  sparkles.hidden = false;
  fillSparkles();
  renderStopping(lastHeights.length || timings.barsNormal);
}

/* Desenho de repouso: a onda parada, sem timer nem botões.
   O HUD nunca fica oco — uma caixa vazia não comunica nada, e foi o que
   apareceu no primeiro teste com a janela visível. */
function desenharRepouso() {
  stopTimers();
  clearHudFlags();
  hud.setAttribute("data-dictation", "");
  hud.removeAttribute("data-reading");
  player.hidden = true;
  hud.hidden = false;

  const barras = Array.from(
    { length: timings.barsNormal },
    (unused, indice) => `<span class="wf-bar" style="--i:${indice}; height:2px"></span>`,
  ).join("");
  content.innerHTML =
    `<div class="hud-lane"><div class="hud-waveform"><div class="wf">${barras}</div></div></div>`;
}

/* --- estados vindos do backend --- */
listen("vox://hud", (event) => {
  const payload = event.payload ?? {};

  switch (payload.state) {
    case "idle":
      desenharRepouso();
      break;

    case "recording":
      startRecording(payload);
      break;

    case "transcribing":
      startTranscribing();
      break;

    case "success":
      stopTimers();
      clearHudFlags();
      hud.setAttribute("data-success", "");
      variant.setAttribute("data-variant", "success");
      renderSuccess(payload.title);
      break;

    case "warning":
      stopTimers();
      clearHudFlags();
      hud.setAttribute("data-error", "");
      variant.setAttribute("data-variant", "warning");
      renderMessage(payload.title ?? "Atenção", payload.message, "var(--yellow)");
      break;

    case "error":
      stopTimers();
      clearHudFlags();
      hud.setAttribute("data-error", "");
      variant.setAttribute("data-variant", "failure");
      renderMessage(payload.title ?? "Falhou", payload.message, "var(--red)");
      break;

    default:
      break;
  }
});

const playerIcons = {
  play:  '<svg viewBox="0 0 20 20" fill="currentColor"><path d="M6.5 4l9 6-9 6z"/></svg>',
  pause: '<svg viewBox="0 0 20 20" fill="currentColor"><rect x="6" y="4.5" width="3" height="11" rx="1"/><rect x="11" y="4.5" width="3" height="11" rx="1"/></svg>',
  back:  '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5 6 10l5 5"/><path d="M15 5l-5 5 5 5"/></svg>',
  ahead: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M9 5l5 5-5 5"/><path d="M5 5l5 5-5 5"/></svg>',
  stop:  '<svg viewBox="0 0 20 20" fill="currentColor"><rect x="5.5" y="5.5" width="9" height="9" rx="1.8"/></svg>',
  details: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="6.5"/><path d="M10 13.5V9.5"/><path d="M10 6.8v.01"/></svg>',
  captions: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="M3.5 6h13"/><path d="M3.5 10h9"/><path d="M3.5 14h11"/></svg>',
};

const RING_CIRCUMFERENCE = 2 * Math.PI * 17;

/* ---------------------------------------------------------------- legenda */

/* Quanto a janela leva para crescer e encolher. Tem que bater com a transição
   de `.player-captions` no CSS: a janela é o recipiente e o card é o desenho,
   e se um chegar antes do outro o texto aparece cortado. */
const CAPTIONS_ANIMATION_MS = 420;

let captionsEnabled = false;
/* Os trechos que o backend planejou ler, e os elementos que os desenham. */
let captionSegments = [];
let captionIndex = -1;
/* Ordem da palavra destacada dentro do trecho corrente. Guardada para o realce
   reaparecer no lugar certo quando a legenda é aberta no meio da leitura. */
let captionWord = -1;
/* A pessoa rolou o texto com a roda: o acompanhamento automatico para ate ela
   voltar a tocar, ou ate passar o tempo de espera. */
let captionUserScrolled = false;
let captionScrollHandle = null;

/* A preferência é lida uma vez, na partida: a pílula precisa nascer do tamanho
   certo na primeira leitura, e não crescer depois que ela já apareceu. */
invoke("get_settings")
  .then((settings) => { captionsEnabled = Boolean(settings?.reading_captions); })
  .catch(() => {});

/* Quebra a frase em palavras com a fração em que cada uma começa e termina.
 *
 * O mesmo cálculo por caractere que a janela de leitura usa. As duas mostram a
 * mesma fala ao mesmo tempo, e se discordassem sobre onde ela está o olho
 * perceberia na hora — é o tipo de divergência que só aparece em uso. */
function buildCaptionSentence(text, index) {
  const sentence = document.createElement("span");
  sentence.className = "cap-sentence";
  sentence.dataset.index = String(index);
  sentence.setAttribute("role", "button");
  sentence.tabIndex = 0;
  sentence.setAttribute("aria-label", `Ouvir a partir do trecho ${index + 1}`);

  const totalChars = text.length || 1;
  const pieces = text.split(/(\s+)/);
  let cursor = 0;

  for (const piece of pieces) {
    if (!piece) continue;
    if (/^\s+$/.test(piece)) {
      sentence.appendChild(document.createTextNode(piece));
      cursor += piece.length;
      continue;
    }
    const word = document.createElement("span");
    word.className = "cap-word";
    word.textContent = piece;
    word.dataset.from = String(cursor / totalChars);
    word.dataset.to = String((cursor + piece.length) / totalChars);
    sentence.appendChild(word);
    cursor += piece.length;
  }

  sentence.appendChild(document.createTextNode(" "));
  return sentence;
}

function renderCaptions(texts) {
  const alvo = document.getElementById("captionsText");
  captionSegments = [];
  captionIndex = -1;
  captionWord = -1;
  if (!alvo) return;

  alvo.innerHTML = "";
  // O realce é um elemento só, que anda por baixo das palavras. Pintar o fundo
  // de cada palavra faria o destaque piscar de uma para a outra: some aqui,
  // aparece ali. Sendo um retângulo que se move, ele escorrega.
  const cursor = document.createElement("span");
  cursor.className = "cap-cursor";
  cursor.id = "capCursor";
  alvo.appendChild(cursor);

  for (const [index, text] of texts.entries()) {
    const sentence = buildCaptionSentence(text, index);
    alvo.appendChild(sentence);
    captionSegments.push({ element: sentence, words: sentence.querySelectorAll(".cap-word") });
  }
  alvo.scrollTop = 0;
  ultimoTopoDoCursor = null;
}

/* Onde o realce estava na última vez, para saber se a próxima palavra está na
   mesma linha. */
let ultimoTopoDoCursor = null;

/* Leva o realce até a palavra.
 *
 * Dentro da linha ele desliza — é o movimento que a leitura tem, e o olho
 * acompanha sem precisar reencontrar o destaque a cada palavra. Na quebra de
 * linha ele salta: escorregar da última palavra de uma linha até a primeira da
 * seguinte é uma diagonal atravessando o texto inteiro, que chama mais atenção
 * do que a palavra que ela deveria apontar. */
/* Como o realce se ajusta à palavra: sobra dos lados, aperto em cima e embaixo.
 *
 * Os dois sentidos pedem coisas opostas. Na horizontal, a palavra precisa de ar
 * — sem ele o retângulo termina rente à última letra e parece apertado. Na
 * vertical acontece o contrário: `offsetHeight` de um trecho em linha é a caixa
 * da fonte inteira, que inclui espaço de ascendente e descendente que nenhuma
 * letra ocupa, e o realce fica com quase a altura da linha. Encolher aproxima o
 * retângulo do desenho da palavra, que é o que ele deve marcar. */
const CAPTION_CURSOR_PAD_X = 5;
const CAPTION_CURSOR_TRIM_Y = 2;

/* Largura de referência do realce, em pixels. Ela nunca muda: quem dá a largura
   real é o `scaleX`, para a animação inteira caber numa transformação só —
   ver o comentário de `.cap-cursor` no CSS. */
const CAPTION_CURSOR_BASE_WIDTH = 100;

function moveCaptionCursor(word) {
  const cursor = document.getElementById("capCursor");
  if (!cursor || !word) return;

  const topo = word.offsetTop;
  const mudouDeLinha = ultimoTopoDoCursor === null || Math.abs(topo - ultimoTopoDoCursor) > 2;

  if (mudouDeLinha) cursor.style.transition = "none";

  const largura = word.offsetWidth + CAPTION_CURSOR_PAD_X * 2;
  const escala = largura / CAPTION_CURSOR_BASE_WIDTH;

  cursor.style.transform =
    `translate(${word.offsetLeft - CAPTION_CURSOR_PAD_X}px, ${topo + CAPTION_CURSOR_TRIM_Y}px) ` +
    `scaleX(${escala})`;
  // A altura é a mesma para toda palavra (a caixa da fonte não muda), então
  // atribuí-la direto não produz salto nenhum e fica fora da animação.
  cursor.style.height = `${word.offsetHeight - CAPTION_CURSOR_TRIM_Y * 2}px`;
  cursor.dataset.visivel = "true";

  if (mudouDeLinha) {
    // Ler uma propriedade de layout força o navegador a aplicar a posição nova
    // antes de a transição voltar; sem isso ele junta as duas mudanças no mesmo
    // quadro e anima assim mesmo, que é justamente o que se quer evitar.
    void cursor.offsetWidth;
    cursor.style.transition = "";
  }

  ultimoTopoDoCursor = topo;
}

/* Apaga o realce sem apagar o texto.
 *
 * Quando a leitura termina, o destaque na última palavra deixa de significar
 * alguma coisa e passa a mentir: parece que ainda está falando ali. O texto
 * continua na tela — dá para reler o que acabou de ser lido —, mas nada mais
 * está aceso.
 *
 * Também é chamado ao começar uma leitura nova: entre o "gerando" e a chegada do
 * texto novo, o que está desenhado ainda é o da leitura anterior, com o realce
 * parado na última palavra dela. */
function clearCaptionCursor() {
  const cursor = document.getElementById("capCursor");
  if (cursor) cursor.dataset.visivel = "false";

  const segment = captionSegments[captionIndex];
  if (segment) segment.words.forEach((palavra) => { palavra.dataset.speaking = "false"; });

  captionWord = -1;
  // A próxima aparição salta para o lugar certo em vez de escorregar de onde
  // parou — o realce não atravessa o texto inteiro para voltar ao começo.
  ultimoTopoDoCursor = null;
}

/* Qual palavra está soando — vem pronta da janela de leitura.
 *
 * Antes vinha a fração do trecho e cada janela reconstruía a divisão do texto
 * por conta própria; bastava um espaço a mais para as duas discordarem sobre
 * qual palavra é a atual. Aqui a divisão é a mesma (o texto é o mesmo, a quebra
 * por espaço é a mesma), mas o índice não deixa margem: quem manda é quem tem o
 * áudio. */
function updateCaptions(index, word) {
  if (!captionsEnabled || index == null || index < 0) return;
  const segment = captionSegments[index];
  if (!segment) return;

  if (index !== captionIndex) {
    const anterior = captionSegments[captionIndex];
    if (anterior) {
      anterior.element.dataset.current = "false";
      anterior.words.forEach((palavra) => { palavra.dataset.speaking = "false"; });
    }
    captionIndex = index;
    segment.element.dataset.current = "true";
  }

  const ordem = Number(word);
  if (!Number.isInteger(ordem) || ordem < 0) return;
  captionWord = ordem;

  segment.words.forEach((palavra, posicao) => {
    palavra.dataset.speaking = posicao === ordem ? "true" : "false";
  });

  const atual = segment.words[ordem];
  if (!atual) return;
  moveCaptionCursor(atual);
  keepWordVisible(atual);
}

/* Mantém a palavra falada sempre à vista.
 *
 * Rolar por frase não bastava: uma frase longa ocupa mais linhas do que o card
 * mostra, e a fala saía por baixo enquanto o texto ficava parado. Quem manda na
 * rolagem é a palavra corrente.
 *
 * A faixa confortável exclui as bordas de propósito. Rolar assim que a palavra
 * encosta no limite deixaria a leitura sempre na última linha visível, sem nada
 * à frente; com a folga, o texto anda em blocos e sobra contexto dos dois lados.
 * E só rola quando de fato saiu da faixa — chamar `scrollTo` a cada palavra faz
 * uma rolagem suave interromper a anterior, e o resultado é um tremor
 * permanente. */
const CAPTION_SAFE_TOP = 0.30;
const CAPTION_SAFE_BOTTOM = 0.70;
/* Onde a palavra e recolocada quando precisa rolar: um pouco acima do meio, para
   sobrar mais do que vem do que do que ja passou. */
const CAPTION_SCROLL_TARGET = 0.38;

function keepWordVisible(word, forcar = false) {
  const alvo = document.getElementById("captionsText");
  if (!alvo) return;
  if (captionUserScrolled && !forcar) return;

  const altura = alvo.clientHeight;
  const relativo = word.offsetTop - alvo.scrollTop;
  const dentro =
    relativo >= altura * CAPTION_SAFE_TOP &&
    relativo + word.offsetHeight <= altura * CAPTION_SAFE_BOTTOM;
  if (dentro && !forcar) return;

  const destino = word.offsetTop - altura * CAPTION_SCROLL_TARGET;
  alvo.scrollTo({ top: Math.max(0, destino), behavior: "smooth" });
}

/* Rolar com a roda suspende o acompanhamento automatico.
 *
 * Ler um trecho anterior enquanto a voz continua e um uso legitimo, e brigar com
 * quem esta rolando seria pior do que perder o automatismo por um tempo. Voltar
 * a tocar traz o texto de volta para a palavra falada, que e onde a pessoa quer
 * estar quando aperta play. */
function suspendCaptionAutoScroll() {
  captionUserScrolled = true;
  clearTimeout(captionScrollHandle);
  captionScrollHandle = setTimeout(() => {
    captionUserScrolled = false;
    // Só recupera o foco enquanto há reprodução. Em pausa, a pessoa está
    // navegando o texto deliberadamente e o scroll deve ficar onde ela deixou.
    if (ultimoEstadoDoPlayer === "playing") resumeCaptionAutoScroll();
  }, 2500);
}

function resumeCaptionAutoScroll() {
  captionUserScrolled = false;
  clearTimeout(captionScrollHandle);

  const segment = captionSegments[captionIndex];
  const word = segment?.words[captionWord];
  if (word) keepWordVisible(word, true);
}

/* Liga e desliga a legenda.
 *
 * A ordem entre a janela e o card não é simétrica de propósito. Ao abrir, a
 * janela precisa já ter o tamanho final antes de o card crescer, senão o texto
 * nasce cortado pela borda. Ao fechar, a janela só pode encolher depois que a
 * animação terminou, pelo mesmo motivo ao contrário.
 *
 * O `await` no comando não é formalidade: o redimensionamento acontece do outro
 * lado da ponte, e disparar a animação sem esperar por ele fazia o card crescer
 * dentro da janela antiga por alguns quadros. */
async function setCaptions(enabled) {
  captionsEnabled = enabled;
  const botao = document.getElementById("playerCaptionsToggle");
  if (botao) botao.dataset.on = String(enabled);

  if (enabled) {
    try {
      await invoke("set_reading_captions", { enabled: true });
    } catch {
      // A janela não cresceu; abrir o card assim mesmo mostraria texto cortado.
      captionsEnabled = false;
      if (botao) botao.dataset.on = "false";
      return;
    }
    requestAnimationFrame(() => { player.dataset.captions = "on"; });
    updateCaptions(captionIndex, captionWord);
  } else {
    player.dataset.captions = "off";
    encolherJanelaQuandoFechar();
  }
}

/* Encolhe a janela quando o card terminou de fechar — e não quando o relógio
 * diz que deveria ter terminado.
 *
 * Um `setTimeout` com a mesma duração da transição erra por alguns
 * milissegundos: a animação começa no quadro seguinte ao atributo, então ela
 * ainda tem um resto quando o tempo acaba, e a janela encolhe por cima de um
 * card que ainda é largo — um quadro cortado, que é o piscar. O evento do
 * próprio navegador não erra.
 *
 * O tempo continua aqui como rede de segurança: com movimento reduzido a
 * transição não existe e o evento nunca chega. */
function encolherJanelaQuandoFechar() {
  const painel = player.querySelector(".player-captions");
  let encolhida = false;

  const encolher = () => {
    if (encolhida) return;
    encolhida = true;
    painel?.removeEventListener("transitionend", aoTerminar);
    invoke("set_reading_captions", { enabled: false }).catch(() => {});
  };

  function aoTerminar(event) {
    if (event.propertyName !== "width") return;
    // Um quadro de folga: o `transitionend` chega antes de o navegador ter
    // pintado o estado final.
    requestAnimationFrame(encolher);
  }

  painel?.addEventListener("transitionend", aoTerminar);
  setTimeout(encolher, CAPTIONS_ANIMATION_MS + 140);
}

/* ==========================================================================
   PLAYER DA LEITURA — pílula vertical
   ==========================================================================

   O HUD horizontal desenha nível de microfone real durante o ditado. Na leitura
   não há nível nenhum para mostrar: a onda ali era decoração, e ocupava o espaço
   que o transporte precisa.

   Esta coluna substitui aquilo. O áudio toca na janela de leitura, que é dona da
   linha do tempo; aqui fica o controle rápido — e, quando a legenda está ligada,
   o texto que está sendo falado, ao lado. */

function renderPlayer(state) {
  hud.hidden = true;
  player.hidden = false;
  player.dataset.state = state;
  // Sem transição no primeiro desenho: a pílula já aparece do tamanho que a
  // janela tem, porque quem escolheu a forma foi o backend antes de mostrá-la.
  player.dataset.captions = captionsEnabled ? "on" : "off";

  player.innerHTML =
    `<div class="player-captions"><div class="captions-text" id="captionsText"></div></div>` +
    `<div class="player-controls">` +
      `<span class="player-time" id="playerTime" title="Abrir a janela de leitura">0:00</span>` +
      `<div class="player-main">` +
        `<svg class="player-ring" viewBox="0 0 38 38">` +
          `<circle class="rail"></circle>` +
          `<circle class="value" id="playerRing" ` +
            `stroke-dasharray="${RING_CIRCUMFERENCE}" stroke-dashoffset="${RING_CIRCUMFERENCE}"></circle>` +
        `</svg>` +
        `<button class="player-play" id="playerPlay" type="button" aria-label="Pausar">${playerIcons.pause}</button>` +
      `</div>` +
      `<div class="player-jumps">` +
        `<button class="player-btn" data-jump="-15" type="button" aria-label="Voltar 15 segundos">${playerIcons.back}</button>` +
        `<button class="player-btn" data-jump="15" type="button" aria-label="Avançar 15 segundos">${playerIcons.ahead}</button>` +
      `</div>` +
      `<button class="player-speed" id="playerSpeed" type="button" aria-label="Velocidade">1×</button>` +
      `<button class="player-btn" id="playerCaptionsToggle" type="button" ` +
        `data-on="${captionsEnabled}" aria-label="Mostrar o texto sendo lido">${playerIcons.captions}</button>` +
      `<span class="player-divider"></span>` +
      `<button class="player-btn" id="playerStop" type="button" aria-label="Cancelar leitura">${playerIcons.stop}</button>` +
    `</div>`;
}

function updatePlayerProgress(elapsed, total) {
  const time = document.getElementById("playerTime");
  const ring = document.getElementById("playerRing");
  if (!time || !ring) return;

  const minutes = Math.floor(elapsed / 60);
  const seconds = Math.floor(elapsed % 60);
  time.textContent = `${minutes}:${String(seconds).padStart(2, "0")}`;

  const ratio = total > 0 ? Math.min(1, elapsed / total) : 0;
  ring.setAttribute("stroke-dashoffset", String(RING_CIRCUMFERENCE * (1 - ratio)));
}

/* A janela de leitura é dona do áudio e da linha do tempo; ela publica o
   andamento e este player só reflete. Duas cópias da mesma verdade dariam
   divergência na hora de dar seek. */
let ultimoEstadoDoPlayer = null;

listen("vox://player-progress", (event) => {
  const { elapsed, total, state, speed, index, word } = event.payload ?? {};
  updatePlayerProgress(elapsed ?? 0, total ?? 0);
  updateCaptions(index, word);

  // Voltar a tocar traz o texto de volta para a palavra falada: e onde a pessoa
  // quer estar ao apertar play, mesmo que tenha rolado para outro lugar durante
  // a pausa.
  if (state === "playing" && ultimoEstadoDoPlayer !== "playing") {
    resumeCaptionAutoScroll();
  }
  if (state) ultimoEstadoDoPlayer = state;

  if (state) player.dataset.state = state;

  const play = document.getElementById("playerPlay");
  if (play) {
    const isPlaying = state === "playing";
    play.innerHTML = isPlaying ? playerIcons.pause : playerIcons.play;
    play.setAttribute("aria-label", isPlaying ? "Pausar" : "Continuar");
  }

  const speedButton = document.getElementById("playerSpeed");
  if (speedButton && speed) {
    speedButton.textContent = `${String(speed).replace(".", ",")}×`;
  }
});

/* O atalho de legenda vem pelo backend e cai na mesma função do botão: assim a
   animação, a preferência e o tamanho da janela seguem um caminho só. */
listen("vox://toggle-captions", () => {
  // Só faz sentido com o player na tela; fora da leitura não há o que legendar.
  if (player.hidden) return;
  setCaptions(!captionsEnabled);
});

/* O plano chega logo depois do estado "gerando": é o texto inteiro, antes de
   existir áudio de qualquer trecho. A legenda desenha tudo de uma vez e depois
   só move o destaque. */
listen("vox://reading-plan", (event) => {
  renderCaptions(event.payload?.segments ?? []);
});

/* O fim da leitura apaga o realce, e o começo de uma nova também: nos dois casos
   o que estava aceso deixou de valer. */
listen("vox://reading", (event) => {
  const state = event.payload?.state;
  if (state === "complete" || state === "generating" || state === "failed") {
    clearCaptionCursor();
  }
});

listen("vox://reading", (event) => {
  const state = event.payload?.state;
  if (!state) return;

  if (state === "idle") {
    player.hidden = true;
    player.innerHTML = "";
    captionSegments = [];
    captionIndex = -1;
    captionWord = -1;
    hud.hidden = false;
    return;
  }

  /* A barra do ditado sai de cena já, antes de qualquer decisão sobre o player.
     O conteúdo dela sobrevive escondido de uma sessão para a outra, e quem lia
     logo depois de ditar via o "Copiado" do ditado anterior piscar antes da voz
     começar — o DOM velho aparecendo no instante entre mostrar a janela e o
     estado novo chegar. */
  hud.hidden = true;
  stopTimers();

  /* A falha troca o conteúdo em vez de só pintar o anel de vermelho: os
     controles de transporte não têm o que operar, e deixá-los ali convida o
     clique que não faz nada. O motivo fica na janela de leitura, que tem
     espaço; aqui cabe o aviso e a saída. */
  if (state === "failed") {
    player.hidden = false;
    player.dataset.state = state;
    player.dataset.captions = "off";
    player.innerHTML =
      `<div class="player-controls">` +
        `<span class="player-failed-dot"></span>` +
        `<span class="player-failed-label">Falhou</span>` +
        `<button class="player-btn" id="playerDetails" type="button" ` +
          `aria-label="Ver o motivo">${playerIcons.details}</button>` +
        `<span class="player-divider"></span>` +
        `<button class="player-btn" id="playerStop" type="button" ` +
          `aria-label="Fechar">${playerIcons.stop}</button>` +
      `</div>`;
    return;
  }

  // Redesenha sempre que o que está lá não for o transporte. Testar só por
  // `innerHTML` vazio deixava passar o caso de uma leitura anterior ter
  // terminado em `complete` (que não limpa) e a nova nunca montar o player.
  if (!document.getElementById("playerPlay")) renderPlayer(state);
  else {
    player.hidden = false;
    player.dataset.state = state;
  }
});

/* Os controles não mexem no áudio daqui: mandam o pedido para a janela de
   leitura, que é quem tem a linha do tempo completa. */
/* A roda so tem para onde rolar dentro da legenda; o resto da pilula nao rola.
   O ouvinte fica no player porque o painel e redesenhado a cada leitura, e um
   ouvinte no elemento que some sumiria com ele. */
player.addEventListener("wheel", (event) => {
  if (event.target.closest(".captions-text")) suspendCaptionAutoScroll();
}, { passive: true });

player.addEventListener("click", (event) => {
  const caption = event.target.closest(".cap-sentence");
  if (caption) {
    emit("vox://player-command", { action: "seek-index", index: Number(caption.dataset.index) });
    return;
  }
  const jump = event.target.closest("[data-jump]");
  if (jump) {
    emit("vox://player-command", { action: "seek", seconds: Number(jump.dataset.jump) });
    return;
  }
  if (event.target.closest("#playerPlay")) {
    emit("vox://player-command", { action: "toggle" });
    return;
  }
  if (event.target.closest("#playerSpeed")) {
    emit("vox://player-command", { action: "cycle-speed" });
    return;
  }
  if (event.target.closest("#playerCaptionsToggle")) {
    setCaptions(!captionsEnabled);
    return;
  }
  if (event.target.closest("#playerStop")) {
    invoke("stop_reading").catch(() => {});
    return;
  }
  if (event.target.closest("#playerDetails")) {
    invoke("show_reader").catch(() => {});
    return;
  }
  if (event.target.closest("#playerTime")) {
    invoke("show_reader").catch(() => {});
  }
});

player.addEventListener("keydown", (event) => {
  const caption = event.target.closest(".cap-sentence");
  if (!caption || (event.key !== "Enter" && event.key !== " ")) return;
  event.preventDefault();
  emit("vox://player-command", { action: "seek-index", index: Number(caption.dataset.index) });
});

// Um widget sem moldura não é uma aba comum: duplo clique na área arrastável
// nunca pode virar maximizar/snap do Windows.
window.addEventListener("dblclick", (event) => { event.preventDefault(); });

/* Primeiro desenho, antes de qualquer resposta do backend. Se o `invoke`
   falhar, o HUD ainda mostra a onda em vez de um retângulo vazio. */
desenharRepouso();

/* --- ações dos botões --- */
content.addEventListener("click", (event) => {
  const button = event.target.closest("[data-action]");
  if (!button) return;

  if (button.dataset.action === "cancel") {
    invoke("cancel_dictation").catch(() => {});
  }
});
