/* Vox — aplicativo de preferências.
 *
 * ## A regra que organiza este arquivo
 *
 * O backend é a fonte da verdade. Este módulo lê o `Settings` inteiro, desenha,
 * e devolve o objeto inteiro a cada mudança. Mandar só o campo alterado exigiria
 * um comando por preferência, ou um merge do lado de lá — e as duas coisas dão
 * na mesma com mais peças para quebrar.
 *
 * O `save_settings` normaliza o que recebe (limita a velocidade, deduplica o
 * vocabulário), então o que volta do disco pode ser diferente do que foi
 * enviado. Por isso a tela é redesenhada com a **resposta**, e não com o que a
 * pessoa digitou: o que aparece é o que ficou valendo.
 *
 * ## Sobre os controles próprios
 *
 * O `<select>` nativo continua no DOM e continua sendo quem guarda o valor —
 * ele responde por teclado e por acessibilidade. O que se vê é um botão e uma
 * lista desenhados aqui, sobrepostos a ele. Trocar o elemento por uma lista
 * feita à mão exigiria reimplementar navegação por setas, busca por digitação e
 * foco, e a primeira dessas três já não vale o preço.
 */

function mostrarFalha(motivo) {
  const alvo = document.body;
  if (!alvo || alvo.dataset.falhou === "true") return;
  alvo.dataset.falhou = "true";
  const caixa = document.createElement("div");
  caixa.setAttribute("style",
    "position:fixed;inset:0;z-index:9999;padding:20px;overflow:auto;" +
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
const { getCurrentWindow } = window.__TAURI__.window;

const elemento = (id) => document.getElementById(id);

/* O que está no disco. Toda gravação parte daqui, com o campo alterado por
   cima — nunca de um objeto montado do zero, que perderia o que o painel não
   mostra (a posição do widget, por exemplo). */
let atual = null;
let carregando = true;

/* Campos que são só "id do elemento" ↔ "chave do settings". O resto tem
   tratamento próprio mais abaixo. */
const campos = {
  soundsEnabled: "sounds_enabled",
  startWithWindows: "start_with_windows",
  serviceEnabled: "service_enabled",
  soundsVolume: "sounds_volume",
  inputDevice: "input_device",
  transcriptionModel: "transcription_model",
  outputAction: "output_action",
  submitMode: "submit_mode",
  submitKeyword: "submit_keyword",
  submitKey: "submit_key",
  liveTranscription: "live_transcription",
  showLiveTranscription: "show_live_transcription",
  sttRewriteEnabled: "stt_rewrite_enabled",
  sttRewriteModel: "stt_rewrite_model",
  voice: "voice",
  speed: "speed",
  prebufferRatio: "prebuffer_ratio",
  readingCaptions: "reading_captions",
  normalizeBeforeReading: "normalize_before_reading",
  openReaderOnRead: "open_reader_on_read",
  customInstructions: "custom_instructions",
  bridgeEnabled: "bridge_enabled",
  bridgePort: "bridge_port",
};

/* Cores de destaque. Poucas e escolhidas: um seletor de cor livre deixa a
   pessoa escolher um amarelo que some no fundo claro do realce. */
const CORES = [
  { id: "#966aff", nome: "Roxo" },
  { id: "#4f8cff", nome: "Azul" },
  { id: "#37d67a", nome: "Verde" },
  { id: "#f5a04a", nome: "Âmbar" },
  { id: "#f4585c", nome: "Vermelho" },
  { id: "#c9c9d4", nome: "Neutro" },
];

const SECOES = {
  geral: ["Geral", "Som, aparência e o comportamento geral do aplicativo."],
  ditado: ["Ditado", "De onde vem o áudio, quem transcreve e como o texto é entregue."],
  ajuste: ["Ajuste por IA", "Reescrever a fala crua antes de colar — e o quanto mexer nela."],
  leitura: ["Leitura", "Voz, ritmo e como acompanhar o texto que está sendo falado."],
  vocabulario: ["Vocabulário", "O que o reconhecedor precisa saber para não errar seus termos."],
  gravacoes: ["Gravações", "O backup de áudio de cada ditado — pra quando algo dá errado."],
  atalhos: ["Atalhos", "As combinações globais, e o que fazer quando outro app toma uma."],
  widget: ["Widget", "A pílula flutuante: onde ela aparece e como ela se comporta."],
  extensao: ["Extensão", "A ponte local que a extensão de navegador usa."],
  sobre: ["Sobre", "Para onde o áudio vai, e o que está rodando do outro lado."],
};

/* ---------------------------------------------------------------- navegação */

function abrirSecao(nome) {
  for (const item of document.querySelectorAll(".nav-item")) {
    item.setAttribute("aria-current", String(item.dataset.secao === nome));
  }
  for (const secao of document.querySelectorAll(".secao")) {
    secao.dataset.ativa = String(secao.dataset.secao === nome);
  }
  const [titulo, descricao] = SECOES[nome] ?? ["Vox", ""];
  elemento("tituloSecao").textContent = titulo;
  elemento("descricaoSecao").textContent = descricao;
  elemento("rolagem").scrollTop = 0;

  // Carregada ao abrir, e não junto do resto no início: a lista muda a cada
  // ditado, e ninguém olha pra ela o tempo todo — só quando precisa.
  if (nome === "gravacoes") carregarGravacoes();
}

for (const item of document.querySelectorAll(".nav-item")) {
  item.addEventListener("click", () => abrirSecao(item.dataset.secao));
}

/* ------------------------------------------------------- seletores próprios */

/* Desenha o botão e a lista por cima de um `<select>` que continua existindo.
 *
 * A lista é reconstruída a cada abertura porque as opções mudam em tempo de
 * execução — vozes e microfones vêm da API e do sistema. */
function prepararSelect(caixa) {
  const select = caixa.querySelector("select");
  if (!select || caixa.dataset.pronto === "true") return;
  caixa.dataset.pronto = "true";

  const botao = document.createElement("div");
  botao.className = "select-botao";
  botao.innerHTML =
    '<span class="valor"></span>' +
    '<svg class="seta" viewBox="0 0 20 20" fill="none" stroke="currentColor" ' +
    'stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">' +
    '<path d="M6 8l4 4 4-4"/></svg>';

  const lista = document.createElement("div");
  lista.className = "select-lista";

  caixa.append(botao, lista);

  const sincronizar = () => {
    const escolhida = select.options[select.selectedIndex];
    botao.querySelector(".valor").textContent = escolhida ? escolhida.textContent : "—";
  };

  const desenharLista = () => {
    lista.innerHTML = "";
    for (const opcao of select.options) {
      const linha = document.createElement("div");
      linha.className = "select-opcao";
      linha.dataset.escolhida = String(opcao.selected);
      linha.innerHTML =
        `<span>${opcao.textContent}</span>` +
        '<svg class="marca-check" viewBox="0 0 20 20" fill="none" stroke="currentColor" ' +
        'stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">' +
        '<path d="M5 10.5l3.5 3.5L15 7"/></svg>';
      linha.addEventListener("click", () => {
        select.value = opcao.value;
        // O `change` sintético é o que faz o resto do painel reagir como se a
        // pessoa tivesse usado o elemento nativo — nada aqui grava sozinho.
        select.dispatchEvent(new Event("change", { bubbles: true }));
        fechar();
      });
      lista.appendChild(linha);
    }
  };

  const abrir = () => {
    for (const outra of document.querySelectorAll('.select[data-aberto="true"]')) {
      outra.dataset.aberto = "false";
    }
    desenharLista();
    caixa.dataset.aberto = "true";
  };
  const fechar = () => { caixa.dataset.aberto = "false"; };

  botao.addEventListener("mousedown", (event) => {
    event.preventDefault();
    if (caixa.dataset.aberto === "true") fechar();
    else abrir();
  });

  // O nativo continua respondendo ao teclado; a lista só reflete.
  select.addEventListener("change", () => { sincronizar(); fechar(); });
  select.addEventListener("keydown", (event) => {
    if (event.key === "Escape") fechar();
  });

  caixa.sincronizar = sincronizar;
  sincronizar();
}

document.addEventListener("mousedown", (event) => {
  if (event.target.closest(".select")) return;
  for (const caixa of document.querySelectorAll('.select[data-aberto="true"]')) {
    caixa.dataset.aberto = "false";
  }
});

function sincronizarSelects() {
  for (const caixa of document.querySelectorAll(".select")) {
    prepararSelect(caixa);
    caixa.sincronizar?.();
  }
}

/* ------------------------------------------------------------------ desenho */

const porcentagem = (valor) => `${Math.round(valor * 100)}%`;

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

  elemento("soundsVolumeValor").textContent = porcentagem(settings.sounds_volume ?? 1);
  elemento("speedValor").textContent = `${(settings.speed ?? 1).toFixed(2).replace(".", ",")}×`;
  elemento("prebufferValor").textContent = porcentagem(settings.prebuffer_ratio ?? 0);

  desenharVocabulario(settings.vocabulary ?? []);
  desenharIntensidade(settings.stt_rewrite_intensity ?? 2);
  desenharPresets(settings.stt_rewrite_preset);
  aplicarCor(settings.theme_accent || CORES[0].id);
  desenharPosicao(settings.hud_position_dictation, settings.hud_position_reading);

  atualizarDependencias();
  sincronizarSelects();
  carregando = false;
}

/* Campos que só fazem sentido junto de outro. Uma palavra-chave de envio sem
   envio por palavra-chave é uma caixa de texto que não decide nada. */
function atualizarDependencias() {
  const modo = elemento("submitMode").value;
  elemento("linhaPalavra").hidden = modo !== "keyword";
  elemento("linhaTecla").hidden = modo === "disabled";
}

function desenharPosicao(ditado, leitura) {
  const alvo = elemento("posicaoAtual");
  if (!ditado && !leitura) {
    alvo.textContent =
      "Nunca foi arrastado: aparece no centro inferior no ditado e na borda direita na leitura.";
    return;
  }

  /* Cada papel lembra o próprio lugar: arrastar a barra do ditado não move a
     pílula da leitura. A leitura é guardada pela borda direita, que é o que não
     muda quando a legenda abre. */
  const ditadoTexto = ditado
    ? `ditado em x ${Math.round(ditado.x)}, y ${Math.round(ditado.y)}`
    : "ditado no lugar padrão";
  const leituraTexto = leitura
    ? `leitura com a borda direita em x ${Math.round(leitura.x)}, y ${Math.round(leitura.y)}`
    : "leitura no lugar padrão";

  alvo.textContent =
    `Guardado: ${ditadoTexto}; ${leituraTexto}. ` +
    "Arraste a pílula para mudar — cada modo grava o seu sozinho.";
}

/* --------------------------------------------------------------- vocabulário */

let vocabulario = [];

function desenharVocabulario(termos) {
  vocabulario = [...termos];
  const caixa = elemento("fichasVocabulario");
  const entrada = elemento("vocabularyEntrada");

  for (const ficha of caixa.querySelectorAll(".ficha")) ficha.remove();

  for (const termo of vocabulario) {
    const ficha = document.createElement("span");
    ficha.className = "ficha";
    ficha.append(document.createTextNode(termo));

    const remover = document.createElement("button");
    remover.type = "button";
    remover.setAttribute("aria-label", `Remover ${termo}`);
    remover.innerHTML =
      '<svg viewBox="0 0 20 20" width="10" height="10" fill="none" stroke="currentColor" ' +
      'stroke-width="2.2" stroke-linecap="round"><path d="M5 5l10 10M15 5L5 15"/></svg>';
    remover.addEventListener("click", () => {
      vocabulario = vocabulario.filter((item) => item !== termo);
      desenharVocabulario(vocabulario);
      gravar();
    });

    ficha.appendChild(remover);
    caixa.insertBefore(ficha, entrada);
  }
}

elemento("vocabularyEntrada").addEventListener("keydown", (event) => {
  const entrada = event.target;

  // Vírgula também confirma: quem cola uma lista pronta não vai apertar Enter
  // entre cada termo.
  if (event.key === "Enter" || event.key === ",") {
    event.preventDefault();
    const termos = entrada.value.split(",").map((item) => item.trim()).filter(Boolean);
    if (!termos.length) return;
    for (const termo of termos) {
      if (!vocabulario.some((item) => item.toLowerCase() === termo.toLowerCase())) {
        vocabulario.push(termo);
      }
    }
    entrada.value = "";
    desenharVocabulario(vocabulario);
    gravar();
    return;
  }

  // Backspace no campo vazio apaga a última ficha: é o que todo campo de
  // etiquetas faz, e a mão já espera isso.
  if (event.key === "Backspace" && entrada.value === "" && vocabulario.length) {
    vocabulario.pop();
    desenharVocabulario(vocabulario);
    gravar();
  }
});

/* --------------------------------------------------------------- atalhos */

/* A lista é montada com o catálogo do backend, e não com uma cópia daqui: um
   comando novo aparece nesta tela sem uma versão nova dela, e a falha de
   registro chega junto do campo que a causou. */
let atalhos = {};

async function montarAtalhos(catalogoPronto) {
  const caixa = elemento("listaAtalhos");
  caixa.innerHTML = "";

  let catalogo = catalogoPronto;
  if (!catalogo) {
    try {
      catalogo = await invoke("command_catalog");
    } catch {
      caixa.textContent = "Não consegui ler o catálogo de comandos.";
      return;
    }
  }

  atalhos = {};

  for (const comando of catalogo) {
    atalhos[comando.id] = comando.binding;

    const linha = document.createElement("div");
    linha.className = "linha";

    const rotulo = document.createElement("span");
    rotulo.className = "rotulo";
    const titulo = document.createElement("span");
    titulo.textContent = comando.label;
    const dica = document.createElement("small");
    // A falha substitui a dica: quando o comando não responde, saber por que
    // importa mais do que saber o que ele faria.
    dica.textContent = comando.failure
      ? `Não registrou: ${comando.failure}. Outro programa provavelmente tem esta combinação.`
      : comando.hint;
    if (comando.failure) dica.style.color = "var(--amarelo)";
    rotulo.append(titulo, dica);

    const controle = document.createElement("div");
    controle.className = "controle";
    const campo = document.createElement("input");
    campo.type = "text";
    campo.spellcheck = false;
    campo.value = comando.binding;
    campo.placeholder = comando.default_binding;
    campo.style.width = "180px";
    campo.addEventListener("input", () => {
      atalhos[comando.id] = campo.value;
      agendarGravacao();
    });
    controle.appendChild(campo);

    linha.append(rotulo, controle);
    caixa.appendChild(linha);
  }
}

/* ------------------------------------------------------------ ajuste por IA */

function desenharIntensidade(valor) {
  for (const botao of elemento("intensidade").querySelectorAll("button")) {
    botao.setAttribute("aria-pressed", String(Number(botao.dataset.valor) === Number(valor)));
  }
}

elemento("intensidade").addEventListener("click", (event) => {
  const botao = event.target.closest("button");
  if (!botao) return;
  desenharIntensidade(botao.dataset.valor);
  gravar();
});

let presetsDisponiveis = [];

function desenharPresets(escolhido) {
  const caixa = elemento("presets");
  caixa.innerHTML = "";

  for (const preset of presetsDisponiveis) {
    const cartao = document.createElement("button");
    cartao.type = "button";
    cartao.className = "preset";
    cartao.dataset.id = preset.id;
    cartao.setAttribute("aria-pressed", String(preset.id === escolhido));

    const titulo = document.createElement("strong");
    titulo.textContent = preset.label;
    const descricao = document.createElement("small");
    descricao.textContent = preset.description;
    cartao.append(titulo, descricao);

    cartao.addEventListener("click", () => {
      desenharPresets(preset.id);
      gravar();
    });
    caixa.appendChild(cartao);
  }

  if (!presetsDisponiveis.length) {
    const aviso = document.createElement("small");
    aviso.style.color = "var(--fg-fraco)";
    aviso.textContent =
      "Não consegui listar os moldes — o serviço de voz não respondeu. " +
      "O ajuste continua funcionando com o molde padrão.";
    caixa.appendChild(aviso);
  }
}

function presetEscolhido() {
  const marcado = elemento("presets").querySelector('[aria-pressed="true"]');
  return marcado?.dataset.id ?? atual?.stt_rewrite_preset ?? "fala-limpa";
}

function intensidadeEscolhida() {
  const marcado = elemento("intensidade").querySelector('[aria-pressed="true"]');
  return Number(marcado?.dataset.valor ?? 2);
}

/* A prévia existe porque o efeito de um preset não se explica em uma linha de
   texto: ver a mesma fala virar cinco coisas diferentes decide a escolha em
   segundos. */
elemento("previaRodar").addEventListener("click", async () => {
  const entrada = elemento("previaEntrada").value.trim();
  const saida = elemento("previaSaida");
  const nota = elemento("previaNota");

  if (!entrada) {
    saida.textContent = "Escreva ou cole uma fala acima primeiro.";
    return;
  }

  saida.dataset.cheia = "false";
  saida.textContent = "Reescrevendo…";
  nota.textContent = "";

  const comecou = performance.now();
  try {
    const resposta = await invoke("rewrite_preview", {
      text: entrada,
      preset: presetEscolhido(),
      intensity: intensidadeEscolhida(),
    });
    saida.textContent = resposta.text;
    saida.dataset.cheia = "true";

    const relato = resposta.rewritten ?? {};
    const motivo = relato.error || relato.skipped;
    nota.textContent = motivo
      ? `não aplicado: ${motivo}`
      : `${Math.round(performance.now() - comecou)} ms`;
  } catch (erro) {
    saida.textContent = String(erro);
    saida.dataset.cheia = "true";
  }
});

/* ------------------------------------------------------------------ aparência */

function aplicarCor(cor) {
  document.documentElement.style.setProperty("--accent", cor);
  // As variantes derivam da escolhida para o realce e o fundo continuarem
  // combinando sem uma tabela de seis cores para cada uma.
  document.documentElement.style.setProperty("--accent-suave", `${cor}29`);
  document.documentElement.style.setProperty("--accent-forte", `${cor}61`);

  for (const botao of elemento("cores").querySelectorAll(".cor")) {
    botao.setAttribute("aria-pressed", String(botao.dataset.cor === cor));
  }
}

function montarCores() {
  const caixa = elemento("cores");
  for (const { id, nome } of CORES) {
    const botao = document.createElement("button");
    botao.type = "button";
    botao.className = "cor";
    botao.dataset.cor = id;
    botao.style.background = id;
    botao.title = nome;
    botao.setAttribute("aria-label", nome);
    botao.addEventListener("click", () => { aplicarCor(id); gravar(); });
    caixa.appendChild(botao);
  }
}

function corEscolhida() {
  const marcada = elemento("cores").querySelector('[aria-pressed="true"]');
  return marcada?.dataset.cor ?? CORES[0].id;
}

/* ---------------------------------------------------------------- gravação */

let avisoHandle = null;

function confirmarGravacao() {
  const aviso = elemento("salvo");
  aviso.dataset.visivel = "true";
  clearTimeout(avisoHandle);
  avisoHandle = setTimeout(() => { aviso.dataset.visivel = "false"; }, 1500);
}

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

  const modelo = elemento("sttRewriteModel").value.trim();
  settings.stt_rewrite_model = modelo === "" ? null : modelo;

  settings.vocabulary = [...vocabulario];
  settings.shortcuts = { ...settings.shortcuts, ...atalhos };
  settings.stt_rewrite_preset = presetEscolhido();
  settings.stt_rewrite_intensity = intensidadeEscolhida();
  settings.theme_accent = corEscolhida();

  return settings;
}

