/* Painel de preferências.
 *
 * O backend é a fonte da verdade: este módulo lê o `Settings` inteiro, mostra,
 * e devolve o objeto inteiro a cada mudança. Mandar só o campo alterado exigiria
 * um comando por preferência, ou um merge do lado de lá — e as duas coisas dão
 * na mesma com mais peças para quebrar.
 *
 * O `save_settings` já normaliza (clamp de velocidade, vocabulário único), então
 * o que volta do disco pode ser diferente do que foi enviado. Por isso a tela é
 * redesenhada a partir da resposta, e não do que o usuário digitou.
 */

function mostrarFalha(motivo) {
  const alvo = document.body;
  if (!alvo || alvo.dataset.falhou === "true") return;
  alvo.dataset.falhou = "true";
  const caixa = document.createElement("div");
  caixa.setAttribute("style",
    "position:fixed;inset:0;z-index:9999;padding:16px;overflow:auto;" +
    "background:#2a1113;color:#ffb4b4;font:12px/1.5 ui-monospace,monospace;" +
    "white-space:pre-wrap;word-break:break-word");
  caixa.textContent = "Vox — falha nas preferências:\n\n" + motivo;
  alvo.appendChild(caixa);
}

window.addEventListener("error", (event) => {
  mostrarFalha(String(event.message) + "\n" + (event.filename || "") + ":" + event.lineno);
});
window.addEventListener("unhandledrejection", (event) => {
  mostrarFalha("promessa rejeitada: " + String(event.reason));
});

if (!window.__TAURI__) {
  mostrarFalha("window.__TAURI__ não existe: o bootstrap de IPC não carregou.");
  throw new Error("bootstrap do Tauri ausente");
}

const { invoke } = window.__TAURI__.core;

const aviso = document.getElementById("salvo");

/* O que está no disco. Toda gravação parte daqui, com o campo alterado por
   cima — nunca de um objeto montado do zero, que perderia o que o painel ainda
   não mostra (vocabulário, instruções). */
let atual = null;
let carregando = true;

const campos = {
  soundsEnabled: "sounds_enabled",
  soundsVolume: "sounds_volume",
  inputDevice: "input_device",
  transcriptionModel: "transcription_model",
  outputAction: "output_action",
  submitMode: "submit_mode",
  submitKeyword: "submit_keyword",
  submitKey: "submit_key",
  liveTranscription: "live_transcription",
  showLiveTranscription: "show_live_transcription",
  voice: "voice",
  speed: "speed",
  prebufferRatio: "prebuffer_ratio",
  readingCaptions: "reading_captions",
  openReaderOnRead: "open_reader_on_read",
  shortcutDictate: "shortcut_dictate",
  shortcutRead: "shortcut_read",
  bridgeEnabled: "bridge_enabled",
  bridgePort: "bridge_port",
};

function elemento(id) {
  return document.getElementById(id);
}

/* ------------------------------------------------------------------ desenho */

function porcentagem(valor) {
  return `${Math.round(valor * 100)}%`;
}

function desenhar(settings) {
  atual = settings;
  carregando = true;

  for (const [id, chave] of Object.entries(campos)) {
    const campo = elemento(id);
    if (!campo) continue;
    const valor = settings[chave];

    if (campo.type === "checkbox") campo.checked = Boolean(valor);
    else campo.value = valor === null || valor === undefined ? "" : String(valor);
  }

  elemento("externalSounds").value = settings.external_sounds_directory ?? "";
  elemento("bridgeToken").value = settings.bridge_token ?? "";

  elemento("soundsVolumeValor").textContent = porcentagem(settings.sounds_volume);
  elemento("speedValor").textContent = `${settings.speed.toFixed(2).replace(".", ",")}×`;
  elemento("prebufferValor").textContent = porcentagem(settings.prebuffer_ratio);

  atualizarDependencias();
  carregando = false;
}

/* Campos que só fazem sentido junto de outro. Uma palavra-chave de envio sem
   envio por palavra-chave é uma caixa de texto que não decide nada. */
function atualizarDependencias() {
  const modo = elemento("submitMode").value;
  elemento("linhaPalavra").hidden = modo !== "keyword";
  elemento("linhaTecla").hidden = modo === "disabled";
}

let avisoHandle = null;

function confirmarGravacao() {
  aviso.hidden = false;
  aviso.dataset.visivel = "true";
  clearTimeout(avisoHandle);
  avisoHandle = setTimeout(() => { aviso.dataset.visivel = "false"; }, 1400);
}

/* ---------------------------------------------------------------- gravação */

function coletar() {
  const settings = { ...atual };

  for (const [id, chave] of Object.entries(campos)) {
    const campo = elemento(id);
    if (!campo) continue;

    if (campo.type === "checkbox") settings[chave] = campo.checked;
    else if (campo.type === "number" || campo.type === "range") settings[chave] = Number(campo.value);
    else settings[chave] = campo.value;
  }

  // O microfone padrão do sistema é `null`, e não a string vazia: é assim que o
  // backend distingue "o que o Windows escolher" de "um dispositivo chamado ''".
  if (!settings.input_device) settings.input_device = null;

  const pasta = elemento("externalSounds").value.trim();
  settings.external_sounds_directory = pasta === "" ? null : pasta;

  return settings;
}

