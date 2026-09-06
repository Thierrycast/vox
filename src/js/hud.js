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
const timings = { pollMs: 50, successHoldMs: 2000, barsNormal: 12, barsPushToTalk: 22 };

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

/* ==========================================================================
   PLAYER DA LEITURA — pílula vertical
   ==========================================================================

   O HUD horizontal desenha nível de microfone real durante o ditado. Na leitura
   não há nível nenhum para mostrar: a onda ali era decoração, e ocupava o espaço
   que o transporte precisa.

   Esta coluna substitui aquilo. O áudio continua tocando aqui (o elemento vive
   nesta janela), mas quem exibe tempo, progresso e texto é a janela de leitura —
   aqui fica só o controle rápido, para mexer sem tirar o foco do que se lê. */

const playerIcons = {
  play:  '<svg viewBox="0 0 20 20" fill="currentColor"><path d="M6.5 4l9 6-9 6z"/></svg>',
  pause: '<svg viewBox="0 0 20 20" fill="currentColor"><rect x="6" y="4.5" width="3" height="11" rx="1"/><rect x="11" y="4.5" width="3" height="11" rx="1"/></svg>',
  back:  '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5 6 10l5 5"/><path d="M15 5l-5 5 5 5"/></svg>',
  ahead: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M9 5l5 5-5 5"/><path d="M5 5l5 5-5 5"/></svg>',
  stop:  '<svg viewBox="0 0 20 20" fill="currentColor"><rect x="5.5" y="5.5" width="9" height="9" rx="1.8"/></svg>',
  details: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="6.5"/><path d="M10 13.5V9.5"/><path d="M10 6.8v.01"/></svg>',
};

const RING_CIRCUMFERENCE = 2 * Math.PI * 17;

function renderPlayer(state) {
  hud.hidden = true;
  player.hidden = false;
  player.dataset.state = state;

  player.innerHTML =
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
    `<span class="player-divider"></span>` +
    `<button class="player-btn" id="playerStop" type="button" aria-label="Cancelar leitura">${playerIcons.stop}</button>`;
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
listen("vox://player-progress", (event) => {
  const { elapsed, total, state, speed } = event.payload ?? {};
  updatePlayerProgress(elapsed ?? 0, total ?? 0);

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

listen("vox://reading", (event) => {
  const state = event.payload?.state;
  if (!state) return;

  if (state === "idle") {
    player.hidden = true;
    player.innerHTML = "";
    return;
  }

  /* A falha troca o conteúdo em vez de só pintar o anel de vermelho: os
     controles de transporte não têm o que operar, e deixá-los ali convida o
     clique que não faz nada. O motivo fica na janela de leitura, que tem
     espaço; aqui cabe o aviso e a saída. */
  if (state === "failed") {
    player.hidden = false;
    player.dataset.state = state;
    player.innerHTML =
      `<span class="player-failed-dot"></span>` +
      `<span class="player-failed-label">Falhou</span>` +
      `<button class="player-btn" id="playerDetails" type="button" ` +
        `aria-label="Ver o motivo">${playerIcons.details}</button>` +
      `<span class="player-divider"></span>` +
      `<button class="player-btn" id="playerStop" type="button" ` +
        `aria-label="Fechar">${playerIcons.stop}</button>`;
    return;
  }

  if (!player.innerHTML || player.dataset.state === "failed") renderPlayer(state);
  else player.dataset.state = state;
});

/* Os controles não mexem no áudio daqui: mandam o pedido para a janela de
   leitura, que é quem tem a linha do tempo completa. */
player.addEventListener("click", (event) => {
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