let gravacaoHandle = null;

function agendarGravacao() {
  if (carregando) return;
  // Arrastar um controle deslizante dispara dezenas de eventos; gravar em cada
  // um escreveria o arquivo dezenas de vezes por segundo.
  clearTimeout(gravacaoHandle);
  gravacaoHandle = setTimeout(gravar, 240);
}

async function gravar() {
  if (carregando) return;
  clearTimeout(gravacaoHandle);
  try {
    await invoke("save_settings", { settings: coletar() });
    // Relê em vez de confiar no que foi enviado: o backend corrige valores fora
    // de faixa, e a tela tem que mostrar o que ficou valendo.
    desenhar(await invoke("get_settings"));
    confirmarGravacao();
  } catch (erro) {
    mostrarFalha(String(erro));
  }
}

/* ------------------------------------------------------------------ eventos */

for (const id of Object.keys(campos)) {
  const campo = elemento(id);
  if (!campo) continue;

  campo.addEventListener("change", agendarGravacao);

  if (campo.type === "range") {
    campo.addEventListener("input", () => {
      const valor = Number(campo.value);
      if (id === "soundsVolume") elemento("soundsVolumeValor").textContent = porcentagem(valor);
      if (id === "speed") elemento("speedValor").textContent = `${valor.toFixed(2).replace(".", ",")}×`;
      if (id === "prebufferRatio") elemento("prebufferValor").textContent = porcentagem(valor);
      agendarGravacao();
    });
  }
  if (campo.type === "text" || campo.tagName === "TEXTAREA") {
    campo.addEventListener("input", agendarGravacao);
  }
}

