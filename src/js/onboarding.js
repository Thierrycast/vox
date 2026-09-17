/* Vox — assistente de primeiro uso.
 *
 * Roda uma vez (até `onboarding_completed` virar `true`) e só recolhe o que
 * de fato precisa de uma decisão da pessoa: servidor, microfone. O resto —
 * voz, atalhos, tema — já tem um padrão sensato e fica pras preferências.
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
  caixa.textContent = "Vox — falha no assistente:\n\n" + motivo;
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
const janela = getCurrentWindow();

/* -------------------------------------------------------- seletor próprio
 * Mesmo componente de `settings.js` — duplicado, e não importado, porque os
 * dois arquivos carregam como módulos independentes em páginas diferentes e
 * a única coisa compartilhável seria isto sozinho. */
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

/* -------------------------------------------------------------- navegação */

const PASSOS = ["boas-vindas", "servidor", "microfone", "atalhos", "fim"];
let passoAtual = 0;

function desenharPontos() {
  const alvo = elemento("obPontos");
  alvo.innerHTML = "";
  for (let indice = 0; indice < PASSOS.length; indice += 1) {
    const ponto = document.createElement("span");
    ponto.dataset.atual = String(indice === passoAtual);
    alvo.appendChild(ponto);
  }
}

function mostrarPasso(indice) {
  passoAtual = indice;
  for (const secao of document.querySelectorAll(".passo")) {
    secao.dataset.ativo = String(secao.dataset.passo === PASSOS[indice]);
  }
  elemento("obVoltar").hidden = indice === 0;
  elemento("obAvancar").textContent = indice === PASSOS.length - 1 ? "Concluir" : "Continuar";
  desenharPontos();

  if (PASSOS[indice] === "microfone") carregarMicrofones();
  if (PASSOS[indice] === "atalhos") carregarAtalhos();
}

elemento("obVoltar").addEventListener("click", () => {
  if (passoAtual > 0) mostrarPasso(passoAtual - 1);
});

elemento("obAvancar").addEventListener("click", async () => {
  if (passoAtual < PASSOS.length - 1) {
    mostrarPasso(passoAtual + 1);
    return;
  }
  await concluir();
});

elemento("janelaFechar").addEventListener("click", async () => {
  // Fechar sem terminar é "decido isso depois, nas preferências" — não deve
  // voltar a interromper a próxima abertura. Só marca o primeiro-uso como
  // visto; nenhum outro campo é gravado.
  try {
    const settings = await invoke("get_settings");
    settings.onboarding_completed = true;
    await invoke("save_settings", { settings });
  } catch {
    // Sem preferências pra carregar não há o que marcar — fechar já resolve.
  }
  janela.close();
});

/* -------------------------------------------------------------- servidor */

elemento("obTestar").addEventListener("click", async () => {
  const botao = elemento("obTestar");
  const resultado = elemento("obTesteResultado");
  botao.disabled = true;
  resultado.dataset.tom = "";
  resultado.textContent = "Testando…";

  try {
    await invoke("test_server_connection", {
      baseUrl: elemento("obBaseUrl").value.trim(),
      user: elemento("obUser").value.trim(),
      password: elemento("obPassword").value,
    });
    resultado.dataset.tom = "ok";
    resultado.textContent = "Conectou";
  } catch (erro) {
    resultado.dataset.tom = "erro";
    resultado.textContent = String(erro);
  } finally {
    botao.disabled = false;
  }
});

/* ------------------------------------------------------------- microfone */

let microfonesCarregados = false;

async function carregarMicrofones() {
  if (microfonesCarregados) return;
  microfonesCarregados = true;

  const select = elemento("obInputDevice");
  const opcao = (valor, texto) => {
    const item = document.createElement("option");
    item.value = valor;
    item.textContent = texto;
    return item;
  };

  select.appendChild(opcao("", "O padrão do sistema"));
  try {
    for (const device of await invoke("list_input_devices")) {
      select.appendChild(opcao(device.id, device.is_default ? `${device.name} — padrão` : device.name));
    }
  } catch {
    // Sem lista, o padrão do sistema continua sendo uma escolha válida.
  }

  prepararSelect(select.closest(".select"));
}

/* --------------------------------------------------------------- atalhos */

let atalhosCarregados = false;

async function carregarAtalhos() {
  if (atalhosCarregados) return;
  atalhosCarregados = true;

  const alvo = elemento("obAtalhos");
  try {
    const catalogo = await invoke("command_catalog");
    for (const comando of catalogo) {
      const linha = document.createElement("div");
      linha.className = "onboarding-atalho";
      linha.innerHTML =
        `<span>${comando.label}</span><span>${comando.binding || "sem atalho"}</span>`;
      alvo.appendChild(linha);
    }
  } catch (erro) {
    mostrarFalha(String(erro));
  }
}

/* -------------------------------------------------------------- conclusão */

async function concluir() {
  const botao = elemento("obAvancar");
  botao.disabled = true;
  botao.textContent = "Só um instante…";

  try {
    const settings = await invoke("get_settings");

    const baseUrl = elemento("obBaseUrl").value.trim();
    const user = elemento("obUser").value.trim();
    const password = elemento("obPassword").value;
    const inputDevice = elemento("obInputDevice").value;

    settings.api_base_url = baseUrl || null;
    settings.api_user = user || null;
    settings.api_password = password || null;
    settings.input_device = inputDevice || null;
    settings.onboarding_completed = true;

    await invoke("save_settings", { settings });
    // O cliente do servidor só é montado uma vez, na partida — reiniciar é
    // o que faz o endereço escolhido aqui valer de verdade.
    await invoke("restart_app");
  } catch (erro) {
    botao.disabled = false;
    botao.textContent = "Concluir";
    mostrarFalha(String(erro));
  }
}

mostrarPasso(0);