let gravacaoHandle = null;

function agendarGravacao() {
  if (carregando) return;
  // Arrastar um controle deslizante dispara dezenas de eventos; gravar em cada
  // um escreveria o arquivo dezenas de vezes por segundo.
  clearTimeout(gravacaoHandle);
  gravacaoHandle = setTimeout(gravar, 220);
}

async function gravar() {
  const settings = coletar();
  try {
    await invoke("save_settings", { settings });
    // Relê em vez de confiar no que foi enviado: o backend corrige valores fora
    // de faixa, e a tela tem que mostrar o que ficou valendo.
    desenhar(await invoke("get_settings"));
    confirmarGravacao();
  } catch (erro) {
    mostrarFalha(String(erro));
  }
}

/* ------------------------------------------------------------------ listas */

async function preencherMicrofones(selecionado) {
  const select = elemento("inputDevice");
  select.innerHTML = "";

  const padrao = document.createElement("option");
  padrao.value = "";
  padrao.textContent = "O padrão do sistema";
  select.appendChild(padrao);

  try {
    for (const device of await invoke("list_input_devices")) {
      const opcao = document.createElement("option");
      opcao.value = device.id;
      opcao.textContent = device.is_default ? `${device.name} (padrão)` : device.name;
      select.appendChild(opcao);
    }
  } catch {
    // Sem lista, o campo continua servindo: o valor guardado permanece
    // selecionado e nada se perde por não conseguir enumerar os dispositivos.
  }

  select.value = selecionado ?? "";
  if (select.value !== (selecionado ?? "")) manterValorDesconhecido(select, selecionado);
}

async function preencherVozes(selecionada) {
  const select = elemento("voice");
  select.innerHTML = "";

  try {
    const catalogo = await invoke("api_voices");
    const nomes = [...(catalogo.custom ?? []), ...(catalogo.base ?? [])];
    for (const nome of nomes) {
      const opcao = document.createElement("option");
      opcao.value = nome;
      opcao.textContent = nome === catalogo.default ? `${nome} (padrão da API)` : nome;
      select.appendChild(opcao);
    }
  } catch {
    // A API pode estar fora do ar, e isso não pode impedir de mexer no volume.
  }

  select.value = selecionada ?? "";
  if (select.value !== (selecionada ?? "")) manterValorDesconhecido(select, selecionada);
}

/* Um valor que não está na lista continua sendo o valor.
 *
 * Sem isto, abrir o painel com a API fora do ar zeraria a voz configurada no
 * primeiro `save` — a tela mostraria vazio, e vazio é o que seria gravado. */
function manterValorDesconhecido(select, valor) {
  if (!valor) return;
  const opcao = document.createElement("option");
  opcao.value = valor;
  opcao.textContent = `${valor} (não listado agora)`;
  select.appendChild(opcao);
  select.value = valor;
}

/* ------------------------------------------------------------------ eventos */

for (const id of Object.keys(campos)) {
  const campo = elemento(id);
  if (!campo) continue;
  campo.addEventListener("change", agendarGravacao);
  if (campo.type === "range") {
    campo.addEventListener("input", () => {
      if (id === "soundsVolume") elemento("soundsVolumeValor").textContent = porcentagem(Number(campo.value));
      if (id === "speed") elemento("speedValor").textContent = `${Number(campo.value).toFixed(2).replace(".", ",")}×`;
      if (id === "prebufferRatio") elemento("prebufferValor").textContent = porcentagem(Number(campo.value));
      agendarGravacao();
    });
  }
  if (campo.type === "text") campo.addEventListener("input", agendarGravacao);
}

elemento("externalSounds").addEventListener("input", agendarGravacao);
elemento("submitMode").addEventListener("change", atualizarDependencias);

/* O volume só se ajusta de ouvido. O som toca depois da gravação para ser o
   volume novo, e não o anterior. */
elemento("testarSom").addEventListener("click", async () => {
  clearTimeout(gravacaoHandle);
  await gravar();
  invoke("preview_sound").catch(() => {});
});

elemento("copiarToken").addEventListener("click", async () => {
  const botao = elemento("copiarToken");
  try {
    await navigator.clipboard.writeText(elemento("bridgeToken").value);
    botao.textContent = "Copiado";
  } catch {
    // Sem permissão de área de transferência, seleciona para o Ctrl+C manual.
    elemento("bridgeToken").select();
    botao.textContent = "Selecionado";
  }
  setTimeout(() => { botao.textContent = "Copiar"; }, 1600);
});

/* ------------------------------------------------------------------ partida */

invoke("get_settings")
  .then(async (settings) => {
    await preencherMicrofones(settings.input_device);
    await preencherVozes(settings.voice);
    desenhar(settings);
  })
  .catch((erro) => mostrarFalha("invoke('get_settings') falhou:\n" + String(erro)));