/* Estes dois não passam pela gravação comum: cada um tem efeito imediato no
   sistema — soltar os atalhos, escrever no registro — e precisa do comando que
   faz isso, não só do campo gravado. */
elemento("serviceEnabled").addEventListener("change", async (event) => {
  await invoke("set_service_enabled", { enabled: event.target.checked });
  desenhar(await invoke("get_settings"));
  confirmarGravacao();
});

elemento("startWithWindows").addEventListener("change", async (event) => {
  try {
    await invoke("set_autostart", { enabled: event.target.checked });
    confirmarGravacao();
  } catch (erro) {
    // A caixa volta ao que era: marcar algo que não aconteceu é pior do que
    // não marcar nada.
    event.target.checked = !event.target.checked;
    mostrarFalha(String(erro));
  }
});

elemento("externalSounds").addEventListener("input", agendarGravacao);
elemento("submitMode").addEventListener("change", atualizarDependencias);

/* O volume só se ajusta de ouvido. O som toca depois de gravar, para ser o
   volume novo e não o anterior. */
elemento("testarSom").addEventListener("click", async () => {
  await gravar();
  invoke("preview_sound").catch(() => {});
});

elemento("copiarToken").addEventListener("click", async () => {
  const botao = elemento("copiarToken");
  try {
    await navigator.clipboard.writeText(elemento("bridgeToken").value);
    botao.textContent = "Copiado";
  } catch {
    elemento("bridgeToken").select();
    botao.textContent = "Selecionado";
  }
  setTimeout(() => { botao.textContent = "Copiar"; }, 1600);
});

