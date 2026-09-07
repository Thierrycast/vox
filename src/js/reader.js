/* Janela de leitura.
 *
 * ## Como a linha do tempo funciona
 *
 * O backend gera um arquivo por frase e o elemento de áudio toca um de cada
 * vez. A linha do tempo global vive aqui, no front, porque é aqui que estão as
 * durações reais (que só o próprio elemento de áudio sabe, ao carregar).
 *
 *     posição global = soma das durações anteriores + player.currentTime
 *
 * Voltar 15 s é traduzir uma posição global de volta para (índice, deslocamento).
 *
 * ## Sobre a precisão do destaque
 *
 * Dois níveis, e eles são honestos sobre o que sabem:
 *
 * - **Frase: exata.** Cada trecho é uma frase e conhecemos a duração real do
 *   áudio dela.
 * - **Palavra: estimada.** O Kokoro não devolve marcação de tempo por palavra,
 *   então interpolamos pela posição do caractere dentro da frase. Numa frase de
 *   poucos segundos o desvio é pequeno, mas não é cravado — e por isso o realce
 *   da palavra é visualmente mais discreto que o da frase.
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

const JUMP_SECONDS = 15;
/* Um respiro à frente: destacar a palavra alguns décimos adiantado lê melhor
   do que atrasado — o olho chega antes da voz, não depois. */
const WORD_LEAD_SECONDS = 0.12;

const body = document.body;
const player = document.getElementById("player");
const readerView = document.getElementById("reader");
const transport = document.getElementById("transport");
const emptyView = document.getElementById("empty");
const statusLabel = document.getElementById("statusLabel");

const elapsedLabel = document.getElementById("elapsed");
const durationLabel = document.getElementById("duration");
const track = document.getElementById("track");
const progressBar = document.getElementById("progress");
const bufferedBar = document.getElementById("buffered");
const playhead = document.getElementById("playhead");
const playPauseButton = document.getElementById("playPause");
const playPauseIcon = document.getElementById("playPauseIcon");

const ICON_PLAY = '<path d="M6 4l10 6-10 6z"/>';
const ICON_PAUSE = '<rect x="6" y="4.5" width="3" height="11" rx="1"/><rect x="11" y="4.5" width="3" height="11" rx="1"/>';

/** Um trecho: a frase, seus elementos de palavra, o áudio e a duração real. */
let segments = [];
let currentIndex = -1;
let readingState = "idle";
/* O usuário rolou manualmente: paramos de arrastar a rolagem atrás dele até a
   próxima frase, senão brigamos com quem quer reler algo acima. */
let userScrolled = false;
let scrollResetHandle = null;