/* Aplicar sem reiniciar resolve duas coisas: a combinação nova passa a valer na
   hora, e o que estava tomado é testado outra vez — a disputa muda quando o
   outro programa fecha, e antes a única saída era reiniciar o Vox. */
elemento("reaplicarAtalhos").addEventListener("click", async () => {
  const botao = elemento("reaplicarAtalhos");
  botao.textContent = "Aplicando…";

  // Grava antes: o backend registra o que está no disco, não o que está na tela.
  await gravar();

  try {
    const catalogo = await invoke("reapply_shortcuts");
    await montarAtalhos(catalogo);
    const falhas = catalogo.filter((comando) => comando.failure).length;
    botao.textContent = falhas === 0
      ? "Todos no ar"
      : `${falhas} em conflito`;
  } catch (erro) {
    botao.textContent = "Falhou";
    mostrarFalha(String(erro));
  }

  setTimeout(() => { botao.textContent = "Aplicar"; }, 2400);
});

elemento("resetarPosicao").addEventListener("click", async () => {
  // Quem esquece as posições é o backend: o `gravar` do painel não as toca.
  await invoke("reset_hud_position").catch(() => {});
  atual = { ...atual, hud_position_dictation: null, hud_position_reading: null };
  desenharPosicao(null, null);
});

/* O processo derruba a própria janela ao reiniciar — não há resposta do
   `invoke` para esperar, então o texto do botão nem chega a mudar de volta. */
elemento("reiniciarVox").addEventListener("click", () => {
  invoke("restart_app").catch((erro) => mostrarFalha(String(erro)));
});

/* Sem moldura nativa, a própria janela cuida de minimizar, maximizar e
   fechar. Fechar aqui é só esconder — o backend já intercepta o pedido
   (`CloseRequested`) e devolve a janela para a bandeja em vez de destruí-la. */
const janela = getCurrentWindow();

elemento("janelaMinimizar").addEventListener("click", () => janela.minimize());
elemento("janelaFechar").addEventListener("click", () => janela.close());

const botaoMaximizar = elemento("janelaMaximizar");
async function atualizarBotaoMaximizar() {
  const maximizada = await janela.isMaximized();
  botaoMaximizar.dataset.maximizada = String(maximizada);
  const rotulo = maximizada ? "Restaurar tamanho" : "Maximizar";
  botaoMaximizar.setAttribute("aria-label", rotulo);
  botaoMaximizar.title = rotulo;
}
botaoMaximizar.addEventListener("click", async () => {
  await janela.toggleMaximize();
  atualizarBotaoMaximizar();
});
janela.onResized(atualizarBotaoMaximizar);
atualizarBotaoMaximizar();