function formatTime(totalSeconds) {
  if (!Number.isFinite(totalSeconds) || totalSeconds < 0) return "0:00";
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = Math.floor(totalSeconds % 60);
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/* ---------------------------------------------------------------- desenho */

/** Quebra a frase em palavras, cada uma com onde começa e termina no texto. */
function buildSentence(text, index) {
  const paragraph = document.createElement("span");
  paragraph.className = "sentence";
  paragraph.dataset.index = String(index);

  const totalChars = text.length || 1;
  // Mantém os separadores para o texto continuar legível ao ser remontado.
  const pieces = text.split(/(\s+)/);
  let cursor = 0;

  for (const piece of pieces) {
    if (!piece) continue;
    if (/^\s+$/.test(piece)) {
      paragraph.appendChild(document.createTextNode(piece));
      cursor += piece.length;
      continue;
    }
    const word = document.createElement("span");
    word.className = "word";
    word.textContent = piece;
    // Fração do trecho em que esta palavra começa e termina.
    word.dataset.from = String(cursor / totalChars);
    word.dataset.to = String((cursor + piece.length) / totalChars);
    paragraph.appendChild(word);
    cursor += piece.length;
  }

  paragraph.appendChild(document.createTextNode(" "));
  return paragraph;
}

function renderPlan(texts) {
  segments = texts.map((text, index) => ({
    text,
    audio: null,
    duration: 0,
    element: buildSentence(text, index),
    words: null,
  }));

  readerView.innerHTML = "";
  for (const segment of segments) {
    segment.words = segment.element.querySelectorAll(".word");
    readerView.appendChild(segment.element);
  }

  const awaiting = document.createElement("span");
  awaiting.className = "awaiting";
  awaiting.id = "awaiting";
  awaiting.textContent = "Gerando a voz…";
  readerView.appendChild(awaiting);

  currentIndex = -1;
  userScrolled = false;
  emptyView.hidden = true;
  readerView.hidden = false;
  transport.hidden = false;
  updateTimeline();
}

/* ------------------------------------------------------------ linha do tempo */

/** Duração conhecida até (sem incluir) o trecho `index`. */
function offsetBefore(index) {
  let total = 0;
  for (let position = 0; position < index && position < segments.length; position += 1) {
    total += segments[position].duration;
  }
  return total;
}

/** Segundos já gerados no total — o fim da parte navegável. */
function generatedDuration() {
  return segments.reduce((sum, segment) => sum + segment.duration, 0);
}

function globalPosition() {
  if (currentIndex < 0) return 0;
  return offsetBefore(currentIndex) + (player.currentTime || 0);
}

/** Traduz uma posição global de volta para trecho e deslocamento. */
function locate(position) {
  let remaining = Math.max(0, position);
  for (let index = 0; index < segments.length; index += 1) {
    const duration = segments[index].duration;
    if (duration <= 0) break;           // daqui para a frente ainda não gerou
    if (remaining < duration) return { index, offset: remaining };
    remaining -= duration;
  }
  const last = segments.findLastIndex((segment) => segment.duration > 0);
  if (last < 0) return { index: 0, offset: 0 };
  return { index: last, offset: Math.max(0, segments[last].duration - 0.05) };
}

/* A janela de leitura é a dona da linha do tempo: só ela tem as durações reais
   e o elemento de áudio. O player flutuante apenas reflete o que sai daqui —
   duas cópias da mesma verdade divergiriam na hora de dar seek. */
function publishProgress() {
  emit("vox://player-progress", {
    elapsed: globalPosition(),
    total: generatedDuration(),
    state: readingState,
    speed: Number(document.getElementById("speed").value),
  }).catch(() => {});
}

function updateTimeline() {
  const total = generatedDuration();
  const position = globalPosition();
  const ratio = total > 0 ? Math.min(1, position / total) : 0;

  elapsedLabel.textContent = formatTime(position);
  durationLabel.textContent = formatTime(total);
  progressBar.style.width = `${ratio * 100}%`;
  playhead.style.left = `${ratio * 100}%`;

  // A barra clara mostra a dianteira: o que já existe em áudio além do ponto
  // atual. Ver isso crescer explica por que a leitura às vezes segura o play.
  bufferedBar.style.width = "100%";

  track.setAttribute("aria-valuenow", String(Math.round(position)));
  track.setAttribute("aria-valuemax", String(Math.round(total)));
  track.setAttribute("aria-valuetext", `${formatTime(position)} de ${formatTime(total)}`);
  publishProgress();
}

/* O `timeupdate` do elemento de áudio dispara umas quatro vezes por segundo,
   mas o navegador não garante o ritmo. O limitador evita que uma rajada vire
   uma rajada de chamadas ao backend — a extensão interpola entre as leituras e
   não perde nada com isso. */
let ultimoEnvioDeProgresso = 0;

function reportarProgresso(index, ratio) {
  const agora = performance.now();
  if (agora - ultimoEnvioDeProgresso < 90) return;
  ultimoEnvioDeProgresso = agora;
  invoke("reading_progress", { index, ratio }).catch(() => {});
}

/* ---------------------------------------------------------------- destaque */

function markCurrent(index) {
  if (index === currentIndex) return;

  const previous = segments[currentIndex];
  if (previous) {
    previous.element.dataset.current = "false";
    previous.element.dataset.done = "true";
    previous.words.forEach((word) => (word.dataset.speaking = "false"));
  }

  currentIndex = index;
  const segment = segments[index];
  if (!segment) return;

  segment.element.dataset.current = "true";
  segment.element.dataset.done = "false";

  // O cursor do backend precisa acompanhar: a dianteira necessária é calculada
  // sobre o que ainda falta, não sobre o texto inteiro.
  invoke("reading_cursor", { index }).catch(() => {});

  if (!userScrolled) scrollIntoView(segment.element);
}

function scrollIntoView(element) {
  const box = element.getBoundingClientRect();
  const view = readerView.getBoundingClientRect();
  // Mantém a frase corrente no terço superior: o que vem a seguir fica visível,
  // que é o que importa em leitura acompanhada.
  const target = readerView.scrollTop + (box.top - view.top) - view.height * 0.32;
  readerView.scrollTo({ top: Math.max(0, target), behavior: "smooth" });
}

function updateWordHighlight() {
  const segment = segments[currentIndex];
  if (!segment || !segment.duration) return;

  const ratio = Math.min(
    1,
    Math.max(0, (player.currentTime + WORD_LEAD_SECONDS) / segment.duration),
  );

  /* A mesma posição vai para o backend, que a publica para a extensão de
     navegador desenhar o destaque na página original. Esta janela e a página
     mostram o mesmo texto de lugares diferentes; a posição tem que ser uma só,
     e ela nasce aqui, onde o áudio de fato toca. */
  reportarProgresso(currentIndex, ratio);

  for (const word of segment.words) {
    const from = Number(word.dataset.from);
    const to = Number(word.dataset.to);
    word.dataset.speaking = ratio >= from && ratio < to ? "true" : "false";
  }
}

/* ---------------------------------------------------------- reprodução */

function loadSegment(index, offset = 0) {
  const segment = segments[index];
  if (!segment?.audio) return false;

  markCurrent(index);
  player.src = segment.audio;
  player.playbackRate = Number(document.getElementById("speed").value);

  const start = () => {
    player.currentTime = offset;
    if (readingState !== "paused") {
      player.play().catch((error) => console.error("[vox] falha ao tocar", error));
    }
  };

  if (player.readyState >= 1) start();
  else player.addEventListener("loadedmetadata", start, { once: true });

  return true;
}

function playNextIfReady() {
  const next = currentIndex + 1;
  if (next >= segments.length) {
    invoke("reading_finished").catch(() => {});
    setState("complete");
    return;
  }
  if (segments[next]?.audio) {
    loadSegment(next);
  }
  // Sem áudio ainda: a chegada do próximo trecho dispara a reprodução.
}

function seekTo(position) {
  const total = generatedDuration();
  if (total <= 0) return;
  const target = locate(Math.min(Math.max(0, position), total - 0.05));
  loadSegment(target.index, target.offset);
  updateTimeline();
}

function seekBy(delta) {
  seekTo(globalPosition() + delta);
}

/* ---------------------------------------------------------------- estados */

function setState(next) {
  readingState = next;
  body.dataset.state = next;

  const labels = {
    idle: "Parado",
    generating: "Gerando a voz",
    playing: "Lendo",
    paused: "Pausado",
    complete: "Terminou",
    failed: "Falhou",
  };
  statusLabel.textContent = labels[next] ?? next;
  publishProgress();

  const isPlaying = next === "playing";
  playPauseIcon.innerHTML = isPlaying ? ICON_PAUSE : ICON_PLAY;
  playPauseButton.title = isPlaying ? "Pausar" : "Continuar";
  playPauseButton.setAttribute("aria-label", playPauseButton.title);

  const awaiting = document.getElementById("awaiting");
  if (awaiting) awaiting.hidden = next !== "generating";

  const failure = document.getElementById("failure");
  if (failure) failure.hidden = next !== "failed";

  // Numa falha não há o que operar: o transporte sumir evita o botão de pausa
  // que não pausa nada, que foi o que apareceu no primeiro teste.
  if (next === "failed") {
    transport.hidden = true;
    readerView.hidden = true;
    emptyView.hidden = true;
  }
}

/* ------------------------------------------------------------ eventos do app */

listen("vox://reading-plan", (event) => {
  renderPlan(event.payload.segments ?? []);
  setState("generating");
});

listen("vox://reading-chunk", (event) => {
  const { index, audio } = event.payload;
  const segment = segments[index];
  if (!segment) return;

  segment.audio = audio;

  // Descobre a duração real sem tocar: é ela que alimenta a linha do tempo e a
  // interpolação do destaque por palavra.
  const probe = new Audio(audio);
  probe.addEventListener("loadedmetadata", () => {
    segment.duration = probe.duration || 0;
    updateTimeline();
    // Primeiro trecho pronto, ou o que estávamos esperando: começa a tocar.
    if (currentIndex < 0 && index === 0) loadSegment(0);
    else if (player.ended && index === currentIndex + 1) loadSegment(index);
  }, { once: true });
});

listen("vox://reading", (event) => {
  const state = event.payload?.state;
  const motivo = event.payload?.message;

  const campo = document.getElementById("failureReason");
  if (campo) campo.textContent = motivo ?? "";

  if (state) setState(state);
  if (state === "idle") {
    player.pause();
    player.removeAttribute("src");
    readerView.hidden = true;
    transport.hidden = true;
    emptyView.hidden = false;
    segments = [];
    currentIndex = -1;
  }
});

/* ---------------------------------------------------------- eventos do player */

player.addEventListener("timeupdate", () => {
  updateWordHighlight();
  updateTimeline();
});

player.addEventListener("ended", playNextIfReady);

/* ---------------------------------------------------------------- controles */

playPauseButton.addEventListener("click", () => {
  if (readingState === "playing") {
    player.pause();
    setState("paused");
  } else {
    player.play().catch(() => {});
    setState("playing");
  }
  invoke("toggle_reading").catch(() => {});
});

document.getElementById("back15").addEventListener("click", () => seekBy(-JUMP_SECONDS));
document.getElementById("forward15").addEventListener("click", () => seekBy(JUMP_SECONDS));
document.getElementById("restart").addEventListener("click", () => seekTo(0));
document.getElementById("stop").addEventListener("click", () => {
  invoke("stop_reading").catch(() => {});
});

document.getElementById("speed").addEventListener("change", (event) => {
  player.playbackRate = Number(event.target.value);
});

track.addEventListener("click", (event) => {
  const box = track.getBoundingClientRect();
  const ratio = (event.clientX - box.left) / box.width;
  seekTo(ratio * generatedDuration());
});

track.addEventListener("keydown", (event) => {
  if (event.key === "ArrowLeft") { seekBy(-JUMP_SECONDS); event.preventDefault(); }
  if (event.key === "ArrowRight") { seekBy(JUMP_SECONDS); event.preventDefault(); }
});

/* Clicar numa frase pula para ela: reler um trecho é o gesto mais natural que
   existe num leitor, e obrigar a caçar pela barra seria pior. */
readerView.addEventListener("click", (event) => {
  const sentence = event.target.closest(".sentence");
  if (!sentence) return;
  const index = Number(sentence.dataset.index);
  if (segments[index]?.audio) loadSegment(index);
});

/* Rolagem manual suspende o acompanhamento automático por alguns segundos. */
readerView.addEventListener("wheel", () => {
  userScrolled = true;
  clearTimeout(scrollResetHandle);
  scrollResetHandle = setTimeout(() => { userScrolled = false; }, 4000);
});

/* ------------------------------------------------------- entrada por colagem */

document.getElementById("readPasted").addEventListener("click", () => {
  const text = document.getElementById("pasteArea").value.trim();
  if (!text) return;
  invoke("read_text", { text }).catch((error) => mostrarFalha(String(error)));
});

/* Atalhos locais da janela: espaço pausa, setas navegam. */
document.addEventListener("keydown", (event) => {
  if (event.target.matches("textarea, input, select")) return;
  if (event.code === "Space") { playPauseButton.click(); event.preventDefault(); }
  if (event.code === "ArrowLeft") { seekBy(-JUMP_SECONDS); event.preventDefault(); }
  if (event.code === "ArrowRight") { seekBy(JUMP_SECONDS); event.preventDefault(); }
});

/* Comandos vindos da pílula flutuante. */
const SPEEDS = [0.75, 1, 1.25, 1.5, 1.75, 2];

listen("vox://player-command", (event) => {
  const { action, seconds } = event.payload ?? {};
  if (action === "seek") seekBy(seconds ?? 0);
  else if (action === "toggle") playPauseButton.click();
  else if (action === "cycle-speed") {
    const select = document.getElementById("speed");
    const next = SPEEDS[(SPEEDS.indexOf(Number(select.value)) + 1) % SPEEDS.length];
    select.value = String(next);
    player.playbackRate = next;
    publishProgress();
  }
});

setState("idle");

/* "Tentar de novo" remonta o texto a partir do plano que já chegou. É o mesmo
   texto que falhou, sem depender de a seleção original ainda existir — quando a
   síntese falha o usuário já mexeu em outra janela. */
document.getElementById("retry")?.addEventListener("click", () => {
  const texto = segments.map((segment) => segment.text).join(" ").trim();
  if (!texto) return;
  invoke("read_text", { text: texto }).catch((error) => mostrarFalha(String(error)));
});