/* ----------------------------------------------------------------- gravações
 *
 * O backup é escrito pelo backend a cada ditado, antes de qualquer coisa que
 * possa falhar (ver `recordings.rs`). Aqui só se lê, reenvia, marca como
 * salvo ou apaga — nada disto grava áudio novo. */

const STATUS_GRAVACAO = {
  pendente: ["Pendente", "neutro"],
  sem_fala: ["Sem fala detectada", "atencao"],
  transcrito: ["Transcrito", "ok"],
  vazio: ["Veio vazio", "atencao"],
  falhou: ["Falhou", "erro"],
};

function formatarQuando(ms) {
  const diffMin = Math.round((Date.now() - ms) / 60000);
  if (diffMin < 1) return "agora mesmo";
  if (diffMin < 60) return `há ${diffMin} min`;
  const diffHoras = Math.round(diffMin / 60);
  if (diffHoras < 24) return `há ${diffHoras}h`;
  return new Date(ms).toLocaleString("pt-BR", {
    day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit",
  });
}

function linhaGravacao(gravacao) {
  const [rotuloStatus, tom] = STATUS_GRAVACAO[gravacao.status] ?? [gravacao.status, "neutro"];

  const linha = document.createElement("div");
  linha.className = "gravacao";

  const cabecalho = document.createElement("div");
  cabecalho.className = "gravacao-cabecalho";

  const status = document.createElement("span");
  status.className = "gravacao-status";
  status.dataset.tom = tom;
  status.textContent = rotuloStatus;
  cabecalho.appendChild(status);

  const quando = document.createElement("span");
  quando.className = "gravacao-quando";
  quando.textContent = `${formatarQuando(gravacao.created_at_ms)} · ${gravacao.duration_seconds.toFixed(1)}s`;
  cabecalho.appendChild(quando);

  const dispositivo = document.createElement("span");
  dispositivo.className = "gravacao-dispositivo";
  dispositivo.textContent = gravacao.device_name;
  cabecalho.appendChild(dispositivo);

  linha.appendChild(cabecalho);

  if (gravacao.text) {
    const texto = document.createElement("div");
    texto.className = "gravacao-texto";
    texto.textContent = gravacao.text;
    linha.appendChild(texto);
  }
  if (gravacao.error) {
    const erro = document.createElement("div");
    erro.className = "gravacao-erro";
    erro.textContent = gravacao.error;
    linha.appendChild(erro);
  }

  const acoes = document.createElement("div");
  acoes.className = "gravacao-acoes";

  const reenviar = document.createElement("button");
  reenviar.type = "button";
  reenviar.className = "botao";
  reenviar.textContent = "Reenviar";
  reenviar.addEventListener("click", async () => {
    reenviar.disabled = true;
    reenviar.textContent = "Reenviando…";
    try {
      const atualizada = await invoke("retry_recording", { id: gravacao.id });
      linha.replaceWith(linhaGravacao(atualizada));
    } catch (erro) {
      reenviar.disabled = false;
      reenviar.textContent = "Reenviar";
      mostrarFalha(String(erro));
    }
  });
  acoes.appendChild(reenviar);

  if (gravacao.text) {
    const copiar = document.createElement("button");
    copiar.type = "button";
    copiar.className = "botao";
    copiar.textContent = "Copiar";
    copiar.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(gravacao.text);
        copiar.textContent = "Copiado";
      } catch {
        copiar.textContent = "Falhou";
      }
      setTimeout(() => { copiar.textContent = "Copiar"; }, 1600);
    });
    acoes.appendChild(copiar);
  }

  const salvar = document.createElement("button");
  salvar.type = "button";
  salvar.className = "botao";
  salvar.textContent = gravacao.saved ? "Salvo — fora da limpeza" : "Salvar";
  salvar.disabled = gravacao.saved;
  salvar.addEventListener("click", async () => {
    try {
      await invoke("save_recording", { id: gravacao.id });
      salvar.textContent = "Salvo — fora da limpeza";
      salvar.disabled = true;
    } catch (erro) {
      mostrarFalha(String(erro));
    }
  });
  acoes.appendChild(salvar);

  const excluir = document.createElement("button");
  excluir.type = "button";
  excluir.className = "botao";
  excluir.textContent = "Excluir";
  excluir.addEventListener("click", async () => {
    try {
      await invoke("delete_recording", { id: gravacao.id });
      linha.remove();
      if (!elemento("listaGravacoes").children.length) {
        elemento("gravacoesVazio").hidden = false;
      }
    } catch (erro) {
      mostrarFalha(String(erro));
    }
  });
  acoes.appendChild(excluir);

  linha.appendChild(acoes);
  return linha;
}

async function carregarGravacoes() {
  const lista = elemento("listaGravacoes");
  const vazio = elemento("gravacoesVazio");
  lista.innerHTML = "";

  let gravacoes = [];
  try {
    gravacoes = await invoke("list_recordings");
  } catch (erro) {
    mostrarFalha(String(erro));
    return;
  }

  vazio.hidden = gravacoes.length > 0;
  for (const gravacao of gravacoes) {
    lista.appendChild(linhaGravacao(gravacao));
  }
}

elemento("abrirPastaGravacoes").addEventListener("click", () => {
  invoke("open_recordings_folder").catch((erro) => mostrarFalha(String(erro)));
});

/* -------------------------------------------------------------------- listas */

function opcao(valor, texto) {
  const item = document.createElement("option");
  item.value = valor;
  item.textContent = texto;
  return item;
}

/* Um valor que não está na lista continua sendo o valor.
 *
 * Sem isto, abrir o painel com a API fora do ar zeraria a voz configurada no
 * primeiro `save` — a tela mostraria vazio, e vazio é o que seria gravado. */
function manterDesconhecido(select, valor) {
  if (!valor || select.value === valor) return;
  select.appendChild(opcao(valor, `${valor} (não listado agora)`));
  select.value = valor;
}

async function preencherMicrofones(selecionado) {
  const select = elemento("inputDevice");
  select.innerHTML = "";
  select.appendChild(opcao("", "O padrão do sistema"));

  try {
    for (const device of await invoke("list_input_devices")) {
      select.appendChild(opcao(device.id, device.is_default ? `${device.name} — padrão` : device.name));
    }
  } catch {
    // Sem lista o campo continua servindo: o valor guardado permanece.
  }

  select.value = selecionado ?? "";
  manterDesconhecido(select, selecionado);
}

async function preencherVozes(selecionada) {
  const select = elemento("voice");
  select.innerHTML = "";

  try {
    const catalogo = await invoke("api_voices");
    const nomes = [...(catalogo.custom ?? []), ...(catalogo.base ?? [])];
    elemento("sobreVozes").textContent = `${nomes.length}`;
    for (const nome of nomes) {
      select.appendChild(opcao(nome, nome === catalogo.default ? `${nome} — padrão da API` : nome));
    }
  } catch {
    // A API pode estar fora do ar, e isso não pode impedir de mexer no volume.
  }

  select.value = selecionada ?? "";
  manterDesconhecido(select, selecionada);
}

async function carregarPresets(escolhido) {
  try {
    const catalogo = await invoke("rewrite_presets");
    presetsDisponiveis = catalogo.presets ?? [];
  } catch {
    presetsDisponiveis = [];
  }
  desenharPresets(escolhido);
}

async function verificarApi() {
  const caixa = elemento("statusApi");
  const texto = elemento("statusTexto");
  try {
    const saude = await invoke("api_health");
    caixa.dataset.estado = "ok";
    texto.textContent = `API ${saude.version ?? ""} no ar`.trim();
    elemento("sobreVersao").textContent = saude.version ?? "—";
  } catch (erro) {
    caixa.dataset.estado = "erro";
    texto.textContent = "serviço de voz fora do ar";
    elemento("sobreVersao").textContent = String(erro).slice(0, 60);
  }
}

/* ------------------------------------------------------------------ partida */

montarCores();

invoke("get_settings")
  .then(async (settings) => {
    await preencherMicrofones(settings.input_device);
    await preencherVozes(settings.voice);
    await carregarPresets(settings.stt_rewrite_preset);
    await montarAtalhos();
    desenhar(settings);

    invoke("api_base_url").then((url) => { elemento("sobreUrl").textContent = url; }).catch(() => {});
    invoke("settings_path").then((caminho) => { elemento("sobreConfig").textContent = caminho; }).catch(() => {});
    verificarApi();
  })
  .catch((erro) => mostrarFalha("invoke('get_settings') falhou:\n" + String(erro)));
